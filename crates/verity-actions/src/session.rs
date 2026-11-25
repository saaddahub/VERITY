//! The session side of the actions (docs/06-session-mode.md): apply the
//! profile's session actions when a game becomes active, treat background
//! processes that start during the session, pause while the game is not in
//! front, and put everything back at the end.

use std::collections::{BTreeMap, HashSet};

use serde_json::json;
use verity_core::actions::{JournalFilter, ProcError, ProcInfo, ProcKey};

use crate::catalog::{self, SESSION_ORDER};
use crate::exclude::{self, Context};
use crate::{ActionError, Engine, RevertOutcome};

const BACKGROUND: [&str; 2] = ["sched.background_ecoqos", "sched.background_cpusets"];

/// What the agent reports, refreshed by the service before each call.
#[derive(Default)]
pub struct Signals {
    /// The agent (the desktop app) reported recently. Without it Verity cannot
    /// tell which app is in front or playing sound, so background apps are left alone.
    pub agent: bool,
    pub own_pid: u32,
    pub audio: HashSet<u32>,
    pub recent_foreground: HashSet<u32>,
}

/// One running session's changes.
pub struct SessionRun {
    pub txn: String,
    game: ProcKey,
    game_session: Option<u32>,
    enabled: Vec<&'static str>,
    extra: Vec<String>,
    /// Open background rows, so a pause can undo just those.
    background: Vec<i64>,
    paused: bool,
    /// Why each skipped process or action was skipped, with counts.
    pub skipped: BTreeMap<String, usize>,
    pub applied: usize,
}

impl SessionRun {
    /// Applies the enabled session actions in catalog order. `enabled` holds
    /// the profile's enabled ids; ids not in the M3 catalog are reported.
    pub fn start(
        eng: &Engine,
        txn: String,
        game: ProcKey,
        enabled: &[String],
        extra: Vec<String>,
        signals: &Signals,
    ) -> Self {
        let mut run = Self {
            txn,
            game,
            game_session: None,
            enabled: SESSION_ORDER
                .into_iter()
                .filter(|id| enabled.iter().any(|e| e == id))
                .collect(),
            extra,
            background: Vec::new(),
            paused: false,
            skipped: BTreeMap::new(),
            applied: 0,
        };
        for id in enabled {
            if !SESSION_ORDER.contains(&id.as_str()) {
                run.skip(format!("{id}: not available yet"));
            }
        }
        if run.enabled.contains(&"power.session_overlay") {
            if let Some(a) = catalog::find("power.session_overlay") {
                match eng.apply(a, json!({}), &run.txn, "") {
                    Ok(_) => run.applied += 1,
                    Err(e) => run.skip(format!("power mode: {e}")),
                }
            }
        }
        let procs = eng.sys.processes().unwrap_or_default();
        run.game_session = procs.iter().find(|p| p.key == game).map(|p| p.session);
        run.treat(eng, &procs, None, signals);
        run
    }

    fn skip(&mut self, why: String) {
        *self.skipped.entry(why).or_default() += 1;
    }

    /// Applies the background actions to eligible processes (all, or one).
    fn treat(
        &mut self,
        eng: &Engine,
        procs: &[ProcInfo],
        only: Option<ProcKey>,
        signals: &Signals,
    ) {
        if self.paused {
            return;
        }
        if !signals.agent {
            self.skip(
                "background apps: the Verity app is not running, so Verity cannot tell                  which apps you are using or which play sound"
                    .into(),
            );
            return;
        }
        let Some(game_session) = self.game_session else {
            self.skip("background apps: the game's session is unknown".into());
            return;
        };
        let ctx = Context {
            game: self.game,
            game_session,
            own_pid: signals.own_pid,
            audio: &signals.audio,
            recent_foreground: &signals.recent_foreground,
            extra: &self.extra,
        };
        let targets: Vec<ProcInfo> = exclude::eligible(procs, &ctx)
            .into_iter()
            .filter(|p| only.is_none_or(|k| p.key == k))
            .cloned()
            .collect();
        for id in BACKGROUND {
            if !self.enabled.contains(&id) {
                continue;
            }
            let Some(a) = catalog::find(id) else { continue };
            for p in &targets {
                let params = json!({"pid": p.key.pid, "created": p.key.created, "name": p.name});
                match eng.apply(a, params, &self.txn, "") {
                    Ok(row) => {
                        self.background.push(row);
                        self.applied += 1;
                    }
                    Err(ActionError::NotApplicable(why)) => self.skip(format!("{id}: {why}")),
                    Err(ActionError::Proc(ProcError::Denied | ProcError::Gone)) => {}
                    Err(e) => self.skip(format!("{id}: {e}")),
                }
            }
        }
    }

