//! Frame Story events that come from samples and event logs rather than the
//! kernel trace (docs/07-frame-story.md step 2). Pure functions.

use std::collections::HashMap;

use verity_core::telemetry::{parse_gpu_engine, SysEvent};
use verity_sys::story_sampler::StorySample;

/// Sample period of the Standard-mode sampler.
pub const PERIOD_MS: u64 = 250;
/// A non-game process above this many cores in one window is a CPU burst.
const BURST_CORES: f32 = 1.0;
/// Another process above this share of the game's GPU engine.
const GPU_OTHER_PCT: f32 = 20.0;
/// A performance-limit drop this large is a throttle event.
const THROTTLE_POINTS: f32 = 10.0;
const THROTTLE_MAX_S: u64 = 10;

/// Processes never blamed for CPU: idle accounting and Verity itself.
fn ignored(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "idle" | "_total" | "system idle process" | "verity-service" | "presentmon"
    )
}

/// `name:pid` (Process V2 instance) → (name, pid).
fn split_instance(inst: &str) -> Option<(&str, u32)> {
    let (name, pid) = inst.rsplit_once(':')?;
    Some((name, pid.parse().ok()?))
}

/// Appends `ev` or extends the same subject's event that ended where this one starts.
fn push_merged(out: &mut Vec<SysEvent>, open: &mut HashMap<String, usize>, ev: SysEvent) {
    if let Some(&i) = open.get(&ev.subject) {
        let prev = &mut out[i];
        if prev.qpc + prev.duration_ticks >= ev.qpc {
            prev.duration_ticks = ev.qpc + ev.duration_ticks - prev.qpc;
            prev.magnitude = prev.magnitude.max(ev.magnitude);
            return;
        }
    }
    open.insert(ev.subject.clone(), out.len());
    out.push(ev);
}

/// `cpu_burst`, `gpu_other`, `throttle` and `power_source` from 4 Hz samples.
/// `names` maps pid to image name for GPU engine instances (which carry pids only).
pub fn from_samples(
    samples: &[StorySample],
    game_pid: u32,
    freq: u64,
    names: &HashMap<u32, String>,
) -> Vec<SysEvent> {
    let period = PERIOD_MS * freq / 1000;
    let mut out = Vec::new();
    let mut open_cpu = HashMap::new();
    let mut open_gpu = HashMap::new();

    for s in samples {
        let start = s.qpc.saturating_sub(period);
        for (inst, pct) in &s.proc_cpu {
            let Some((name, pid)) = split_instance(inst) else {
                continue;
            };
            let cores = pct / 100.0;
            if pid == game_pid || pid == 0 || ignored(name) || cores <= BURST_CORES {
                continue;
            }
            let subject = format!("{name}.exe");
            push_merged(
                &mut out,
                &mut open_cpu,
                SysEvent {
                    qpc: start,
                    duration_ticks: period,
                    kind: "cpu_burst".into(),
                    subject,
                    magnitude: cores as f64,
                },
            );
        }

        // The game's busiest engine (adapter + engine index), then anyone else on it.
        let engine_key = |inst: &str| {
            let at = inst.find("_luid_")?;
            let tail = &inst[at..];
            let eng = tail.find("_eng_")?;
            let end = tail[eng + 5..]
                .find('_')
                .map_or(tail.len(), |e| eng + 5 + e);
            Some(tail[..end].to_string())
        };
        let game_engine = s
            .gpu_engines
            .iter()
            .filter(|(i, _)| parse_gpu_engine(i).is_some_and(|(p, _)| p == game_pid))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .and_then(|(i, _)| engine_key(i));
        if let Some(key) = game_engine {
            let mut per_pid: HashMap<u32, f32> = HashMap::new();
            for (inst, pct) in &s.gpu_engines {
                if let Some((pid, _)) = parse_gpu_engine(inst) {
                    if pid != game_pid && engine_key(inst).as_deref() == Some(key.as_str()) {
                        *per_pid.entry(pid).or_default() += pct;
                    }
                }
            }
            for (pid, pct) in per_pid {
                if pct > GPU_OTHER_PCT {
                    let subject = names
                        .get(&pid)
                        .cloned()
                        .unwrap_or_else(|| format!("pid {pid}"));
                    push_merged(
                        &mut out,
                        &mut open_gpu,
                        SysEvent {
                            qpc: start,
                            duration_ticks: period,
                            kind: "gpu_other".into(),
                            subject,
                            magnitude: pct as f64,
                        },
                    );
                }
            }
        }
    }

    // Throttle: limit falls ≥ 10 points below the best seen so far, until it recovers.
    let mut best = f32::NAN;
    let mut open: Option<(u64, f32)> = None;
    let max = THROTTLE_MAX_S * freq;
    let close = |out: &mut Vec<SysEvent>, start: u64, end: u64, drop: f32| {
        out.push(SysEvent {
            qpc: start,
            duration_ticks: (end - start).min(max),
            kind: "throttle".into(),
            subject: "CPU power/thermal limit".into(),
            magnitude: drop as f64,
        });
    };
    for s in samples {
        let v = s.perf_limit_pct;
        if !v.is_finite() {
            continue;
        }
        if !best.is_finite() || v > best {
            best = v;
        }
        let low = v <= best - THROTTLE_POINTS;
        match (&mut open, low) {
            (None, true) => open = Some((s.qpc, best - v)),
            (Some((_, d)), true) => *d = d.max(best - v),
            (Some((start, d)), false) => {
                close(&mut out, *start, s.qpc, *d);
                open = None;
            }
            (None, false) => {}
        }
    }
    if let (Some((start, d)), Some(last)) = (open, samples.last()) {
        close(&mut out, start, last.qpc, d);
    }

    // Power source changes.
    for w in samples.windows(2) {
        if let (Some(a), Some(b)) = (w[0].on_ac, w[1].on_ac) {
            if a != b {
                out.push(SysEvent {
                    qpc: w[1].qpc,
                    duration_ticks: 0,
                    kind: "power_source".into(),
                    subject: if b { "battery to AC" } else { "AC to battery" }.into(),
                    magnitude: 1.0,
                });
            }
        }
    }
    out.sort_by_key(|e| e.qpc);
    out
}

