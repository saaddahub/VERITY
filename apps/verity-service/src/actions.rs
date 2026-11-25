//! Session actions in the service (docs/05-actions.md, docs/06-session-mode.md):
//! the real [`SessionActions`] on top of the journal, and crash recovery.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use verity_actions::session::{SessionRun, Signals};
use verity_actions::{Engine, RevertOutcome};
use verity_core::actions::{
    ActionSys, Executor, JournalFilter, JournalRow, JournalState, JournalStore, NewJournalRow,
    ProcKey, Scope,
};
use verity_session::machine::ProcId;
use verity_session::profile::Profile;
use verity_store::Store;
use verity_sys::actions::WinActionSys;

use crate::session_mode::SessionActions;

/// A process counts as "recently in front" for this long after losing focus.
const RECENT_FRONT: Duration = Duration::from_secs(120);
/// The agent counts as present for this long after its last report; it
/// reports at least every 5 s while it runs.
const AGENT_FRESH: Duration = Duration::from_secs(15);
/// How often reverts that had to wait (the power mode on battery) are retried.
const RETRY_EVERY: Duration = Duration::from_secs(30);

/// The store behind the service's mutex, locked per journal write.
pub struct Locked<'a>(pub &'a Mutex<Store>);

impl Locked<'_> {
    fn s(&self) -> std::sync::MutexGuard<'_, Store> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl JournalStore for Locked<'_> {
    fn journal_insert(&self, row: &NewJournalRow) -> Result<i64, String> {
        self.s().journal_insert(row)
    }
    fn journal_set_state(
        &self,
        id: i64,
        state: JournalState,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.s().journal_set_state(id, state, error)
    }
    fn journal_get(&self, id: i64) -> Result<Option<JournalRow>, String> {
        self.s().journal_get(id)
    }
    fn journal_list(&self, f: &JournalFilter) -> Result<Vec<JournalRow>, String> {
        self.s().journal_list(f)
    }
}

/// One line for the user about reverts that did not simply succeed.
pub fn summarize(outcomes: &[(i64, RevertOutcome)]) -> Option<String> {
    let count = |f: fn(&RevertOutcome) -> bool| outcomes.iter().filter(|(_, o)| f(o)).count();
    let kept = count(|o| matches!(o, RevertOutcome::Superseded));
    let waiting = count(|o| matches!(o, RevertOutcome::Deferred(_)));
    let failed = count(|o| matches!(o, RevertOutcome::Failed(_)));
    let mut parts = Vec::new();
    if kept > 0 {
        parts.push(format!("{kept} setting(s) you changed yourself were kept"));
    }
    if waiting > 0 {
        parts.push("the power mode goes back once the PC is plugged in".to_string());
    }
    if failed > 0 {
        parts.push(format!(
            "{failed} change(s) could not be put back (see Changes)"
        ));
    }
    (!parts.is_empty()).then(|| format!("Verity put things back: {}.", parts.join("; ")))
}

/// Crash recovery at service start, before anything else runs: every session
/// change left open is put back. Returns a line for the user, if any.
pub fn recover(store: &Store) -> Option<String> {
    let eng = Engine {
        sys: &WinActionSys,
        journal: store,
    };
    match eng.recover() {
        Ok(report) if report.is_empty() => None,
        Ok(report) => {
            for (row, outcome) in &report {
                tracing::info!(id = row.id, action = %row.row.action_id, ?outcome, "crash recovery");
            }
            let done = report
                .iter()
                .filter(|(_, o)| matches!(o, RevertOutcome::Reverted))
                .count();
            let outcomes: Vec<_> = report.into_iter().map(|(r, o)| (r.id, o)).collect();
            Some(
                [
                    format!("Verity put back {done} change(s) left from before it last stopped."),
                    summarize(&outcomes).unwrap_or_default(),
                ]
                .join(" ")
                .trim()
                .to_string(),
            )
        }
        Err(e) => {
            tracing::error!("crash recovery failed: {e}");
            Some(format!("Verity could not check its earlier changes: {e}"))
        }
    }
}

pub struct JournalActions {
    store: Arc<Mutex<Store>>,
    run: Option<SessionRun>,
    audio: HashSet<u32>,
    front: Option<u32>,
    left_front: HashMap<u32, Instant>,
    last_agent: Option<Instant>,
    last_retry: Instant,
}