    /// A process started during the session.
    pub fn on_new_process(&mut self, eng: &Engine, key: ProcKey, signals: &Signals) {
        if self.paused || !BACKGROUND.iter().any(|id| self.enabled.contains(id)) {
            return;
        }
        let procs = eng.sys.processes().unwrap_or_default();
        self.treat(eng, &procs, Some(key), signals);
    }

    /// The game lost the foreground for a while: undo the background changes.
    pub fn pause(&mut self, eng: &Engine) {
        if self.paused {
            return;
        }
        self.paused = true;
        let rows = self.open_rows(eng, Some(&self.background));
        eng.revert_all(&rows);
        self.background.clear();
    }

    pub fn resume(&mut self, eng: &Engine, signals: &Signals) {
        if !self.paused {
            return;
        }
        self.paused = false;
        let procs = eng.sys.processes().unwrap_or_default();
        self.treat(eng, &procs, None, signals);
    }

    /// Puts back everything this session changed, newest first.
    pub fn end(self, eng: &Engine) -> Vec<(i64, RevertOutcome)> {
        let rows = self.open_rows(eng, None);
        eng.revert_all(&rows)
    }

    fn open_rows(
        &self,
        eng: &Engine,
        only: Option<&[i64]>,
    ) -> Vec<verity_core::actions::JournalRow> {
        eng.journal
            .journal_list(&JournalFilter {
                txn_id: Some(&self.txn),
                open_only: true,
                ..Default::default()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|r| only.is_none_or(|ids| ids.contains(&r.id)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::BEST_PERFORMANCE;
    use crate::fake::{FakeSys, MemJournal};
    use verity_core::actions::{JournalStore, Throttle, EXECUTION_SPEED};

    const BALANCED: &str = "00000000-0000-0000-0000-000000000000";

    fn setup() -> FakeSys {
        let sys = FakeSys::default();
        sys.set_power(Some(true), Some(BALANCED));
        sys.add_proc(
            ProcKey {
                pid: 10,
                created: 5,
            },
            1,
            "steam.exe",
        );
        sys.add_child(
            ProcKey {
                pid: 20,
                created: 9,
            },
            1,
            "game.exe",
            10,
        );
        sys.add_proc(
            ProcKey {
                pid: 30,
                created: 6,
            },
            1,
            "chrome.exe",
        );
        sys.add_proc(
            ProcKey {
                pid: 31,
                created: 6,
            },
            1,
            "spotify.exe",
        );
        sys.add_proc(
            ProcKey {
                pid: 40,
                created: 6,
            },
            0,
            "svchost.exe",
        );
        sys
    }

    fn enabled() -> Vec<String> {
        [
            "power.session_overlay",
            "sched.background_ecoqos",
            "sched.background_cpusets",
            "sched.game_priority",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn a_session_applies_its_actions_and_puts_everything_back() {
        let sys = setup();
        let j = MemJournal::default();
        let eng = Engine {
            sys: &sys,
            journal: &j,
        };
        let signals = Signals {
            agent: true,
            own_pid: 1,
            audio: [31].into(),
            recent_foreground: HashSet::new(),
        };
        let game = ProcKey {
            pid: 20,
            created: 9,
        };
        let mut run = SessionRun::start(&eng, "s1".into(), game, &enabled(), Vec::new(), &signals);
        assert_eq!(sys.overlay().as_deref(), Some(BEST_PERFORMANCE));
        let chrome = ProcKey {
            pid: 30,
            created: 6,
        };
        assert_eq!(sys.throttle(chrome).state, EXECUTION_SPEED);
        // The game, its launcher, the audio app and session 0 are untouched.
        for pid in [10, 20, 31, 40] {
            let created = if pid == 20 {
                9
            } else if pid == 10 {
                5
            } else {
                6
            };
            assert_eq!(
                sys.throttle(ProcKey { pid, created }),
                Throttle::default(),
                "pid {pid}"
            );
        }
        assert!(run
            .skipped
            .contains_key("sched.game_priority: not available yet"));
        // One process listing for the whole start, not one per process.
        assert_eq!(sys.listings(), 1);
        assert!(run
            .skipped
            .keys()
            .any(|k| k.contains("no efficiency cores")));

        // A background app started mid-session gets the same treatment.
        let late = ProcKey {
            pid: 50,
            created: 20,
        };
        sys.add_proc(late, 1, "updater.exe");
        run.on_new_process(&eng, late, &signals);
        assert_eq!(sys.throttle(late).state, EXECUTION_SPEED);

        let outcomes = run.end(&eng);
        assert!(
            outcomes.iter().all(|(_, o)| *o == RevertOutcome::Reverted),
            "{outcomes:?}"
        );
        assert_eq!(sys.overlay().as_deref(), Some(BALANCED));
        assert_eq!(sys.throttle(chrome), Throttle::default());
        assert_eq!(sys.throttle(late), Throttle::default());
        assert!(j
            .journal_list(&JournalFilter {
                open_only: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn pause_undoes_background_changes_and_resume_reapplies() {
        let sys = setup();
        let j = MemJournal::default();
        let eng = Engine {
            sys: &sys,
            journal: &j,
        };
        let signals = Signals {
            agent: true,
            ..Default::default()
        };
        let mut run = SessionRun::start(
            &eng,
            "s".into(),
            ProcKey {
                pid: 20,
                created: 9,
            },
            &enabled(),
            Vec::new(),
            &signals,
        );
        let chrome = ProcKey {
            pid: 30,
            created: 6,
        };
        run.pause(&eng);
        assert_eq!(sys.throttle(chrome), Throttle::default());
        // The power mode stays for the whole session.
        assert_eq!(sys.overlay().as_deref(), Some(BEST_PERFORMANCE));
        let late = ProcKey {
            pid: 50,
            created: 20,
        };
        sys.add_proc(late, 1, "updater.exe");
        run.on_new_process(&eng, late, &signals);
        assert_eq!(sys.throttle(late), Throttle::default());
        run.resume(&eng, &signals);
        assert_eq!(sys.throttle(chrome).state, EXECUTION_SPEED);
        assert_eq!(sys.throttle(late).state, EXECUTION_SPEED);
        run.end(&eng);
        assert_eq!(sys.throttle(chrome), Throttle::default());
    }

    #[test]
    fn on_battery_the_power_mode_is_left_alone() {
        let sys = setup();
        sys.set_power(Some(false), Some(BALANCED));
        let j = MemJournal::default();
        let eng = Engine {
            sys: &sys,
            journal: &j,
        };
        let run = SessionRun::start(
            &eng,
            "s".into(),
            ProcKey {
                pid: 20,
                created: 9,
            },
            &enabled(),
            Vec::new(),
            &Signals::default(),
        );
        assert_eq!(sys.overlay().as_deref(), Some(BALANCED));
        assert!(run.skipped.keys().any(|k| k.contains("on battery")));
        run.end(&eng);
    }

    #[test]
    fn without_the_app_background_apps_are_left_alone() {
        let sys = setup();
        let j = MemJournal::default();
        let eng = Engine {
            sys: &sys,
            journal: &j,
        };
        let game = ProcKey {
            pid: 20,
            created: 9,
        };
        let run = SessionRun::start(
            &eng,
            "s".into(),
            game,
            &enabled(),
            Vec::new(),
            &Signals::default(),
        );
        let chrome = ProcKey {
            pid: 30,
            created: 6,
        };
        assert_eq!(sys.throttle(chrome), Throttle::default());
        assert!(run.skipped.keys().any(|k| k.contains("app is not running")));
        // The power mode does not depend on the app.
        assert_eq!(sys.overlay().as_deref(), Some(BEST_PERFORMANCE));
        run.end(&eng);
    }
}
