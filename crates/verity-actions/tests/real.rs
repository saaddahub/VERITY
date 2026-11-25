//! Real Windows integration tests for the M3 actions (docs/11-roadmap.md, M3).
//! Ignored by default: they change real settings (and put them back).
//!
//! `cargo test -p verity-actions --test real -- --ignored --test-threads=1`
//!
//! Each test applies through the journal, checks the change, reverts,
//! compares with the state before, then repeats with drift (someone changes
//! the value after Verity) and expects `superseded`.

use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use verity_actions::exclude::{self, Context};
use verity_actions::{catalog, ActionError, Engine, RevertOutcome};
use verity_core::actions::{
    ActionSys, JournalFilter, JournalStore, ProcKey, RegValue, Throttle, EXECUTION_SPEED,
};
use verity_store::Store;
use verity_sys::actions::WinActionSys;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn row(j: &Store, id: i64) -> verity_core::actions::JournalRow {
    j.journal_get(id).unwrap().unwrap()
}

/// Apply, verify, revert, compare; then apply, drift, expect superseded and
/// the drifted value kept; then clean up with `restore`.
fn round_trip<S: PartialEq + std::fmt::Debug>(
    id: &str,
    params: Value,
    read: impl Fn() -> S,
    drift: impl Fn(),
    restore: impl Fn(),
) {
    let sys = WinActionSys;
    let j = store();
    let eng = Engine {
        sys: &sys,
        journal: &j,
    };
    let a = catalog::find(id).unwrap();
    let before = read();
    let prepared = a.prepare(&sys, params.clone()).unwrap();
    println!("{id}: {}", a.plan(&sys, &prepared).unwrap());
    assert_eq!(read(), before, "plan changed something");

    let r = eng.apply(a, params.clone(), "real", "").unwrap();
    assert!(a.applied(&sys, &row(&j, r).row.params).unwrap());
    assert_ne!(read(), before, "{id} changed nothing");
    assert_eq!(eng.revert(&row(&j, r)), RevertOutcome::Reverted);
    assert_eq!(read(), before, "{id} revert did not restore the snapshot");

    let r = eng.apply(a, params, "real", "").unwrap();
    drift();
    let drifted = read();
    assert_eq!(eng.revert(&row(&j, r)), RevertOutcome::Superseded);
    assert_eq!(
        read(),
        drifted,
        "{id} overwrote a value changed after Verity"
    );
    restore();
    assert_eq!(read(), before, "{id} test cleanup");
}

