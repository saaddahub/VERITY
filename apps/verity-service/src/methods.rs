//! The methods the service answers. Every client is untrusted: parameters are
//! validated here, and nothing a client sends becomes a path the service
//! writes to, a command it runs, or a library it loads.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use verity_actions::{catalog, Engine};
use verity_core::actions::{
    Executor, JournalFilter, JournalRow, JournalState, JournalStore, NewJournalRow,
};
use verity_core::metrics::Thresholds;
use verity_core::telemetry::{resolve_target, ProcessApi};
use verity_core::SystemInfoApi;
use verity_ipc::dispatch::{Caller, Handler};
use verity_ipc::{
    RpcError, CAPTURE_FAILED, FORBIDDEN, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
    NOT_FOUND,
};
use verity_session::machine::Input;
use verity_store::{NewGame, NewSession, Store, StoreError};
use verity_sys::actions::WinActionSys;
use verity_sys::pdh::PdhCounters;
use verity_sys::process::{qpc_freq, WinProcesses};
use verity_sys::WinSystemInfo;
use verity_telemetry::capture::{self, CaptureConfig};

use crate::actions::Locked;
use crate::session_mode::{self, Handle, Msg};

/// Largest params and snapshot an agent may journal (bytes of JSON).
const MAX_AGENT_PARAMS: usize = 8 * 1024;
const MAX_AGENT_SNAPSHOT: usize = 64 * 1024;
/// Most journal rows `journal.list` returns (the newest).
const MAX_JOURNAL_ROWS: usize = 500;
const SETTING_RESTORE_POINT: &str = "actions.restore_point_done";

/// Longest capture a client may ask for.
const MAX_DURATION_S: f64 = 6.0 * 3600.0;
/// Most timeline events `story.get` returns.
const MAX_STORY_EVENTS: usize = 4000;

pub struct Service {
    store: Arc<Mutex<Store>>,
    /// One capture at a time: there is one `VerityPM` trace session.
    capturing: Arc<tokio::sync::Mutex<()>>,
    presentmon: PathBuf,
    pub session: Handle,
    /// One maintenance job (Optimize Drives, junk cleanup) at a time.
    maintenance: tokio::sync::Mutex<()>,
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

#[derive(Deserialize)]
struct IdParams {
    id: i64,
}

#[derive(Deserialize)]
struct AddGameParams {
    exe_path: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct AnswerParams {
    exe: String,
    answer: String,
}

#[derive(Deserialize)]
struct ProfileParams {
    game_id: Option<i64>,
    profile: Option<Value>,
}

#[derive(Deserialize)]
struct StatusParams {
    #[serde(default)]
    since: u64,
}

#[derive(Deserialize)]
struct StartParams {
    pid: Option<u32>,
    exe: Option<String>,
}

#[derive(Deserialize)]
struct EnableParams {
    on: bool,
}

#[derive(Deserialize)]
struct ForegroundParams {
    pid: u32,
    /// Processes with an active audio session, when the agent can tell.
    audio: Option<Vec<u32>>,
}

#[derive(Deserialize)]
struct ActionParams {
    id: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
struct JournalListParams {
    #[serde(default)]
    open_only: bool,
}

#[derive(Deserialize)]
struct JournalRecordParams {
    txn_id: String,
    action_id: String,
    params: Value,
    snapshot: Value,
}

#[derive(Deserialize)]
struct JournalMarkParams {
    id: i64,
    state: String,
    error: Option<String>,
}

#[derive(Deserialize)]
struct RestorePointParams {
    /// The user agreed to go on without a new restore point.
    #[serde(default)]
    proceed_without: bool,
}

#[derive(Deserialize)]
struct JunkParams {
    /// Catalog ids (`verity_storage::junk::CATALOG`) that run in the service.
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct OptimizeParams {
    /// A drive letter, checked against the volumes Windows lists.
    drive: String,
}

fn params<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(v).map_err(|e| RpcError::new(INVALID_PARAMS, e.to_string()))
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(INTERNAL_ERROR, e.to_string())
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(INVALID_PARAMS, msg)
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

/// An executable file name such as `game.exe`: no folders, no drive.
fn valid_exe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 260
        && s.to_ascii_lowercase().ends_with(".exe")
        && !s.contains(['\\', '/', ':', '\0'])
}

/// A full path to an existing `.exe` on a local drive. The service stores it
/// to recognise the game; it never runs, opens for writing, or loads it.
fn valid_exe_path(s: &str) -> Result<PathBuf, RpcError> {
    let p = Path::new(s);
    let b = s.as_bytes();
    let drive = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    if !drive || s.len() > 1024 || s.contains('\0') || s.contains("\\\\?\\") {
        return Err(invalid(
            "exe_path must be a full path like C:\\Games\\game.exe",
        ));
    }
    if !s.to_ascii_lowercase().ends_with(".exe") {
        return Err(invalid("exe_path must point at an .exe file"));
    }
    if !p.is_file() {
        return Err(invalid(format!("{s} does not exist")));
    }
    Ok(p.to_path_buf())
}

impl Service {
    pub fn new(
        store: Store,
        presentmon: PathBuf,
        startup_notice: Option<String>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let store = Arc::new(Mutex::new(store));
        let capturing = Arc::new(tokio::sync::Mutex::new(()));
        let (session, task) = session_mode::start(
            store.clone(),
            capturing.clone(),
            presentmon.clone(),
            startup_notice,
        );
        (
            Self {
                store,
                capturing,
                presentmon,
                session,
                maintenance: tokio::sync::Mutex::new(()),
            },
            task,
        )
    }