impl JournalActions {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        Self {
            store,
            run: None,
            audio: HashSet::new(),
            front: None,
            left_front: HashMap::new(),
            last_agent: None,
            last_retry: Instant::now(),
        }
    }

    fn signals(&mut self) -> Signals {
        self.left_front.retain(|_, t| t.elapsed() < RECENT_FRONT);
        Signals {
            agent: self.last_agent.is_some_and(|t| t.elapsed() < AGENT_FRESH),
            own_pid: std::process::id(),
            audio: self.audio.clone(),
            recent_foreground: self.left_front.keys().copied().chain(self.front).collect(),
        }
    }

    fn with_engine<R>(&self, f: impl FnOnce(&Engine) -> R) -> R {
        let journal = Locked(&self.store);
        f(&Engine {
            sys: &WinActionSys,
            journal: &journal,
        })
    }
}

impl SessionActions for JournalActions {
    fn apply(&mut self, game: ProcId, profile: &Profile) {
        let created = if game.start != 0 {
            game.start
        } else {
            // A manual start after a service restart: look the identity up.
            WinActionSys
                .processes()
                .ok()
                .and_then(|ps| ps.into_iter().find(|p| p.key.pid == game.pid))
                .map_or(0, |p| p.key.created)
        };
        let enabled: Vec<String> = profile
            .actions
            .iter()
            .filter(|(_, s)| s.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        let signals = self.signals();
        let txn = format!("session-{}-{}", verity_store::utc_now(), game.pid);
        let started = Instant::now();
        let run = self.with_engine(|eng| {
            SessionRun::start(
                eng,
                txn,
                ProcKey {
                    pid: game.pid,
                    created,
                },
                &enabled,
                profile.extra_exclusions.clone(),
                &signals,
            )
        });
        tracing::info!(
            applied = run.applied,
            ms = started.elapsed().as_millis() as u64,
            skipped = ?run.skipped,
            "session actions applied"
        );
        self.run = Some(run);
    }

    fn on_new_process(&mut self, pid: u32, create_time: u64) {
        let signals = self.signals();
        if let Some(mut run) = self.run.take() {
            self.with_engine(|eng| {
                run.on_new_process(
                    eng,
                    ProcKey {
                        pid,
                        created: create_time,
                    },
                    &signals,
                )
            });
            self.run = Some(run);
        }
    }

    fn foreground(&mut self, pid: u32) {
        self.last_agent = Some(Instant::now());
        if let Some(prev) = self.front.replace(pid) {
            if prev != pid {
                self.left_front.insert(prev, Instant::now());
            }
        }
    }

    fn audio(&mut self, pids: Vec<u32>) {
        self.audio = pids.into_iter().collect();
    }

    fn pause(&mut self) {
        if let Some(mut run) = self.run.take() {
            self.with_engine(|eng| run.pause(eng));
            self.run = Some(run);
        }
    }

    fn resume(&mut self) {
        let signals = self.signals();
        if let Some(mut run) = self.run.take() {
            self.with_engine(|eng| run.resume(eng, &signals));
            self.run = Some(run);
        }
    }

    fn revert_all(&mut self) -> Option<String> {
        let run = self.run.take()?;
        let outcomes = self.with_engine(|eng| run.end(eng));
        for (id, o) in &outcomes {
            if !matches!(o, RevertOutcome::Reverted) {
                tracing::info!(id, ?o, "session revert");
            }
        }
        summarize(&outcomes)
    }

    fn tick(&mut self) {
        if self.last_retry.elapsed() < RETRY_EVERY {
            return;
        }
        self.last_retry = Instant::now();
        let current = self.run.as_ref().map(|r| r.txn.clone());
        self.with_engine(|eng| {
            let open = eng
                .journal
                .journal_list(&JournalFilter {
                    scope: Some(Scope::Session),
                    executor: Some(Executor::Service),
                    open_only: true,
                    ..Default::default()
                })
                .unwrap_or_default();
            let left: Vec<_> = open
                .into_iter()
                .filter(|r| Some(&r.row.txn_id) != current.as_ref())
                .collect();
            if !left.is_empty() {
                eng.revert_all(&left);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_says_what_was_not_simply_put_back() {
        assert_eq!(summarize(&[(1, RevertOutcome::Reverted)]), None);
        let s = summarize(&[
            (1, RevertOutcome::Superseded),
            (2, RevertOutcome::Deferred("x".into())),
            (3, RevertOutcome::Failed("x".into())),
        ])
        .unwrap();
        assert!(s.contains("1 setting(s) you changed yourself were kept"));
        assert!(s.contains("plugged in"));
        assert!(s.contains("1 change(s) could not be put back"));
    }
}
