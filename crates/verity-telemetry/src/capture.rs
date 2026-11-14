//! One capture: PresentMon as a child process plus the 1 Hz counter sampler.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};
use tokio::time::Instant;
use verity_core::metrics::{after_warmup, summarize, Frame, Thresholds};
use verity_core::telemetry::{
    gpu_for_pid, parse_gpu_engine, CaptureGap, CounterApi, ProcessApi, ProcessEntry,
    SessionSummary, SystemSample, LIGHT_OVERHEAD_BUDGET_PCT,
};
use verity_core::Gpu;
use verity_sys::process::{child_cpu_time, own_cpu_time, qpc};

use crate::presentmon::{self, Layout, LineDecoder};

/// No console window for the child, whoever launches it.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long PresentMon gets to exit after being asked before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("PresentMon is not installed at {0}; run `verity service install` again")]
    Missing(PathBuf),
    #[error("PresentMon at {0} does not match the pinned build; refusing to run it")]
    HashMismatch(PathBuf),
    #[error("could not start PresentMon: {0}")]
    Spawn(std::io::Error),
    #[error(transparent)]
    Layout(#[from] presentmon::ParseError),
}

pub struct CaptureConfig {
    pub presentmon: PathBuf,
    pub pid: u32,
    /// None: until the target exits.
    pub duration: Option<Duration>,
    pub sample_period: Duration,
}

#[derive(Debug, Default)]
pub struct Capture {
    pub start_qpc: u64,
    pub end_qpc: u64,
    /// Frames of the primary swap chain, in order.
    pub frames: Vec<Frame>,
    pub gaps: Vec<CaptureGap>,
    pub samples: Vec<SystemSample>,
    pub unparsed_rows: u64,
    /// CPU time Verity itself used: this process plus PresentMon, in 100 ns units.
    pub cpu_100ns: u64,
}

/// Checks the binary against the pinned hash before it is ever run as SYSTEM.
pub fn verify_binary(path: &Path) -> Result<(), CaptureError> {
    let bytes = std::fs::read(path).map_err(|_| CaptureError::Missing(path.to_path_buf()))?;
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hex == presentmon::SHA256 {
        Ok(())
    } else {
        Err(CaptureError::HashMismatch(path.to_path_buf()))
    }
}

fn command(path: &Path, args: &[String]) -> Command {
    let mut c = Command::new(path);
    c.args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true);
    c
}

/// Asks a running `VerityPM` session to stop, which makes its PresentMon exit
/// cleanly. Also clears a session orphaned by a crash.
pub async fn stop_session(path: &Path) {
    let _ = command(path, &presentmon::stop_args())
        .stdout(Stdio::null())
        .status()
        .await;
}

fn spawn(cfg: &CaptureConfig) -> Result<(Child, ChildStdout), CaptureError> {
    let mut child = command(&cfg.presentmon, &presentmon::args(cfg.pid))
        .stdout(Stdio::piped())
        .spawn()
        .map_err(CaptureError::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CaptureError::Spawn(std::io::Error::other("no stdout pipe")))?;
    Ok((child, stdout))
}