    fn store(&self) -> Result<std::sync::MutexGuard<'_, Store>, RpcError> {
        self.store
            .lock()
            .map_err(|_| internal("store lock poisoned"))
    }

    async fn record(&self, p: RecordParams) -> Result<Value, RpcError> {
        let duration = match p.duration_s {
            Some(s) if (1.0..=MAX_DURATION_S).contains(&s) => Some(Duration::from_secs_f64(s)),
            Some(_) => {
                return Err(invalid(format!(
                    "duration_s must be between 1 and {MAX_DURATION_S}"
                )))
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
        .map_err(invalid)?;
        tracing::info!(pid = target.pid, exe = %target.name, "capture starting");

        let mut counters = PdhCounters::new().map_err(internal)?;
        let started_utc = verity_store::utc_now();
        let cfg = CaptureConfig {
            presentmon: self.presentmon.clone(),
            pid: target.pid,
            duration,
            sample_period: Duration::from_secs(1),
            control: None,
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
                // shortcut: unknown for a manual recording, since reading it would
                // open a handle to the target; Session Mode sessions record it.
                process_start_filetime: 0,
                game_id: None,
                profile_json: None,
                events: &[],
            })
            .map_err(store_error)?;
        tracing::info!(session = id, frames = cap.frames.len(), "capture stored");
        Ok(json!({ "id": id, "started_utc": started_utc, "summary": summary }))
    }

    /// A session for `sessions.get` (`full`) or a row of `sessions.list`. List
    /// rows leave out the per-stutter and per-gap lists, which grow with the
    /// session, so the list stays small however many sessions are stored.
    // shortcut: no paging; slim rows are about 600 bytes, so the list passes the
    // 1 MiB message limit near 1,500 sessions. Page it before that is plausible.
    fn session_json(row: verity_store::SessionRow, full: bool) -> Result<Value, RpcError> {
        let groups = &row.summary.stats.hitch_groups;
        let hitches = json!({
            "count": groups.len(),
            "worst_ms": groups.iter().map(|g| g.worst_ms).fold(0.0, f32::max),
        });
        let mut summary = to_json(row.summary)?;
        if !full {
            if let Some(s) = summary.as_object_mut() {
                s.remove("capture_gaps");
                if let Some(stats) = s.get_mut("stats").and_then(Value::as_object_mut) {
                    stats.remove("hitch_groups");
                }
            }
        }
        Ok(json!({
            "id": row.id,
            "started_utc": row.started_utc,
            "frame_chunks": row.frame_chunks,
            "hitches": hitches,
            "summary": summary,
        }))
    }

    fn status(&self, since: u64) -> Value {
        let view = self.session.view.lock().unwrap_or_else(|p| p.into_inner());
        let notices: Vec<&session_mode::Notice> =
            view.notices.iter().filter(|n| n.id > since).collect();
        json!({
            "status": view.status,
            "notices": notices,
            "capabilities": view.capabilities,
        })
    }

    async fn games_scan(&self) -> Result<Value, RpcError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || session_mode::rescan(&store))
            .await
            .map_err(internal)?
            .map_err(internal)?;
        let _ = self.session.tx.send(Msg::LibraryChanged);
        to_json(self.store()?.games().map_err(store_error)?)
    }