/// Fixed event-log channels Verity watches.
pub const DEFENDER: &str = "Microsoft-Windows-Windows Defender/Operational";
pub const UPDATE: &str = "Microsoft-Windows-WindowsUpdateClient/Operational";
pub const TASKS: &str = "Microsoft-Windows-TaskScheduler/Operational";

/// One parsed event-log record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub channel: &'static str,
    pub id: u32,
    /// FILETIME of TimeCreated.
    pub filetime: u64,
    pub task_name: Option<String>,
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    let b = s[a..].find(close)? + a;
    Some(&s[a..b])
}

/// Parses the rendered XML of one event.
pub fn parse_log_xml(channel: &'static str, xml: &str) -> Option<LogRecord> {
    let id_text = between(xml, "<EventID", "</EventID>")?;
    let id = id_text.rsplit('>').next()?.trim().parse().ok()?;
    let time = between(xml, "SystemTime='", "'").or_else(|| between(xml, "SystemTime=\"", "\""))?;
    let filetime = iso_to_filetime(time)?;
    let task_name = between(xml, "<Data Name='TaskName'>", "</Data>")
        .or_else(|| between(xml, "<Data Name=\"TaskName\">", "</Data>"))
        .map(|t| t.trim_start_matches('\\').to_string());
    Some(LogRecord {
        channel,
        id,
        filetime,
        task_name,
    })
}

/// `2026-11-17T12:34:56.1234567Z` → FILETIME (100 ns since 1601, UTC).
pub fn iso_to_filetime(s: &str) -> Option<u64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant), relative to 1970-01-01.
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    let digits: String = frac.chars().take(7).collect();
    let ticks_frac: i64 = if digits.is_empty() {
        0
    } else {
        digits.parse::<i64>().ok()? * 10i64.pow(7 - digits.len() as u32)
    };
    // 1601 → 1970 is 11 644 473 600 s.
    u64::try_from((secs + 11_644_473_600) * 10_000_000 + ticks_frac).ok()
}