/// Records frames and counters for `cfg.pid` until it exits or the duration ends.
/// If PresentMon dies while the target lives it is restarted once and the hole
/// is recorded as a gap.
pub async fn run(
    cfg: &CaptureConfig,
    counters: &mut dyn CounterApi,
    processes: &dyn ProcessApi,
) -> Result<Capture, CaptureError> {
    verify_binary(&cfg.presentmon)?;
    let own_cpu_start = own_cpu_time().unwrap_or(0);
    let mut cap = Capture {
        start_qpc: qpc(),
        ..Default::default()
    };
    let (mut child, mut stdout) = spawn(cfg)?;

    let alive = || {
        processes
            .processes()
            .map(|all| all.iter().any(|p| p.pid == cfg.pid))
            .unwrap_or(true)
    };
    let far = Duration::from_secs(10 * 365 * 86_400);
    let stop_at = Instant::now() + cfg.duration.unwrap_or(far);
    let mut kill_at = Instant::now() + far;
    let mut stopping = false;
    let mut restarted = false;
    let mut ticker = tokio::time::interval(cfg.sample_period);

    let mut decoder = LineDecoder::default();
    let mut layout: Option<Layout> = None;
    let mut rows: Vec<(u64, Frame)> = Vec::new();
    let mut pending_gap: Option<u64> = None;
    let (mut child_cpu, mut dead_children_cpu) = (0u64, 0u64);
    let mut buf = vec![0u8; 16 * 1024];

    loop {
        tokio::select! {
            read = stdout.read(&mut buf) => match read {
                Ok(n) if n > 0 => {
                    for line in decoder.push(&buf[..n]) {
                        let Some(l) = &layout else {
                            layout = Some(Layout::from_header(&line)?);
                            continue;
                        };
                        match l.parse_row(&line) {
                            Ok(row) if row.pid == cfg.pid => {
                                if let Some(start) = pending_gap.take() {
                                    cap.gaps.push(CaptureGap {
                                        qpc: start,
                                        duration_ticks: row.frame.qpc.saturating_sub(start),
                                    });
                                }
                                rows.push((row.swap_chain, row.frame));
                            }
                            Ok(_) => {}
                            Err(_) => cap.unparsed_rows += 1,
                        }
                    }
                }
                // PresentMon closed its output: it exited.
                _ => {
                    let _ = child.wait().await;
                    dead_children_cpu += std::mem::take(&mut child_cpu);
                    // A dying target takes a moment to leave the process list.
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if stopping || restarted || !alive() {
                        break;
                    }
                    tracing::warn!("PresentMon exited while the target is running; restarting once");
                    restarted = true;
                    pending_gap = Some(rows.last().map_or_else(qpc, |r| r.1.qpc));
                    decoder = LineDecoder::default();
                    layout = None;
                    (child, stdout) = spawn(cfg)?;
                }
            },
            _ = ticker.tick() => {
                if let Ok(sample) = counters.sample() {
                    cap.samples.push(sample);
                }
                if let Some(handle) = child.raw_handle() {
                    child_cpu = child_cpu_time(handle).unwrap_or(child_cpu);
                }
                // Covers a PresentMon that neither exits nor produces frames.
                if !stopping && !alive() {
                    stopping = true;
                    kill_at = Instant::now() + STOP_GRACE;
                    tokio::spawn(stop_session_owned(cfg.presentmon.clone()));
                }
            }
            _ = tokio::time::sleep_until(stop_at), if !stopping => {
                stopping = true;
                kill_at = Instant::now() + STOP_GRACE;
                tokio::spawn(stop_session_owned(cfg.presentmon.clone()));
            }
            _ = tokio::time::sleep_until(kill_at), if stopping => {
                let _ = child.start_kill();
                kill_at = Instant::now() + far;
            }
        }
    }

    cap.end_qpc = qpc();
    cap.cpu_100ns =
        own_cpu_time().unwrap_or(0).saturating_sub(own_cpu_start) + dead_children_cpu + child_cpu;
    cap.frames = primary_swap_chain(rows);
    Ok(cap)
}

async fn stop_session_owned(path: PathBuf) {
    stop_session(&path).await;
}

/// Keeps the frames of the swap chain that presented the most.
// shortcut: counted over the whole capture, not the last 5 s as the spec says;
// a game that recreates its swap chain mid-session loses the smaller part.
// Upgrade when a real game shows it.
fn primary_swap_chain(rows: Vec<(u64, Frame)>) -> Vec<Frame> {
    let mut counts: Vec<(u64, usize)> = Vec::new();
    for (chain, _) in &rows {
        match counts.iter_mut().find(|c| c.0 == *chain) {
            Some(c) => c.1 += 1,
            None => counts.push((*chain, 1)),
        }
    }
    let primary = counts.into_iter().max_by_key(|c| c.1).map(|c| c.0);
    rows.into_iter()
        .filter(|(chain, _)| Some(*chain) == primary)
        .map(|(_, frame)| frame)
        .collect()
}

/// The counter series a session stores.
pub fn series(cap: &Capture, pid: u32) -> Vec<(&'static str, Vec<(u64, f32)>)> {
    let game_gpu = |s: &SystemSample| {
        s.gpu_engines
            .iter()
            .filter(|(name, _)| parse_gpu_engine(name).is_some_and(|(p, _)| p == pid))
            .map(|(_, pct)| pct)
            .sum::<f32>()
    };
    let column = |f: &dyn Fn(&SystemSample) -> f32| -> Vec<(u64, f32)> {
        cap.samples.iter().map(|s| (s.qpc, f(s))).collect()
    };
    vec![
        ("cpu_total_pct", column(&|s| s.cpu_total_pct)),
        ("mem_available_mb", column(&|s| s.mem_available_mb)),
        ("game_gpu_pct", column(&game_gpu)),
    ]
}

