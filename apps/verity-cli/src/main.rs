//! `verity` command-line client.

mod actions;
mod service;
mod session;
mod sessions;

use std::process::ExitCode;

use anyhow::Context;
use serde_json::{json, Value};
use verity_core::{CoreKind, SystemInfo};
use verity_ipc::{Client, Role, PIPE_NAME};

const USAGE: &str = "usage:
  verity service install [--exe <path to verity-service.exe> | --start]   (elevated)
  verity service start | stop | uninstall                       (elevated)
  verity system info [--json]
  verity record --pid <pid> | --exe <name> [--duration <seconds>]
  verity sessions list
  verity sessions show <id> [--json]
  verity sessions export <id> --csv <path>
  verity session status | start --pid <pid> | start --exe <name> | stop | panic | on | off
  verity games scan | list | add <exe> | remove <id>
  verity profile show [--game <id>] | set [--game <id>] --file profile.json
  verity story show <session> [--json] | export <session> --html <path>
  verity actions list | plan <id> [--param k=v] | apply <id> [--param k=v] [--yes] | revert <journal-id>
  verity journal list [--open] | revert-all";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["service", "install"] => service::install(None),
        ["service", "install", "--start"] => service::install(None).and_then(|()| service::start()),
        ["service", "install", "--exe", path] => service::install(Some(path.into())),
        ["service", "start"] => service::start(),
        ["service", "stop"] => service::stop(),
        ["service", "uninstall"] => service::uninstall(),
        ["system", "info"] => system_info(false),
        ["system", "info", "--json"] => system_info(true),
        ["record", rest @ ..] => sessions::record(rest),
        ["sessions", "list"] => sessions::list(),
        ["sessions", "show", id] => sessions::show(id, false),
        ["sessions", "show", id, "--json"] => sessions::show(id, true),
        ["sessions", "export", id, "--csv", path] => sessions::export(id, path),
        ["session", rest @ ..] => session::session(rest),
        ["games", rest @ ..] => session::games(rest),
        ["profile", rest @ ..] => session::profile(rest),
        ["story", rest @ ..] => session::story(rest),
        ["actions", rest @ ..] => actions::actions(rest),
        ["journal", rest @ ..] => actions::journal_cmd(rest),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// One request to the service.
fn call(method: &str, params: Value) -> anyhow::Result<Value> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut client = Client::connect(PIPE_NAME, Role::Cli).await?;
            client.call(method, params).await
        })?)
}

fn system_info(as_json: bool) -> anyhow::Result<()> {
    let value = call("system.info", json!({}))?;
    let mut info: SystemInfo = serde_json::from_value(value).context("decode system.info")?;

    // The service runs in session 0 and may not see the user's desktop, so the
    // display list comes from this (user-session) process. See docs/notes/decisions.md.
    let service_displays = std::mem::replace(&mut info.displays, verity_sys::displays());
    if service_displays != info.displays {
        eprintln!(
            "note: the service saw {} display(s); showing the {} this session sees",
            service_displays.len(),
            info.displays.len()
        );
    }

    if as_json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        print!("{}", render(&info));
    }
    Ok(())
}

fn render(i: &SystemInfo) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let w = &i.windows;
    let _ = writeln!(
        s,
        "Windows  {} {}(build {}.{})",
        w.product_name(),
        w.display_version
            .as_deref()
            .map(|v| format!("{v} "))
            .unwrap_or_default(),
        w.build,
        w.ubr
    );

    let indices = |kind| -> Vec<usize> {
        i.cpu
            .logical
            .iter()
            .enumerate()
            .filter(|(_, lp)| lp.kind == kind)
            .map(|(n, _)| n)
            .collect()
    };
    let (perf, eff) = (
        indices(CoreKind::Performance),
        indices(CoreKind::Efficiency),
    );
    let _ = writeln!(
        s,
        "CPU      {} — {} logical processors",
        i.cpu.model,
        i.cpu.logical.len()
    );
    if perf.is_empty() {
        let _ = writeln!(
            s,
            "         not hybrid: all cores share one efficiency class"
        );
    } else {
        let _ = writeln!(
            s,
            "         performance ({}): {}",
            perf.len(),
            ranges(&perf)
        );
        let _ = writeln!(s, "         efficiency  ({}): {}", eff.len(), ranges(&eff));
    }

    for g in &i.gpus {
        let gb = g.dedicated_vram_bytes as f64 / (1u64 << 30) as f64;
        let _ = writeln!(
            s,
            "GPU      {} ({}, {gb:.1} GB dedicated)",
            g.name, g.vendor
        );
    }
    if i.displays.is_empty() {
        let _ = writeln!(s, "Display  none visible from this session");
    }
    for d in &i.displays {
        let _ = writeln!(
            s,
            "Display  {} — {}x{} at {} Hz (max {} Hz)",
            d.device, d.width, d.height, d.current_hz, d.max_hz
        );
    }

    let p = &i.power;
    let source = match p.ac_online {
        Some(true) => "plugged in",
        Some(false) => "on battery",
        None => "power source unknown",
    };
    let battery = match (p.battery_present, p.battery_percent) {
        (false, _) => "no battery".to_string(),
        (true, pct) => format!(
            "battery {}{}",
            pct.map(|v| format!("{v}%")).unwrap_or_else(|| "?".into()),
            if p.charging { ", charging" } else { "" }
        ),
    };
    let _ = writeln!(s, "Power    {source}, {battery}");
    let _ = writeln!(
        s,
        "Scheme   {} ({})",
        i.power_scheme.name.as_deref().unwrap_or("unnamed"),
        i.power_scheme.guid
    );
    s
}

/// [0,1,2,5,7,8] -> "0-2, 5, 7-8"
fn ranges(v: &[usize]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        while i + 1 < v.len() && v[i + 1] == v[i] + 1 {
            i += 1;
        }
        parts.push(if v[i] == start {
            start.to_string()
        } else {
            format!("{start}-{}", v[i])
        });
        i += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::ranges;

    #[test]
    fn compact_ranges() {
        assert_eq!(ranges(&[0, 1, 2, 5, 7, 8]), "0-2, 5, 7-8");
        assert_eq!(ranges(&[3]), "3");
        assert_eq!(ranges(&[]), "");
    }
}
