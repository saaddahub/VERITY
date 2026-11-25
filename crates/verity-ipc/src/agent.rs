//! The user agent's half of the actions (docs/02-architecture.md): per-user
//! settings are changed here, in the user's own process, and journaled in the
//! service before each change. Used by the CLI and the desktop app.
//!
//! Everything here blocks; call it from a plain thread or `spawn_blocking`.

use std::sync::Mutex;

use serde_json::{json, Value};
use verity_actions::{catalog, Engine, RevertOutcome};
use verity_core::actions::{
    Executor, JournalFilter, JournalRow, JournalState, JournalStore, NewJournalRow,
};
use verity_sys::actions::WinActionSys;

use crate::{Client, ClientError, Role, PIPE_NAME};

/// The service's journal, reached over the pipe.
pub struct RemoteJournal {
    rt: tokio::runtime::Runtime,
    client: Mutex<Client>,
}

impl RemoteJournal {
    pub fn connect(role: Role) -> Result<Self, ClientError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = rt.block_on(Client::connect(PIPE_NAME, role))?;
        Ok(Self {
            rt,
            client: Mutex::new(client),
        })
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        let mut c = self.client.lock().unwrap_or_else(|p| p.into_inner());
        self.rt.block_on(c.call(method, params))
    }
}

impl JournalStore for RemoteJournal {
    fn journal_insert(&self, row: &NewJournalRow) -> Result<i64, String> {
        let r = self
            .call(
                "journal.record",
                json!({
                    "txn_id": row.txn_id,
                    "action_id": row.action_id,
                    "params": row.params,
                    "snapshot": row.snapshot,
                }),
            )
            .map_err(|e| e.to_string())?;
        r["id"].as_i64().ok_or_else(|| "no id in reply".to_string())
    }

    fn journal_set_state(
        &self,
        id: i64,
        state: JournalState,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.call(
            "journal.mark",
            json!({ "id": id, "state": state.as_str(), "error": error }),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    fn journal_get(&self, id: i64) -> Result<Option<JournalRow>, String> {
        Ok(self
            .journal_list(&JournalFilter::default())?
            .into_iter()
            .find(|r| r.id == id))
    }

    fn journal_list(&self, f: &JournalFilter) -> Result<Vec<JournalRow>, String> {
        let v = self
            .call("journal.list", json!({ "open_only": f.open_only }))
            .map_err(|e| e.to_string())?;
        let rows: Vec<JournalRow> = serde_json::from_value(v).map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .filter(|r| f.txn_id.is_none_or(|t| r.row.txn_id == t))
            .filter(|r| f.scope.is_none_or(|s| r.row.scope == s))
            .filter(|r| f.executor.is_none_or(|e| r.row.executor == e))
            .collect())
    }
}

/// What the restore-point step decided before a first persistent change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreCheck {
    /// A restore point exists (made now or before): go ahead.
    Ready,
    /// None could be made ("skipped", "disabled" or "failed"); the user must
    /// agree to go on without one.
    NeedsConsent(String),
}

pub fn restore_check(
    j: &RemoteJournal,
    proceed_without: bool,
) -> Result<RestoreCheck, ClientError> {
    let r = j.call(
        "system.restore_point",
        json!({ "proceed_without": proceed_without }),
    )?;
    let status = r["status"].as_str().unwrap_or("failed");
    Ok(match status {
        "done" | "created" => RestoreCheck::Ready,
        _ if proceed_without => RestoreCheck::Ready,
        "skipped" => RestoreCheck::NeedsConsent(
            "Windows made no restore point: it allows one a day and already made one today.".into(),
        ),
        "disabled" => RestoreCheck::NeedsConsent(
            "System Restore is turned off on this PC, so no restore point can be made.".into(),
        ),
        _ => RestoreCheck::NeedsConsent(format!(
            "Windows could not make a restore point ({}).",
            r["error"].as_str().unwrap_or("unknown error")
        )),
    })
}

/// The exact change an app-side action would make, or why it would not.
pub fn plan(id: &str, params: Value) -> Result<Result<String, String>, String> {
    let a = catalog::find(id).ok_or_else(|| format!("{id} is not in the action catalog"))?;
    if a.info().executor != Executor::Agent {
        return Err(format!("{id} runs in the service"));
    }
    let sys = WinActionSys;
    let r = a.prepare(&sys, params).and_then(|p| a.plan(&sys, &p));
    match r {
        Ok(change) => Ok(Ok(change)),
        Err(verity_actions::ActionError::NotApplicable(why)) => Ok(Err(why)),
        Err(e) => Err(e.to_string()),
    }
}

/// Applies one app-side action through the journal protocol. The caller has
/// already settled the restore point.
pub fn apply(j: &RemoteJournal, id: &str, params: Value) -> Result<i64, String> {
    let a = catalog::find(id).ok_or_else(|| format!("{id} is not in the action catalog"))?;
    if a.info().executor != Executor::Agent {
        return Err(format!("{id} runs in the service"));
    }
    let txn = format!("app-{}", std::process::id());
    Engine {
        sys: &WinActionSys,
        journal: j,
    }
    .apply(a, params, &txn, "")
    .map_err(|e| e.to_string())
}

/// Puts back app-side rows (the caller's own, from the service), newest first.
pub fn revert(j: &RemoteJournal, rows: &[JournalRow]) -> Vec<(i64, RevertOutcome)> {
    Engine {
        sys: &WinActionSys,
        journal: j,
    }
    .revert_all(rows)
}

/// "Undo everything": the service reverts its changes and returns this
/// user's app changes, which are reverted here.
pub fn revert_everything(j: &RemoteJournal) -> Result<Value, String> {
    let r = j
        .call("journal.revert_all", json!({}))
        .map_err(|e| e.to_string())?;
    let rows: Vec<JournalRow> =
        serde_json::from_value(r["agent_rows"].clone()).map_err(|e| e.to_string())?;
    let agent = revert(j, &rows);
    Ok(json!({
        "service": r["service"],
        "agent": agent.iter().map(|(id, o)| json!({ "id": id, "result": o })).collect::<Vec<_>>(),
    }))
}
