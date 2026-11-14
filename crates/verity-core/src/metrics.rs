//! Derived frame metrics, defined once and used everywhere (docs/03-telemetry.md §6).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// One presented frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// CPU start of the frame, in QPC ticks.
    pub qpc: u64,
    /// CPU start to next CPU start.
    pub frame_time_ms: f32,
    pub cpu_busy_ms: f32,
    pub gpu_busy_ms: f32,
    /// NaN if the frame was not displayed.
    #[serde(with = "nan_as_null")]
    pub displayed_time_ms: f32,
    /// NaN if unknown.
    #[serde(with = "nan_as_null")]
    pub display_latency_ms: f32,
    pub present_mode: u8,
    pub flags: u8,
}

/// JSON has no NaN: write it as null and read null back as NaN.
mod nan_as_null {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &f32, s: S) -> Result<S::Ok, S::Error> {
        if v.is_nan() {
            s.serialize_none()
        } else {
            s.serialize_f32(*v)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
        Ok(Option::<f32>::deserialize(d)?.unwrap_or(f32::NAN))
    }
}

pub const FLAG_DROPPED: u8 = 1;
pub const FLAG_TEARING_ALLOWED: u8 = 2;

/// Spec thresholds; each becomes a config value with these defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// A hitch is longer than this multiple of the rolling median...
    pub hitch_ratio: f32,
    /// ...and longer than the rolling median plus this many milliseconds.
    pub hitch_min_excess_ms: f32,
    /// Frames in the rolling median.
    pub median_window: usize,
    /// Fewest previous frames needed before a frame can be judged.
    pub median_min_frames: usize,
    /// Hitches closer together than this are one group.
    pub group_gap_ms: f64,
    /// Ignored at the start of a capture.
    pub warmup_s: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            hitch_ratio: 2.0,
            hitch_min_excess_ms: 4.0,
            median_window: 120,
            median_min_frames: 20,
            group_gap_ms: 500.0,
            warmup_s: 5.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hitch {
    /// CPU start of the long frame.
    pub qpc: u64,
    pub frame_time_ms: f32,
    /// The rolling median it was judged against.
    pub median_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HitchGroup {
    pub start_qpc: u64,
    /// End of the last long frame in the group.
    pub end_qpc: u64,
    pub hitches: u32,
    pub worst_ms: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    pub frame_count: u64,
    /// Sum of the counted frame times, so warm-up and capture gaps are excluded.
    pub duration_s: f64,
    pub avg_fps: f64,
    pub low_1pct_fps: f64,
    pub low_01pct_fps: f64,
    pub pacing_spread_ms: f64,
    pub hitch_groups: Vec<HitchGroup>,
    pub hitch_rate_per_min: f64,
}

/// Drops frames inside the warm-up period after `capture_start_qpc`.
pub fn after_warmup<'a>(
    frames: &'a [Frame],
    capture_start_qpc: u64,
    qpc_freq: u64,
    t: &Thresholds,
) -> &'a [Frame] {
    let cutoff = capture_start_qpc + (t.warmup_s * qpc_freq as f64) as u64;
    &frames[frames.partition_point(|f| f.qpc < cutoff)..]
}

/// Nearest-rank percentile of an ascending slice; `p` in 0..=100.
pub fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return f32::NAN;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Frames longer than both `ratio × median` and `median + excess`, where the
/// median is over the previous `median_window` frames.
pub fn detect_hitches(frames: &[Frame], t: &Thresholds) -> Vec<Hitch> {
    let mut window: VecDeque<f32> = VecDeque::with_capacity(t.median_window + 1);
    let mut sorted: Vec<f32> = Vec::with_capacity(t.median_window + 1);
    let mut out = Vec::new();
    for f in frames {
        let ft = f.frame_time_ms;
        if !ft.is_finite() {
            continue;
        }
        if sorted.len() >= t.median_min_frames.max(1) {
            let median = sorted[sorted.len() / 2];
            if ft > t.hitch_ratio * median && ft > median + t.hitch_min_excess_ms {
                out.push(Hitch {
                    qpc: f.qpc,
                    frame_time_ms: ft,
                    median_ms: median,
                });
            }
        }
        window.push_back(ft);
        sorted.insert(sorted.partition_point(|&x| x < ft), ft);
        if window.len() > t.median_window {
            if let Some(old) = window.pop_front() {
                sorted.remove(sorted.partition_point(|&x| x < old));
            }
        }
    }
    out
}