    fn games_add(&self, p: AddGameParams) -> Result<Value, RpcError> {
        let path = valid_exe_path(&p.exe_path)?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = p.name.unwrap_or(stem);
        if name.trim().is_empty() || name.len() > 200 {
            return Err(invalid("name must be 1 to 200 characters"));
        }
        let id = self
            .store()?
            .add_game(&NewGame {
                name: name.trim(),
                exe_path: &p.exe_path,
                source: "manual",
                launcher_id: None,
            })
            .map_err(store_error)?;
        let _ = self.session.tx.send(Msg::LibraryChanged);
        Ok(json!({ "id": id }))
    }

    fn profile(&self, p: ProfileParams) -> Result<Value, RpcError> {
        let mut store = self.store()?;
        if let Some(id) = p.game_id {
            store.game(id).map_err(store_error)?;
        }
        let Some(v) = p.profile else {
            let json = store.profile(p.game_id).map_err(store_error)?;
            let is_default = json.is_none();
            let profile = verity_session::profile::load(json.as_deref());
            return Ok(json!({ "profile": profile, "stored": !is_default }));
        };
        let (profile, warnings) = verity_session::profile::parse(v).map_err(invalid)?;
        let json = serde_json::to_string(&profile).map_err(internal)?;
        store.set_profile(p.game_id, &json).map_err(store_error)?;
        Ok(json!({ "profile": profile, "warnings": warnings }))
    }

    async fn storage_optimize(&self, p: OptimizeParams) -> Result<Value, RpcError> {
        let mut chars = p.drive.trim_end_matches([':', '\\']).chars();
        let letter = match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => c,
            _ => return Err(invalid("drive must be a letter like C")),
        };
        if !self.session_idle() {
            return Err(invalid("Optimize waits until no game session is running"));
        }
        let _one_at_a_time = self
            .maintenance
            .try_lock()
            .map_err(|_| invalid("Verity is already doing drive maintenance"))?;
        let code = tokio::task::spawn_blocking(move || crate::storage::optimize(letter))
            .await
            .map_err(internal)?
            .map_err(internal)?;
        Ok(json!({ "drive": letter.to_ascii_uppercase().to_string(), "result": code }))
    }

    /// No game session is arming or running.
    fn session_idle(&self) -> bool {
        self.session
            .view
            .lock()
            .map(|v| v.status.get("state").and_then(Value::as_str) == Some("idle"))
            .unwrap_or(false)
    }

    /// Measures (`clean` false) or clears machine-wide junk categories through
    /// Windows' Disk Cleanup handlers. Only service-run catalog ids are accepted.
    async fn junk(&self, p: JunkParams, clean: bool) -> Result<Value, RpcError> {
        use verity_storage::junk::{find, How, Runs};
        let mut handlers = Vec::new();
        for id in &p.ids {
            match find(id) {
                Some(c) if c.runs == Runs::Service => match c.how {
                    How::Handler(h) => handlers.push((c.id, h)),
                    How::BrowserCaches => return Err(invalid(format!("{id} runs in the app"))),
                },
                _ => return Err(invalid(format!("{id} is not a service cleanup category"))),
            }
        }
        let _one_at_a_time = if clean {
            if !self.session_idle() {
                return Err(invalid("Cleaning waits until no game session is running"));
            }
            Some(
                self.maintenance
                    .try_lock()
                    .map_err(|_| invalid("Verity is already doing drive maintenance"))?,
            )
        } else {
            None
        };
        let results = tokio::task::spawn_blocking(move || crate::storage::junk(&handlers, clean))
            .await
            .map_err(internal)?;
        Ok(Value::Array(results))
    }

