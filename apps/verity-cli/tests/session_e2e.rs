//! Admin-only end-to-end tests of M4 (Session Mode) and M5 (Frame Story)
//! against the installed service, with framegen and the stressors.
//! Run from an elevated terminal after `cargo build --workspace` and a service
//! reinstall (see tools/check.ps1 -Admin):
//!     cargo test -p verity-cli --test session_e2e -- --ignored --test-threads=1

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use verity_ipc::{Client, Role, PIPE_NAME};

fn rpc(method: &str, params: Value) -> Value {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut c = Client::connect(PIPE_NAME, Role::Cli)
                .await
                .expect("service running");
            c.call(method, params).await
        })
        .unwrap_or_else(|e| panic!("{method} failed: {e}"))
}

fn tool(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_BIN_EXE_verity")).with_file_name(name);
    assert!(
        p.is_file(),
        "build the workspace first: {} is missing",
        p.display()
    );
    p
}

fn state() -> Value {
    rpc("session.status", json!({ "since": u64::MAX }))["status"].clone()
}

fn state_name() -> String {
    state()["state"].as_str().unwrap_or_default().to_string()
}

/// Session Mode on, framegen in the library.
fn prepare() {
    rpc("session.enable", json!({ "on": true }));
    let exe = tool("framegen.exe");
    rpc(
        "games.add",
        json!({ "exe_path": exe.to_string_lossy(), "name": "framegen" }),
    );
    wait_for("idle", Duration::from_secs(30));
}

fn framegen(args: &[&str]) -> Child {
    let log = std::env::temp_dir().join(format!("verity-e2e-{}.csv", std::process::id()));
    Command::new(tool("framegen.exe"))
        .args(args)
        .arg("--log")
        .arg(log)
        .spawn()
        .expect("start framegen")
}

