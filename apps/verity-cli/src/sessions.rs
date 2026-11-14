//! `verity record` and `verity sessions ...`.

use std::io::Write;

use anyhow::{bail, Context};
use serde::Deserialize;
use serde_json::{json, Value};
use verity_core::metrics::Frame;
use verity_core::telemetry::SessionSummary;

use crate::call;

#[derive(Deserialize)]
struct Session {
    id: i64,
    started_utc: String,
    #[serde(default)]
    frame_chunks: u32,
    summary: SessionSummary,
}

/// Reads `--name value` pairs, rejecting anything not in `allowed`.
fn flags<'a>(args: &[&'a str], allowed: &[&str]) -> anyhow::Result<Vec<(&'a str, &'a str)>> {
    let mut out = Vec::new();
    for pair in args.chunks(2) {
        match pair {
            [name, value] if allowed.contains(name) => out.push((*name, *value)),
            [name, ..] => bail!("unknown or incomplete option {name}"),
            [] => {}
        }
    }
    Ok(out)
}

fn flag<'a>(flags: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    flags.iter().find(|f| f.0 == name).map(|f| f.1)
}

pub fn record(args: &[&str]) -> anyhow::Result<()> {
    let f = flags(args, &["--pid", "--exe", "--duration", "--mode"])?;
    let pid: Option<u32> = flag(&f, "--pid")
        .map(|v| v.parse().context("--pid must be a number"))
        .transpose()?;
    let exe = flag(&f, "--exe");
    let duration: Option<f64> = flag(&f, "--duration")
        .map(|v| v.parse().context("--duration must be a number of seconds"))
        .transpose()?;
    if pid.is_none() && exe.is_none() {
        bail!("pass --pid <pid> or --exe <name>");
    }
    if flag(&f, "--mode").is_some_and(|m| m != "light") {
        bail!("only --mode light exists so far; standard and deep arrive with kernel tracing");
    }
    eprintln!(
        "recording {}...",
        match duration {
            Some(s) => format!("for {s} s"),
            None => "until the process exits".into(),
        }
    );
    let s: Session = serde_json::from_value(call(
        "record.run",
        json!({ "pid": pid, "exe": exe, "duration_s": duration }),
    )?)?;
    print!("{}", render(&s));
    Ok(())
}

pub fn list() -> anyhow::Result<()> {
    let sessions: Vec<Session> = serde_json::from_value(call("sessions.list", json!({}))?)?;
    if sessions.is_empty() {
        println!("no sessions recorded yet");
    }
    for s in sessions {
        let st = &s.summary.stats;
        println!(
            "{:>4}  {}  {:<24} {:>7.1} FPS  1% low {:>6.1}  {} hitch(es)  {:.0} s",
            s.id,
            s.started_utc,
            s.summary.exe,
            st.avg_fps,
            st.low_1pct_fps,
            st.hitch_groups.len(),
            s.summary.capture_s
        );
    }
    Ok(())
}

fn get(id: &str) -> anyhow::Result<(Session, Value)> {
    let id: i64 = id.parse().context("session id must be a number")?;
    let raw = call("sessions.get", json!({ "id": id }))?;
    Ok((serde_json::from_value(raw.clone())?, raw))
}

pub fn show(id: &str, as_json: bool) -> anyhow::Result<()> {
    let (session, raw) = get(id)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&raw)?);
    } else {
        print!("{}", render(&session));
    }
    Ok(())
}

pub fn export(id: &str, path: &str) -> anyhow::Result<()> {
    let (session, _) = get(id)?;
    let file = std::fs::File::create(path).with_context(|| format!("create {path}"))?;
    let mut out = std::io::BufWriter::new(file);
    writeln!(out, "{CSV_HEADER}")?;
    let mut total = 0;
    for seq in 0..session.frame_chunks {
        let frames: Vec<Frame> = serde_json::from_value(call(
            "sessions.frames",
            json!({ "id": session.id, "seq": seq }),
        )?)?;
        total += frames.len();
        write_csv(&mut out, &frames)?;
    }
    out.flush()?;
    println!("wrote {total} frames to {path}");
    Ok(())
}

const CSV_HEADER: &str = "qpc,frame_time_ms,cpu_busy_ms,gpu_busy_ms,displayed_time_ms,\
display_latency_ms,present_mode,flags";

/// Rust prints the shortest text that reads back to the same f32, so the file
/// round-trips exactly; an unmeasured value is `NaN`.
fn write_csv(out: &mut impl Write, frames: &[Frame]) -> std::io::Result<()> {
    for f in frames {
        writeln!(
            out,
            "{},{},{},{},{},{},{},{}",
            f.qpc,
            f.frame_time_ms,
            f.cpu_busy_ms,
            f.gpu_busy_ms,
            f.displayed_time_ms,
            f.display_latency_ms,
            f.present_mode,
            f.flags
        )?;
    }
    Ok(())
}