/// Maps a FILETIME to QPC ticks using one (qpc, filetime) pair read together.
pub fn filetime_to_qpc(ft: u64, anchor_qpc: u64, anchor_ft: u64, freq: u64) -> u64 {
    let delta = ft as i128 - anchor_ft as i128; // 100 ns units
    let q = anchor_qpc as i128 + delta * freq as i128 / 10_000_000;
    q.max(0) as u64
}

/// Pairs start/end records into `scan`, `update` and `task` events. A start
/// with no end runs to `end_ft`. Times stay FILETIMEs; convert afterwards.
pub fn from_log(records: &[LogRecord], end_ft: u64) -> Vec<(u64, u64, &'static str, String)> {
    let mut out = Vec::new();
    let mut scan: Option<u64> = None;
    let mut download: Option<u64> = None;
    let mut install: Option<u64> = None;
    for r in records {
        match (r.channel, r.id) {
            (DEFENDER, 1000) => scan = Some(r.filetime),
            (DEFENDER, 1001 | 1002 | 1005) => {
                if let Some(s) = scan.take() {
                    out.push((s, r.filetime, "scan", "Defender scan".to_string()));
                }
            }
            (UPDATE, 44) => download = Some(r.filetime),
            (UPDATE, 41 | 31) => {
                let s = download.take().unwrap_or(r.filetime);
                out.push((
                    s,
                    r.filetime,
                    "update",
                    "Windows Update download".to_string(),
                ));
            }
            (UPDATE, 43) => install = Some(r.filetime),
            (UPDATE, 19 | 20) => {
                let s = install.take().unwrap_or(r.filetime);
                out.push((
                    s,
                    r.filetime,
                    "update",
                    "Windows Update install".to_string(),
                ));
            }
            (TASKS, 100) => out.push((
                r.filetime,
                r.filetime,
                "task",
                r.task_name
                    .clone()
                    .unwrap_or_else(|| "scheduled task".into()),
            )),
            _ => {}
        }
    }
    if let Some(s) = scan {
        out.push((s, end_ft.max(s), "scan", "Defender scan".into()));
    }
    if let Some(s) = download {
        out.push((s, end_ft.max(s), "update", "Windows Update download".into()));
    }
    if let Some(s) = install {
        out.push((s, end_ft.max(s), "update", "Windows Update install".into()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQ: u64 = 10_000_000;

    fn sample(
        t: f64,
        cpu: &[(&str, f32)],
        gpu: &[(&str, f32)],
        limit: f32,
        ac: Option<bool>,
    ) -> StorySample {
        StorySample {
            qpc: (t * FREQ as f64) as u64,
            proc_cpu: cpu.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            gpu_engines: gpu.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            perf_limit_pct: limit,
            on_ac: ac,
        }
    }

    #[test]
    fn cpu_bursts_merge_and_skip_the_game() {
        let s = [
            sample(
                1.0,
                &[("stress:50", 180.0), ("game:7", 400.0), ("Idle:0", 700.0)],
                &[],
                f32::NAN,
                None,
            ),
            sample(1.25, &[("stress:50", 150.0)], &[], f32::NAN, None),
            sample(1.5, &[("stress:50", 50.0)], &[], f32::NAN, None),
            sample(1.75, &[("stress:50", 120.0)], &[], f32::NAN, None),
        ];
        let ev = from_samples(&s, 7, FREQ, &HashMap::new());
        let got: Vec<(u64, u64, &str)> = ev
            .iter()
            .map(|e| (e.qpc, e.duration_ticks, e.subject.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (7_500_000, 5_000_000, "stress.exe"), // 0.75–1.25 s, merged
                (15_000_000, 2_500_000, "stress.exe"),
            ]
        );
        assert!((ev[0].magnitude - 1.8).abs() < 1e-6);
    }

    #[test]
    fn gpu_other_only_counts_the_games_engine() {
        let g = |pid: u32, eng: u32, v: f32| {
            (
                format!("pid_{pid}_luid_0x00000000_0x0000AAAA_phys_0_eng_{eng}_engtype_3D"),
                v,
            )
        };
        let engines = [g(7, 0, 60.0), g(9, 0, 30.0), g(8, 3, 90.0), g(10, 0, 5.0)];
        let refs: Vec<(&str, f32)> = engines.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        let s = [sample(2.0, &[], &refs, f32::NAN, None)];
        let names = HashMap::from([(9, "encoder.exe".to_string())]);
        let ev = from_samples(&s, 7, FREQ, &names);
        assert_eq!(ev.len(), 1);
        assert_eq!(
            (ev[0].kind.as_str(), ev[0].subject.as_str()),
            ("gpu_other", "encoder.exe")
        );
    }

    #[test]
    fn throttle_and_power_source() {
        let s = [
            sample(0.0, &[], &[], 100.0, Some(true)),
            sample(1.0, &[], &[], 95.0, Some(true)),
            sample(2.0, &[], &[], 85.0, Some(false)),
            sample(3.0, &[], &[], 70.0, Some(false)),
            sample(4.0, &[], &[], 99.0, Some(false)),
            sample(5.0, &[], &[], f32::NAN, None),
        ];
        let ev = from_samples(&s, 7, FREQ, &HashMap::new());
        let got: Vec<(&str, u64, u64)> = ev
            .iter()
            .map(|e| (e.kind.as_str(), e.qpc, e.duration_ticks))
            .collect();
        assert_eq!(
            got,
            [
                ("throttle", 20_000_000, 20_000_000),
                ("power_source", 20_000_000, 0),
            ]
        );
        assert!((ev[0].magnitude - 30.0).abs() < 1e-6);
        assert_eq!(ev[1].subject, "AC to battery");
    }

    #[test]
    fn log_xml_and_pairing() {
        let xml = "<Event><System><Provider Name='x'/><EventID>1000</EventID>\
                   <TimeCreated SystemTime='2026-11-17T12:00:00.5000000Z'/></System></Event>";
        let r = parse_log_xml(DEFENDER, xml).unwrap();
        assert_eq!(r.id, 1000);
        assert_eq!(
            r.filetime,
            iso_to_filetime("2026-11-17T12:00:00.5Z").unwrap()
        );
        let task = "<Event><System><EventID Qualifiers='0'>100</EventID>\
                    <TimeCreated SystemTime=\"2026-11-17T12:00:01Z\"/></System>\
                    <EventData><Data Name='TaskName'>\\Microsoft\\Foo</Data></EventData></Event>";
        let t = parse_log_xml(TASKS, task).unwrap();
        assert_eq!(t.task_name.as_deref(), Some("Microsoft\\Foo"));
        assert!(parse_log_xml(DEFENDER, "<Event/>").is_none());

        let end = r.filetime + 100;
        let recs = [
            r.clone(),
            LogRecord {
                id: 1001,
                filetime: r.filetime + 50,
                ..r.clone()
            },
            t,
            LogRecord {
                channel: UPDATE,
                id: 44,
                filetime: r.filetime + 60,
                task_name: None,
            },
        ];
        let ev = from_log(&recs, end);
        let kinds: Vec<(&str, u64, u64)> = ev
            .iter()
            .map(|(s, e, k, _)| (*k, e - s, *s - r.filetime))
            .collect();
        // Scan closed by 1001; task is instant; the download is still open at the end.
        assert_eq!(
            kinds,
            [("scan", 50, 0), ("task", 0, 5_000_000), ("update", 40, 60)]
        );
    }

    #[test]
    fn iso_and_anchor_conversion() {
        // Unix epoch in FILETIME.
        assert_eq!(
            iso_to_filetime("1970-01-01T00:00:00Z"),
            Some(116_444_736_000_000_000)
        );
        assert_eq!(
            iso_to_filetime("2000-02-29T00:00:00.1Z"),
            Some(125_962_560_001_000_000)
        );
        assert_eq!(iso_to_filetime("2026-13-01T00:00:00Z"), None);
        assert_eq!(iso_to_filetime("garbage"), None);
        assert_eq!(filetime_to_qpc(1_000, 500, 1_000, FREQ), 500);
        assert_eq!(filetime_to_qpc(11_000, 500, 1_000, FREQ), 10_500);
        assert_eq!(filetime_to_qpc(0, 10, 1_000_000, FREQ), 0);
    }
}
