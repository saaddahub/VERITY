//! Session Mode in the service (docs/06-session-mode.md): watches process
//! starts, drives the state machine, runs the capture for each game session,
//! and stores the result with its Frame Story events.
//!
//! One task owns all session state; everything else talks to it through
//! [`Msg`]s, so there are no locks around the machine.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use verity_core::metrics::Thresholds;
use verity_core::telemetry::{
    CaptureGap, ProcessApi, ProcessEntry, SysEvent, STANDARD_OVERHEAD_BUDGET_PCT,
};
use verity_core::SystemInfoApi;
use verity_session::library::never_a_game;
use verity_session::machine::{Command, EndReason, GameRef, Input, Machine, ProcId};
use verity_session::profile::{self, Profile};
use verity_store::{NewGame, NewSession, Store};
use verity_sys::etw::{Config, Provider, Trace};
use verity_sys::pdh::PdhCounters;
use verity_sys::process::{qpc, qpc_freq, WinProcesses};
use verity_sys::WinSystemInfo;
use verity_telemetry::capture::{self, Capture, CaptureConfig, CaptureControl};
use verity_telemetry::kernel::{self, ProcEvent};

use crate::story_capture::StoryCapture;

pub const PROC_SESSION: &str = "VerityProc";
const PROC_GUID: u128 = 0x7e5a_0001_6c3d_4b1e_9a40_56e1_0f2b_7c91;
const SETTING_ENABLED: &str = "session_mode.enabled";
/// A candidate must stay in front with 3D load this many seconds before Verity asks.
const ASK_AFTER_S: u32 = 10;
const ASK_GPU_PCT: f32 = 20.0;
const MAX_NOTICES: usize = 50;

/// Session-scoped system changes (M3, `crate::actions::JournalActions`).
pub trait SessionActions: Send {
    fn apply(&mut self, game: ProcId, profile: &Profile);
    fn on_new_process(&mut self, pid: u32, create_time: u64);
    /// The agent's foreground and audio reports, for the exclusion list.
    fn foreground(&mut self, pid: u32);
    fn audio(&mut self, pids: Vec<u32>);
    fn pause(&mut self);
    fn resume(&mut self);
    /// Puts back everything; a line for the user when something was kept or failed.
    fn revert_all(&mut self) -> Option<String>;
    /// Once a second: retries reverts that had to wait.
    fn tick(&mut self);
}

pub enum Msg {
    Proc(ProcEvent),
    /// A machine input; the sender (if any) is told once its commands ran.
    Input(Input, Option<oneshot::Sender<()>>),
    /// `session start` by pid or exe name.
    ManualStart(
        Option<u32>,
        Option<String>,
        oneshot::Sender<Result<(), String>>,
    ),
    Foreground(u32),
    /// Processes with an active audio session, from the agent.
    Audio(Vec<u32>),
    LibraryChanged,
    Answer {
        exe: String,
        answer: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub id: u64,
    /// "info", "session_end" or "ask_game".
    pub kind: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}

/// What `session.status` reports; written by the runner, read by IPC calls.
#[derive(Default)]
pub struct View {
    pub status: Value,
    pub notices: VecDeque<Notice>,
    next_id: u64,
    pub capabilities: Vec<String>,
}

impl View {
    fn notice(
        &mut self,
        kind: &'static str,
        text: String,
        session_id: Option<i64>,
        exe: Option<String>,
    ) {
        self.next_id += 1;
        self.notices.push_back(Notice {
            id: self.next_id,
            kind,
            text,
            session_id,
            exe,
        });
        while self.notices.len() > MAX_NOTICES {
            self.notices.pop_front();
        }
    }
}

#[derive(Clone)]
pub struct Handle {
    pub tx: mpsc::UnboundedSender<Msg>,
    pub view: Arc<Mutex<View>>,
}

impl Handle {
    /// Sends an input and waits until its commands have run (bounded).
    pub async fn input(&self, input: Input) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Input(input, Some(tx))).is_err() {
            return false;
        }
        tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .is_ok()
    }
}

