//! Probe for the M3 actions (docs/05-actions.md, items marked verify).
//!
//! `cargo run -p verity-probes --bin actions` reads state only, except that it
//! starts a throwaway `ping` child and toggles efficiency mode on it.
//! `-- --overlay` also sets the power mode to Best performance and back.

use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use verity_core::actions::{ActionSys, Throttle, EXECUTION_SPEED};
use verity_sys::actions::WinActionSys;

const BEST: &str = "ded574b5-45a0-4f42-8737-46345c09c238";
const POWER_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes";

fn reg_overlay() -> String {
    let out = Command::new("reg").args(["query", POWER_KEY]).output();
    out.map(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| l.contains("Overlay"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" | ")
    })
    .unwrap_or_default()
}

fn main() {
    let sys = WinActionSys;
    println!("on AC: {:?}", sys.on_ac());
    println!("power overlay: {:?}", sys.power_overlay());
    println!("registry: {}", reg_overlay());
    println!("efficiency cpu sets: {:?}", sys.efficiency_cpu_sets());
    match sys.primary_display() {
        Ok(d) => {
            let cur = sys.display_mode(&d);
            let max = sys.display_modes(&d).map(|m| {
                let c = cur.as_ref().ok();
                m.into_iter()
                    .filter(|x| {
                        c.is_some_and(|c| {
                            x.width == c.width && x.height == c.height && x.bits == c.bits
                        })
                    })
                    .map(|x| x.hz)
                    .max()
            });
            println!("display {d}: {cur:?}, max at this size {max:?}");
        }
        Err(e) => println!("display: {e}"),
    }
    for name in ["AutoGameModeEnabled", "AllowAutoGameMode"] {
        println!(
            "GameBar {name}: {:?}",
            sys.reg_get(r"Software\Microsoft\GameBar", name)
        );
    }
    match sys.startup_items() {
        Ok(items) => {
            for i in items {
                println!("startup {:?} {} enabled={}", i.place, i.name, i.enabled);
            }
        }
        Err(e) => println!("startup items: {e}"),
    }

    // Efficiency mode on a throwaway child.
    let mut child = Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn ping");
    sleep(Duration::from_millis(300));
    let procs = sys.processes().expect("processes");
    let me = procs
        .iter()
        .find(|p| p.key.pid == child.id())
        .expect("child listed");
    println!(
        "child {:?} session {} parent {}",
        me.key, me.session, me.parent
    );
    println!("  throttling before: {:?}", sys.throttling(me.key));
    let on = Throttle {
        control: EXECUTION_SPEED,
        state: EXECUTION_SPEED,
    };
    println!("  set efficiency: {:?}", sys.set_throttling(me.key, on));
    println!("  throttling after: {:?}", sys.throttling(me.key));
    println!(
        "  clear: {:?}",
        sys.set_throttling(me.key, Throttle::default())
    );
    println!("  throttling cleared: {:?}", sys.throttling(me.key));
    println!("  default cpu sets: {:?}", sys.default_cpu_sets(me.key));
    let reused = verity_core::actions::ProcKey {
        created: me.key.created + 1,
        ..me.key
    };
    println!("  wrong creation time: {:?}", sys.throttling(reused));
    let system = procs
        .iter()
        .find(|p| p.session == 0 && p.name.eq_ignore_ascii_case("lsass.exe"));
    if let Some(s) = system {
        println!("lsass {:?}: throttling {:?}", s.key, sys.throttling(s.key));
    }
    let _ = child.kill();
    let _ = child.wait();

    if std::env::args().any(|a| a == "--overlay") {
        let before = sys.power_overlay().ok().flatten();
        println!("set Best performance: {:?}", sys.set_power_overlay(BEST));
        println!("  now: {:?} | {}", sys.power_overlay(), reg_overlay());
        if let Some(b) = before {
            println!("restore {b}: {:?}", sys.set_power_overlay(&b));
            println!("  now: {:?} | {}", sys.power_overlay(), reg_overlay());
        }
    }
}