    fn story(&self, id: i64) -> Result<crate::story::StoryOut, RpcError> {
        let store = self.store()?;
        crate::story::for_session(&store, id).map_err(store_error)
    }
}

fn action(id: &str) -> Result<&'static dyn verity_actions::Action, RpcError> {
    catalog::find(id).ok_or_else(|| invalid(format!("{id} is not in the action catalog")))
}

fn action_error(e: verity_actions::ActionError) -> RpcError {
    match e {
        verity_actions::ActionError::Params(_) | verity_actions::ActionError::NotApplicable(_) => {
            invalid(e.to_string())
        }
        e => internal(e),
    }
}

/// Rows a caller may see: every service row, and only their own app rows.
fn visible(row: &JournalRow, caller: &Caller) -> bool {
    row.row.executor == Executor::Service || (!caller.sid.is_empty() && row.row.owner == caller.sid)
}

/// Whether Session Mode is arming or running a session.
fn session_running(session: &Handle) -> bool {
    let view = session.view.lock().unwrap_or_else(|p| p.into_inner());
    view.status
        .get("state")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "idle")
}

impl Service {
    fn engine<R>(&self, f: impl FnOnce(&Engine) -> R) -> R {
        let journal = Locked(&self.store);
        f(&Engine {
            sys: &WinActionSys,
            journal: &journal,
        })
    }

    /// The action and journal methods (docs/02-architecture.md); None for
    /// any other method.
    async fn actions_call(
        &self,
        caller: &Caller,
        method: &str,
        p: &Value,
    ) -> Option<Result<Value, RpcError>> {
        let p = p.clone();
        Some(match method {
            "actions.catalog" => {
                to_json(catalog::all().iter().map(|a| a.info()).collect::<Vec<_>>())
            }
            "actions.plan" => params(p).and_then(|a| self.action_plan(a)),
            "actions.apply" => params(p).and_then(|a| self.action_apply(caller, a)),
            "actions.revert" => {
                params::<IdParams>(p).and_then(|i| self.action_revert(caller, i.id))
            }
            "journal.list" => {
                params::<JournalListParams>(p).and_then(|l| self.journal_list(caller, l.open_only))
            }
            "journal.revert_all" => self.journal_revert_all(caller).await,
            "journal.record" => params(p).and_then(|r| self.journal_record(caller, r)),
            "journal.mark" => params(p).and_then(|m| self.journal_mark(caller, m)),
            "system.restore_point" => match params(p) {
                Ok(r) => self.restore_point(r).await,
                Err(e) => Err(e),
            },
            _ => return None,
        })
    }

    fn action_plan(&self, p: ActionParams) -> Result<Value, RpcError> {
        let a = action(&p.id)?;
        if a.info().executor != Executor::Service {
            return Err(invalid(format!(
                "{} runs in the app, not the service",
                p.id
            )));
        }
        let plan = self.engine(|eng| {
            let prepared = a.prepare(eng.sys, p.params)?;
            a.plan(eng.sys, &prepared)
        });
        match plan {
            Ok(change) => Ok(json!({ "change": change })),
            Err(verity_actions::ActionError::NotApplicable(why)) => {
                Ok(json!({ "change": null, "not_applicable": why }))
            }
            Err(e) => Err(action_error(e)),
        }
    }

