//! Admin-only end-to-end tests of M3 (actions and journal) against the
//! installed service. Run elevated after a service reinstall
//! (tools/m3-admin.ps1):
//!     cargo test -p verity-cli --test actions_e2e -- --ignored --test-threads=1

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use verity_core::actions::{ActionSys, ProcKey, Throttle, EXECUTION_SPEED};
use verity_ipc::{Client, ClientError, Role, PIPE_NAME};
use verity_sys::actions::WinActionSys;

const BEST: &str = "ded574b5-45a0-4f42-8737-46345c09c238";

fn try_rpc(method: &str, params: Value) -> Result<Value, ClientError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut c = Client::connect(PIPE_NAME, Role::Cli).await?;
            c.call(method, params).await
        })
}

fn rpc(method: &str, params: Value) -> Value {
    try_rpc(method, params).unwrap_or_else(|e| panic!("{method} failed: {e}"))
}

fn tool(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_verity")).with_file_name(name)
}

fn idle_child() -> (Child, ProcKey) {
    let child = Command::new("ping")
        .args(["-n", "120", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn ping");
    std::thread::sleep(Duration::from_millis(300));
    let key = WinActionSys
        .processes()
        .unwrap()
        .into_iter()
        .find(|p| p.key.pid == child.id())
        .unwrap()
        .key;
    (child, key)
}

fn eco(k: ProcKey) -> bool {
    let t = WinActionSys.throttling(k).unwrap();
    t.control & EXECUTION_SPEED != 0 && t.state & EXECUTION_SPEED != 0
}

fn plugged_in() -> bool {
    WinActionSys.on_ac().unwrap() == Some(true)
}

fn overlay() -> String {
    WinActionSys.power_overlay().unwrap().unwrap_or_default()
}

fn open_rows() -> Vec<Value> {
    rpc("journal.list", json!({ "open_only": true }))
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn restart_service_hard() {
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
    let t = Instant::now();
    while try_rpc("system.info", json!({})).is_err() {
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "service did not come back"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// M3: kill the service after `applied` (and before a revert); on restart,
/// crash recovery puts everything back and tells the user.
#[test]
#[ignore = "elevated; kills and restarts the service"]
fn killed_service_after_apply_is_recovered_at_start() {
    let before = overlay();
    let (mut child, k) = idle_child();
    rpc(
        "actions.apply",
        json!({ "id": "sched.background_ecoqos", "params": { "pid": k.pid } }),
    );
    assert!(eco(k));
    let overlay_applied = plugged_in() && before != BEST;
    if overlay_applied {
        rpc("actions.apply", json!({ "id": "power.session_overlay" }));
        assert_eq!(overlay(), BEST);
    }
    assert!(!open_rows().is_empty());

    restart_service_hard();

    assert!(!eco(k), "efficiency mode still on after recovery");
    if overlay_applied {
        assert_eq!(overlay(), before, "power mode not put back after recovery");
    }
    let session_open: Vec<_> = open_rows()
        .into_iter()
        .filter(|r| r["scope"] == "session")
        .collect();
    assert!(session_open.is_empty(), "{session_open:?}");
    let notices = rpc("session.status", json!({ "since": 0 }))["notices"].clone();
    assert!(
        notices.to_string().contains("put back"),
        "no recovery notice: {notices}"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// `actions.plan` describes the change and makes none.
#[test]
#[ignore = "needs the service"]
fn plan_changes_nothing() {
    let (mut child, k) = idle_child();
    let before = WinActionSys.throttling(k).unwrap();
    let v = rpc(
        "actions.plan",
        json!({ "id": "sched.background_ecoqos", "params": { "pid": k.pid } }),
    );
    println!("{v}");
    assert!(v["change"].as_str().unwrap().contains("efficiency mode on"));
    assert_eq!(WinActionSys.throttling(k).unwrap(), before);
    let _ = child.kill();
    let _ = child.wait();
}

/// M4 with M3: background apps get efficiency mode when the game becomes
/// active, so does one started mid-session, and all is put back at exit.
#[test]
#[ignore = "elevated; about 40 s"]
fn session_treats_background_apps_and_puts_them_back() {
    rpc("session.enable", json!({ "on": true }));
    let exe = tool("framegen.exe");
    rpc(
        "games.add",
        json!({ "exe_path": exe.to_string_lossy(), "name": "framegen" }),
    );
    let before = overlay();
    let (mut a, ka) = idle_child();

    // Stand in for the desktop app: report the game in front, no audio.
    let front = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let beat = {
        let (front, stop) = (front.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let pid = front.load(Ordering::Relaxed);
                if pid != 0 {
                    let _ = try_rpc("agent.foreground", json!({ "pid": pid, "audio": [] }));
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
    };
    let mut g = Command::new(&exe)
        .args(["--fps", "60", "--duration", "60"])
        .spawn()
        .expect("framegen");
    front.store(g.id(), Ordering::Relaxed);
    let t = Instant::now();
    while rpc("session.status", json!({ "since": u64::MAX }))["status"]["state"] != "active" {
        assert!(t.elapsed() < Duration::from_secs(30), "no active session");
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(2));
    assert!(eco(ka), "a background app was not put in efficiency mode");
    let game = WinActionSys
        .processes()
        .unwrap()
        .into_iter()
        .find(|p| p.key.pid == g.id())
        .unwrap();
    assert_eq!(
        WinActionSys.throttling(game.key).unwrap(),
        Throttle::default(),
        "the game itself was touched"
    );
    if plugged_in() {
        assert_eq!(overlay(), BEST);
    }

    let (mut b, kb) = idle_child();
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        eco(kb),
        "a background app started mid-session was not treated"
    );

    let _ = g.kill();
    let _ = g.wait();
    let t = Instant::now();
    while rpc("session.status", json!({ "since": u64::MAX }))["status"]["state"] != "idle" {
        assert!(t.elapsed() < Duration::from_secs(30), "session did not end");
        std::thread::sleep(Duration::from_millis(250));
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(!eco(ka) && !eco(kb), "background apps not put back");
    assert_eq!(overlay(), before, "power mode not put back");
    assert!(open_rows().iter().all(|r| r["scope"] != "session"));

    stop.store(true, Ordering::Relaxed);
    let _ = beat.join();
    for c in [&mut a, &mut b] {
        let _ = c.kill();
        let _ = c.wait();
    }
}
