//! Capture-side types and OS traits (docs/03-telemetry.md).

use serde::{Deserialize, Serialize};

use crate::metrics::FrameStats;
use crate::{Gpu, OsError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    /// Executable file name, e.g. `game.exe`.
    pub name: String,
}

/// Lists running processes without opening any of them.
pub trait ProcessApi: Send + Sync {
    fn processes(&self) -> Result<Vec<ProcessEntry>, OsError>;
}

/// One reading of the system counters.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemSample {
    pub qpc: u64,
    pub cpu_total_pct: f32,
    pub mem_available_mb: f32,
    /// (`GPU Engine` instance name, utilization percent).
    pub gpu_engines: Vec<(String, f32)>,
}

pub trait CounterApi: Send {
    fn sample(&mut self) -> Result<SystemSample, OsError>;
}

/// Picks the process to capture from a pid or an executable name.
pub fn resolve_target(
    processes: &[ProcessEntry],
    pid: Option<u32>,
    exe: Option<&str>,
) -> Result<ProcessEntry, String> {
    match (pid, exe) {
        (Some(pid), _) => processes
            .iter()
            .find(|p| p.pid == pid)
            .cloned()
            .ok_or_else(|| format!("no running process has pid {pid}")),
        (None, Some(exe)) => {
            let hits: Vec<&ProcessEntry> = processes
                .iter()
                .filter(|p| p.name.eq_ignore_ascii_case(exe))
                .collect();
            match hits.as_slice() {
                [one] => Ok((*one).clone()),
                [] => Err(format!("no running process is named {exe}")),
                many => Err(format!(
                    "{} processes are named {exe} (pids {}); pass --pid",
                    many.len(),
                    many.iter()
                        .map(|p| p.pid.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        (None, None) => Err("pass a pid or an executable name".into()),
    }
}

/// Parses `pid_<pid>_luid_0x<high>_0x<low>_phys_..._engtype_...` into (pid, luid).
pub fn parse_gpu_engine(instance: &str) -> Option<(u32, u64)> {
    let mut parts = instance.split('_');
    let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok();
    if parts.next()? != "pid" {
        return None;
    }
    let pid = parts.next()?.parse().ok()?;
    if parts.next()? != "luid" {
        return None;
    }
    let (high, low) = (hex(parts.next()?)?, hex(parts.next()?)?);
    Some((pid, high << 32 | low))
}

/// The adapter whose engines did the most work for `pid` over the samples:
/// which GPU the game rendered on, learned without touching the game.
pub fn gpu_for_pid<'a>(samples: &[SystemSample], pid: u32, gpus: &'a [Gpu]) -> Option<&'a Gpu> {
    let mut totals: Vec<(u64, f32)> = Vec::new();
    for (instance, pct) in samples.iter().flat_map(|s| &s.gpu_engines) {
        if let Some((p, luid)) = parse_gpu_engine(instance) {
            if p == pid && *pct > 0.0 {
                match totals.iter_mut().find(|(l, _)| *l == luid) {
                    Some(t) => t.1 += pct,
                    None => totals.push((luid, *pct)),
                }
            }
        }
    }
    let (luid, _) = totals.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    gpus.iter().find(|g| g.luids.contains(&luid))
}

/// A stretch with no frame data because the frame source was down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureGap {
    pub qpc: u64,
    pub duration_ticks: u64,
}

/// Stored as `summary_json`; what lists and comparisons read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub exe: String,
    pub pid: u32,
    pub capture_mode: String,
    /// Wall-clock length of the capture, warm-up included.
    pub capture_s: f64,
    pub warmup_excluded_s: f64,
    /// All frames stored, warm-up included.
    pub total_frames: u64,
    pub stats: FrameStats,
    /// None when the game did no measurable GPU work.
    pub gpu_adapter: Option<String>,
    pub capture_gaps: Vec<CaptureGap>,
    pub unparsed_rows: u64,
    pub samples: u64,
    /// Verity's own CPU use (service plus frame source) as a share of all cores.
    pub overhead_cpu_pct: f64,
    pub overhead_budget_pct: f64,
}

/// One qualifying system event for Frame Story (docs/07-frame-story.md step 2),
/// stored as an `events` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SysEvent {
    pub qpc: u64,
    pub duration_ticks: u64,
    pub kind: String,
    /// What to blame: driver file, process image name, task name, ...
    pub subject: String,
    /// Kind-specific size (µs of DPC time, MB moved, CPU cores, ...).
    pub magnitude: f64,
}

/// Light-mode budget from docs/03-telemetry.md.
pub const LIGHT_OVERHEAD_BUDGET_PCT: f64 = 0.3;

#[cfg(test)]
mod tests {
    use super::*;

    fn procs() -> Vec<ProcessEntry> {
        [
            (10, "a.exe"),
            (11, "Game.exe"),
            (12, "dup.exe"),
            (13, "dup.exe"),
        ]
        .map(|(pid, name)| ProcessEntry {
            pid,
            name: name.into(),
        })
        .to_vec()
    }

    #[test]
    fn target_by_pid_or_name() {
        let p = procs();
        assert_eq!(resolve_target(&p, Some(10), None).unwrap().name, "a.exe");
        assert_eq!(resolve_target(&p, None, Some("game.EXE")).unwrap().pid, 11);
        assert!(resolve_target(&p, Some(99), None)
            .unwrap_err()
            .contains("99"));
        assert!(resolve_target(&p, None, Some("nope.exe")).is_err());
        let dup = resolve_target(&p, None, Some("dup.exe")).unwrap_err();
        assert!(dup.contains("12, 13"), "{dup}");
        assert!(resolve_target(&p, None, None).is_err());
    }

    #[test]
    fn gpu_engine_names() {
        let name = "pid_4242_luid_0x00000001_0x0000D3A5_phys_0_eng_0_engtype_3D";
        assert_eq!(parse_gpu_engine(name), Some((4242, 0x1_0000_D3A5)));
        assert_eq!(parse_gpu_engine("pid_x_luid_0x0_0x1"), None);
        assert_eq!(parse_gpu_engine("_Total"), None);
    }

    #[test]
    fn game_gpu_is_the_one_doing_its_work() {
        let gpu = |name: &str, luids: &[u64]| Gpu {
            name: name.into(),
            vendor_id: 0,
            vendor: String::new(),
            dedicated_vram_bytes: 0,
            luids: luids.to_vec(),
        };
        let gpus = [
            gpu("discrete", &[0xAAAA]),
            gpu("integrated", &[0xBBBB, 0xCCCC]),
        ];
        let sample = |engines: &[(&str, f32)]| SystemSample {
            qpc: 0,
            cpu_total_pct: 0.0,
            mem_available_mb: 0.0,
            gpu_engines: engines.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        };
        let samples = [
            sample(&[
                (
                    "pid_7_luid_0x00000000_0x0000CCCC_phys_0_eng_0_engtype_3D",
                    30.0,
                ),
                (
                    "pid_7_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D",
                    2.0,
                ),
                // Another process hammering the discrete GPU must not count.
                (
                    "pid_8_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D",
                    99.0,
                ),
            ]),
            sample(&[(
                "pid_7_luid_0x00000000_0x0000CCCC_phys_0_eng_0_engtype_3D",
                25.0,
            )]),
        ];
        assert_eq!(gpu_for_pid(&samples, 7, &gpus).unwrap().name, "integrated");
        // Missing data: no engine activity for the pid.
        assert!(gpu_for_pid(&samples, 9, &gpus).is_none());
        assert!(gpu_for_pid(&[], 7, &gpus).is_none());
    }
}
