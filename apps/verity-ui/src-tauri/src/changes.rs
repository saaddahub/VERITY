//! The Changes screen (docs/05-actions.md): Verity's journal, per-user
//! actions run here as the user, and "Undo everything".

use serde_json::{json, Value};
use verity_actions::catalog;
use verity_core::actions::{ActionSys, Executor, JournalRow};
use verity_ipc::agent::{self, RemoteJournal, RestoreCheck};
use verity_ipc::Role;
use verity_sys::actions::WinActionSys;

use crate::{call, Res};

/// Runs blocking agent work off the UI thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn journal() -> Res<RemoteJournal> {
    RemoteJournal::connect(Role::Ui).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn changes_catalog() -> Res<Value> {
    serde_json::to_value(catalog::all().iter().map(|a| a.info()).collect::<Vec<_>>())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn changes_list() -> Res<Value> {
    call("journal.list", json!({})).await
}

#[tauri::command]
pub async fn changes_startup_items() -> Res<Value> {
    blocking(|| {
        let items = WinActionSys.startup_items().map_err(|e| e.to_string())?;
        serde_json::to_value(items).map_err(|e| e.to_string())
    })
    .await
}

/// The exact change, or why there is nothing to change. App-side actions only.
#[tauri::command]
pub async fn changes_plan(id: String, params: Value) -> Res<Value> {
    blocking(move || {
        Ok(match agent::plan(&id, params)? {
            Ok(change) => json!({ "change": change }),
            Err(why) => json!({ "change": null, "not_applicable": why }),
        })
    })
    .await
}

/// Applies an app-side action. Before the first change ever, the service makes
/// a restore point; if it cannot, the user is asked first (`needs_consent`)
/// and the call is repeated with `proceed_without`.
#[tauri::command]
pub async fn changes_apply(id: String, params: Value, proceed_without: bool) -> Res<Value> {
    blocking(move || {
        let a = catalog::find(&id).ok_or("not in the action catalog")?;
        if a.info().executor != Executor::Agent {
            return Err("this change is made by Session Mode, not from here".into());
        }
        let j = journal()?;
        if let RestoreCheck::NeedsConsent(why) =
            agent::restore_check(&j, proceed_without).map_err(|e| e.to_string())?
        {
            return Ok(json!({ "status": "needs_consent", "reason": why }));
        }
        let row = agent::apply(&j, &id, params)?;
        Ok(json!({ "status": "applied", "id": row }))
    })
    .await
}

#[tauri::command]
pub async fn changes_revert(id: i64) -> Res<Value> {
    blocking(move || {
        let j = journal()?;
        let rows: Vec<JournalRow> = serde_json::from_value(
            j.call("journal.list", json!({}))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let row = rows
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("this change is no longer listed")?;
        match row.row.executor {
            Executor::Service => j
                .call("actions.revert", json!({ "id": id }))
                .map_err(|e| e.to_string()),
            Executor::Agent => {
                serde_json::to_value(&agent::revert(&j, &[row])[0].1).map_err(|e| e.to_string())
            }
        }
    })
    .await
}

#[tauri::command]
pub async fn changes_undo_all() -> Res<Value> {
    blocking(|| agent::revert_everything(&journal()?)).await
}

/// "Undo all Verity changes" from the tray, without the window.
pub fn undo_all_now() {
    std::thread::spawn(|| {
        if let Ok(j) = journal() {
            let _ = agent::revert_everything(&j);
        }
    });
}