struct Running {
    proc: ProcId,
    stop: watch::Sender<bool>,
    frames: Arc<AtomicU64>,
    task: JoinHandle<Result<Capture, String>>,
    story: Option<StoryCapture>,
    _lock: tokio::sync::OwnedMutexGuard<()>,
}

/// One finished segment: the frames of one process, and its system events.
struct Segment {
    capture: Capture,
    events: Vec<SysEvent>,
    caveats: Vec<String>,
}

/// A session being recorded (from Arming on; dropped if Arming fails).
struct Acc {
    proc: ProcId,
    started_utc: String,
    profile: Profile,
    standard: bool,
    pending: Vec<JoinHandle<Option<Segment>>>,
    events: Vec<SysEvent>,
    background_since: Option<u64>,
}

struct Runner {
    store: Arc<Mutex<Store>>,
    capturing: Arc<tokio::sync::Mutex<()>>,
    presentmon: PathBuf,
    view: Arc<Mutex<View>>,
    tx: mpsc::UnboundedSender<Msg>,
    machine: Machine,
    actions: Box<dyn SessionActions>,
    library: Vec<(String, GameRef)>,
    devices: Vec<(String, String)>,
    procs: HashMap<u32, (u64, String)>,
    running: Option<Running>,
    /// A capture to start once the previous segment of this session has
    /// released the capture lock (a relaunch right after an exit).
    wanted: Option<ProcId>,
    acc: Option<Acc>,
    finishers: Vec<JoinHandle<()>>,
    watcher: bool,
    foreground: Option<u32>,
    gpu: Option<PdhCounters>,
    ask_streak: (u32, u32),
    asked: HashSet<String>,
}

