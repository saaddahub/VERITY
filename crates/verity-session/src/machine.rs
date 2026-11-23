//! The Session Mode state machine (docs/06-session-mode.md, "State machine").
//!
//! Pure logic: the runner feeds it [`Input`]s with a monotonic time in seconds
//! and carries out the [`Command`]s it returns, in order, before the next input.
//! Ending is not a resting state: the commands that end a session (stop capture,
//! revert, finish) are returned together and the machine is Idle afterwards.

use serde::Serialize;

/// A process, identified by pid and creation time (never a bare pid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProcId {
    pub pid: u32,
    /// Creation time as a FILETIME (100 ns since 1601).
    pub start: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameRef {
    /// Library id; None for a manual `session start --pid` of an unknown exe.
    pub id: Option<i64>,
    pub name: String,
    /// Executable file name, e.g. `game.exe`.
    pub exe: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A process started; `game` is set when its exe is in the library.
    ProcessStart {
        proc: ProcId,
        game: Option<GameRef>,
    },
    ProcessExit {
        proc: ProcId,
    },
    /// The capture of `proc` has seen frames.
    Frames {
        proc: ProcId,
    },
    /// The foreground window now belongs to `pid` (reported by the agent).
    Foreground {
        pid: u32,
    },
    /// Time passed; drives the arming, relaunch and focus timers.
    Tick,
    /// `session start`: begin a session for this process now, skipping Arming.
    ManualStart {
        proc: ProcId,
        game: GameRef,
    },
    /// `session stop`.
    Stop,
    /// Panic hotkey or tray: revert now and disable Session Mode.
    Panic,
    Suspend,
    ServiceStopping,
    Logoff,
    /// The user turned Session Mode on or off.
    SetEnabled(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    GameExited,
    Stopped,
    Panic,
    Suspend,
    ServiceStopping,
    Logoff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start capturing frames from this process (also used during Arming).
    StartCapture {
        proc: ProcId,
    },
    /// Stop the capture; `keep` false discards it (Arming that never became a session).
    StopCapture {
        keep: bool,
    },
    /// Apply the game's profile (session actions in catalog order).
    ApplyProfile {
        game: GameRef,
        game_pid: u32,
    },
    /// Revert every session-scoped change, in reverse order.
    Revert,
    StartWatcher {
        game_pid: u32,
    },
    StopWatcher,
    PauseThrottling,
    ResumeThrottling,
    /// Write the summary, run Frame Story, notify the user.
    Finish {
        game: GameRef,
        reason: EndReason,
    },
    /// Record a session event (`events` row).
    Event {
        kind: &'static str,
        detail: String,
    },
    Notify(String),
    /// Persist the Session Mode on/off setting.
    SaveEnabled(bool),
}

/// Seconds a game must stay alive in Arming before its profile applies.
pub const ARMING_S: f64 = 10.0;
/// Arming gives up if the process presents no frame in this long (launchers).
pub const ARMING_GIVE_UP_S: f64 = 120.0;
/// A matching process this soon after exit continues the same session.
pub const RELAUNCH_S: f64 = 15.0;
/// Background throttling pauses after the game loses focus this long.
pub const FOCUS_LOST_S: f64 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Idle {
        enabled: bool,
    },
    Arming {
        enabled: bool,
        game: GameRef,
        proc: ProcId,
        since: f64,
        frames: bool,
    },
    Active {
        enabled: bool,
        game: GameRef,
        proc: ProcId,
        since: f64,
        /// Time of exit while waiting for a relaunch.
        exited_at: Option<f64>,
        throttling_paused: bool,
    },
}