    fn action_apply(&self, caller: &Caller, p: ActionParams) -> Result<Value, RpcError> {
        let a = action(&p.id)?;
        if a.info().executor != Executor::Service {
            return Err(invalid(format!(
                "{} runs in the app, not the service",
                p.id
            )));
        }
        if !caller.elevated {
            return Err(RpcError::new(
                FORBIDDEN,
                "changing this for the whole PC needs Verity run as administrator",
            ));
        }
        let txn = format!("manual-{}", verity_store::utc_now());
        let id = self
            .engine(|eng| eng.apply(a, p.params, &txn, ""))
            .map_err(action_error)?;
        tracing::info!(id, action = %p.id, "action applied on request");
        Ok(json!({ "id": id }))
    }

    fn action_revert(&self, caller: &Caller, id: i64) -> Result<Value, RpcError> {
        let row = Locked(&self.store)
            .journal_get(id)
            .map_err(internal)?
            .filter(|r| visible(r, caller))
            .ok_or_else(|| RpcError::new(NOT_FOUND, format!("journal entry {id} not found")))?;
        if row.row.executor != Executor::Service {
            return Err(invalid("this change was made by the app; undo it there"));
        }
        to_json(self.engine(|eng| eng.revert(&row)))
    }

    fn journal_list(&self, caller: &Caller, open_only: bool) -> Result<Value, RpcError> {
        let rows = Locked(&self.store)
            .journal_list(&JournalFilter {
                open_only,
                ..Default::default()
            })
            .map_err(internal)?;
        let mut rows: Vec<_> = rows.into_iter().filter(|r| visible(r, caller)).collect();
        let skip = rows.len().saturating_sub(MAX_JOURNAL_ROWS);
        rows.drain(..skip);
        to_json(rows)
    }

    /// "Undo everything": ends a running session's changes, reverts every open
    /// service change, and hands back the caller's own app changes to undo.
    async fn journal_revert_all(&self, caller: &Caller) -> Result<Value, RpcError> {
        if session_running(&self.session) && !self.session.input(Input::Panic).await {
            return Err(internal("Session Mode did not respond"));
        }
        let journal = Locked(&self.store);
        let open = journal
            .journal_list(&JournalFilter {
                executor: Some(Executor::Service),
                open_only: true,
                ..Default::default()
            })
            .map_err(internal)?;
        let outcomes = self.engine(|eng| eng.revert_all(&open));
        let agent = if caller.sid.is_empty() {
            Vec::new()
        } else {
            journal
                .journal_list(&JournalFilter {
                    executor: Some(Executor::Agent),
                    owner: Some(&caller.sid),
                    open_only: true,
                    ..Default::default()
                })
                .map_err(internal)?
        };
        Ok(json!({
            "service": outcomes
                .iter()
                .map(|(id, o)| json!({ "id": id, "result": o }))
                .collect::<Vec<_>>(),
            "agent_rows": agent,
        }))
    }

    fn journal_record(&self, caller: &Caller, p: JournalRecordParams) -> Result<Value, RpcError> {
        let a = action(&p.action_id)?;
        if a.info().executor != Executor::Agent {
            return Err(invalid(format!("{} runs in the service", p.action_id)));
        }
        if caller.sid.is_empty() {
            return Err(RpcError::new(
                FORBIDDEN,
                "the service could not tell who is asking",
            ));
        }
        if p.txn_id.is_empty() || p.txn_id.len() > 100 {
            return Err(invalid("txn_id must be 1 to 100 characters"));
        }
        if p.params.to_string().len() > MAX_AGENT_PARAMS
            || p.snapshot.to_string().len() > MAX_AGENT_SNAPSHOT
        {
            return Err(invalid("params or snapshot too large"));
        }
        let id = Locked(&self.store)
            .journal_insert(&NewJournalRow {
                txn_id: p.txn_id,
                action_id: p.action_id,
                scope: a.info().scope,
                executor: Executor::Agent,
                params: p.params,
                snapshot: p.snapshot,
                owner: caller.sid.clone(),
            })
            .map_err(internal)?;
        Ok(json!({ "id": id }))
    }

