//! The shareable report (`verity story export --html`): one self-contained
//! file with the summary, the causes and a static timeline. Process and driver
//! names only; every name is reduced to its file name, so no folder (and so no
//! user name) can appear.

use std::fmt::Write;

use verity_core::metrics::Frame;

use crate::analyze::{Story, StoryEvent};

pub struct Report<'a> {
    /// The game's exe; reduced to a file name here.
    pub exe: &'a str,
    pub started_utc: &'a str,
    pub avg_fps: f64,
    pub low_1pct_fps: f64,
    pub story: &'a Story,
    pub frames: &'a [Frame],
    pub events: &'a [StoryEvent],
    pub qpc_freq: u64,
    pub capture_start: u64,
    pub capture_end: u64,
}

/// File name only, so a full path never leaks into the report.
pub fn file_name(s: &str) -> &str {
    s.rsplit(['\\', '/']).next().unwrap_or(s)
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Plain words for each event kind.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "dpc" => "Driver work (DPC)",
        "isr" => "Driver interrupt (ISR)",
        "hard_fault" => "Memory read from disk",
        "disk_io" => "Disk activity",
        "cpu_burst" => "Background CPU use",
        "proc_start" => "Program started",
        "throttle" => "CPU slowed (power or heat)",
        "power_source" => "Charger plugged or unplugged",
        "scan" => "Defender scan",
        "update" => "Windows Update",
        "task" => "Scheduled task",
        "gpu_other" => "Another app using the GPU",
        "foreground" => "Focus changed",
        "ctx_contention" => "Game thread waiting for a core",
        _ => "Other",
    }
}

const W: f64 = 960.0;
const PLOT_H: f64 = 160.0;
const LANE_H: f64 = 18.0;
const MAX_LANE_EVENTS: usize = 1500;