#[derive(Debug, Clone)]
enum State {
    Idle,
    Arming {
        game: GameRef,
        proc: ProcId,
        since: f64,
        frames: bool,
    },
    Active {
        game: GameRef,
        proc: ProcId,
        since: f64,
        exited_at: Option<f64>,
        focus_lost_at: Option<f64>,
        paused: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Machine {
    state: State,
    enabled: bool,
}

impl Machine {
    pub fn new(enabled: bool) -> Self {
        Self {
            state: State::Idle,
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn status(&self) -> Status {
        let enabled = self.enabled;
        match &self.state {
            State::Idle => Status::Idle { enabled },
            State::Arming {
                game,
                proc,
                since,
                frames,
            } => Status::Arming {
                enabled,
                game: game.clone(),
                proc: *proc,
                since: *since,
                frames: *frames,
            },
            State::Active {
                game,
                proc,
                since,
                exited_at,
                paused,
                ..
            } => Status::Active {
                enabled,
                game: game.clone(),
                proc: *proc,
                since: *since,
                exited_at: *exited_at,
                throttling_paused: *paused,
            },
        }
    }

    pub fn handle(&mut self, now: f64, input: Input) -> Vec<Command> {
        let mut out = Vec::new();
        match input {
            Input::SetEnabled(on) => {
                self.enabled = on;
                out.push(Command::SaveEnabled(on));
            }
            Input::Panic => {
                out.extend(self.end(EndReason::Panic));
                if self.enabled {
                    self.enabled = false;
                    out.push(Command::SaveEnabled(false));
                }
                out.push(Command::Event {
                    kind: "panic",
                    detail: "Session Mode disabled".into(),
                });
            }
            Input::Stop => out.extend(self.end(EndReason::Stopped)),
            Input::Suspend => out.extend(self.end(EndReason::Suspend)),
            Input::ServiceStopping => out.extend(self.end(EndReason::ServiceStopping)),
            Input::Logoff => out.extend(self.end(EndReason::Logoff)),
            Input::ManualStart { proc, game } => {
                if matches!(self.state, State::Idle) {
                    out.push(Command::StartCapture { proc });
                    out.extend(self.activate(now, game, proc));
                } else {
                    out.push(Command::Event {
                        kind: "start_ignored",
                        detail: format!("{} (a session is already running)", game.exe),
                    });
                }
            }
            Input::ProcessStart { proc, game } => out.extend(self.on_start(now, proc, game)),
            Input::ProcessExit { proc } => out.extend(self.on_exit(now, proc)),
            Input::Frames { proc } => {
                if let State::Arming {
                    proc: p, frames, ..
                } = &mut self.state
                {
                    if *p == proc {
                        *frames = true;
                    }
                }
                out.extend(self.tick(now));
            }
            Input::Foreground { pid } => out.extend(self.on_foreground(now, pid)),
            Input::Tick => out.extend(self.tick(now)),
        }
        out
    }

    fn on_start(&mut self, now: f64, proc: ProcId, game: Option<GameRef>) -> Vec<Command> {
        let Some(game) = game else {
            return Vec::new();
        };
        match &mut self.state {
            State::Idle if self.enabled => {
                self.state = State::Arming {
                    game,
                    proc,
                    since: now,
                    frames: false,
                };
                vec![Command::StartCapture { proc }]
            }
            State::Idle => Vec::new(),
            State::Active {
                game: g,
                proc: p,
                exited_at,
                ..
            } if exited_at.is_some() && same_game(g, &game) => {
                // Relaunch inside the grace period: the same session continues.
                *p = proc;
                *exited_at = None;
                vec![
                    Command::StartCapture { proc },
                    Command::Event {
                        kind: "game_relaunched",
                        detail: game.exe,
                    },
                ]
            }
            _ => vec![Command::Event {
                kind: "second_game_ignored",
                detail: game.exe,
            }],
        }
    }

    fn on_exit(&mut self, now: f64, proc: ProcId) -> Vec<Command> {
        match &mut self.state {
            State::Arming { proc: p, .. } if *p == proc => {
                // Died during Arming (crash on start, splash screen): never a session.
                self.state = State::Idle;
                vec![Command::StopCapture { keep: false }]
            }
            State::Active {
                proc: p, exited_at, ..
            } if *p == proc && exited_at.is_none() => {
                *exited_at = Some(now);
                vec![Command::StopCapture { keep: true }]
            }
            _ => Vec::new(),
        }
    }

    fn on_foreground(&mut self, now: f64, pid: u32) -> Vec<Command> {
        let State::Active {
            proc,
            focus_lost_at,
            paused,
            exited_at: None,
            ..
        } = &mut self.state
        else {
            return Vec::new();
        };
        if pid == proc.pid {
            *focus_lost_at = None;
            if *paused {
                *paused = false;
                return vec![
                    Command::ResumeThrottling,
                    Command::Event {
                        kind: "foreground_regained",
                        detail: String::new(),
                    },
                ];
            }
        } else if focus_lost_at.is_none() {
            *focus_lost_at = Some(now);
        }
        Vec::new()
    }

    fn tick(&mut self, now: f64) -> Vec<Command> {
        match &mut self.state {
            State::Idle => Vec::new(),
            State::Arming {
                game,
                proc,
                since,
                frames,
            } => {
                let age = now - *since;
                if *frames && age >= ARMING_S {
                    let (game, proc) = (game.clone(), *proc);
                    self.activate(now, game, proc)
                } else if age >= ARMING_GIVE_UP_S {
                    let exe = game.exe.clone();
                    self.state = State::Idle;
                    vec![
                        Command::StopCapture { keep: false },
                        Command::Event {
                            kind: "arming_gave_up",
                            detail: format!("{exe} presented no frames"),
                        },
                    ]
                } else {
                    Vec::new()
                }
            }
            State::Active {
                exited_at: Some(t), ..
            } if now - *t >= RELAUNCH_S => self.end(EndReason::GameExited),
            State::Active {
                focus_lost_at: Some(t),
                paused,
                exited_at: None,
                ..
            } if !*paused && now - *t >= FOCUS_LOST_S => {
                *paused = true;
                vec![
                    Command::PauseThrottling,
                    Command::Event {
                        kind: "foreground_lost",
                        detail: String::new(),
                    },
                ]
            }
            State::Active { .. } => Vec::new(),
        }
    }

    fn activate(&mut self, now: f64, game: GameRef, proc: ProcId) -> Vec<Command> {
        let out = vec![
            Command::ApplyProfile {
                game: game.clone(),
                game_pid: proc.pid,
            },
            Command::StartWatcher { game_pid: proc.pid },
            Command::Event {
                kind: "session_started",
                detail: game.exe.clone(),
            },
            Command::Notify(format!("Verity: session started for {}", game.name)),
        ];
        self.state = State::Active {
            game,
            proc,
            since: now,
            exited_at: None,
            focus_lost_at: None,
            paused: false,
        };
        out
    }

    fn end(&mut self, reason: EndReason) -> Vec<Command> {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => Vec::new(),
            State::Arming { .. } => vec![Command::StopCapture { keep: false }],
            State::Active {
                game, exited_at, ..
            } => {
                let mut out = vec![Command::StopWatcher];
                if exited_at.is_none() {
                    out.push(Command::StopCapture { keep: true });
                }
                out.push(Command::Revert);
                out.push(Command::Finish { game, reason });
                out
            }
        }
    }
}

fn same_game(a: &GameRef, b: &GameRef) -> bool {
    match (a.id, b.id) {
        (Some(x), Some(y)) => x == y,
        _ => a.exe.eq_ignore_ascii_case(&b.exe),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> GameRef {
        GameRef {
            id: Some(1),
            name: "Framegen".into(),
            exe: "framegen.exe".into(),
        }
    }

    fn p(pid: u32) -> ProcId {
        ProcId {
            pid,
            start: 1000 + pid as u64,
        }
    }

    fn start(m: &mut Machine, t: f64, pid: u32) -> Vec<Command> {
        m.handle(
            t,
            Input::ProcessStart {
                proc: p(pid),
                game: Some(game()),
            },
        )
    }

    fn state(m: &Machine) -> &'static str {
        match m.status() {
            Status::Idle { .. } => "idle",
            Status::Arming { .. } => "arming",
            Status::Active { .. } => "active",
        }
    }

    fn has(cmds: &[Command], f: impl Fn(&Command) -> bool) -> bool {
        cmds.iter().any(f)
    }

    #[test]
    fn idle_arming_active_ending_idle() {
        let mut m = Machine::new(true);
        assert_eq!(
            start(&mut m, 0.0, 7),
            [Command::StartCapture { proc: p(7) }]
        );
        assert_eq!(state(&m), "arming");
        // Frames arrive early, but the profile waits for 10 s.
        assert!(m.handle(1.0, Input::Frames { proc: p(7) }).is_empty());
        assert!(m.handle(9.9, Input::Tick).is_empty());
        let c = m.handle(10.0, Input::Tick);
        assert!(matches!(c[0], Command::ApplyProfile { game_pid: 7, .. }));
        assert!(has(&c, |c| matches!(
            c,
            Command::StartWatcher { game_pid: 7 }
        )));
        assert_eq!(state(&m), "active");

        let c = m.handle(100.0, Input::ProcessExit { proc: p(7) });
        assert_eq!(c, [Command::StopCapture { keep: true }]);
        assert_eq!(state(&m), "active"); // waiting for a possible relaunch
        assert!(m.handle(114.9, Input::Tick).is_empty());
        let c = m.handle(115.0, Input::Tick);
        assert_eq!(
            c,
            [
                Command::StopWatcher,
                Command::Revert,
                Command::Finish {
                    game: game(),
                    reason: EndReason::GameExited
                }
            ]
        );
        assert_eq!(state(&m), "idle");
    }

    #[test]
    fn exit_within_ten_seconds_never_applies_a_profile() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        m.handle(2.0, Input::Frames { proc: p(7) });
        let c = m.handle(9.0, Input::ProcessExit { proc: p(7) });
        assert_eq!(c, [Command::StopCapture { keep: false }]);
        assert_eq!(state(&m), "idle");
        assert!(m.handle(20.0, Input::Tick).is_empty());
    }

    #[test]
    fn arming_waits_for_frames_then_gives_up() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        assert!(m.handle(60.0, Input::Tick).is_empty());
        // Late first frame (shader compilation at launch) still arms.
        let c = m.handle(61.0, Input::Frames { proc: p(7) });
        assert!(matches!(c[0], Command::ApplyProfile { .. }));

        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        let c = m.handle(ARMING_GIVE_UP_S, Input::Tick);
        assert_eq!(c[0], Command::StopCapture { keep: false });
        assert_eq!(state(&m), "idle");
    }

    #[test]
    fn frames_from_another_process_do_not_arm() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        let other = ProcId { pid: 7, start: 1 }; // pid reused, different process
        m.handle(1.0, Input::Frames { proc: other });
        assert!(m.handle(30.0, Input::Tick).is_empty());
        assert_eq!(state(&m), "arming");
    }

