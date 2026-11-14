//! Admin-only end-to-end test of M1: the real service records `framegen` and the
//! results are checked against framegen's ground-truth log.
//! Run from an elevated terminal after `cargo build --workspace`:
//!     cargo test --workspace -- --ignored --test-threads=1

use std::path::PathBuf;
use std::process::{Child, Command, Output};
use std::time::Duration;

use serde_json::Value;

const FREQ_SLACK_MS: u64 = 20;

fn verity(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_verity"))
        .args(args)
        .output()
        .expect("run verity")
}

fn ok(args: &[&str]) -> String {
    let out = verity(args);
    assert!(
        out.status.success(),
        "verity {args:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("verity-e2e-{}-{name}", std::process::id()))
}

fn framegen(args: &[&str], truth: &PathBuf) -> Child {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_verity")).with_file_name("framegen.exe");
    assert!(
        exe.is_file(),
        "build the workspace first: {} is missing",
        exe.display()
    );
    let child = Command::new(exe)
        .args(args)
        .arg("--log")
        .arg(truth)
        .spawn()
        .expect("start framegen");
    std::thread::sleep(Duration::from_millis(1200)); // let it open its window
    child
}

/// Records framegen until it exits and returns the stored session as JSON.
fn record_until_exit(mut game: Child) -> Value {
    let text = ok(&["record", "--exe", "framegen.exe"]);
    game.wait().expect("framegen exit");
    session_from(&text)
}

fn session_from(record_output: &str) -> Value {
    let id = record_output
        .strip_prefix("Session ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no session id in: {record_output}"));
    serde_json::from_str(&ok(&["sessions", "show", id, "--json"])).expect("session JSON")
}

/// (qpc_freq, planted hitch QPCs) from framegen's log.
fn truth(path: &PathBuf) -> (u64, Vec<u64>) {
    let text = std::fs::read_to_string(path).expect("truth log");
    let freq = text
        .lines()
        .find_map(|l| l.strip_prefix("# qpc_freq="))
        .and_then(|v| v.trim().parse().ok())
        .expect("qpc_freq line");
    let hitches = text
        .lines()
        .filter_map(|l| l.strip_prefix("hitch,"))
        .filter_map(|l| l.split(',').next()?.parse().ok())
        .collect();
    (freq, hitches)
}

