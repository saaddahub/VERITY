//! The methods the service answers. Every client is untrusted: parameters are
//! validated here, and nothing a client sends becomes a path or a command.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use verity_core::metrics::Thresholds;
use verity_core::telemetry::{resolve_target, ProcessApi};
use verity_core::SystemInfoApi;
use verity_ipc::dispatch::Handler;
use verity_ipc::{
    RpcError, CAPTURE_FAILED, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND, NOT_FOUND,
};
use verity_store::{NewSession, Store, StoreError};
use verity_sys::pdh::PdhCounters;
use verity_sys::process::{qpc_freq, WinProcesses};
use verity_sys::WinSystemInfo;
use verity_telemetry::capture::{self, CaptureConfig};

/// Longest capture a client may ask for.
const MAX_DURATION_S: f64 = 6.0 * 3600.0;

pub struct Service {
    store: Mutex<Store>,
    /// One capture at a time: there is one `VerityPM` trace session.
    capturing: tokio::sync::Mutex<()>,
    presentmon: PathBuf,
}

#[derive(Deserialize)]
struct RecordParams {
    pid: Option<u32>,
    exe: Option<String>,
    duration_s: Option<f64>,
}

#[derive(Deserialize)]
struct SessionParams {
    id: i64,
    #[serde(default)]
    seq: u32,
}

fn params<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(v).map_err(|e| RpcError::new(INVALID_PARAMS, e.to_string()))
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(INTERNAL_ERROR, e.to_string())
}

fn store_error(e: StoreError) -> RpcError {
    match e {
        StoreError::NotFound(_) => RpcError::new(NOT_FOUND, e.to_string()),
        _ => internal(e),
    }
}

fn to_json(v: impl serde::Serialize) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

impl Service {
    pub fn new(store: Store, presentmon: PathBuf) -> Self {
        Self {
            store: Mutex::new(store),
            capturing: tokio::sync::Mutex::new(()),
            presentmon,
        }
    }

    // shortcut: database calls run inline on the async runtime; move them to the
    // single writer task of docs/02-architecture.md when sessions overlap (M4).
    fn store(&self) -> Result<std::sync::MutexGuard<'_, Store>, RpcError> {
        self.store
            .lock()
            .map_err(|_| internal("store lock poisoned"))
    }

    async fn record(&self, p: RecordParams) -> Result<Value, RpcError> {
        let duration = match p.duration_s {
            Some(s) if (1.0..=MAX_DURATION_S).contains(&s) => Some(Duration::from_secs_f64(s)),
            Some(_) => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("duration_s must be between 1 and {MAX_DURATION_S}"),
                ))
            }
            None => None,
        };
        let _one_at_a_time = self
            .capturing
            .try_lock()
            .map_err(|_| RpcError::new(CAPTURE_FAILED, "a capture is already running"))?;

        let target = resolve_target(
            &WinProcesses.processes().map_err(internal)?,
            p.pid,
            p.exe.as_deref(),
        )
        .map_err(|e| RpcError::new(INVALID_PARAMS, e))?;
        tracing::info!(pid = target.pid, exe = %target.name, "capture starting");

        let mut counters = PdhCounters::new().map_err(internal)?;
        let started_utc = verity_store::utc_now();
        let cfg = CaptureConfig {
            presentmon: self.presentmon.clone(),
            pid: target.pid,
            duration,
            sample_period: Duration::from_secs(1),
        };
        let cap = capture::run(&cfg, &mut counters, &WinProcesses)
            .await
            .map_err(|e| RpcError::new(CAPTURE_FAILED, e.to_string()))?;
        if cap.frames.is_empty() {
            return Err(RpcError::new(
                CAPTURE_FAILED,
                format!("{} presented no frames during the capture", target.name),
            ));
        }

        let info = WinSystemInfo.system_info().map_err(internal)?;
        let freq = qpc_freq();
        let summary = capture::summarize_capture(
            &cap,
            &target,
            &info.gpus,
            freq,
            info.cpu.logical.len(),
            &Thresholds::default(),
        );
        let id = self
            .store()?
            .insert_session(&NewSession {
                pid: target.pid,
                started_qpc: cap.start_qpc,
                ended_qpc: cap.end_qpc,
                qpc_freq: freq,
                started_utc: &started_utc,
                summary: &summary,
                frames: &cap.frames,
                series: &capture::series(&cap, target.pid),
                gaps: &cap.gaps,
            })
            .map_err(store_error)?;
        tracing::info!(session = id, frames = cap.frames.len(), "capture stored");
        Ok(json!({ "id": id, "started_utc": started_utc, "summary": summary }))
    }

    fn session_json(row: verity_store::SessionRow) -> Result<Value, RpcError> {
        Ok(json!({
            "id": row.id,
            "started_utc": row.started_utc,
            "frame_chunks": row.frame_chunks,
            "summary": to_json(row.summary)?,
        }))
    }
}

impl Handler for Service {
    async fn call(&self, method: &str, p: Value) -> Result<Value, RpcError> {
        match method {
            "system.info" => to_json(WinSystemInfo.system_info().map_err(internal)?),
            "record.run" => self.record(params(p)?).await,
            "sessions.list" => {
                let rows = self.store()?.list().map_err(store_error)?;
                rows.into_iter()
                    .map(Self::session_json)
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            "sessions.get" => {
                let p: SessionParams = params(p)?;
                Self::session_json(self.store()?.get(p.id).map_err(store_error)?)
            }
            "sessions.frames" => {
                let p: SessionParams = params(p)?;
                to_json(self.store()?.frames(p.id, p.seq).map_err(store_error)?)
            }
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method {other}"),
            )),
        }
    }
}