    #[test]
    fn relaunch_within_grace_continues_the_session() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        m.handle(10.0, Input::Frames { proc: p(7) });
        m.handle(50.0, Input::ProcessExit { proc: p(7) });
        let c = start(&mut m, 55.0, 8);
        assert_eq!(c[0], Command::StartCapture { proc: p(8) });
        assert!(!has(&c, |c| matches!(c, Command::ApplyProfile { .. })));
        assert!(m.handle(80.0, Input::Tick).is_empty());
        assert_eq!(state(&m), "active");
        // The old process's exit event arriving late changes nothing.
        assert!(m.handle(81.0, Input::ProcessExit { proc: p(7) }).is_empty());
    }

    #[test]
    fn second_game_is_recorded_and_ignored() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        m.handle(10.0, Input::Frames { proc: p(7) });
        let other = GameRef {
            id: Some(2),
            name: "Other".into(),
            exe: "other.exe".into(),
        };
        let c = m.handle(
            20.0,
            Input::ProcessStart {
                proc: p(9),
                game: Some(other),
            },
        );
        assert_eq!(
            c,
            [Command::Event {
                kind: "second_game_ignored",
                detail: "other.exe".into()
            }]
        );
    }

    #[test]
    fn non_games_and_disabled_mode_do_nothing() {
        let mut m = Machine::new(true);
        assert!(m
            .handle(
                0.0,
                Input::ProcessStart {
                    proc: p(3),
                    game: None
                }
            )
            .is_empty());
        let mut m = Machine::new(false);
        assert!(start(&mut m, 0.0, 7).is_empty());
        // A manual start still works while automatic mode is off.
        let c = m.handle(
            1.0,
            Input::ManualStart {
                proc: p(7),
                game: game(),
            },
        );
        assert_eq!(c[0], Command::StartCapture { proc: p(7) });
        assert!(matches!(c[1], Command::ApplyProfile { .. }));
    }

    #[test]
    fn panic_reverts_and_disables() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        m.handle(10.0, Input::Frames { proc: p(7) });
        let c = m.handle(20.0, Input::Panic);
        assert_eq!(
            &c[..5],
            [
                Command::StopWatcher,
                Command::StopCapture { keep: true },
                Command::Revert,
                Command::Finish {
                    game: game(),
                    reason: EndReason::Panic
                },
                Command::SaveEnabled(false),
            ]
        );
        assert!(!m.enabled());
        assert!(start(&mut m, 30.0, 8).is_empty());
        // Panic while idle still disables and does not revert anything.
        let mut m = Machine::new(true);
        let c = m.handle(0.0, Input::Panic);
        assert!(!has(&c, |c| matches!(c, Command::Revert)));
        assert!(!m.enabled());
    }

    #[test]
    fn suspend_logoff_and_service_stop_end_the_session() {
        for (input, reason) in [
            (Input::Suspend, EndReason::Suspend),
            (Input::Logoff, EndReason::Logoff),
            (Input::ServiceStopping, EndReason::ServiceStopping),
            (Input::Stop, EndReason::Stopped),
        ] {
            let mut m = Machine::new(true);
            start(&mut m, 0.0, 7);
            m.handle(10.0, Input::Frames { proc: p(7) });
            let c = m.handle(11.0, input);
            assert!(has(&c, |c| *c == Command::Revert));
            assert!(has(
                &c,
                |c| matches!(c, Command::Finish { reason: r, .. } if *r == reason)
            ));
            assert_eq!(state(&m), "idle");
            assert!(m.enabled());
        }
        // Ending while Arming only discards the capture.
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        assert_eq!(
            m.handle(1.0, Input::Suspend),
            [Command::StopCapture { keep: false }]
        );
    }

    #[test]
    fn focus_loss_pauses_throttling_after_a_minute() {
        let mut m = Machine::new(true);
        start(&mut m, 0.0, 7);
        m.handle(10.0, Input::Frames { proc: p(7) });
        assert!(m.handle(20.0, Input::Foreground { pid: 99 }).is_empty());
        assert!(m.handle(79.0, Input::Tick).is_empty());
        let c = m.handle(80.0, Input::Tick);
        assert_eq!(c[0], Command::PauseThrottling);
        assert!(m.handle(81.0, Input::Tick).is_empty()); // only once
        let c = m.handle(90.0, Input::Foreground { pid: 7 });
        assert_eq!(c[0], Command::ResumeThrottling);
        // Brief focus loss does not pause.
        m.handle(100.0, Input::Foreground { pid: 99 });
        m.handle(120.0, Input::Foreground { pid: 7 });
        assert!(m.handle(200.0, Input::Tick).is_empty());
    }
}
