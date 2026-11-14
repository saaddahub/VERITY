//! Admin-only: installs, starts, queries, stops and removes the real service.
//! Run from an elevated terminal after `cargo build --workspace`:
//!     cargo test --workspace -- --ignored

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use verity_core::SystemInfo;

fn verity(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_verity"))
        .args(args)
        .output()
        .expect("run verity")
}

fn ok(args: &[&str]) -> Output {
    let out = verity(args);
    assert!(
        out.status.success(),
        "verity {args:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Runs `verity system info --json` with a basic-user (non-admin) token via
/// `runas /trustlevel`, which detaches, so output goes through a file.
fn system_info_as_standard_user() -> SystemInfo {
    use std::os::windows::process::CommandExt;
    let out_file: PathBuf =
        std::env::temp_dir().join(format!("verity-std-user-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&out_file);
    let cmd = format!(
        r#"/trustlevel:0x20000 "cmd /c \"\"{}\" system info --json > \"{}\"\"""#,
        env!("CARGO_BIN_EXE_verity"),
        out_file.display()
    );
    let status = Command::new("runas")
        .raw_arg(cmd)
        .status()
        .expect("run runas");
    assert!(status.success(), "runas failed: {status}");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(text) = std::fs::read_to_string(&out_file) {
            if let Ok(info) = serde_json::from_str(&text) {
                let _ = std::fs::remove_file(&out_file);
                return info;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no output from the standard-user run"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
#[ignore = "needs an elevated terminal; installs a Windows service"]
fn install_start_query_stop_uninstall() {
    let _ = verity(&["service", "uninstall"]); // clean slate if a previous run died

    ok(&["service", "install"]);
    ok(&["service", "start"]);

    let info: SystemInfo =
        serde_json::from_slice(&ok(&["system", "info", "--json"]).stdout).expect("valid JSON");
    assert!(info.windows.build >= 19045, "Windows 10 22H2 or newer");
    assert!(!info.cpu.model.is_empty());
    assert!(!info.cpu.logical.is_empty());
    assert!(!info.gpus.is_empty(), "at least one hardware GPU");

    let as_user = system_info_as_standard_user();
    assert_eq!(as_user.windows, info.windows);

    let logs = verity_sys::paths::data_dir().join("logs");
    assert!(
        std::fs::read_dir(&logs).map(|d| d.count()).unwrap_or(0) > 0,
        "service wrote a log in {}",
        logs.display()
    );

    ok(&["service", "stop"]);
    ok(&["service", "uninstall"]);
    assert!(
        !verity(&["system", "info"]).status.success(),
        "service gone"
    );
}