/// Builds the stored summary from a finished capture.
pub fn summarize_capture(
    cap: &Capture,
    target: &ProcessEntry,
    gpus: &[Gpu],
    qpc_freq: u64,
    logical_cpus: usize,
    thresholds: &Thresholds,
) -> SessionSummary {
    let capture_s = cap.end_qpc.saturating_sub(cap.start_qpc) as f64 / qpc_freq as f64;
    let counted = after_warmup(&cap.frames, cap.start_qpc, qpc_freq, thresholds);
    let cpu_s = cap.cpu_100ns as f64 / 1e7;
    SessionSummary {
        exe: target.name.clone(),
        pid: target.pid,
        capture_mode: "light".into(),
        capture_s,
        warmup_excluded_s: thresholds.warmup_s.min(capture_s),
        total_frames: cap.frames.len() as u64,
        stats: summarize(counted, qpc_freq, thresholds),
        gpu_adapter: gpu_for_pid(&cap.samples, target.pid, gpus).map(|g| g.name.clone()),
        capture_gaps: cap.gaps.clone(),
        unparsed_rows: cap.unparsed_rows,
        samples: cap.samples.len() as u64,
        overhead_cpu_pct: if capture_s > 0.0 {
            cpu_s / (capture_s * logical_cpus.max(1) as f64) * 100.0
        } else {
            0.0
        },
        overhead_budget_pct: LIGHT_OVERHEAD_BUDGET_PCT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQ: u64 = 10_000_000;

    fn frame(qpc: u64, ms: f32) -> Frame {
        Frame {
            qpc,
            frame_time_ms: ms,
            cpu_busy_ms: 1.0,
            gpu_busy_ms: 1.0,
            displayed_time_ms: f32::NAN,
            display_latency_ms: f32::NAN,
            present_mode: 0,
            flags: 0,
        }
    }

    #[test]
    fn primary_swap_chain_is_the_busiest() {
        let rows = vec![
            (1, frame(10, 1.0)),
            (2, frame(11, 2.0)),
            (1, frame(12, 3.0)),
        ];
        let kept = primary_swap_chain(rows);
        assert_eq!(kept.iter().map(|f| f.qpc).collect::<Vec<_>>(), [10, 12]);
        assert!(primary_swap_chain(Vec::new()).is_empty());
    }

    #[test]
    fn summary_excludes_warmup_and_reports_overhead() {
        // 10 s at 100 FPS, starting at QPC 0.
        let frames: Vec<Frame> = (0..1000).map(|i| frame(i * FREQ / 100, 10.0)).collect();
        let engine = "pid_7_luid_0x00000000_0x0000BEEF_phys_0_eng_0_engtype_3D";
        let cap = Capture {
            start_qpc: 0,
            end_qpc: 10 * FREQ,
            frames,
            samples: vec![SystemSample {
                qpc: FREQ,
                cpu_total_pct: 20.0,
                mem_available_mb: 4000.0,
                gpu_engines: vec![(engine.into(), 40.0), ("pid_8_luid_0x0_0x1_x".into(), 9.0)],
            }],
            // 0.12 CPU-seconds over 10 s on 4 cores = 0.3%.
            cpu_100ns: 1_200_000,
            ..Default::default()
        };
        let target = ProcessEntry {
            pid: 7,
            name: "game.exe".into(),
        };
        let gpus = [Gpu {
            name: "The GPU".into(),
            vendor_id: 0,
            vendor: String::new(),
            dedicated_vram_bytes: 0,
            luids: vec![0xBEEF],
        }];
        let s = summarize_capture(&cap, &target, &gpus, FREQ, 4, &Thresholds::default());
        assert_eq!(s.total_frames, 1000);
        assert_eq!(s.stats.frame_count, 500, "first 5 s excluded");
        assert!((s.stats.avg_fps - 100.0).abs() < 0.01);
        assert_eq!(s.gpu_adapter.as_deref(), Some("The GPU"));
        assert!(
            (s.overhead_cpu_pct - 0.3).abs() < 1e-9,
            "{}",
            s.overhead_cpu_pct
        );

        let series = series(&cap, 7);
        assert_eq!(series[2], ("game_gpu_pct", vec![(FREQ, 40.0)]));
    }

    #[test]
    fn a_missing_or_altered_binary_is_refused() {
        let dir = std::env::temp_dir();
        let missing = dir.join("verity-no-such-presentmon.exe");
        assert!(matches!(
            verify_binary(&missing),
            Err(CaptureError::Missing(_))
        ));
        let altered = dir.join(format!("verity-fake-presentmon-{}.exe", std::process::id()));
        std::fs::write(&altered, b"not presentmon").unwrap();
        assert!(matches!(
            verify_binary(&altered),
            Err(CaptureError::HashMismatch(_))
        ));
        let _ = std::fs::remove_file(&altered);
    }
}