fn now_s() -> f64 {
    qpc() as f64 / qpc_freq().max(1) as f64
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Starts the runner and the process watcher. Returns the handle IPC uses.
pub fn start(
    store: Arc<Mutex<Store>>,
    capturing: Arc<tokio::sync::Mutex<()>>,
    presentmon: PathBuf,
    startup_notice: Option<String>,
) -> (Handle, JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let view = Arc::new(Mutex::new(View::default()));
    if let Some(text) = startup_notice {
        lock(&view).notice("info", text, None, None);
    }
    let enabled = lock(&store)
        .setting(SETTING_ENABLED)
        .ok()
        .flatten()
        .is_none_or(|v| v != "0");
    let actions = Box::new(crate::actions::JournalActions::new(store.clone()));
    let runner = Runner {
        store,
        capturing,
        presentmon,
        view: view.clone(),
        tx: tx.clone(),
        machine: Machine::new(enabled),
        actions,
        library: Vec::new(),
        devices: verity_sys::games::dos_devices(),
        procs: HashMap::new(),
        running: None,
        wanted: None,
        acc: None,
        finishers: Vec::new(),
        watcher: false,
        foreground: None,
        gpu: None,
        ask_streak: (0, 0),
        asked: HashSet::new(),
    };
    let task = tokio::spawn(runner.run(rx));
    (Handle { tx, view }, task)
}

impl Runner {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        let proc_trace = self.start_process_watch();
        self.refresh_library();
        let store = self.store.clone();
        let tx = self.tx.clone();
        // The library scan reads launcher files; keep it off the runner.
        tokio::task::spawn_blocking(move || {
            if let Err(e) = scan_library(&store) {
                tracing::warn!("library scan failed: {e}");
            }
            let _ = tx.send(Msg::LibraryChanged);
        });

        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(msg) = msg else { break };
                    let stopping = matches!(msg, Msg::Input(Input::ServiceStopping, _));
                    self.on_msg(msg).await;
                    if stopping {
                        break;
                    }
                }
                _ = tick.tick() => self.on_tick().await,
            }
            self.publish();
        }
        drop(proc_trace);
    }

    fn start_process_watch(&mut self) -> Option<Trace> {
        let tx = self.tx.clone();
        match Trace::start(
            Config {
                name: PROC_SESSION,
                guid: PROC_GUID,
                kernel_flags: 0,
                providers: vec![Provider {
                    guid: kernel::KERNEL_PROCESS,
                    keywords: kernel::KERNEL_PROCESS_KEYWORD,
                    level: 4,
                }],
            },
            Box::new(move |e| {
                if let Some(p) = kernel::decode_process(e) {
                    let _ = tx.send(Msg::Proc(p));
                }
            }),
        ) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!("process watch unavailable: {e}");
                lock(&self.view)
                    .capabilities
                    .push(format!("Automatic game detection unavailable: {e}"));
                None
            }
        }
    }

    fn publish(&self) {
        let mut status = serde_json::to_value(self.machine.status()).unwrap_or(Value::Null);
        if let (Some(r), Value::Object(map)) = (&self.running, &mut status) {
            map.insert(
                "frames".into(),
                Value::from(r.frames.load(Ordering::Relaxed)),
            );
        }
        lock(&self.view).status = status;
    }

    fn notice(
        &self,
        kind: &'static str,
        text: String,
        session_id: Option<i64>,
        exe: Option<String>,
    ) {
        lock(&self.view).notice(kind, text, session_id, exe);
    }

    fn refresh_library(&mut self) {
        let games = lock(&self.store).games().unwrap_or_default();
        self.library = games
            .into_iter()
            .map(|g| {
                let exe = kernel::file_name(&g.exe_path).to_string();
                (
                    g.exe_path.to_lowercase(),
                    GameRef {
                        id: Some(g.id),
                        name: g.name,
                        exe,
                    },
                )
            })
            .collect();
    }

    /// `\Device\HarddiskVolume3\x.exe` → `c:\x.exe` (lowercase).
    fn dos_path(&mut self, nt: &str) -> Option<String> {
        let lower = nt.to_lowercase();
        for attempt in 0..2 {
            for (device, drive) in &self.devices {
                let d = device.to_lowercase();
                if lower.starts_with(&d) && lower[d.len()..].starts_with('\\') {
                    return Some(format!("{}{}", drive.to_lowercase(), &lower[d.len()..]));
                }
            }
            if attempt == 0 {
                self.devices = verity_sys::games::dos_devices();
            }
        }
        None
    }

    /// The process the machine is arming or running a session for.
    fn session_proc(&self) -> Option<ProcId> {
        match self.machine.status() {
            verity_session::machine::Status::Arming { proc, .. }
            | verity_session::machine::Status::Active { proc, .. } => Some(proc),
            verity_session::machine::Status::Idle { .. } => None,
        }
    }

    fn game_for(&mut self, nt_image: &str) -> Option<GameRef> {
        let dos = self.dos_path(nt_image)?;
        self.library
            .iter()
            .find(|(path, _)| *path == dos)
            .map(|(_, g)| g.clone())
    }

    async fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Proc(ProcEvent::Start {
                pid,
                create_time,
                image,
                qpc: started,
            }) => {
                self.procs.insert(pid, (create_time, image.clone()));
                if self.procs.len() > 20_000 {
                    self.procs.retain(|p, _| *p == pid);
                }
                if let Some(r) = &self.running {
                    if let Some(s) = &r.story {
                        s.process_started(pid, &image);
                    }
                }
                if let Some(acc) = &mut self.acc {
                    acc.events.push(SysEvent {
                        qpc: started,
                        duration_ticks: 0,
                        kind: "proc_start".into(),
                        subject: kernel::file_name(&image).to_string(),
                        magnitude: 1.0,
                    });
                }
                if self.watcher {
                    self.actions.on_new_process(pid, create_time);
                }
                let game = self.game_for(&image);
                let proc = ProcId {
                    pid,
                    start: create_time,
                };
                self.feed(Input::ProcessStart { proc, game }).await;
            }
            Msg::Proc(ProcEvent::Stop { pid, create_time }) => {
                if self.procs.get(&pid).is_some_and(|p| p.0 == create_time) {
                    self.procs.remove(&pid);
                }
                let mut proc = ProcId {
                    pid,
                    start: create_time,
                };
                // A process that was running before the watcher started (a
                // manual start after a service restart) is held with start 0
                // until this event brings its creation time. Its pid cannot be
                // reused while it lives, so the first stop for that pid is its own.
                let unknown = ProcId { pid, start: 0 };
                if self.session_proc() == Some(unknown) {
                    if let Some(acc) = self.acc.as_mut().filter(|a| a.proc == unknown) {
                        acc.proc.start = create_time;
                    }
                    proc = unknown;
                }
                self.feed(Input::ProcessExit { proc }).await;
            }
            Msg::Input(input, ack) => {
                // Commands run synchronously here, so by the time the ack is
                // sent every session change has been reverted.
                self.feed(input.clone()).await;
                if matches!(input, Input::ServiceStopping) {
                    // Store what was recorded before the service goes away.
                    let finishers = std::mem::take(&mut self.finishers);
                    let _ = tokio::time::timeout(Duration::from_secs(20), async {
                        for f in finishers {
                            let _ = f.await;
                        }
                    })
                    .await;
                }
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
            }
            Msg::ManualStart(pid, exe, reply) => {
                let r = self.manual_start(pid, exe.as_deref()).await;
                let _ = reply.send(r);
            }
            Msg::Foreground(pid) => {
                self.actions.foreground(pid);
                self.on_foreground(pid).await
            }
            Msg::Audio(pids) => self.actions.audio(pids),
            Msg::LibraryChanged => self.refresh_library(),
            Msg::Answer { exe, answer } => self.on_answer(&exe, &answer).await,
        }
    }

    async fn manual_start(&mut self, pid: Option<u32>, exe: Option<&str>) -> Result<(), String> {
        let all = WinProcesses.processes().map_err(|e| e.to_string())?;
        let target = verity_core::telemetry::resolve_target(&all, pid, exe)?;
        let game = self
            .library
            .iter()
            .find(|(path, _)| kernel::file_name(path).eq_ignore_ascii_case(&target.name))
            .map(|(_, g)| g.clone())
            .unwrap_or(GameRef {
                id: None,
                name: target.name.clone(),
                exe: target.name.clone(),
            });
        // 0 when the process started before the watcher; its stop event fills
        // in the creation time (see the `ProcEvent::Stop` handling).
        let start = self.procs.get(&target.pid).map_or(0, |p| p.0);
        if !matches!(
            self.machine.status(),
            verity_session::machine::Status::Idle { .. }
        ) {
            return Err("a session is already running; stop it first".into());
        }
        self.feed(Input::ManualStart {
            proc: ProcId {
                pid: target.pid,
                start,
            },
            game,
        })
        .await;
        Ok(())
    }

    async fn on_foreground(&mut self, pid: u32) {
        if self.foreground == Some(pid) {
            return;
        }
        self.foreground = Some(pid);
        self.ask_streak = (pid, 0);
        let t = qpc();
        if let Some(acc) = &mut self.acc {
            acc.events.push(SysEvent {
                qpc: t,
                duration_ticks: 0,
                kind: "foreground".into(),
                subject: "focus change".into(),
                magnitude: 1.0,
            });
            let game_pid = acc.proc.pid;
            match (pid == game_pid, acc.background_since) {
                (true, Some(since)) => {
                    acc.events.push(SysEvent {
                        qpc: since,
                        duration_ticks: t.saturating_sub(since),
                        kind: "background".into(),
                        subject: "game not in focus".into(),
                        magnitude: 1.0,
                    });
                    acc.background_since = None;
                }
                (false, None) => acc.background_since = Some(t),
                _ => {}
            }
        }
        self.feed(Input::Foreground { pid }).await;
    }

    async fn on_tick(&mut self) {
        self.actions.tick();
        if let Some(proc) = self.wanted.take() {
            if self.running.is_none() && self.session_proc() == Some(proc) {
                self.start_capture(proc);
            }
        }
        let mut inputs = vec![Input::Tick];
        if let Some(r) = &self.running {
            if r.frames.load(Ordering::Relaxed) > 0 {
                inputs.insert(0, Input::Frames { proc: r.proc });
            }
            // Safety net if process events are unavailable: the capture ends
            // with its process, so treat that as the exit.
            if r.task.is_finished() {
                inputs.push(Input::ProcessExit { proc: r.proc });
            }
        }
        for i in inputs {
            self.feed(i).await;
        }
        self.finishers.retain(|f| !f.is_finished());
        self.check_candidate();
    }

    /// The heuristic: a foreground process with 3D load for 10 s that is not
    /// in the library, not on the never list and not answered gets one question.
    fn check_candidate(&mut self) {
        let Some(fg) = self.foreground else { return };
        if !matches!(
            self.machine.status(),
            verity_session::machine::Status::Idle { .. }
        ) {
            return;
        }
        let Some((_, image)) = self.procs.get(&fg).cloned() else {
            return;
        };
        let exe = kernel::file_name(&image).to_string();
        let key = exe.to_lowercase();
        if never_a_game(&exe) || self.asked.contains(&key) || self.game_for(&image).is_some() {
            return;
        }
        if lock(&self.store).game_answer(&exe).ok().flatten().is_some() {
            return;
        }
        if self.gpu.is_none() {
            self.gpu = PdhCounters::new().ok();
        }
        let Some(gpu) = self.gpu.as_mut() else { return };
        let Ok(sample) = verity_core::telemetry::CounterApi::sample(gpu) else {
            return;
        };
        let load: f32 = sample
            .gpu_engines
            .iter()
            .filter(|(i, _)| i.ends_with("engtype_3D"))
            .filter(|(i, _)| {
                verity_core::telemetry::parse_gpu_engine(i).is_some_and(|(p, _)| p == fg)
            })
            .map(|(_, v)| v)
            .sum();
        if load > ASK_GPU_PCT {
            self.ask_streak.1 += 1;
        } else {
            self.ask_streak.1 = 0;
        }
        if self.ask_streak.1 >= ASK_AFTER_S {
            self.asked.insert(key);
            self.notice("ask_game", format!("Is {exe} a game?"), None, Some(exe));
        }
    }

    async fn on_answer(&mut self, exe: &str, answer: &str) {
        let image = self
            .procs
            .iter()
            .find(|(_, (_, img))| kernel::file_name(img).eq_ignore_ascii_case(exe))
            .map(|(pid, (ct, img))| (*pid, *ct, img.clone()));
        if answer == "game" {
            if let Some((pid, ct, img)) = image {
                if let Some(dos) = self.dos_path(&img) {
                    let name = Path::new(exe)
                        .file_stem()
                        .map_or_else(|| exe.to_string(), |s| s.to_string_lossy().into_owned());
                    let added = lock(&self.store).add_game(&NewGame {
                        name: &name,
                        exe_path: &dos,
                        source: "manual",
                        launcher_id: None,
                    });
                    if added.is_ok() {
                        self.refresh_library();
                        // It is running now: start the session as if it had just launched.
                        let game = self.game_for(&img);
                        self.feed(Input::ProcessStart {
                            proc: ProcId { pid, start: ct },
                            game,
                        })
                        .await;
                    }
                }
            }
        }
        let _ = lock(&self.store).set_game_answer(exe, answer);
    }

    async fn feed(&mut self, input: Input) {
        let cmds = self.machine.handle(now_s(), input);
        for c in cmds {
            self.exec(c).await;
        }
    }

    fn profile_for(&self, game: &GameRef) -> Profile {
        let store = lock(&self.store);
        let json = game
            .id
            .and_then(|id| store.profile(Some(id)).ok().flatten())
            .or_else(|| store.profile(None).ok().flatten());
        profile::load(json.as_deref())
    }

    async fn exec(&mut self, cmd: Command) {
        match cmd {
            Command::StartCapture { proc } => self.start_capture(proc),
            Command::StopCapture { keep } => self.stop_capture(keep),
            Command::ApplyProfile { game, game_pid } => {
                let profile = self.profile_for(&game);
                let proc = self
                    .session_proc()
                    .filter(|p| p.pid == game_pid)
                    .unwrap_or(ProcId {
                        pid: game_pid,
                        start: 0,
                    });
                self.actions.apply(proc, &profile);
            }
            Command::Revert => {
                if let Some(text) = self.actions.revert_all() {
                    self.notice("info", text, None, None);
                }
            }
            Command::StartWatcher { .. } => self.watcher = true,
            Command::StopWatcher => self.watcher = false,
            Command::PauseThrottling => self.actions.pause(),
            Command::ResumeThrottling => self.actions.resume(),
            Command::Finish { game, reason } => self.finish(game, reason),
            Command::Event { kind, detail } => {
                tracing::info!(kind, %detail, "session event");
                if let Some(acc) = &mut self.acc {
                    acc.events.push(SysEvent {
                        qpc: qpc(),
                        duration_ticks: 0,
                        kind: kind.into(),
                        subject: detail,
                        magnitude: 0.0,
                    });
                }
            }
            Command::Notify(text) => self.notice("info", text, None, None),
            Command::SaveEnabled(on) => {
                let _ = lock(&self.store).set_setting(SETTING_ENABLED, if on { "1" } else { "0" });
                if !on {
                    self.notice(
                        "info",
                        "Session Mode is off. Turn it back on in Verity when you are ready.".into(),
                        None,
                        None,
                    );
                }
            }
        }
    }

    fn start_capture(&mut self, proc: ProcId) {
        let Ok(lock_guard) = self.capturing.clone().try_lock_owned() else {
            // This session's previous segment still holds the lock while its
            // PresentMon and kernel trace shut down: start once it lets go.
            let ours = self
                .acc
                .as_ref()
                .is_some_and(|a| a.pending.iter().any(|p| !p.is_finished()));
            if ours {
                self.wanted = Some(proc);
                return;
            }
            self.notice(
                "info",
                "Verity is already recording; this session runs without a capture.".into(),
                None,
                None,
            );
            return;
        };
        let game = match self.machine.status() {
            verity_session::machine::Status::Arming { game, .. }
            | verity_session::machine::Status::Active { game, .. } => game,
            verity_session::machine::Status::Idle { .. } => GameRef {
                id: None,
                name: String::new(),
                exe: String::new(),
            },
        };
        if self.acc.is_none() {
            let profile = self.profile_for(&game);
            self.acc = Some(Acc {
                standard: profile.capture_mode != "light",
                profile,
                proc,
                started_utc: verity_store::utc_now(),
                pending: Vec::new(),
                events: Vec::new(),
                background_since: None,
            });
        }
        let standard = self.acc.as_ref().is_some_and(|a| a.standard);
        if let Some(acc) = &mut self.acc {
            acc.proc = proc;
        }
        let (stop_tx, stop_rx) = watch::channel(false);
        let frames = Arc::new(AtomicU64::new(0));
        let cfg = CaptureConfig {
            presentmon: self.presentmon.clone(),
            pid: proc.pid,
            duration: None,
            sample_period: Duration::from_secs(1),
            control: Some(CaptureControl {
                stop: stop_rx,
                frames: frames.clone(),
            }),
        };
        let task = tokio::spawn(async move {
            let mut counters = PdhCounters::new().map_err(|e| e.to_string())?;
            capture::run(&cfg, &mut counters, &WinProcesses)
                .await
                .map_err(|e| e.to_string())
        });
        let story = standard.then(|| StoryCapture::start(proc.pid));
        self.running = Some(Running {
            proc,
            stop: stop_tx,
            frames,
            task,
            story,
            _lock: lock_guard,
        });
    }

    fn stop_capture(&mut self, keep: bool) {
        self.wanted = None;
        let Some(r) = self.running.take() else {
            if !keep {
                self.acc = None;
            }
            return;
        };
        let _ = r.stop.send(true);
        let Running {
            task, story, _lock, ..
        } = r;
        let seg = tokio::spawn(async move {
            let capture = task.await.ok().and_then(|c| match c {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!("capture failed: {e}");
                    None
                }
            });
            let (events, caveats) = match story {
                Some(s) => tokio::task::spawn_blocking(move || s.finish())
                    .await
                    .unwrap_or_default(),
                None => (Vec::new(), Vec::new()),
            };
            drop(_lock);
            capture.map(|capture| Segment {
                capture,
                events,
                caveats,
            })
        });
        if keep {
            if let Some(acc) = &mut self.acc {
                acc.pending.push(seg);
            }
        } else {
            self.acc = None;
        }
    }

    fn finish(&mut self, game: GameRef, reason: EndReason) {
        let Some(mut acc) = self.acc.take() else {
            return;
        };
        if let Some(since) = acc.background_since.take() {
            acc.events.push(SysEvent {
                qpc: since,
                duration_ticks: qpc().saturating_sub(since),
                kind: "background".into(),
                subject: "game not in focus".into(),
                magnitude: 1.0,
            });
        }
        let store = self.store.clone();
        let view = self.view.clone();
        let task = tokio::spawn(async move {
            let mut segments = Vec::new();
            for p in acc.pending.drain(..) {
                if let Ok(Some(s)) = p.await {
                    segments.push(s);
                }
            }
            match store_session(&store, &acc, &game, segments) {
                Ok(Some((id, line))) => lock(&view).notice("session_end", line, Some(id), None),
                Ok(None) => lock(&view).notice(
                    "info",
                    format!("Session for {} ended with no frames recorded.", game.name),
                    None,
                    None,
                ),
                Err(e) => {
                    tracing::error!("storing session failed: {e}");
                    lock(&view).notice(
                        "info",
                        format!("Could not save the session: {e}"),
                        None,
                        None,
                    );
                }
            }
            tracing::info!(?reason, game = %game.exe, "session finished");
        });
        self.finishers.push(task);
    }
}