fn render(s: &Session) -> String {
    use std::fmt::Write;
    let (m, st) = (&s.summary, &s.summary.stats);
    let mut o = String::new();
    let _ = writeln!(
        o,
        "Session {} — {} (pid {}), {}",
        s.id, m.exe, m.pid, s.started_utc
    );
    let _ = writeln!(
        o,
        "  Captured  {:.1} s, {} frames; the first {:.0} s are left out of the numbers below",
        m.capture_s, m.total_frames, m.warmup_excluded_s
    );
    if st.frame_count == 0 {
        let _ = writeln!(o, "  No frames after the warm-up; record for longer.");
        return o;
    }
    let _ = writeln!(
        o,
        "  Average   {:.1} FPS over {:.1} s",
        st.avg_fps, st.duration_s
    );
    let _ = writeln!(
        o,
        "  Lows      1%: {:.1} FPS   0.1%: {:.1} FPS",
        st.low_1pct_fps, st.low_01pct_fps
    );
    let _ = writeln!(o, "  Pacing    {:.2} ms spread", st.pacing_spread_ms);
    let worst = st
        .hitch_groups
        .iter()
        .map(|g| g.worst_ms)
        .fold(0.0, f32::max);
    let _ = match st.hitch_groups.len() {
        0 => writeln!(o, "  Hitches   none"),
        n => writeln!(
            o,
            "  Hitches   {n} ({:.1} per minute), worst {worst:.1} ms",
            st.hitch_rate_per_min
        ),
    };
    let _ = writeln!(
        o,
        "  GPU       {}",
        m.gpu_adapter
            .as_deref()
            .unwrap_or("not determined (no GPU activity seen)")
    );
    if !m.capture_gaps.is_empty() {
        let _ = writeln!(
            o,
            "  Gaps      {} stretch(es) without frame data; not counted as hitches",
            m.capture_gaps.len()
        );
    }
    let _ = writeln!(
        o,
        "  Overhead  {:.3}% CPU (budget {:.2}%){}",
        m.overhead_cpu_pct,
        m.overhead_budget_pct,
        if m.overhead_cpu_pct > m.overhead_budget_pct {
            "  OVER BUDGET"
        } else {
            ""
        }
    );
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_csv(text: &str) -> Vec<Frame> {
        text.lines()
            .skip(1)
            .map(|l| {
                let v: Vec<&str> = l.split(',').collect();
                let f = |i: usize| v[i].parse::<f32>().unwrap();
                Frame {
                    qpc: v[0].parse().unwrap(),
                    frame_time_ms: f(1),
                    cpu_busy_ms: f(2),
                    gpu_busy_ms: f(3),
                    displayed_time_ms: f(4),
                    display_latency_ms: f(5),
                    present_mode: v[6].parse().unwrap(),
                    flags: v[7].parse().unwrap(),
                }
            })
            .collect()
    }

    #[test]
    fn csv_round_trips_bit_for_bit() {
        let frames: Vec<Frame> = (0..500u64)
            .map(|i| Frame {
                qpc: 1_580_624_186_008 + i * 166_667,
                frame_time_ms: 16.5621 + i as f32 * 0.0137,
                cpu_busy_ms: 16.3538,
                gpu_busy_ms: 0.1988,
                displayed_time_ms: if i % 9 == 0 { f32::NAN } else { 20.8228 },
                display_latency_ms: f32::NAN,
                present_mode: 7,
                flags: (i % 4) as u8,
            })
            .collect();
        let mut out = format!("{CSV_HEADER}\n").into_bytes();
        write_csv(&mut out, &frames).unwrap();
        let back = parse_csv(std::str::from_utf8(&out).unwrap());
        assert_eq!(back.len(), frames.len());
        for (a, b) in back.iter().zip(&frames) {
            assert_eq!(a.qpc, b.qpc);
            assert_eq!(a.frame_time_ms.to_bits(), b.frame_time_ms.to_bits());
            assert_eq!(a.displayed_time_ms.is_nan(), b.displayed_time_ms.is_nan());
            assert_eq!((a.present_mode, a.flags), (b.present_mode, b.flags));
        }
    }

    #[test]
    fn options_are_checked() {
        assert_eq!(
            flags(&["--pid", "5"], &["--pid"]).unwrap(),
            [("--pid", "5")]
        );
        assert!(flags(&["--nope", "5"], &["--pid"]).is_err());
        assert!(flags(&["--pid"], &["--pid"]).is_err());
        assert!(record(&[]).is_err());
        assert!(record(&["--exe", "g.exe", "--mode", "deep"]).is_err());
    }
}