    fn journal_mark(&self, caller: &Caller, p: JournalMarkParams) -> Result<Value, RpcError> {
        let journal = Locked(&self.store);
        let row = journal
            .journal_get(p.id)
            .map_err(internal)?
            .filter(|r| r.row.executor == Executor::Agent && visible(r, caller))
            .ok_or_else(|| RpcError::new(NOT_FOUND, format!("journal entry {} not found", p.id)))?;
        let state = JournalState::parse(&p.state).ok_or_else(|| invalid("unknown state"))?;
        let allowed = match (row.state, state) {
            (JournalState::Pending, JournalState::Applied) => true,
            (JournalState::Pending | JournalState::Applied, s) => matches!(
                s,
                JournalState::Reverted | JournalState::RevertFailed | JournalState::Superseded
            ),
            _ => false,
        };
        if !allowed {
            return Err(invalid(format!(
                "cannot move a {} entry to {}",
                row.state.as_str(),
                state.as_str()
            )));
        }
        let error: Option<String> = p.error.map(|e| e.chars().take(1024).collect());
        journal
            .journal_set_state(p.id, state, error.as_deref())
            .map_err(internal)?;
        Ok(json!({ "ok": true }))
    }

    /// One restore point before Verity's first persistent change
    /// (docs/09-safety.md section 1.4). Afterwards it reports "done".
    async fn restore_point(&self, p: RestorePointParams) -> Result<Value, RpcError> {
        if self
            .store()?
            .setting(SETTING_RESTORE_POINT)
            .map_err(store_error)?
            .is_some()
        {
            return Ok(json!({ "status": "done" }));
        }
        let made = tokio::task::spawn_blocking(|| {
            verity_sys::restore::create("Verity: before its first change")
        })
        .await
        .map_err(internal)?;
        let status = match &made {
            Ok(verity_sys::restore::RestorePoint::Created) => "created",
            Ok(verity_sys::restore::RestorePoint::Skipped) => "skipped",
            Ok(verity_sys::restore::RestorePoint::Disabled) => "disabled",
            Err(_) => "failed",
        };
        if status == "created" || p.proceed_without {
            self.store()?
                .set_setting(SETTING_RESTORE_POINT, status)
                .map_err(store_error)?;
        }
        tracing::info!(status, "restore point");
        Ok(json!({
            "status": status,
            "error": made.err().map(|e| e.to_string()),
        }))
    }
}

