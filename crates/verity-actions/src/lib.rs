//! Reversible actions (docs/05-actions.md): the catalog, the journal protocol
//! that records every change before making it, and the session runner that
//! applies a profile's actions and puts everything back.

pub mod catalog;
pub mod exclude;
#[cfg(test)]
mod fake;
pub mod session;

use serde::Serialize;
use serde_json::Value;
use verity_core::actions::{
    ActionSys, Executor, JournalFilter, JournalRow, JournalState, JournalStore, NewJournalRow,
    ProcError, Scope,
};
use verity_core::OsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
}

#[derive(Debug, Serialize)]
pub struct ActionInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub scope: Scope,
    pub executor: Executor,
    pub risk: Risk,
    pub needs_reboot: bool,
    pub default_enabled: bool,
    pub what: &'static str,
    pub why: &'static str,
    pub downside: &'static str,
    pub undo: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    /// Nothing to do here, said plainly ("already on", "on battery").
    #[error("{0}")]
    NotApplicable(String),
    #[error("invalid parameters: {0}")]
    Params(String),
    /// The revert must wait (the power mode while on battery).
    #[error("{0}")]
    Deferred(String),
    #[error(transparent)]
    Proc(#[from] ProcError),
    #[error(transparent)]
    Os(#[from] OsError),
    #[error("journal: {0}")]
    Journal(String),
}

/// One reversible change. `params` were validated by `plan` before any other
/// call; every method reads the current state fresh from Windows.
pub trait Action: Send + Sync {
    fn info(&self) -> &'static ActionInfo;
    /// Validates the parameters and fills in defaults (the primary display,
    /// a process's creation time); the result is what gets journaled.
    fn prepare(&self, _sys: &dyn ActionSys, params: Value) -> Result<Value, ActionError> {
        Ok(params)
    }
    /// The exact change, without making it. `NotApplicable` when there is
    /// nothing to change.
    fn plan(&self, sys: &dyn ActionSys, params: &Value) -> Result<String, ActionError>;
    /// Everything needed to undo, self-contained.
    fn snapshot(&self, sys: &dyn ActionSys, params: &Value) -> Result<Value, ActionError>;
    fn apply(&self, sys: &dyn ActionSys, params: &Value) -> Result<(), ActionError>;
    /// Is Verity's change still in effect? False means someone changed it since.
    fn applied(&self, sys: &dyn ActionSys, params: &Value) -> Result<bool, ActionError>;
    fn revert(
        &self,
        sys: &dyn ActionSys,
        params: &Value,
        snapshot: &Value,
    ) -> Result<(), ActionError>;
}

fn params<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, ActionError> {
    serde_json::from_value(v.clone()).map_err(|e| ActionError::Params(e.to_string()))
}

fn snapshot_of<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, ActionError> {
    serde_json::from_value(v.clone())
        .map_err(|e| ActionError::Journal(format!("unreadable snapshot: {e}")))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", content = "note", rename_all = "snake_case")]
pub enum RevertOutcome {
    Reverted,
    /// Already gone (the process exited) or never took effect.
    NothingToUndo(String),
    /// Someone changed it after Verity did; their value is kept.
    Superseded,
    /// Will be retried (the power mode waits until the PC is plugged in).
    Deferred(String),
    Failed(String),
}

/// Runs the journal protocol against one store and one OS.
pub struct Engine<'a> {
    pub sys: &'a dyn ActionSys,
    pub journal: &'a dyn JournalStore,
}

impl Engine<'_> {
    /// Plan, snapshot, journal as `pending` (committed), apply, mark `applied`.
    /// A failed apply is reverted at once. Returns the journal id.
    pub fn apply(
        &self,
        action: &dyn Action,
        params: Value,
        txn_id: &str,
        owner: &str,
    ) -> Result<i64, ActionError> {
        let params = action.prepare(self.sys, params)?;
        action.plan(self.sys, &params)?;
        let snapshot = action.snapshot(self.sys, &params)?;
        let info = action.info();
        let id = self
            .journal
            .journal_insert(&NewJournalRow {
                txn_id: txn_id.into(),
                action_id: info.id.into(),
                scope: info.scope,
                executor: info.executor,
                params: params.clone(),
                snapshot: snapshot.clone(),
                owner: owner.into(),
            })
            .map_err(ActionError::Journal)?;
        match action.apply(self.sys, &params) {
            Ok(()) => {
                self.journal
                    .journal_set_state(id, JournalState::Applied, None)
                    .map_err(ActionError::Journal)?;
                Ok(id)
            }
            Err(e) => {
                let (state, note) = match action.revert(self.sys, &params, &snapshot) {
                    Ok(()) => (JournalState::Reverted, format!("apply failed: {e}")),
                    Err(r) => (
                        JournalState::RevertFailed,
                        format!("apply failed: {e}; undo failed: {r}"),
                    ),
                };
                let _ = self.journal.journal_set_state(id, state, Some(&note));
                Err(e)
            }
        }
    }

    /// Puts one row back, unless someone changed the value since (superseded).
    pub fn revert(&self, row: &JournalRow) -> RevertOutcome {
        if !row.state.open() {
            return RevertOutcome::NothingToUndo(format!("already {}", row.state.as_str()));
        }
        let Some(action) = catalog::find(&row.row.action_id) else {
            return self.mark(row.id, RevertOutcome::Failed("unknown action".into()));
        };
        let p = &row.row.params;
        let outcome = match action.applied(self.sys, p) {
            Ok(false) if row.state == JournalState::Pending => {
                RevertOutcome::NothingToUndo("the change never took effect".into())
            }
            Ok(false) => RevertOutcome::Superseded,
            Ok(true) => match action.revert(self.sys, p, &row.row.snapshot) {
                Ok(()) => RevertOutcome::Reverted,
                Err(e) => classify(e),
            },
            Err(e) => classify(e),
        };
        self.mark(row.id, outcome)
    }

    fn mark(&self, id: i64, outcome: RevertOutcome) -> RevertOutcome {
        let set = |state, note: Option<&str>| self.journal.journal_set_state(id, state, note);
        let r = match &outcome {
            RevertOutcome::Reverted => set(JournalState::Reverted, None),
            RevertOutcome::NothingToUndo(n) => set(JournalState::Reverted, Some(n)),
            RevertOutcome::Superseded => set(
                JournalState::Superseded,
                Some("changed by someone else after Verity; their value was kept"),
            ),
            RevertOutcome::Deferred(_) => Ok(()),
            RevertOutcome::Failed(e) => set(JournalState::RevertFailed, Some(e)),
        };
        match r {
            Ok(()) => outcome,
            Err(e) => RevertOutcome::Failed(format!("journal: {e}")),
        }
    }

    /// Reverts rows newest first.
    pub fn revert_all(&self, rows: &[JournalRow]) -> Vec<(i64, RevertOutcome)> {
        rows.iter().rev().map(|r| (r.id, self.revert(r))).collect()
    }

    /// Crash recovery (docs/05-actions.md): every session row the service
    /// left open is put back, newest first.
    pub fn recover(&self) -> Result<Vec<(JournalRow, RevertOutcome)>, ActionError> {
        let rows = self
            .journal
            .journal_list(&JournalFilter {
                scope: Some(Scope::Session),
                executor: Some(Executor::Service),
                open_only: true,
                ..Default::default()
            })
            .map_err(ActionError::Journal)?;
        Ok(rows
            .into_iter()
            .rev()
            .map(|r| {
                let o = self.revert(&r);
                (r, o)
            })
            .collect())
    }
}