fn wait_for(want: &str, limit: Duration) -> Duration {
    let t = Instant::now();
    while t.elapsed() < limit {
        if state_name() == want {
            return t.elapsed();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!(
        "Session Mode did not reach {want} within {limit:?}; status {}",
        state()
    );
}

/// Waits for the session-end notice after `since` and returns the session id.
fn session_end(since: u64, limit: Duration) -> i64 {
    let t = Instant::now();
    while t.elapsed() < limit {
        let v = rpc("session.status", json!({ "since": since }));
        for n in v["notices"].as_array().into_iter().flatten() {
            if n["kind"] == "session_end" {
                return n["session_id"].as_i64().expect("session id");
            }
            if n["kind"] == "info" && n["text"].as_str().unwrap_or("").contains("Could not save") {
                panic!("session not saved: {n}");
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("no session_end notice within {limit:?}");
}

fn last_notice_id() -> u64 {
    rpc("session.status", json!({ "since": 0 }))["notices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n["id"].as_u64())
        .max()
        .unwrap_or(0)
}

#[test]
#[ignore = "elevated; needs the installed service; about 20 s"]
fn process_that_exits_within_ten_seconds_never_becomes_a_session() {
    prepare();
    let mut g = framegen(&["--fps", "60", "--duration", "7"]);
    let t = Instant::now();
    let mut seen_arming = false;
    while t.elapsed() < Duration::from_secs(10) {
        let s = state_name();
        assert_ne!(s, "active", "a 7 s process must never trigger a profile");
        seen_arming |= s == "arming";
        std::thread::sleep(Duration::from_millis(150));
    }
    g.wait().expect("framegen exit");
    assert!(seen_arming, "framegen was never detected (Idle → Arming)");
    wait_for("idle", Duration::from_secs(5));
}

#[test]
#[ignore = "elevated; needs the installed service; about 2.5 min"]
fn session_lifecycle_and_planted_cause() {
    prepare();
    let since = last_notice_id();
    let event = format!("verity-e2e-{}", std::process::id());
    let mut g = framegen(&[
        "--fps",
        "60",
        "--duration",
        "120",
        "--hitch-ms",
        "60",
        "--hitch-on-event",
        &event,
    ]);
    wait_for("arming", Duration::from_secs(10));
    let to_active = wait_for("active", Duration::from_secs(25));
    assert!(
        to_active >= Duration::from_secs(5),
        "armed too fast: {to_active:?}"
    );

    // The planted cause: 20 times, a stressor starts and plants a hitch at the
    // same moment, 5 s apart, from about 15 s into the session.
    std::thread::sleep(Duration::from_secs(5));
    let mut stressors = Vec::new();
    for _ in 0..20 {
        stressors.push(
            Command::new(tool("stress.exe"))
                .args(["hitch", "--event", &event, "--threads", "4", "--ms", "300"])
                .spawn()
                .expect("start stress"),
        );
        std::thread::sleep(Duration::from_secs(5));
    }
    for mut s in stressors {
        let _ = s.wait();
    }
    g.wait().expect("framegen exit");
    let id = session_end(since, Duration::from_secs(60));
    wait_for("idle", Duration::from_secs(5));

    let session = rpc("sessions.get", json!({ "id": id }));
    let summary = &session["summary"];
    assert_eq!(summary["capture_mode"], "standard", "{summary}");
    let overhead = summary["overhead_cpu_pct"].as_f64().unwrap();
    let budget = summary["overhead_budget_pct"].as_f64().unwrap();
    println!("overhead {overhead:.3}% of all cores, budget {budget}%");
    assert!(
        overhead <= budget,
        "overhead {overhead}% over budget {budget}%"
    );

    let story = rpc("story.get", json!({ "id": id }));
    let s = &story["story"];
    println!("{}", serde_json::to_string_pretty(s).unwrap());
    let caveats = s["caveats"].to_string();
    assert!(
        !caveats.contains("Kernel trace could not start"),
        "kernel trace did not run: {caveats}"
    );
    // The stressor's start is planted at every hitch, so its proc_start events
    // must cover nearly all of them (one may be lost to a natural hitch nearby).
    let planted = s["causes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| {
            c["subject"] == "stress.exe" && c["kind"] == "proc_start" && c["label"] == "likely"
        })
        .unwrap_or_else(|| panic!("stress.exe start not a likely cause: {}", s["causes"]));
    assert!(
        planted["lift"].as_f64().unwrap() >= 3.0,
        "lift below 3: {planted}"
    );
    assert!(
        planted["k"].as_u64().unwrap() >= 18,
        "covers too few hitches: {planted}"
    );

    let html = rpc("story.export", json!({ "id": id }))["html"]
        .as_str()
        .unwrap()
        .to_string();
    let user = std::env::var("USERNAME").unwrap_or_default();
    assert!(html.starts_with("<!doctype html>"));
    assert!(
        !user.is_empty() && !html.contains(&user),
        "report names the user"
    );
    for leak in [r":\", "Users", "\\Device\\"] {
        assert!(!html.contains(leak), "report contains {leak}");
    }
}

#[test]
#[ignore = "elevated; needs the installed service; about 2.5 min"]
fn decoy_on_a_fixed_period_is_not_reported() {
    prepare();
    let since = last_notice_id();
    // Hitches at fixed but unrelated times; the decoy starts a child every 7 s.
    let hitch_at: Vec<String> = (0..18)
        .map(|i| format!("{}", 13 + i * 6 + (i * 37) % 5))
        .collect();
    let mut g = framegen(&[
        "--fps",
        "60",
        "--duration",
        "125",
        "--hitch-ms",
        "60",
        "--hitch-at",
        &hitch_at.join(","),
    ]);
    wait_for("active", Duration::from_secs(25));
    let mut decoy = Command::new(tool("stress.exe"))
        .args(["decoy", "--period-s", "7", "--count", "15"])
        .spawn()
        .expect("start decoy");
    g.wait().expect("framegen exit");
    let _ = decoy.kill();
    let _ = decoy.wait();
    let id = session_end(since, Duration::from_secs(60));
    let s = rpc("story.get", json!({ "id": id }))["story"].clone();
    println!("{}", serde_json::to_string_pretty(&s).unwrap());
    assert!(s["hitch_groups"].as_u64().unwrap() >= 15, "{s}");
    let reported = s["causes"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|c| c["subject"] == "stress.exe");
    assert!(!reported, "the decoy was reported: {}", s["causes"]);
}

#[test]
#[ignore = "elevated; needs the installed service; about 40 s"]
fn panic_reverts_within_a_second_and_disables() {
    prepare();
    let mut g = framegen(&["--fps", "60", "--duration", "60"]);
    wait_for("active", Duration::from_secs(25));
    let t = Instant::now();
    let v = rpc("session.panic", json!({}));
    let took = t.elapsed();
    println!("panic answered in {took:?}");
    assert!(took < Duration::from_secs(1), "panic took {took:?}");
    assert_eq!(v["status"]["state"], "idle", "{v}");
    assert_eq!(v["status"]["enabled"], false, "{v}");
    let _ = g.kill();
    let _ = g.wait();
    rpc("session.enable", json!({ "on": true }));
}

#[test]
#[ignore = "elevated; restarts the service; about 40 s"]
fn killed_service_mid_session_leaves_nothing_running() {
    prepare();
    let mut g = framegen(&["--fps", "60", "--duration", "60"]);
    wait_for("active", Duration::from_secs(25));
    assert!(verity_sys::etw::is_running("VerityKernel"));
    let kill = Command::new("taskkill")
        .args(["/f", "/im", "verity-service.exe"])
        .status()
        .expect("taskkill");
    assert!(kill.success());
    std::thread::sleep(Duration::from_secs(2));
    let start = Command::new(env!("CARGO_BIN_EXE_verity"))
        .args(["service", "start"])
        .status()
        .expect("start service");
    assert!(start.success());
    std::thread::sleep(Duration::from_secs(3));
    // The orphaned kernel trace is gone (only a running game would start a new one,
    // and framegen is not a fresh start, so no session begins).
    assert!(
        !verity_sys::etw::is_running("VerityKernel"),
        "kernel trace left running after a crash"
    );
    assert_eq!(state_name(), "idle");
    let _ = g.kill();
    let _ = g.wait();
}