pub fn render(r: &Report) -> String {
    let s = r.story;
    let span = r.capture_end.saturating_sub(r.capture_start).max(1) as f64;
    let x = |q: u64| (q.saturating_sub(r.capture_start) as f64 / span * W).clamp(0.0, W);

    // Frame-time line: the slowest frame per pixel column, so spikes survive.
    let mut cols = vec![0f32; W as usize + 1];
    for f in r.frames {
        if f.frame_time_ms.is_finite() {
            let c = x(f.qpc) as usize;
            cols[c] = cols[c].max(f.frame_time_ms);
        }
    }
    let mut sorted: Vec<f32> = r
        .frames
        .iter()
        .map(|f| f.frame_time_ms)
        .filter(|v| v.is_finite())
        .collect();
    sorted.sort_by(f32::total_cmp);
    let p999 = sorted
        .get((sorted.len() as f64 * 0.999) as usize)
        .copied()
        .unwrap_or(50.0);
    let top = (p999 as f64 * 1.15).clamp(20.0, 250.0);
    let y = |ms: f32| PLOT_H - (ms as f64 / top * PLOT_H).min(PLOT_H);
    let mut path = String::new();
    for (i, v) in cols.iter().enumerate().filter(|(_, v)| **v > 0.0) {
        let _ = write!(
            path,
            "{}{:.1},{:.1}",
            if path.is_empty() { "M" } else { "L" },
            i,
            y(*v)
        );
    }

    let mut kinds: Vec<&str> = r.events.iter().map(|e| e.kind.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let svg_h = PLOT_H + 24.0 + kinds.len() as f64 * LANE_H;

    let mut svg = format!(
        "<svg viewBox=\"0 0 {W} {svg_h}\" role=\"img\" aria-label=\"Frame time with event lanes\">"
    );
    for g in &s.groups {
        let (a, b) = (x(g.start_qpc), x(g.end_qpc).max(x(g.start_qpc) + 2.0));
        let cls = if g.class == "cause" { "hc" } else { "hu" };
        let _ = write!(
            svg,
            "<rect class=\"{cls}\" x=\"{a:.1}\" y=\"0\" width=\"{:.1}\" height=\"{svg_h}\"/>",
            b - a
        );
    }
    let _ = write!(
        svg,
        "<path class=\"ft\" d=\"{path}\"/><text class=\"ax\" x=\"4\" y=\"12\">{top:.0} ms</text>\
         <line class=\"base\" x1=\"0\" x2=\"{W}\" y1=\"{PLOT_H}\" y2=\"{PLOT_H}\"/>"
    );
    for (i, kind) in kinds.iter().enumerate() {
        let ly = PLOT_H + 24.0 + i as f64 * LANE_H;
        let _ = write!(
            svg,
            "<text class=\"ax\" x=\"4\" y=\"{:.1}\">{}</text>",
            ly + 12.0,
            esc(kind_label(kind))
        );
        // Bounded so a long session still fits one report; the busiest lanes
        // keep their largest events.
        let mut lane: Vec<&StoryEvent> = r.events.iter().filter(|e| e.kind == *kind).collect();
        if lane.len() > MAX_LANE_EVENTS {
            lane.sort_by(|a, b| b.magnitude.total_cmp(&a.magnitude));
            lane.truncate(MAX_LANE_EVENTS);
        }
        for e in lane {
            let a = x(e.qpc);
            let w = (e.duration_ticks as f64 / span * W).max(1.5);
            let _ = write!(
                svg,
                "<rect class=\"ev\" x=\"{a:.1}\" y=\"{:.1}\" width=\"{w:.1}\" height=\"10\"/>",
                ly + 3.0
            );
        }
    }
    svg.push_str("</svg>");

    let mut causes = String::new();
    if s.causes.is_empty() {
        causes.push_str(
            "<p class=\"quiet\">No system event coincides with these hitches more often than chance.</p>",
        );
    } else {
        causes.push_str(
            "<table><thead><tr><th>Coincides with</th><th>What</th><th>Label</th>\
             <th class=\"n\">Hitches</th><th class=\"n\">Lift</th><th class=\"n\">Share of stutter</th></tr></thead><tbody>",
        );
        for c in &s.causes {
            let _ = write!(
                causes,
                "<tr><td class=\"mono\">{}</td><td>{}</td><td><span class=\"tag {}\">{}</span></td>\
                 <td class=\"n mono\">{} of {}</td><td class=\"n mono\">{:.1}×</td><td class=\"n mono\">{:.0}%</td></tr>",
                esc(file_name(&c.subject)),
                esc(kind_label(&c.kind)),
                esc(&c.label),
                if c.label == "likely" { "Likely" } else { "Possible" },
                c.k,
                c.n,
                c.lift,
                c.excess_share * 100.0
            );
        }
        causes.push_str("</tbody></table>");
    }
    let u = &s.unexplained;
    let mut unexplained = String::new();
    for (n, text) in [
        (u.in_game_cpu, "Originate inside the game on the CPU side (often shader compilation or asset streaming). System tweaks will not remove these."),
        (u.in_game_gpu, "The GPU took longer on these frames. Likely scene load; lowering GPU-heavy settings can help."),
        (u.unknown, "No matching system event at this capture level."),
    ] {
        if n > 0 {
            let _ = write!(
                unexplained,
                "<li><span class=\"mono\">{n}</span> {}</li>",
                esc(text)
            );
        }
    }
    let caveats: String = s
        .caveats
        .iter()
        .map(|c| format!("<li>{}</li>", esc(c)))
        .collect();

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Verity Frame Story: {exe}</title><style>{CSS}</style></head><body><main>\
<p class=\"eyebrow\">Verity Frame Story</p>\
<h1>{exe}</h1>\
<p class=\"meta mono\">{date} · {groups} hitch groups · {rate:.1} per minute · {fps:.0} FPS average · {low:.0} FPS 1% low</p>\
<section><h2>What the hitches coincide with</h2>{causes}</section>\
{unexp}\
<section><h2>Timeline</h2><figure>{svg}</figure>\
<p class=\"legend\"><span class=\"k hc\"></span>Hitch with a reported cause <span class=\"k hu\"></span>Other hitch</p></section>\
<section class=\"notes\"><h2>How to read this</h2><ul>\
<li>“Coincides with” means the event was running while the slow frame was made, more often than chance would explain. It is evidence, not proof.</li>\
<li>Lift is how many times more often than chance. Likely: at least 3 hitches, lift 3 or more, and under a 1% chance of a fluke after correcting for {pairs} things checked.</li>\
{caveats}</ul></section>\
<footer>Made by Verity. Contains process and driver names only.</footer></main></body></html>",
        exe = esc(file_name(r.exe)),
        date = esc(r.started_utc),
        groups = s.hitch_groups,
        rate = s.hitch_rate_per_min,
        fps = r.avg_fps,
        low = r.low_1pct_fps,
        unexp = if unexplained.is_empty() {
            String::new()
        } else {
            format!("<section><h2>Hitches with no outside cause</h2><ul>{unexplained}</ul></section>")
        },
        pairs = s.pairs_tested,
    )
}