fn classify(e: ActionError) -> RevertOutcome {
    match e {
        ActionError::Proc(ProcError::Gone) => {
            RevertOutcome::NothingToUndo("the process has exited".into())
        }
        ActionError::Deferred(why) => RevertOutcome::Deferred(why),
        e => RevertOutcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeSys, MemJournal};
    use serde_json::json;
    use verity_core::actions::{ProcKey, Throttle, EXECUTION_SPEED};

    fn ecoqos() -> &'static dyn Action {
        catalog::find("sched.background_ecoqos").unwrap()
    }

    fn key() -> ProcKey {
        ProcKey {
            pid: 40,
            created: 7,
        }
    }

    #[test]
    fn apply_journals_before_changing_and_revert_restores() {
        let sys = FakeSys::default();
        sys.add_proc(key(), 1, "app.exe");
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        let p = json!({"pid": 40, "created": 7, "name": "app.exe"});
        let id = e.apply(ecoqos(), p, "t1", "").unwrap();
        let row = j.journal_get(id).unwrap().unwrap();
        assert_eq!(row.state, JournalState::Applied);
        assert_eq!(row.row.snapshot, json!({"control": 0, "state": 0}));
        assert_eq!(j.states_seen(id), vec!["pending", "applied"]);
        assert_eq!(sys.throttle(key()).control, EXECUTION_SPEED);

        assert_eq!(e.revert(&row), RevertOutcome::Reverted);
        assert_eq!(sys.throttle(key()), Throttle::default());
        assert_eq!(
            j.journal_get(id).unwrap().unwrap().state,
            JournalState::Reverted
        );
    }

    #[test]
    fn revert_after_drift_keeps_the_users_value() {
        let sys = FakeSys::default();
        sys.add_proc(key(), 1, "app.exe");
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        let id = e
            .apply(
                ecoqos(),
                json!({"pid": 40, "created": 7, "name": "app.exe"}),
                "t",
                "",
            )
            .unwrap();
        // The app turned efficiency mode off itself.
        let mine = Throttle {
            control: EXECUTION_SPEED,
            state: 0,
        };
        sys.set_throttle(key(), mine);
        let row = j.journal_get(id).unwrap().unwrap();
        assert_eq!(e.revert(&row), RevertOutcome::Superseded);
        assert_eq!(sys.throttle(key()), mine);
        assert_eq!(
            j.journal_get(id).unwrap().unwrap().state,
            JournalState::Superseded
        );
    }

    #[test]
    fn a_failed_apply_is_reverted_and_recorded() {
        let sys = FakeSys::default();
        sys.add_proc(key(), 1, "app.exe");
        sys.fail_sets(true);
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        assert!(e
            .apply(
                ecoqos(),
                json!({"pid": 40, "created": 7, "name": "app.exe"}),
                "t",
                ""
            )
            .is_err());
        let rows = j.journal_list(&JournalFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].state.open());
        assert!(rows[0].error.as_deref().unwrap().contains("apply failed"));
    }

    #[test]
    fn crash_recovery_reverts_pending_and_applied_rows() {
        let sys = FakeSys::default();
        sys.add_proc(key(), 1, "app.exe");
        let other = ProcKey {
            pid: 41,
            created: 8,
        };
        sys.add_proc(other, 1, "b.exe");
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        // Applied, then the service died before the session ended.
        e.apply(
            ecoqos(),
            json!({"pid": 40, "created": 7, "name": "app.exe"}),
            "s",
            "",
        )
        .unwrap();
        // Journaled as pending, changed, then the service died before `applied`.
        let a = ecoqos();
        let p = json!({"pid": 41, "created": 8, "name": "b.exe"});
        let snap = a.snapshot(&sys, &p).unwrap();
        j.journal_insert(&NewJournalRow {
            txn_id: "s".into(),
            action_id: a.info().id.into(),
            scope: Scope::Session,
            executor: Executor::Service,
            params: p.clone(),
            snapshot: snap,
            owner: String::new(),
        })
        .unwrap();
        a.apply(&sys, &p).unwrap();
        // And one journaled as pending whose change never happened.
        let gone = ProcKey {
            pid: 42,
            created: 9,
        };
        sys.add_proc(gone, 1, "c.exe");
        let pc = json!({"pid": 42, "created": 9, "name": "c.exe"});
        j.journal_insert(&NewJournalRow {
            txn_id: "s".into(),
            action_id: a.info().id.into(),
            scope: Scope::Session,
            executor: Executor::Service,
            snapshot: a.snapshot(&sys, &pc).unwrap(),
            params: pc,
            owner: String::new(),
        })
        .unwrap();

        let report = e.recover().unwrap();
        assert_eq!(report.len(), 3);
        assert_eq!(sys.throttle(key()), Throttle::default());
        assert_eq!(sys.throttle(other), Throttle::default());
        assert!(matches!(report[0].1, RevertOutcome::NothingToUndo(_)));
        assert!(j
            .journal_list(&JournalFilter {
                open_only: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn exited_process_has_nothing_to_undo() {
        let sys = FakeSys::default();
        sys.add_proc(key(), 1, "app.exe");
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        let id = e
            .apply(
                ecoqos(),
                json!({"pid": 40, "created": 7, "name": "app.exe"}),
                "t",
                "",
            )
            .unwrap();
        sys.remove_proc(40);
        let row = j.journal_get(id).unwrap().unwrap();
        assert!(matches!(e.revert(&row), RevertOutcome::NothingToUndo(_)));
    }
}