/// (start, end) of every hitch group in a session.
fn groups(session: &Value) -> Vec<(u64, u64)> {
    session["summary"]["stats"]["hitch_groups"]
        .as_array()
        .expect("hitch_groups")
        .iter()
        .map(|g| {
            (
                g["start_qpc"].as_u64().unwrap(),
                g["end_qpc"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
#[ignore = "needs an elevated terminal; installs a Windows service and records for about 80 s"]
fn recording_matches_planted_ground_truth() {
    let _ = verity(&["service", "uninstall"]);
    ok(&["service", "install"]);
    ok(&["service", "start"]);
    let result = std::panic::catch_unwind(run_checks);
    let _ = verity(&["service", "stop"]);
    let _ = verity(&["service", "uninstall"]);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn run_checks() {
    // --- 20 planted 50 ms hitches, one per second from 10 s to 29 s ---------
    let truth_path = temp("hitches.csv");
    let at: Vec<String> = (10..30).map(|s| s.to_string()).collect();
    let game = framegen(
        &[
            "--fps",
            "60",
            "--duration",
            "36",
            "--hitch-ms",
            "50",
            "--hitch-at",
            &at.join(","),
        ],
        &truth_path,
    );
    let session = record_until_exit(game);
    let summary = &session["summary"];
    let (freq, planted) = truth(&truth_path);
    assert_eq!(planted.len(), 20);
    let slack = FREQ_SLACK_MS * freq / 1000;
    let inside = |t: u64, g: &(u64, u64)| g.0 <= t + slack && t <= g.1 + slack;
    let found = groups(&session);
    let detected = planted
        .iter()
        .filter(|&&t| found.iter().any(|g| inside(t, g)))
        .count();
    let false_hitches = found
        .iter()
        .filter(|g| !planted.iter().any(|&t| inside(t, g)))
        .count();
    assert!(
        detected >= 19,
        "only {detected} of 20 planted hitches detected: {found:?}"
    );
    assert!(
        false_hitches <= 1,
        "{false_hitches} false hitches: {found:?}"
    );

    // GPU attribution: the summary names a real adapter of this machine.
    let adapter = summary["gpu_adapter"]
        .as_str()
        .expect("gpu_adapter is named");
    let info: Value = serde_json::from_str(&ok(&["system", "info", "--json"])).unwrap();
    let known: Vec<&str> = info["gpus"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["name"].as_str())
        .collect();
    assert!(
        known.contains(&adapter),
        "{adapter} is not one of {known:?}"
    );

    // Overhead is reported and inside the Light budget.
    let overhead = summary["overhead_cpu_pct"].as_f64().unwrap();
    let budget = summary["overhead_budget_pct"].as_f64().unwrap();
    assert!(
        overhead > 0.0 && overhead < budget,
        "overhead {overhead}% vs budget {budget}%"
    );
    assert!(
        summary["samples"].as_u64().unwrap() >= 25,
        "1 Hz counters were sampled"
    );
    assert!(summary["capture_gaps"].as_array().unwrap().is_empty());

    // Export: every stored frame comes back, in order.
    let csv_path = temp("frames.csv");
    let id = session["id"].to_string();
    ok(&[
        "sessions",
        "export",
        &id,
        "--csv",
        csv_path.to_str().unwrap(),
    ]);
    let csv = std::fs::read_to_string(&csv_path).unwrap();
    let qpcs: Vec<u64> = csv
        .lines()
        .skip(1)
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(qpcs.len() as u64, summary["total_frames"].as_u64().unwrap());
    assert!(
        qpcs.windows(2).all(|w| w[0] < w[1]),
        "frames are in time order"
    );
    assert!(ok(&["sessions", "list"]).contains("framegen.exe"));

    // --- a clean 60 FPS run reads as 60 FPS ---------------------------------
    let game = framegen(&["--fps", "60", "--duration", "14"], &temp("clean.csv"));
    let session = record_until_exit(game);
    let fps = session["summary"]["stats"]["avg_fps"].as_f64().unwrap();
    let low = session["summary"]["stats"]["low_1pct_fps"]
        .as_f64()
        .unwrap();
    assert!(low > 55.0, "1% low of a steady 60 FPS run reads {low}");
    assert!(
        (fps - 60.0).abs() <= 0.6,
        "average {fps} FPS is not within 1% of 60"
    );

    // --- PresentMon killed mid-capture: resume, record the gap, no false hitch
    let mut game = framegen(&["--fps", "60", "--duration", "24"], &temp("kill.csv"));
    let recorder = std::thread::spawn(|| ok(&["record", "--exe", "framegen.exe"]));
    std::thread::sleep(Duration::from_secs(9));
    let killed = Command::new("taskkill")
        .args(["/F", "/IM", "PresentMon-2.6.0-x64.exe"])
        .output()
        .expect("run taskkill");
    assert!(
        killed.status.success(),
        "taskkill: {}",
        String::from_utf8_lossy(&killed.stderr)
    );
    let session = session_from(&recorder.join().expect("record thread"));
    game.wait().expect("framegen exit");
    let gaps = session["summary"]["capture_gaps"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(gaps.len(), 1, "exactly one capture gap: {gaps:?}");
    let gap_start = gaps[0]["qpc"].as_u64().unwrap();
    let gap_end = gap_start + gaps[0]["duration_ticks"].as_u64().unwrap();
    let near_gap: Vec<_> = groups(&session)
        .into_iter()
        .filter(|g| g.1 + freq >= gap_start && g.0 <= gap_end + freq)
        .collect();
    assert!(
        near_gap.is_empty(),
        "the gap was counted as a hitch: {near_gap:?}"
    );
    let total = session["summary"]["total_frames"].as_u64().unwrap();
    assert!(
        total > 60 * 12,
        "capture resumed after the kill: only {total} frames"
    );

    for f in ["hitches.csv", "frames.csv", "clean.csv", "kill.csv"] {
        let _ = std::fs::remove_file(temp(f));
    }
}