impl Handler for Service {
    async fn call(&self, caller: &Caller, method: &str, p: Value) -> Result<Value, RpcError> {
        if let Some(r) = self.actions_call(caller, method, &p).await {
            return r;
        }
        match method {
            "system.info" => to_json(WinSystemInfo.system_info().map_err(internal)?),
            "record.run" => self.record(params(p)?).await,
            "sessions.list" => {
                let rows = self.store()?.list().map_err(store_error)?;
                rows.into_iter()
                    .map(|r| Self::session_json(r, false))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
            "sessions.get" => {
                let p: SessionParams = params(p)?;
                Self::session_json(self.store()?.get(p.id).map_err(store_error)?, true)
            }
            "sessions.frames" => {
                let p: SessionParams = params(p)?;
                to_json(self.store()?.frames(p.id, p.seq).map_err(store_error)?)
            }

            "games.list" => to_json(self.store()?.games().map_err(store_error)?),
            "games.scan" => self.games_scan().await,
            "games.add" => self.games_add(params(p)?),
            "games.remove" => {
                let p: IdParams = params(p)?;
                self.store()?.remove_game(p.id).map_err(store_error)?;
                let _ = self.session.tx.send(Msg::LibraryChanged);
                Ok(json!({ "removed": p.id }))
            }
            "games.answer" => {
                let p: AnswerParams = params(p)?;
                if !valid_exe_name(&p.exe) {
                    return Err(invalid("exe must be a file name like game.exe"));
                }
                if !matches!(p.answer.as_str(), "game" | "not_game" | "never") {
                    return Err(invalid("answer must be game, not_game or never"));
                }
                let _ = self.session.tx.send(Msg::Answer {
                    exe: p.exe,
                    answer: p.answer,
                });
                Ok(json!({ "ok": true }))
            }
            "profile.get" | "profile.set" => {
                let mut p: ProfileParams = params(p)?;
                if method == "profile.get" {
                    p.profile = None;
                } else if p.profile.is_none() {
                    return Err(invalid("profile.set needs a profile"));
                }
                self.profile(p)
            }

            "session.status" => {
                let p: StatusParams = params(if p.is_null() { json!({}) } else { p })?;
                Ok(self.status(p.since))
            }
            "session.start" => {
                let p: StartParams = params(p)?;
                if let Some(exe) = &p.exe {
                    if !valid_exe_name(exe) {
                        return Err(invalid("exe must be a file name like game.exe"));
                    }
                }
                let (tx, rx) = oneshot::channel();
                self.session
                    .tx
                    .send(Msg::ManualStart(p.pid, p.exe, tx))
                    .map_err(internal)?;
                rx.await.map_err(internal)?.map_err(invalid)?;
                Ok(self.status(u64::MAX))
            }
            "session.stop" => {
                self.session.input(Input::Stop).await;
                Ok(self.status(u64::MAX))
            }
            "session.panic" => {
                if !self.session.input(Input::Panic).await {
                    return Err(internal("Session Mode did not respond"));
                }
                Ok(self.status(u64::MAX))
            }
            "session.enable" => {
                let p: EnableParams = params(p)?;
                self.session.input(Input::SetEnabled(p.on)).await;
                Ok(self.status(u64::MAX))
            }
            "agent.foreground" => {
                let p: ForegroundParams = params(p)?;
                let _ = self.session.tx.send(Msg::Foreground(p.pid));
                if let Some(audio) = p.audio {
                    let _ = self
                        .session
                        .tx
                        .send(Msg::Audio(audio.into_iter().take(4096).collect()));
                }
                Ok(json!({ "ok": true }))
            }

            "storage.health" => {
                let (disks, volumes) = tokio::task::spawn_blocking(crate::storage::health)
                    .await
                    .map_err(internal)?
                    .map_err(internal)?;
                Ok(json!({ "disks": disks, "volumes": volumes }))
            }
            "storage.optimize" => self.storage_optimize(params(p)?).await,
            "junk.measure" => self.junk(params(p)?, false).await,
            "junk.clean" => self.junk(params(p)?, true).await,

            "story.get" => {
                let p: IdParams = params(p)?;
                let out = self.story(p.id)?;
                let truncated = out.events.len() > MAX_STORY_EVENTS;
                let mut events = out.events;
                if truncated {
                    events.sort_by(|a, b| b.magnitude.total_cmp(&a.magnitude));
                    events.truncate(MAX_STORY_EVENTS);
                    events.sort_by_key(|e| e.qpc);
                }
                Ok(json!({
                    "session_id": p.id,
                    "story": out.story,
                    "events": events,
                    "events_truncated": truncated,
                    "qpc_freq": out.bounds.qpc_freq,
                    "capture_start": out.bounds.started_qpc,
                    "capture_end": out.bounds.ended_qpc,
                }))
            }
            "story.export" => {
                let p: IdParams = params(p)?;
                // The pipe server turns a reply over the message limit into an error.
                let html = crate::story::html(&self.story(p.id)?);
                Ok(json!({ "html": html }))
            }
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method {other}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_paths_and_names_are_validated() {
        assert!(valid_exe_name("game.exe"));
        assert!(!valid_exe_name(r"C:\game.exe"));
        assert!(!valid_exe_name("../game.exe"));
        assert!(!valid_exe_name("game.dll"));
        assert!(!valid_exe_name(""));
        assert!(valid_exe_path(r"relative\game.exe").is_err());
        assert!(valid_exe_path(r"\\server\share\game.exe").is_err());
        assert!(valid_exe_path(r"\\?\C:\game.exe").is_err());
        assert!(valid_exe_path(r"C:\Windows\notepad.txt").is_err());
        assert!(valid_exe_path(r"C:\does\not\exist.exe").is_err());
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(valid_exe_path(&format!(r"{sys}\System32\cmd.exe")).is_ok());
    }
}
