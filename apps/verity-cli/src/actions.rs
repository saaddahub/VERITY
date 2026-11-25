//! `verity actions` and `verity journal` (docs/05-actions.md, "CLI").

use anyhow::{bail, Context};
use serde_json::{json, Map, Value};
use verity_actions::catalog;
use verity_core::actions::{Executor, JournalRow};
use verity_ipc::agent::{self, RemoteJournal, RestoreCheck};
use verity_ipc::Role;

use crate::call;

/// `--param k=v ...` (numbers become numbers) and `--yes`.
fn parse_params(rest: &[&str]) -> anyhow::Result<(Value, bool)> {
    let mut map = Map::new();
    let mut yes = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--yes" => yes = true,
            "--param" => {
                let kv = it.next().context("--param needs k=v")?;
                let (k, v) = kv.split_once('=').context("--param needs k=v")?;
                let v = v
                    .parse::<u64>()
                    .map_or_else(|_| Value::String(v.into()), Value::from);
                map.insert(k.into(), v);
            }
            other => bail!("unexpected argument {other}"),
        }
    }
    Ok((Value::Object(map), yes))
}

fn find(id: &str) -> anyhow::Result<&'static dyn verity_actions::Action> {
    catalog::find(id)
        .with_context(|| format!("{id} is not in the action catalog (see `verity actions list`)"))
}

fn journal() -> anyhow::Result<RemoteJournal> {
    Ok(RemoteJournal::connect(Role::Cli)?)
}

pub fn actions(args: &[&str]) -> anyhow::Result<()> {
    match args {
        ["list"] => {
            for a in catalog::all() {
                let i = a.info();
                let runs = match i.executor {
                    Executor::Service => "service",
                    Executor::Agent => "this app",
                };
                println!("{:<26} {} ({:?}, runs in {runs})", i.id, i.title, i.scope);
                println!("{:<26} {}", "", i.what);
                println!("{:<26} downside: {}", "", i.downside);
            }
            Ok(())
        }
        ["plan", id, rest @ ..] => {
            let a = find(id)?;
            let (params, _) = parse_params(rest)?;
            let r = match a.info().executor {
                Executor::Service => {
                    let v = call("actions.plan", json!({ "id": id, "params": params }))?;
                    match v["change"].as_str() {
                        Some(c) => Ok(c.to_string()),
                        None => Err(v["not_applicable"].as_str().unwrap_or("").to_string()),
                    }
                }
                Executor::Agent => agent::plan(id, params).map_err(anyhow::Error::msg)?,
            };
            match r {
                Ok(change) => println!("would change: {change}\n(nothing was changed)"),
                Err(why) => println!("nothing to change: {why}"),
            }
            Ok(())
        }
        ["apply", id, rest @ ..] => {
            let a = find(id)?;
            let (params, yes) = parse_params(rest)?;
            if a.info().executor == Executor::Service {
                let v = call("actions.apply", json!({ "id": id, "params": params }))?;
                println!("applied (journal entry {})", v["id"]);
                return Ok(());
            }
            let j = journal()?;
            if let RestoreCheck::NeedsConsent(why) = agent::restore_check(&j, false)? {
                if !yes {
                    bail!("{why}\nRun again with --yes to make the change without a restore point.");
                }
                agent::restore_check(&j, true)?;
            }
            match agent::plan(id, params.clone()).map_err(anyhow::Error::msg)? {
                Ok(change) => println!("changing: {change}"),
                Err(why) => {
                    println!("nothing to change: {why}");
                    return Ok(());
                }
            }
            let row = agent::apply(&j, id, params).map_err(anyhow::Error::msg)?;
            println!("applied (journal entry {row}); undo with `verity actions revert {row}`");
            Ok(())
        }
        ["revert", id] => {
            let id: i64 = id.parse().context("journal id must be a number")?;
            let j = journal()?;
            let rows: Vec<JournalRow> = serde_json::from_value(j.call("journal.list", json!({}))?)?;
            let row = rows
                .into_iter()
                .find(|r| r.id == id)
                .with_context(|| format!("journal entry {id} not found"))?;
            let outcome = match row.row.executor {
                Executor::Service => j.call("actions.revert", json!({ "id": id }))?,
                Executor::Agent => serde_json::to_value(&agent::revert(&j, &[row])[0].1)?,
            };
            println!("{}", outcome_text(&outcome));
            Ok(())
        }
        _ => bail!("usage: verity actions list | plan <id> [--param k=v] | apply <id> [--param k=v] [--yes] | revert <journal-id>"),
    }
}

fn outcome_text(v: &Value) -> String {
    let note = v["note"].as_str().unwrap_or("");
    match v["outcome"].as_str().unwrap_or("") {
        "reverted" => "put back".into(),
        "nothing_to_undo" => format!("nothing to undo: {note}"),
        "superseded" => "kept as is: it was changed by someone else after Verity".into(),
        "deferred" => format!("waiting: {note}"),
        "failed" => format!("could not put back: {note}"),
        other => other.to_string(),
    }
}

pub fn journal_cmd(args: &[&str]) -> anyhow::Result<()> {
    match args {
        ["list", rest @ ..] => {
            let open_only = rest.contains(&"--open");
            let rows: Vec<JournalRow> =
                serde_json::from_value(call("journal.list", json!({ "open_only": open_only }))?)?;
            if rows.is_empty() {
                println!("no changes recorded");
            }
            for r in rows {
                println!(
                    "{:>5}  {:<13} {:<26} {}  {}",
                    r.id,
                    r.state.as_str(),
                    r.row.action_id,
                    r.created_utc,
                    r.row.params
                );
                if let Some(e) = r.error {
                    println!("       {e}");
                }
            }
            Ok(())
        }
        ["revert-all"] => {
            let j = journal()?;
            let r = agent::revert_everything(&j).map_err(anyhow::Error::msg)?;
            let all: Vec<&Value> = r["service"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(r["agent"].as_array().into_iter().flatten())
                .collect();
            if all.is_empty() {
                println!("nothing to undo");
            }
            for o in all {
                println!("{:>5}  {}", o["id"], outcome_text(&o["result"]));
            }
            Ok(())
        }
        _ => bail!("usage: verity journal list [--open] | revert-all"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_parse() {
        let (p, yes) =
            parse_params(&["--param", "pid=42", "--param", "exe=C:\\g.exe", "--yes"]).unwrap();
        assert_eq!(p, json!({"pid": 42, "exe": "C:\\g.exe"}));
        assert!(yes);
        assert!(parse_params(&["--param", "novalue"]).is_err());
        assert!(parse_params(&["--bogus"]).is_err());
    }
}
