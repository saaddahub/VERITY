//! `verity session | games | profile | story` (docs/06-session-mode.md, 07-frame-story.md).

use anyhow::{bail, Context};
use serde_json::{json, Value};

use crate::call;

fn print_status(v: &Value) {
    let s = &v["status"];
    let enabled = s["enabled"].as_bool().unwrap_or(false);
    match s["state"].as_str().unwrap_or("?") {
        "idle" => println!(
            "Session Mode {}: waiting for a game",
            if enabled { "on" } else { "off" }
        ),
        "arming" => println!(
            "Arming: {} (pid {}) must run 10 s and draw frames",
            s["game"]["name"].as_str().unwrap_or("?"),
            s["proc"]["pid"]
        ),
        "active" => println!(
            "Session running: {} (pid {}), {} frames so far",
            s["game"]["name"].as_str().unwrap_or("?"),
            s["proc"]["pid"],
            s["frames"].as_u64().unwrap_or(0)
        ),
        other => println!("state: {other}"),
    }
    for c in v["capabilities"].as_array().into_iter().flatten() {
        println!("note: {}", c.as_str().unwrap_or_default());
    }
}

pub fn session(args: &[&str]) -> anyhow::Result<()> {
    let v = match args {
        ["status"] => {
            let v = call("session.status", json!({ "since": 0 }))?;
            print_status(&v);
            for n in v["notices"].as_array().into_iter().flatten().rev().take(5) {
                println!("  {}", n["text"].as_str().unwrap_or_default());
            }
            return Ok(());
        }
        ["start", "--pid", pid] => call(
            "session.start",
            json!({ "pid": pid.parse::<u32>().context("pid must be a number")? }),
        )?,
        ["start", "--exe", exe] => call("session.start", json!({ "exe": exe }))?,
        ["stop"] => call("session.stop", json!({}))?,
        ["panic"] => call("session.panic", json!({}))?,
        ["on"] => call("session.enable", json!({ "on": true }))?,
        ["off"] => call("session.enable", json!({ "on": false }))?,
        _ => bail!("usage: verity session status | start --pid <pid> | start --exe <name> | stop | panic | on | off"),
    };
    print_status(&v);
    Ok(())
}

pub fn games(args: &[&str]) -> anyhow::Result<()> {
    let list = match args {
        ["scan"] => call("games.scan", json!({}))?,
        ["list"] => call("games.list", json!({}))?,
        ["add", exe] => {
            let path = std::fs::canonicalize(exe).with_context(|| format!("{exe} not found"))?;
            // canonicalize returns \\?\C:\...; the service wants the plain form.
            let plain = path
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_string();
            let v = call("games.add", json!({ "exe_path": plain }))?;
            println!("added game {}", v["id"]);
            return Ok(());
        }
        ["remove", id] => {
            call(
                "games.remove",
                json!({ "id": id.parse::<i64>().context("id must be a number")? }),
            )?;
            println!("removed game {id}");
            return Ok(());
        }
        _ => bail!("usage: verity games scan | list | add <exe> | remove <id>"),
    };
    let games = list.as_array().cloned().unwrap_or_default();
    if games.is_empty() {
        println!("No games found. Add one with: verity games add <path to game.exe>");
    }
    for g in games {
        println!(
            "{:>4}  {:<7} {:<32} {}",
            g["id"],
            g["source"].as_str().unwrap_or_default(),
            g["name"].as_str().unwrap_or_default(),
            g["exe_path"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

pub fn profile(args: &[&str]) -> anyhow::Result<()> {
    let id = |s: &str| s.parse::<i64>().context("game id must be a number");
    let v = match args {
        ["show"] => call("profile.get", json!({}))?,
        ["show", "--game", g] => call("profile.get", json!({ "game_id": id(g)? }))?,
        ["set", "--file", f] => set(None, f)?,
        ["set", "--game", g, "--file", f] => set(Some(id(g)?), f)?,
        _ => bail!(
            "usage: verity profile show [--game <id>] | set [--game <id>] --file profile.json"
        ),
    };
    for w in v["warnings"].as_array().into_iter().flatten() {
        eprintln!("warning: {}", w.as_str().unwrap_or_default());
    }
    println!("{}", serde_json::to_string_pretty(&v["profile"])?);
    Ok(())
}

fn set(game_id: Option<i64>, file: &str) -> anyhow::Result<Value> {
    let text = std::fs::read_to_string(file).with_context(|| format!("read {file}"))?;
    let profile: Value = serde_json::from_str(&text).context("profile is not valid JSON")?;
    call(
        "profile.set",
        json!({ "game_id": game_id, "profile": profile }),
    )
}

pub fn story(args: &[&str]) -> anyhow::Result<()> {
    let id = |s: &str| s.parse::<i64>().context("session id must be a number");
    match args {
        ["show", s] | ["show", s, "--json"] => {
            let v = call("story.get", json!({ "id": id(s)? }))?;
            if args.len() == 3 {
                println!("{}", serde_json::to_string_pretty(&v["story"])?);
                return Ok(());
            }
            print_story(&v["story"]);
            Ok(())
        }
        ["export", s, "--html", path] => {
            let v = call("story.export", json!({ "id": id(s)? }))?;
            let html = v["html"].as_str().context("no report returned")?;
            std::fs::write(path, html).with_context(|| format!("write {path}"))?;
            println!("wrote {path}");
            Ok(())
        }
        _ => bail!("usage: verity story show <session> [--json] | export <session> --html <path>"),
    }
}

fn print_story(s: &Value) {
    println!(
        "{} hitch groups ({:.1} per minute), capture mode {}",
        s["hitch_groups"],
        s["hitch_rate_per_min"].as_f64().unwrap_or(0.0),
        s["capture_mode"].as_str().unwrap_or("?")
    );
    let causes = s["causes"].as_array().cloned().unwrap_or_default();
    if causes.is_empty() {
        println!("No system event coincides with the hitches more often than chance.");
    }
    for c in causes {
        println!(
            "  {:<8} coincides with {} ({}): {} of {} hitches, lift {:.1}x, {:.0}% of stutter time",
            c["label"].as_str().unwrap_or_default(),
            c["subject"].as_str().unwrap_or_default(),
            c["kind"].as_str().unwrap_or_default(),
            c["k"],
            c["n"],
            c["lift"].as_f64().unwrap_or(0.0),
            c["excess_share"].as_f64().unwrap_or(0.0) * 100.0
        );
    }
    let u = &s["unexplained"];
    for (key, text) in [
        (
            "in_game_cpu",
            "inside the game, CPU side (shader compilation or streaming)",
        ),
        ("in_game_gpu", "inside the game, GPU side (scene load)"),
        ("unknown", "no matching system event at this capture level"),
    ] {
        let n = u[key].as_u64().unwrap_or(0);
        if n > 0 {
            println!("  {n} hitch groups: {text}");
        }
    }
    for c in s["caveats"].as_array().into_iter().flatten() {
        println!("note: {}", c.as_str().unwrap_or_default());
    }
}