fn spawn_idle() -> Child {
    Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

fn key_of(child: &Child) -> ProcKey {
    sleep(Duration::from_millis(300));
    WinActionSys
        .processes()
        .unwrap()
        .into_iter()
        .find(|p| p.key.pid == child.id())
        .unwrap()
        .key
}

#[test]
#[ignore]
fn power_overlay() {
    let sys = WinActionSys;
    if sys.on_ac().unwrap() != Some(true) {
        println!("on battery: power.session_overlay is not applicable, as designed");
        return;
    }
    let before = sys.power_overlay().unwrap().unwrap();
    if before == catalog::BEST_PERFORMANCE {
        println!("already Best performance; skipping");
        return;
    }
    let other = "961cc777-2547-4f9d-8174-7d86181b8a7a";
    round_trip(
        "power.session_overlay",
        json!({}),
        || sys.power_overlay().unwrap(),
        || sys.set_power_overlay(other).unwrap(),
        || sys.set_power_overlay(&before).unwrap(),
    );
}

#[test]
#[ignore]
fn background_ecoqos() {
    let sys = WinActionSys;
    let mut child = spawn_idle();
    let k = key_of(&child);
    round_trip(
        "sched.background_ecoqos",
        json!({ "pid": k.pid }),
        || sys.throttling(k).unwrap(),
        || {
            sys.set_throttling(
                k,
                Throttle {
                    control: EXECUTION_SPEED,
                    state: 0,
                },
            )
            .unwrap()
        },
        || sys.set_throttling(k, Throttle::default()).unwrap(),
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
#[ignore]
fn background_cpusets() {
    let sys = WinActionSys;
    let mut child = spawn_idle();
    let k = key_of(&child);
    let a = catalog::find("sched.background_cpusets").unwrap();
    match sys.efficiency_cpu_sets().unwrap() {
        None => {
            assert!(matches!(
                a.prepare(&sys, json!({ "pid": k.pid })),
                Err(ActionError::NotApplicable(_))
            ));
            println!("not a hybrid CPU: not applicable, as designed");
        }
        Some(eff) => round_trip(
            "sched.background_cpusets",
            json!({ "pid": k.pid }),
            || sys.default_cpu_sets(k).unwrap(),
            || sys.set_default_cpu_sets(k, &eff[..1]).unwrap(),
            || sys.set_default_cpu_sets(k, &[]).unwrap(),
        ),
    }
    let _ = child.kill();
    let _ = child.wait();
}

const GPU_KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";

#[test]
#[ignore]
fn gpu_preference() {
    let sys = WinActionSys;
    let exe = r"C:\VerityTest\not-a-real-game.exe";
    round_trip(
        "gpu.preference",
        json!({ "exe": exe }),
        || sys.reg_get(GPU_KEY, exe).unwrap(),
        || {
            sys.reg_set(GPU_KEY, exe, &RegValue::Str("GpuPreference=1;".into()))
                .unwrap()
        },
        || sys.reg_delete(GPU_KEY, exe).unwrap(),
    );
}

#[test]
#[ignore]
fn max_refresh() {
    let sys = WinActionSys;
    let dev = sys.primary_display().unwrap();
    let cur = sys.display_mode(&dev).unwrap();
    let lower = sys
        .display_modes(&dev)
        .unwrap()
        .into_iter()
        .filter(|m| m.width == cur.width && m.height == cur.height && m.bits == cur.bits)
        .map(|m| m.hz)
        .filter(|&hz| hz < cur.hz)
        .min();
    let Some(lower) = lower else {
        println!(
            "only one refresh rate at {}x{}; nothing to test",
            cur.width, cur.height
        );
        return;
    };
    // Start below the maximum so there is something to change.
    sys.set_display_hz(&dev, lower).unwrap();
    sleep(Duration::from_secs(2));
    let d = dev.clone();
    round_trip(
        "display.max_refresh",
        json!({}),
        || sys.display_mode(&dev).unwrap().hz,
        || sys.set_display_hz(&d, lower).unwrap(),
        || {},
    );
    sys.set_display_hz(&dev, cur.hz).unwrap();
    assert_eq!(sys.display_mode(&dev).unwrap().hz, cur.hz);
}

const GAMEBAR: &str = r"Software\Microsoft\GameBar";

#[test]
#[ignore]
fn game_mode() {
    let sys = WinActionSys;
    let read = || {
        (
            sys.reg_get(GAMEBAR, "AutoGameModeEnabled").unwrap(),
            sys.reg_get(GAMEBAR, "AllowAutoGameMode").unwrap(),
        )
    };
    let before = read();
    let a = catalog::find("gamemode.enable").unwrap();
    if a.plan(&sys, &json!({})).is_err() {
        println!("Game Mode is already on: {before:?}; nothing to test");
        return;
    }
    let restore = || {
        for (name, v) in [
            ("AutoGameModeEnabled", &before.0),
            ("AllowAutoGameMode", &before.1),
        ] {
            match v {
                Some(v) => sys.reg_set(GAMEBAR, name, v).unwrap(),
                None => sys.reg_delete(GAMEBAR, name).unwrap(),
            }
        }
    };
    round_trip(
        "gamemode.enable",
        json!({}),
        read,
        || {
            sys.reg_set(GAMEBAR, "AutoGameModeEnabled", &RegValue::Dword(0))
                .unwrap()
        },
        restore,
    );
}

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_RUN: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

#[test]
#[ignore]
fn startup_disable() {
    let sys = WinActionSys;
    let name = "VerityTestItem";
    // A harmless item that only exists for this test.
    sys.reg_set(RUN, name, &RegValue::Str("cmd.exe /c exit".into()))
        .unwrap();
    round_trip(
        "startup.disable_item",
        json!({ "name": name, "place": "run" }),
        || sys.reg_get(APPROVED_RUN, name).unwrap(),
        || {
            let mut on = vec![2u8];
            on.extend([0u8; 11]);
            sys.reg_set(APPROVED_RUN, name, &RegValue::Binary(on))
                .unwrap()
        },
        || sys.reg_delete(APPROVED_RUN, name).unwrap(),
    );
    sys.reg_delete(RUN, name).unwrap();
}

/// M3 acceptance: a crash between the `pending` write and `applied`, and one
/// after `applied`, are both put back by recovery from the database file.
#[test]
#[ignore]
fn crash_recovery_from_the_database_file() {
    let sys = WinActionSys;
    let dir = std::env::temp_dir().join(format!("verity-recovery-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("verity.db");
    let (mut a, mut b) = (spawn_idle(), spawn_idle());
    let (ka, kb) = (key_of(&a), key_of(&b));
    {
        let j = Store::open(&db).unwrap();
        let eng = Engine {
            sys: &sys,
            journal: &j,
        };
        let eco = catalog::find("sched.background_ecoqos").unwrap();
        // Applied, then the service "dies".
        eng.apply(eco, json!({ "pid": ka.pid }), "crash", "")
            .unwrap();
        // Journaled as pending and changed, but never marked applied.
        let p = eco.prepare(&sys, json!({ "pid": kb.pid })).unwrap();
        j.journal_insert(&verity_core::actions::NewJournalRow {
            txn_id: "crash".into(),
            action_id: eco.info().id.into(),
            scope: eco.info().scope,
            executor: eco.info().executor,
            snapshot: eco.snapshot(&sys, &p).unwrap(),
            params: p.clone(),
            owner: String::new(),
        })
        .unwrap();
        eco.apply(&sys, &p).unwrap();
        // Dropped here without any revert: the crash.
    }
    assert_eq!(sys.throttling(ka).unwrap().state, EXECUTION_SPEED);
    assert_eq!(sys.throttling(kb).unwrap().state, EXECUTION_SPEED);

    let j = Store::open(&db).unwrap();
    let report = Engine {
        sys: &sys,
        journal: &j,
    }
    .recover()
    .unwrap();
    assert_eq!(report.len(), 2);
    assert!(
        report.iter().all(|(_, o)| *o == RevertOutcome::Reverted),
        "{report:?}"
    );
    assert_eq!(sys.throttling(ka).unwrap(), Throttle::default());
    assert_eq!(sys.throttling(kb).unwrap(), Throttle::default());
    assert!(j
        .journal_list(&JournalFilter {
            open_only: true,
            ..Default::default()
        })
        .unwrap()
        .is_empty());
    for c in [&mut a, &mut b] {
        let _ = c.kill();
        let _ = c.wait();
    }
    drop(j);
    let _ = std::fs::remove_dir_all(&dir);
}

/// M3 acceptance: a process playing sound, the foreground process and a
/// session-0 process are never eligible.
#[test]
#[ignore]
fn exclusions_on_real_processes() {
    // A process that plays a Windows sound on a loop.
    let mut player = Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            "$p = New-Object System.Media.SoundPlayer \"$env:windir\\Media\\Windows Background.wav\"; $p.PlayLooping(); Start-Sleep 30",
        ])
        .spawn()
        .unwrap();
    let sys = WinActionSys;
    let mut audio = HashSet::new();
    for _ in 0..20 {
        sleep(Duration::from_millis(500));
        audio = verity_sys::agent::audio_pids()
            .unwrap()
            .into_iter()
            .collect();
        if audio.contains(&player.id()) {
            break;
        }
    }
    assert!(
        audio.contains(&player.id()),
        "the sound player was not detected: {audio:?}"
    );
    let front: HashSet<u32> = verity_sys::agent::foreground_pid().into_iter().collect();
    let procs = sys.processes().unwrap();
    let me = procs
        .iter()
        .find(|p| p.key.pid == std::process::id())
        .unwrap();
    let extra = Vec::new();
    let ctx = Context {
        game: me.key,
        game_session: me.session,
        own_pid: 0,
        audio: &audio,
        recent_foreground: &front,
        extra: &extra,
    };
    let eligible: HashSet<u32> = exclude::eligible(&procs, &ctx)
        .iter()
        .map(|p| p.key.pid)
        .collect();
    assert!(
        !eligible.contains(&player.id()),
        "a process playing sound was eligible"
    );
    for pid in &front {
        assert!(
            !eligible.contains(pid),
            "the foreground process was eligible"
        );
    }
    let session0: Vec<_> = procs.iter().filter(|p| p.session == 0).collect();
    assert!(!session0.is_empty());
    assert!(session0.iter().all(|p| !eligible.contains(&p.key.pid)));
    println!(
        "{} eligible of {}; audio {:?}; front {:?}",
        eligible.len(),
        procs.len(),
        audio,
        front
    );
    let _ = player.kill();
    let _ = player.wait();
}

/// A session start treats every background app quickly enough not to hold
/// up Session Mode (journal on disk, as in the service).
#[test]
#[ignore]
fn session_start_speed() {
    use verity_actions::session::{SessionRun, Signals};
    let sys = WinActionSys;
    let dir = std::env::temp_dir().join(format!("verity-speed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let j = Store::open(&dir.join("verity.db")).unwrap();
    let eng = Engine {
        sys: &sys,
        journal: &j,
    };
    let me = sys
        .processes()
        .unwrap()
        .into_iter()
        .find(|p| p.key.pid == std::process::id())
        .unwrap();
    let signals = Signals {
        agent: true,
        own_pid: std::process::id(),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let run = SessionRun::start(
        &eng,
        "speed".into(),
        me.key,
        &["sched.background_ecoqos".to_string()],
        Vec::new(),
        &signals,
    );
    let took = t.elapsed();
    let applied = run.applied;
    let t = std::time::Instant::now();
    let outcomes = run.end(&eng);
    println!(
        "applied {applied} in {took:?}; reverted {} in {:?}",
        outcomes.len(),
        t.elapsed()
    );
    assert!(took < Duration::from_secs(3), "session start took {took:?}");
    drop(j);
    let _ = std::fs::remove_dir_all(&dir);
}