/// Merges hitches less than `group_gap_ms` apart.
pub fn group_hitches(hitches: &[Hitch], qpc_freq: u64, t: &Thresholds) -> Vec<HitchGroup> {
    let ticks = |ms: f64| (ms / 1000.0 * qpc_freq as f64) as u64;
    let mut groups: Vec<HitchGroup> = Vec::new();
    for h in hitches {
        let end = h.qpc + ticks(h.frame_time_ms as f64);
        match groups.last_mut() {
            Some(g) if h.qpc < g.end_qpc + ticks(t.group_gap_ms) => {
                g.end_qpc = end;
                g.hitches += 1;
                g.worst_ms = g.worst_ms.max(h.frame_time_ms);
            }
            _ => groups.push(HitchGroup {
                start_qpc: h.qpc,
                end_qpc: end,
                hitches: 1,
                worst_ms: h.frame_time_ms,
            }),
        }
    }
    groups
}

/// All frame metrics for a slice of frames (warm-up already removed).
pub fn summarize(frames: &[Frame], qpc_freq: u64, t: &Thresholds) -> FrameStats {
    let mut times: Vec<f32> = frames
        .iter()
        .map(|f| f.frame_time_ms)
        .filter(|x| x.is_finite())
        .collect();
    let frame_count = times.len() as u64;
    let duration_s = times.iter().map(|&x| x as f64).sum::<f64>() / 1000.0;
    times.sort_by(f32::total_cmp);

    // Lows come from frame time, not displayed time: when a game's window is
    // hidden, most frames are never shown and the rest "stay on screen" for
    // seconds, which made a steady 60 FPS run report a 0.3 FPS low (M1 test).

    let fps = |ms: f32| if ms > 0.0 { 1000.0 / ms as f64 } else { 0.0 };
    let hitch_groups = group_hitches(&detect_hitches(frames, t), qpc_freq, t);
    FrameStats {
        frame_count,
        duration_s,
        avg_fps: if duration_s > 0.0 {
            frame_count as f64 / duration_s
        } else {
            0.0
        },
        low_1pct_fps: fps(percentile(&times, 99.0)),
        low_01pct_fps: fps(percentile(&times, 99.9)),
        pacing_spread_ms: (percentile(&times, 75.0) - percentile(&times, 25.0)) as f64,
        hitch_rate_per_min: if duration_s > 0.0 {
            hitch_groups.len() as f64 / (duration_s / 60.0)
        } else {
            0.0
        },
        hitch_groups,
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
            cpu_busy_ms: ms,
            gpu_busy_ms: 1.0,
            displayed_time_ms: f32::NAN,
            display_latency_ms: f32::NAN,
            present_mode: 0,
            flags: 0,
        }
    }

    /// Back-to-back frames with the given times.
    fn run(times: &[f32]) -> Vec<Frame> {
        let mut qpc = 0u64;
        times
            .iter()
            .map(|&ms| {
                let f = frame(qpc, ms);
                qpc += (ms as f64 / 1000.0 * FREQ as f64) as u64;
                f
            })
            .collect()
    }

    fn steady_with(hitches: &[(usize, f32)]) -> Vec<f32> {
        let mut v = vec![16.7f32; 600];
        for &(i, ms) in hitches {
            v[i] = ms;
        }
        v
    }

    #[test]
    fn percentile_is_nearest_rank() {
        let v: Vec<f32> = (1..=100).map(|x| x as f32).collect();
        assert_eq!(percentile(&v, 99.0), 99.0);
        assert_eq!(percentile(&v, 99.9), 100.0);
        assert_eq!(percentile(&v, 50.0), 50.0);
        assert!(percentile(&[], 50.0).is_nan());
    }

    #[test]
    fn hitch_needs_both_conditions() {
        let t = Thresholds::default();
        // 66.7 ms against a 16.7 ms median: a hitch.
        let h = detect_hitches(&run(&steady_with(&[(300, 66.7)])), &t);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].frame_time_ms, 66.7);
        // Near miss on the ratio: 33.0 < 2 x 16.7.
        assert!(detect_hitches(&run(&steady_with(&[(300, 33.0)])), &t).is_empty());
        // Near miss on the excess at high FPS: 5 ms is 2.5x a 2 ms median but only +3 ms.
        let mut fast = vec![2.0f32; 600];
        fast[300] = 5.0;
        assert!(detect_hitches(&run(&fast), &t).is_empty());
    }

    #[test]
    fn no_verdict_without_history_and_nan_is_skipped() {
        let t = Thresholds::default();
        let mut early = vec![16.7f32; 100];
        early[5] = 80.0; // only 5 previous frames
        early[50] = f32::NAN;
        assert!(detect_hitches(&run(&early), &t).is_empty());
    }

    #[test]
    fn a_sustained_slowdown_stops_counting_once_it_is_the_norm() {
        let t = Thresholds::default();
        let mut v = vec![10.0f32; 200];
        v.extend(vec![40.0f32; 300]);
        let h = detect_hitches(&run(&v), &t);
        // Flagged until the slow frames dominate the 120-frame window, then not.
        assert!(!h.is_empty() && h.len() <= 61, "got {}", h.len());
    }

    #[test]
    fn close_hitches_merge_into_one_group() {
        let t = Thresholds::default();
        // Two hitches 10 frames (~167 ms) apart, a third far later.
        let frames = run(&steady_with(&[(200, 60.0), (210, 70.0), (500, 60.0)]));
        let groups = group_hitches(&detect_hitches(&frames, &t), FREQ, &t);
        assert_eq!(groups.len(), 2);
        assert_eq!((groups[0].hitches, groups[0].worst_ms), (2, 70.0));
        assert_eq!(groups[1].hitches, 1);
    }

    #[test]
    fn summary_numbers() {
        let t = Thresholds::default();
        let frames = run(&vec![1000.0 / 60.0; 3600]);
        let s = summarize(&frames, FREQ, &t);
        assert_eq!(s.frame_count, 3600);
        assert!((s.avg_fps - 60.0).abs() < 0.01, "{}", s.avg_fps);
        assert!((s.low_1pct_fps - 60.0).abs() < 0.01);
        assert!(s.pacing_spread_ms.abs() < 1e-6);
        assert!(s.hitch_groups.is_empty());

        let empty = summarize(&[], FREQ, &t);
        assert_eq!((empty.frame_count, empty.avg_fps), (0, 0.0));
    }

    #[test]
    fn lows_ignore_displayed_time() {
        // A hidden window: almost nothing is displayed, and what is shown stays
        // up for seconds. The lows must still describe the frames rendered.
        let t = Thresholds::default();
        let mut frames = run(&vec![10.0; 200]);
        for (i, f) in frames.iter_mut().enumerate() {
            f.displayed_time_ms = if i % 50 == 0 { 3000.0 } else { f32::NAN };
        }
        let s = summarize(&frames, FREQ, &t);
        assert!((s.low_1pct_fps - 100.0).abs() < 0.01, "{}", s.low_1pct_fps);
    }

    #[test]
    fn warmup_frames_are_dropped() {
        let t = Thresholds::default();
        let frames = run(&vec![100.0; 100]); // 10 s of frames
        let kept = after_warmup(&frames, 0, FREQ, &t);
        assert_eq!(kept.len(), 50);
        assert_eq!(kept[0].qpc, 5 * FREQ);
    }
}
