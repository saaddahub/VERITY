//! Frame Story steps 1–5 and 7 (docs/07-frame-story.md): hitch groups,
//! coverage by system events, lift and significance, unexplained hitches.

use serde::{Deserialize, Serialize};
use verity_core::metrics::{detect_hitches, group_hitches, Frame, Thresholds};

use crate::stats::{binomial_upper_tail, holm};

/// One qualifying system event (already past the "counts as an event" bar).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoryEvent {
    pub qpc: u64,
    pub duration_ticks: u64,
    pub kind: String,
    /// What to blame: driver file, process image name, task name, ...
    pub subject: String,
    /// Kind-specific size (µs of DPC time, MB read, CPU cores, ...).
    pub magnitude: f64,
}

/// A stretch of time Frame Story must ignore (capture gap, game not in focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub start: u64,
    pub end: u64,
}

pub struct Input<'a> {
    pub frames: &'a [Frame],
    pub qpc_freq: u64,
    pub capture_start: u64,
    pub capture_end: u64,
    /// Capture gaps and non-foreground periods.
    pub excluded: &'a [Interval],
    pub events: &'a [StoryEvent],
    pub capture_mode: &'a str,
    pub thresholds: &'a Thresholds,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HitchGroupInfo {
    pub start_qpc: u64,
    pub end_qpc: u64,
    pub worst_ms: f32,
    /// Sum of frame time above the rolling median, in ms.
    pub excess_ms: f64,
    /// "cause" when a reported cause covers it, else the in-frame class.
    pub class: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Remedy {
    /// "guide", "throttle" or "doctor".
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cause {
    pub kind: String,
    pub subject: String,
    /// "likely" or "possible".
    pub label: String,
    pub k: u64,
    pub n: u64,
    pub lift: f64,
    /// Holm-adjusted p-value.
    pub p: f64,
    pub excess_share: f64,
    pub remedy: Remedy,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Unexplained {
    pub in_game_cpu: u64,
    pub in_game_gpu: u64,
    pub unknown: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Story {
    pub hitch_groups: u64,
    pub hitch_rate_per_min: f64,
    pub causes: Vec<Cause>,
    pub unexplained: Unexplained,
    pub capture_mode: String,
    pub caveats: Vec<String>,
    /// Every group with its class, for the timeline.
    pub groups: Vec<HitchGroupInfo>,
    /// Pairs tested before correction, so the UI can say how many were checked.
    pub pairs_tested: u64,
}

const MS: f64 = 1e-3;

/// How long after an event starts it can plausibly affect a frame (step 2).
pub fn influence_ticks(kind: &str, duration_ticks: u64, freq: u64) -> u64 {
    let s = |sec: f64| (sec * freq as f64) as u64;
    match kind {
        "dpc" | "isr" => duration_ticks + s(20.0 * MS),
        "hard_fault" => s(100.0 * MS),
        "disk_io" => duration_ticks + s(50.0 * MS),
        "proc_start" => s(1.0),
        "throttle" => duration_ticks.min(s(10.0)),
        "power_source" => s(3.0),
        "task" => s(5.0),
        "foreground" => s(500.0 * MS),
        // cpu_burst, gpu_other, scan, update, ctx_contention: the event itself.
        _ => duration_ticks,
    }
}

/// Merges overlapping intervals; input need not be sorted.
fn merge(mut v: Vec<Interval>) -> Vec<Interval> {
    v.sort_by_key(|i| i.start);
    let mut out: Vec<Interval> = Vec::with_capacity(v.len());
    for i in v {
        match out.last_mut() {
            Some(last) if i.start <= last.end => last.end = last.end.max(i.end),
            _ => out.push(i),
        }
    }
    out
}

/// Total length of `a` (merged) inside `b` (merged).
fn overlap_len(a: &[Interval], b: &[Interval]) -> u64 {
    let (mut i, mut j, mut total) = (0, 0, 0u64);
    while i < a.len() && j < b.len() {
        let lo = a[i].start.max(b[j].start);
        let hi = a[i].end.min(b[j].end);
        if hi > lo {
            total += hi - lo;
        }
        if a[i].end < b[j].end {
            i += 1;
        } else {
            j += 1;
        }
    }
    total
}

/// `[start, end)` minus every excluded interval.
fn eligible(start: u64, end: u64, excluded: &[Interval]) -> Vec<Interval> {
    let mut out = Vec::new();
    let mut cursor = start;
    for x in merge(excluded.to_vec()) {
        if x.end <= cursor {
            continue;
        }
        if x.start >= end {
            break;
        }
        if x.start > cursor {
            out.push(Interval {
                start: cursor,
                end: x.start,
            });
        }
        cursor = cursor.max(x.end);
    }
    if cursor < end {
        out.push(Interval { start: cursor, end });
    }
    out
}

fn inside(t: u64, set: &[Interval]) -> bool {
    let i = set.partition_point(|x| x.end <= t);
    set.get(i).is_some_and(|x| x.start <= t)
}

/// Rolling medians of CPU and GPU busy over the frames before each index,
/// the baseline a hitch frame's own busy times are compared with.
fn busy_baseline(frames: &[Frame], idx: usize, window: usize) -> (f32, f32) {
    let from = idx.saturating_sub(window);
    let mut cpu: Vec<f32> = frames[from..idx]
        .iter()
        .map(|f| f.cpu_busy_ms)
        .filter(|x| x.is_finite())
        .collect();
    let mut gpu: Vec<f32> = frames[from..idx]
        .iter()
        .map(|f| f.gpu_busy_ms)
        .filter(|x| x.is_finite())
        .collect();
    let med = |v: &mut Vec<f32>| {
        if v.is_empty() {
            return f32::NAN;
        }
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    (med(&mut cpu), med(&mut gpu))
}

/// Step 5: what the frame itself shows when no system event explains it.
fn in_frame_class(frame: &Frame, base: (f32, f32), t: &Thresholds) -> &'static str {
    let spike = |v: f32, m: f32| {
        if v.is_finite() && m.is_finite() && v > t.hitch_ratio * m && v > m + t.hitch_min_excess_ms
        {
            v / m.max(0.01)
        } else {
            0.0
        }
    };
    let (c, g) = (
        spike(frame.cpu_busy_ms, base.0),
        spike(frame.gpu_busy_ms, base.1),
    );
    match (c > 0.0, g > 0.0) {
        (false, false) => "unknown",
        (true, false) => "in_game_cpu",
        (false, true) => "in_game_gpu",
        (true, true) if c >= g => "in_game_cpu",
        _ => "in_game_gpu",
    }
}

pub fn remedy(kind: &str, subject: &str) -> Remedy {
    let r = |kind: &str, id: &str| Remedy {
        kind: kind.into(),
        id: id.into(),
    };
    match kind {
        "cpu_burst" | "disk_io" | "proc_start" => r("throttle", "background.process"),
        "gpu_other" => r("guide", "background.gpu"),
        "dpc" | "isr" => r("guide", &format!("driver.{}", driver_class(subject))),
        "scan" => r("guide", "schedule.defender"),
        "update" => r("guide", "schedule.windows_update"),
        "task" => r("guide", "schedule.task"),
        "throttle" => r("doctor", "D013"),
        "hard_fault" => r("doctor", "D021"),
        "power_source" => r("guide", "power.charger"),
        "foreground" => r("guide", "focus"),
        _ => r("guide", "general"),
    }
}

/// Device class from a driver file name, for the driver guide.
pub fn driver_class(file: &str) -> &'static str {
    let f = file.to_ascii_lowercase();
    let any = |names: &[&str]| names.iter().any(|n| f.starts_with(n));
    if any(&[
        "ndis", "tcpip", "netwtw", "netwbw", "rt640", "rtwlan", "e1d", "e1r", "athw", "qcamain",
        "mrvl", "bcmwl", "rtux", "killer", "wdiwifi", "vwifi", "netio",
    ]) {
        "network"
    } else if any(&["nvlddmkm", "amdkmdag", "atikmdag", "igdkmd", "dxgkrnl", "dxgmms", "basicdisplay"]) {
        "gpu"
    } else if any(&["storport", "stornvme", "storahci", "iastor", "nvme", "disk", "partmgr", "volmgr", "classpnp"]) {
        "storage"
    } else if any(&["usbxhci", "usbport", "usbhub", "ucx01000", "usbccgp", "hidusb", "hidclass"]) {
        "usb"
    } else if any(&["hdaudbus", "portcls", "ks.sys", "rtkvhd", "intcaudiobus", "audiokse"]) {
        "audio"
    } else if any(&["acpi", "intelppm", "amdppm", "processr"]) {
        "acpi"
    } else {
        "other"
    }
}

pub fn analyze(inp: &Input) -> Story {
    let freq = inp.qpc_freq.max(1);
    let t = inp.thresholds;
    let warm_end = inp.capture_start + (t.warmup_s * freq as f64) as u64;
    let elig = eligible(warm_end, inp.capture_end.max(warm_end), inp.excluded);
    let elig_ticks: u64 = elig.iter().map(|i| i.end - i.start).sum();

    // Step 1: hitch groups in eligible time, with their excess.
    let hitches = detect_hitches(inp.frames, t);
    let groups_raw: Vec<_> = group_hitches(&hitches, freq, t)
        .into_iter()
        .filter(|g| inside(g.start_qpc, &elig))
        .collect();
    let ticks_ms = |ms: f64| (ms / 1000.0 * freq as f64) as u64;
    let mut groups: Vec<(HitchGroupInfo, usize)> = groups_raw
        .iter()
        .map(|g| {
            let excess: f64 = hitches
                .iter()
                .filter(|h| h.qpc >= g.start_qpc && h.qpc < g.end_qpc)
                .map(|h| (h.frame_time_ms - h.median_ms) as f64)
                .sum();
            // The worst frame of the group, for the in-frame classification.
            let first = inp.frames.partition_point(|f| f.qpc < g.start_qpc);
            let last = inp.frames.partition_point(|f| f.qpc < g.end_qpc);
            let worst = (first..last.max(first + 1).min(inp.frames.len()))
                .max_by(|&a, &b| {
                    inp.frames[a]
                        .frame_time_ms
                        .total_cmp(&inp.frames[b].frame_time_ms)
                })
                .unwrap_or(first);
            (
                HitchGroupInfo {
                    start_qpc: g.start_qpc,
                    end_qpc: g.end_qpc,
                    worst_ms: g.worst_ms,
                    excess_ms: excess,
                    class: String::new(),
                },
                worst,
            )
        })
        .collect();
    let n = groups.len() as u64;
    let total_excess: f64 = groups.iter().map(|(g, _)| g.excess_ms).sum();

    // Steps 2–4 per (kind, subject).
    let mut pairs: Vec<(&str, &str)> = inp
        .events
        .iter()
        .map(|e| (e.kind.as_str(), e.subject.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();

    let (front, back) = if n > 0 {
        (
            groups.iter().map(|(g, _)| g.end_qpc - g.start_qpc).sum::<u64>() / n,
            groups
                .iter()
                .map(|(g, _)| ticks_ms(g.worst_ms as f64))
                .sum::<u64>()
                / n,
        )
    } else {
        (0, 0)
    };

    struct Tested<'a> {
        kind: &'a str,
        subject: &'a str,
        covered: Vec<usize>,
        lift: f64,
        p: f64,
    }
    let mut tested: Vec<Tested> = Vec::new();
    for (kind, subject) in pairs {
        let windows: Vec<Interval> = inp
            .events
            .iter()
            .filter(|e| e.kind == kind && e.subject == subject)
            .map(|e| Interval {
                start: e.qpc,
                end: e.qpc + influence_ticks(kind, e.duration_ticks, freq).max(1),
            })
            .collect();
        let windows = merge(windows);
        let covered: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(_, (g, _))| {
                let lo = g.start_qpc.saturating_sub(ticks_ms(g.worst_ms as f64));
                let hi = g.end_qpc;
                let i = windows.partition_point(|w| w.end < lo);
                windows.get(i).is_some_and(|w| w.start <= hi)
            })
            .map(|(i, _)| i)
            .collect();
        let widened = merge(
            windows
                .iter()
                .map(|w| Interval {
                    start: w.start.saturating_sub(front),
                    end: w.end + back,
                })
                .collect(),
        );
        let p0 = if elig_ticks > 0 {
            (overlap_len(&widened, &elig) as f64 / elig_ticks as f64).min(1.0)
        } else {
            0.0
        };
        let k = covered.len() as u64;
        let expected = n as f64 * p0;
        let lift = if expected > 0.0 { k as f64 / expected } else { 0.0 };
        let p = binomial_upper_tail(k, n, p0);
        tested.push(Tested {
            kind,
            subject,
            covered,
            lift,
            p,
        });
    }
    let adjusted = holm(&tested.iter().map(|x| x.p).collect::<Vec<_>>());

    let mut causes = Vec::new();
    let mut explained = vec![false; groups.len()];
    for (x, p) in tested.iter().zip(&adjusted) {
        let k = x.covered.len() as u64;
        let label = if k >= 3 && x.lift >= 3.0 && *p < 0.01 {
            "likely"
        } else if k >= 2 && x.lift >= 2.0 && *p < 0.05 {
            "possible"
        } else {
            continue;
        };
        let covered_excess: f64 = x.covered.iter().map(|&i| groups[i].0.excess_ms).sum();
        for &i in &x.covered {
            explained[i] = true;
        }
        causes.push(Cause {
            kind: x.kind.into(),
            subject: x.subject.into(),
            label: label.into(),
            k,
            n,
            lift: x.lift,
            p: *p,
            excess_share: if total_excess > 0.0 {
                covered_excess / total_excess
            } else {
                0.0
            },
            remedy: remedy(x.kind, x.subject),
        });
    }
    causes.sort_by(|a, b| {
        (a.label != "likely")
            .cmp(&(b.label != "likely"))
            .then(b.excess_share.total_cmp(&a.excess_share))
            .then(b.lift.total_cmp(&a.lift))
    });

    // Step 5.
    let mut unexplained = Unexplained::default();
    for (i, (g, worst)) in groups.iter_mut().enumerate() {
        if explained[i] {
            g.class = "cause".into();
            continue;
        }
        let class = match inp.frames.get(*worst) {
            Some(f) => in_frame_class(f, busy_baseline(inp.frames, *worst, t.median_window), t),
            None => "unknown",
        };
        match class {
            "in_game_cpu" => unexplained.in_game_cpu += 1,
            "in_game_gpu" => unexplained.in_game_gpu += 1,
            _ => unexplained.unknown += 1,
        }
        g.class = class.into();
    }

    let mut caveats = Vec::new();
    if tested
        .iter()
        .any(|x| x.kind == "cpu_burst" || x.kind == "gpu_other")
    {
        caveats.push("cpu_burst and gpu_other timing is sampled at 4 Hz".into());
    }
    if inp.capture_mode == "light" {
        caveats.push(
            "Light capture: no driver, disk or page-fault events; use Standard for those".into(),
        );
    }
    let minutes = elig_ticks as f64 / freq as f64 / 60.0;
    Story {
        hitch_groups: n,
        hitch_rate_per_min: if minutes > 0.0 { n as f64 / minutes } else { 0.0 },
        causes,
        unexplained,
        capture_mode: inp.capture_mode.into(),
        caveats,
        groups: groups.into_iter().map(|(g, _)| g).collect(),
        pairs_tested: tested.len() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQ: u64 = 10_000_000;

    fn sec(s: f64) -> u64 {
        (s * FREQ as f64) as u64
    }

    /// 60 FPS for `secs`, with 60 ms hitches at the given times. A hitch frame
    /// is CPU-heavy, GPU-heavy or neither per `kind`.
    fn frames(secs: f64, hitches: &[(f64, char)]) -> Vec<Frame> {
        let mut out = Vec::new();
        let mut t = 0.0;
        let mut next = hitches.iter().peekable();
        while t < secs {
            let (ft, cpu, gpu) = match next.peek() {
                Some((at, kind)) if t >= *at => {
                    next.next();
                    match kind {
                        'c' => (60.0, 55.0, 5.0),
                        'g' => (60.0, 5.0, 55.0),
                        _ => (60.0, 5.0, 5.0),
                    }
                }
                _ => (1000.0 / 60.0, 5.0, 5.0),
            };
            out.push(Frame {
                qpc: sec(t),
                frame_time_ms: ft,
                cpu_busy_ms: cpu,
                gpu_busy_ms: gpu,
                displayed_time_ms: f32::NAN,
                display_latency_ms: f32::NAN,
                present_mode: 0,
                flags: 0,
            });
            t += ft as f64 / 1000.0;
        }
        out
    }

    fn ev(at: f64, kind: &str, subject: &str, dur: f64) -> StoryEvent {
        StoryEvent {
            qpc: sec(at),
            duration_ticks: sec(dur),
            kind: kind.into(),
            subject: subject.into(),
            magnitude: 1.0,
        }
    }

    fn run(frames: &[Frame], secs: f64, events: &[StoryEvent], excluded: &[Interval]) -> Story {
        analyze(&Input {
            frames,
            qpc_freq: FREQ,
            capture_start: 0,
            capture_end: sec(secs),
            excluded,
            events,
            capture_mode: "standard",
            thresholds: &Thresholds::default(),
        })
    }

    /// The roadmap's planted cause: 20 hitches each at the moment a stressor
    /// starts. It must be "likely" with lift ≥ 3.
    #[test]
    fn planted_cause_is_likely() {
        let times: Vec<f64> = (0..20).map(|i| 10.0 + i as f64 * 5.5).collect();
        let hitches: Vec<(f64, char)> = times.iter().map(|&t| (t + 0.05, 'n')).collect();
        let f = frames(125.0, &hitches);
        let mut events: Vec<StoryEvent> = times
            .iter()
            .map(|&t| ev(t, "proc_start", "stress.exe", 0.0))
            .collect();
        // Background noise at random times, frequent enough to overlap some
        // hitches by chance: must not be reported.
        let mut rng = rand_like::Lcg(7);
        events.extend((0..250).map(|_| ev(rng.next() * 125.0, "dpc", "busy.sys", 0.0005)));
        let s = run(&f, 125.0, &events, &[]);
        assert_eq!(s.hitch_groups, 20);
        assert_eq!(s.causes.len(), 1, "{:#?}", s.causes);
        let c = &s.causes[0];
        assert_eq!((c.kind.as_str(), c.subject.as_str()), ("proc_start", "stress.exe"));
        assert_eq!(c.label, "likely");
        assert_eq!((c.k, c.n), (20, 20));
        assert!(c.lift >= 3.0, "lift {}", c.lift);
        assert!(c.p < 0.01);
        assert!((c.excess_share - 1.0).abs() < 1e-9);
        assert_eq!(c.remedy.kind, "throttle");
        assert_eq!(s.unexplained, Unexplained::default());
        assert!(s.groups.iter().all(|g| g.class == "cause"));
    }

    /// The roadmap's control: a decoy on a fixed period with independent
    /// random hitches is not reported.
    #[test]
    fn decoy_on_fixed_period_is_not_reported() {
        use rand_like::Lcg;
        let mut rng = Lcg(12345);
        for _trial in 0..20 {
            let mut times: Vec<f64> = (0..20).map(|_| 6.0 + rng.next() * 290.0).collect();
            times.sort_by(f64::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 0.6);
            let hitches: Vec<(f64, char)> = times.iter().map(|&t| (t, 'n')).collect();
            let f = frames(300.0, &hitches);
            let events: Vec<StoryEvent> = (0..30)
                .map(|i| ev(i as f64 * 10.0 + 3.0, "proc_start", "decoy.exe", 0.0))
                .collect();
            let s = run(&f, 300.0, &events, &[]);
            assert!(s.causes.is_empty(), "{:#?}", s.causes);
        }
    }

    #[test]
    fn unexplained_hitches_are_classified_by_the_frame() {
        let f = frames(
            60.0,
            &[(10.0, 'c'), (20.0, 'c'), (30.0, 'g'), (40.0, 'n'), (50.0, 'g')],
        );
        let s = run(&f, 60.0, &[], &[]);
        assert_eq!(s.hitch_groups, 5);
        assert_eq!(
            s.unexplained,
            Unexplained {
                in_game_cpu: 2,
                in_game_gpu: 2,
                unknown: 1
            }
        );
        assert!(s.causes.is_empty());
        assert_eq!(s.pairs_tested, 0);
    }

    #[test]
    fn warmup_and_excluded_time_are_ignored() {
        let f = frames(60.0, &[(2.0, 'n'), (20.0, 'n'), (40.0, 'n')]);
        let excluded = [Interval {
            start: sec(15.0),
            end: sec(25.0),
        }];
        let s = run(&f, 60.0, &[], &excluded);
        // 2 s is warm-up, 20 s is excluded: only the 40 s hitch counts.
        assert_eq!(s.hitch_groups, 1);
        // Eligible: 5–15 and 25–60 s = 45 s.
        assert!((s.hitch_rate_per_min - 60.0 / 45.0).abs() < 1e-6);
    }

    #[test]
    fn empty_inputs_do_not_panic() {
        let s = run(&[], 0.0, &[], &[]);
        assert_eq!(s.hitch_groups, 0);
        assert_eq!(s.hitch_rate_per_min, 0.0);
        let s = run(&[], 10.0, &[ev(1.0, "dpc", "x.sys", 0.001)], &[]);
        assert!(s.causes.is_empty());
    }

    #[test]
    fn coverage_window_matches_the_spec() {
        // A DPC 15 ms before the hitch frame (inside duration + 20 ms) covers it;
        // one 40 ms before does not.
        let hitches: Vec<(f64, char)> = (0..6).map(|i| (10.0 + i as f64 * 8.0, 'n')).collect();
        let f = frames(60.0, &hitches);
        let starts: Vec<u64> = analyze(&Input {
            frames: &f,
            qpc_freq: FREQ,
            capture_start: 0,
            capture_end: sec(60.0),
            excluded: &[],
            events: &[],
            capture_mode: "standard",
            thresholds: &Thresholds::default(),
        })
        .groups
        .iter()
        .map(|g| g.start_qpc)
        .collect();
        let near: Vec<StoryEvent> = starts
            .iter()
            .map(|&s| StoryEvent {
                qpc: s - sec(0.075), // 75 ms before: window 20 ms + reach-back of the 60 ms frame
                duration_ticks: 0,
                kind: "dpc".into(),
                subject: "near.sys".into(),
                magnitude: 600.0,
            })
            .collect();
        let far: Vec<StoryEvent> = starts
            .iter()
            .map(|&s| StoryEvent {
                qpc: s - sec(0.2),
                duration_ticks: 0,
                kind: "dpc".into(),
                subject: "far.sys".into(),
                magnitude: 600.0,
            })
            .collect();
        let events: Vec<StoryEvent> = near.into_iter().chain(far).collect();
        let s = run(&f, 60.0, &events, &[]);
        let names: Vec<&str> = s.causes.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(names, ["near.sys"]);
        assert_eq!(s.causes[0].remedy.id, "driver.other");
    }

    #[test]
    fn interval_helpers() {
        let iv = |a, b| Interval { start: a, end: b };
        assert_eq!(merge(vec![iv(5, 8), iv(1, 3), iv(2, 6)]), [iv(1, 8)]);
        assert_eq!(eligible(0, 10, &[iv(2, 4), iv(3, 5), iv(9, 20)]), [iv(0, 2), iv(5, 9)]);
        assert_eq!(overlap_len(&[iv(0, 10)], &[iv(2, 4), iv(8, 12)]), 4);
        assert!(inside(3, &[iv(0, 2), iv(3, 5)]));
        assert!(!inside(2, &[iv(0, 2), iv(3, 5)]));
        assert_eq!(driver_class("Netwtw10.sys"), "network");
        assert_eq!(driver_class("nvlddmkm.sys"), "gpu");
        assert_eq!(driver_class("stornvme.sys"), "storage");
    }

    /// A tiny seeded generator so the decoy test is reproducible without a dependency.
    mod rand_like {
        pub struct Lcg(pub u64);
        impl Lcg {
            pub fn next(&mut self) -> f64 {
                self.0 = self
                    .0
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (self.0 >> 11) as f64 / (1u64 << 53) as f64
            }
        }
    }
}