const CSS: &str = "\
:root{--ground:#f4f4f2;--sheet:#fff;--ink:#17181a;--ink2:#55585d;--rule:#dcdcd8;--ok:#1f8a5b;--look:#a86a0f;--fix:#c7443d;--line:#2d5fd3;--hc:rgba(199,68,61,.16);--hu:rgba(85,88,93,.12);--ev:#55585d;color-scheme:light dark}\
@media (prefers-color-scheme:dark){:root{--ground:#151617;--sheet:#1b1c1e;--ink:#ececec;--ink2:#a4a6aa;--rule:#2e3033;--ok:#3fb27f;--look:#e8a33d;--fix:#ee6a62;--line:#7aa7ff;--hc:rgba(238,106,98,.2);--hu:rgba(185,186,189,.1);--ev:#b9babd}}\
*{box-sizing:border-box}body{margin:0;background:var(--ground);color:var(--ink);font:15px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif}\
main{max-width:1000px;margin:0 auto;padding:32px 16px 48px}.mono{font-family:ui-monospace,'Cascadia Mono',Consolas,monospace;font-variant-numeric:tabular-nums}\
.eyebrow{margin:0;font-size:12px;letter-spacing:.1em;text-transform:uppercase;color:var(--ink2)}h1{margin:4px 0 6px;font-size:28px;line-height:1.15;overflow-wrap:anywhere}\
.meta{margin:0 0 24px;color:var(--ink2);font-size:13px}section{background:var(--sheet);border:1px solid var(--rule);border-radius:10px;padding:16px;margin:0 0 16px}\
h2{margin:0 0 12px;font-size:15px}table{width:100%;border-collapse:collapse}th,td{text-align:left;padding:8px 6px;border-top:1px solid var(--rule);vertical-align:top}\
th{border-top:0;font-size:12px;color:var(--ink2);font-weight:600}.n{text-align:right}.tag{font-size:12px;font-weight:600;padding:2px 8px;border-radius:99px;border:1px solid currentColor}\
.tag.likely{color:var(--fix)}.tag.possible{color:var(--look)}.quiet{color:var(--ink2);margin:0}ul{margin:0;padding-left:20px}li{margin:4px 0}\
figure{margin:0;overflow-x:auto}svg{display:block;width:100%;min-width:640px;height:auto}.ft{fill:none;stroke:var(--line);stroke-width:1.2}.base{stroke:var(--rule)}\
.ax{fill:var(--ink2);font:11px system-ui,sans-serif}.ev{fill:var(--ev)}.hc{fill:var(--hc)}.hu{fill:var(--hu)}\
.legend{margin:10px 0 0;font-size:12px;color:var(--ink2)}.k{display:inline-block;width:12px;height:12px;border-radius:2px;vertical-align:-2px;margin:0 6px 0 12px}.k:first-child{margin-left:0}\
.k.hc{background:var(--hc)}.k.hu{background:var(--hu)}.notes{font-size:13px;color:var(--ink2)}footer{font-size:12px;color:var(--ink2);text-align:center}\
@media (max-width:600px){th:nth-child(2),td:nth-child(2){display:none}}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{Cause, HitchGroupInfo, Remedy, Unexplained};

    fn story() -> Story {
        Story {
            hitch_groups: 2,
            hitch_rate_per_min: 1.5,
            causes: vec![Cause {
                kind: "dpc".into(),
                subject: r"C:\Users\alice\AppData\evil<script>.sys".into(),
                label: "likely".into(),
                k: 2,
                n: 2,
                lift: 9.0,
                p: 0.001,
                excess_share: 1.0,
                remedy: Remedy {
                    kind: "guide".into(),
                    id: "driver.other".into(),
                },
            }],
            unexplained: Unexplained {
                in_game_cpu: 1,
                ..Default::default()
            },
            capture_mode: "standard".into(),
            caveats: vec!["note & caveat".into()],
            groups: vec![HitchGroupInfo {
                start_qpc: 50,
                end_qpc: 60,
                worst_ms: 60.0,
                excess_ms: 40.0,
                class: "cause".into(),
            }],
            pairs_tested: 3,
        }
    }

    #[test]
    fn report_is_self_contained_and_has_no_paths() {
        let s = story();
        let html = render(&Report {
            exe: r"D:\Users\bob\Games\game.exe",
            started_utc: "2026-11-17T10:00:00Z",
            avg_fps: 60.0,
            low_1pct_fps: 40.0,
            story: &s,
            frames: &[],
            events: &[StoryEvent {
                qpc: 55,
                duration_ticks: 1,
                kind: "dpc".into(),
                subject: "x.sys".into(),
                magnitude: 1.0,
            }],
            qpc_freq: 100,
            capture_start: 0,
            capture_end: 100,
        });
        assert!(html.starts_with("<!doctype html>"));
        for leak in ["alice", "bob", "Users", r"C:\", r"D:\", "<script>"] {
            assert!(!html.contains(leak), "report contains {leak}");
        }
        assert!(html.contains("evil&lt;script&gt;.sys"));
        assert!(html.contains("game.exe"));
        assert!(html.contains("note &amp; caveat"));
        // Self-contained: no external fetches.
        for ext in ["http://", "https://", "src=", "<link"] {
            assert!(!html.contains(ext), "report references {ext}");
        }
    }

    #[test]
    fn file_name_strips_folders() {
        assert_eq!(file_name(r"C:\a\b.sys"), "b.sys");
        assert_eq!(file_name("a/b.exe"), "b.exe");
        assert_eq!(file_name("plain.exe"), "plain.exe");
    }
}