fn scan_library(store: &Mutex<Store>) -> Result<(), String> {
    let roots = verity_session::real_roots();
    let found = verity_session::library::scan(&verity_session::StdFs, &roots);
    let rows: Vec<(String, String, &'static str, Option<String>)> = found
        .into_iter()
        .map(|g| {
            (
                g.name,
                g.exe_path.to_string_lossy().into_owned(),
                g.source.as_str(),
                g.launcher_id,
            )
        })
        .collect();
    let new: Vec<NewGame> = rows
        .iter()
        .map(|(n, p, s, l)| NewGame {
            name: n,
            exe_path: p,
            source: s,
            launcher_id: l.as_deref(),
        })
        .collect();
    let r = lock(store)
        .sync_games(&["steam", "epic", "gog", "xbox"], &new, &uninstalled)
        .map_err(|e| e.to_string())?;
    tracing::info!(?r, "library scanned");
    Ok(())
}

/// A game the scan did not find is really uninstalled only when its drive is
/// there and its exe is not. One on an unplugged drive, or still on disk while
/// its launcher updates it, keeps its entry and profile.
fn uninstalled(exe_path: &str) -> bool {
    let p = Path::new(exe_path);
    p.ancestors().last().is_some_and(Path::exists) && !p.exists()
}

/// Public for `games.scan`.
pub fn rescan(store: &Mutex<Store>) -> Result<(), String> {
    scan_library(store)
}

/// Joins relaunch segments into one capture, with a gap between them.
fn merge(mut segs: Vec<Capture>) -> Option<Capture> {
    let mut out = segs.drain(..1.min(segs.len())).next()?;
    for s in segs {
        let gap_start = out.frames.last().map_or(out.end_qpc, |f| f.qpc);
        let gap_end = s.frames.first().map_or(s.start_qpc, |f| f.qpc);
        out.gaps.push(CaptureGap {
            qpc: gap_start,
            duration_ticks: gap_end.saturating_sub(gap_start),
        });
        out.end_qpc = s.end_qpc;
        out.frames.extend(s.frames);
        out.gaps.extend(s.gaps);
        out.samples.extend(s.samples);
        out.unparsed_rows += s.unparsed_rows;
        out.cpu_100ns += s.cpu_100ns;
    }
    Some(out)
}

/// Stores the session and returns its id and the one-line result.
fn store_session(
    store: &Mutex<Store>,
    acc: &Acc,
    game: &GameRef,
    segments: Vec<Segment>,
) -> Result<Option<(i64, String)>, String> {
    let mut events = acc.events.clone();
    let mut caveats = Vec::new();
    let mut caps = Vec::new();
    for s in segments {
        events.extend(s.events);
        caveats.extend(s.caveats);
        caps.push(s.capture);
    }
    let Some(cap) = merge(caps) else {
        return Ok(None);
    };
    if cap.frames.is_empty() {
        return Ok(None);
    }
    let info = WinSystemInfo.system_info().map_err(|e| e.to_string())?;
    let freq = qpc_freq();
    let target = ProcessEntry {
        pid: acc.proc.pid,
        name: game.exe.clone(),
    };
    let mut summary = capture::summarize_capture(
        &cap,
        &target,
        &info.gpus,
        freq,
        info.cpu.logical.len(),
        &Thresholds::default(),
    );
    if acc.standard {
        summary.capture_mode = acc.profile.capture_mode.clone();
        summary.overhead_budget_pct = STANDARD_OVERHEAD_BUDGET_PCT;
    }
    caveats.sort();
    caveats.dedup();
    events.extend(caveats.into_iter().map(|c| SysEvent {
        qpc: cap.start_qpc,
        duration_ticks: 0,
        kind: "caveat".into(),
        subject: c,
        magnitude: 0.0,
    }));
    events.sort_by_key(|e| e.qpc);
    let profile_json = serde_json::to_string(&acc.profile).ok();
    let mut s = lock(store);
    // The game may have been removed from the library during the session.
    let game_id = game.id.filter(|&id| s.game(id).is_ok());
    let id = s
        .insert_session(&NewSession {
            pid: acc.proc.pid,
            started_qpc: cap.start_qpc,
            ended_qpc: cap.end_qpc,
            qpc_freq: freq,
            started_utc: &acc.started_utc,
            summary: &summary,
            frames: &cap.frames,
            series: &capture::series(&cap, acc.proc.pid),
            gaps: &cap.gaps,
            process_start_filetime: acc.proc.start,
            game_id,
            profile_json: profile_json.as_deref(),
            events: &events,
        })
        .map_err(|e| e.to_string())?;
    drop(s);
    let story = crate::story::for_session(&lock(store), id).ok();
    let mut line = format!(
        "{}: {:.0} FPS average, {:.0} FPS 1% low",
        game.name, summary.stats.avg_fps, summary.stats.low_1pct_fps
    );
    if let Some(top) = story.as_ref().and_then(|s| s.story.causes.first()) {
        line.push_str(&format!(
            ". Stutter coincides with {} ({})",
            top.subject,
            verity_story::html::kind_label(&top.kind).to_lowercase()
        ));
    }
    Ok(Some((id, line)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use verity_core::metrics::Frame;

    fn frame(qpc: u64) -> Frame {
        Frame {
            qpc,
            frame_time_ms: 16.0,
            cpu_busy_ms: 1.0,
            gpu_busy_ms: 1.0,
            displayed_time_ms: f32::NAN,
            display_latency_ms: f32::NAN,
            present_mode: 0,
            flags: 0,
        }
    }

    #[test]
    fn only_a_missing_exe_on_a_present_drive_is_uninstalled() {
        let sys = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(!uninstalled(&format!(r"{sys}\System32\cmd.exe")));
        assert!(uninstalled(&format!(r"{sys}\no-such-game\game.exe")));
        let absent = ('D'..='Z')
            .rev()
            .map(|d| format!(r"{d}:\"))
            .find(|root| !Path::new(root).exists())
            .expect("a free drive letter");
        assert!(!uninstalled(&format!(r"{absent}Games\game.exe")));
    }

    #[test]
    fn relaunch_segments_merge_with_a_gap() {
        let a = Capture {
            start_qpc: 0,
            end_qpc: 100,
            frames: vec![frame(10), frame(90)],
            cpu_100ns: 5,
            ..Default::default()
        };
        let b = Capture {
            start_qpc: 200,
            end_qpc: 300,
            frames: vec![frame(250)],
            cpu_100ns: 7,
            ..Default::default()
        };
        let m = merge(vec![a, b]).unwrap();
        assert_eq!((m.start_qpc, m.end_qpc), (0, 300));
        assert_eq!(m.frames.len(), 3);
        assert_eq!(
            m.gaps,
            [CaptureGap {
                qpc: 90,
                duration_ticks: 160
            }]
        );
        assert_eq!(m.cpu_100ns, 12);
        assert!(merge(vec![]).is_none());
    }
}
