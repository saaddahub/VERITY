//! Everything a Standard capture records beside the frames: the private
//! kernel trace, the 4 Hz sampler and the event-log watchers. Started with the
//! frame capture, finished into Frame Story events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use verity_core::telemetry::{ProcessApi, SysEvent};
use verity_sys::etw::{Config, Trace};
use verity_sys::evtlog::{self, Subscription};
use verity_sys::process::{filetime_now, qpc, qpc_freq, WinProcesses};
use verity_sys::story_sampler::{StorySample, StorySampler};
use verity_telemetry::derive::{self, LogRecord};
use verity_telemetry::kernel::{Collector, STANDARD_FLAGS};

pub const KERNEL_SESSION: &str = "VerityKernel";
const KERNEL_GUID: u128 = 0x7e5a_0002_6c3d_4b1e_9a40_56e1_0f2b_7c91;

pub struct StoryCapture {
    trace: Option<Trace>,
    collector: Arc<Mutex<Option<Collector>>>,
    sampler_stop: Arc<AtomicBool>,
    sampler: Option<JoinHandle<Vec<StorySample>>>,
    logs: Arc<Mutex<Vec<LogRecord>>>,
    subs: Vec<Subscription>,
    anchor: (u64, u64),
    game_pid: u32,
    names: Arc<Mutex<HashMap<u32, String>>>,
    pub caveats: Vec<String>,
}

fn snapshot_names() -> HashMap<u32, String> {
    WinProcesses
        .processes()
        .map(|all| all.into_iter().map(|p| (p.pid, p.name)).collect())
        .unwrap_or_default()
}

impl StoryCapture {
    /// Starts whatever is available; a part that cannot start becomes a caveat
    /// instead of failing the session.
    pub fn start(game_pid: u32) -> Self {
        let freq = qpc_freq();
        let names = snapshot_names();
        let mut caveats = Vec::new();
        let collector = Arc::new(Mutex::new(Some(Collector::new(
            freq,
            game_pid,
            names.clone(),
        ))));

        let sink = collector.clone();
        let trace = match Trace::start(
            Config {
                name: KERNEL_SESSION,
                guid: KERNEL_GUID,
                kernel_flags: STANDARD_FLAGS,
                providers: vec![],
            },
            Box::new(move |e| {
                if let Ok(mut c) = sink.lock() {
                    if let Some(c) = c.as_mut() {
                        c.feed(e);
                    }
                }
            }),
        ) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!("kernel trace unavailable: {e}");
                caveats.push(format!(
                    "Kernel trace could not start ({e}); no driver, disk or page-fault events"
                ));
                None
            }
        };

        let sampler_stop = Arc::new(AtomicBool::new(false));
        let stop = sampler_stop.clone();
        let sampler = std::thread::Builder::new()
            .name("story-sampler".into())
            .spawn(move || {
                let mut out = Vec::new();
                let Ok(mut s) = StorySampler::new() else {
                    return out;
                };
                let period = Duration::from_millis(derive::PERIOD_MS);
                // Bounded: 6 h at 4 Hz.
                while !stop.load(Ordering::Relaxed) && out.len() < 86_400 {
                    std::thread::sleep(period);
                    if let Ok(x) = s.sample() {
                        out.push(x);
                    }
                }
                out
            })
            .ok();

        let logs: Arc<Mutex<Vec<LogRecord>>> = Default::default();
        let mut subs = Vec::new();
        for channel in [derive::DEFENDER, derive::UPDATE, derive::TASKS] {
            if channel == derive::TASKS && evtlog::channel_enabled(channel) != Some(true) {
                caveats.push(
                    "Scheduled task visibility unavailable (the Task Scheduler log is off)".into(),
                );
                continue;
            }
            let sink = logs.clone();
            match evtlog::subscribe(
                channel,
                Box::new(move |xml| {
                    if let Some(r) = derive::parse_log_xml(channel, &xml) {
                        if let Ok(mut v) = sink.lock() {
                            if v.len() < 10_000 {
                                v.push(r);
                            }
                        }
                    }
                }),
            ) {
                Ok(s) => subs.push(s),
                Err(e) => tracing::warn!("event log {channel} unavailable: {e}"),
            }
        }

        Self {
            trace,
            collector,
            sampler_stop,
            sampler,
            logs,
            subs,
            anchor: (qpc(), filetime_now()),
            game_pid,
            names: Arc::new(Mutex::new(names)),
            caveats,
        }
    }

    /// A process started during the capture: lets events name it.
    pub fn process_started(&self, pid: u32, image: &str) {
        let name = verity_telemetry::kernel::file_name(image).to_string();
        if let Ok(mut n) = self.names.lock() {
            n.insert(pid, name);
        }
        if let Ok(mut c) = self.collector.lock() {
            if let Some(c) = c.as_mut() {
                c.process_started(pid, image);
            }
        }
    }

    /// Stops everything and returns the Frame Story events plus caveats.
    pub fn finish(mut self) -> (Vec<SysEvent>, Vec<String>) {
        let lost = self.trace.as_ref().map_or(0, |t| t.events_lost());
        if let Some(t) = self.trace.take() {
            t.stop();
        }
        self.subs.clear();
        self.sampler_stop.store(true, Ordering::Relaxed);
        let samples = self
            .sampler
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let freq = qpc_freq();
        let mut events = Vec::new();
        if let Some(c) = self.collector.lock().ok().and_then(|mut c| c.take()) {
            if c.dropped > 0 {
                self.caveats.push(format!(
                    "Kept the first {} kernel events; {} later ones were not stored",
                    verity_telemetry::kernel::MAX_EVENTS,
                    c.dropped
                ));
            }
            let (ev, unattributed) = c.finish();
            if unattributed > 0 {
                tracing::debug!(unattributed, "DPC/ISR events matched no driver");
            }
            events.extend(ev);
        }
        let names = self.names.lock().map(|n| n.clone()).unwrap_or_default();
        events.extend(derive::from_samples(&samples, self.game_pid, freq, &names));
        let logs = self.logs.lock().map(|l| l.clone()).unwrap_or_default();
        let (aq, aft) = self.anchor;
        for (s, e, kind, subject) in derive::from_log(&logs, filetime_now()) {
            let start = derive::filetime_to_qpc(s, aq, aft, freq);
            let end = derive::filetime_to_qpc(e, aq, aft, freq);
            events.push(SysEvent {
                qpc: start,
                duration_ticks: end.saturating_sub(start),
                kind: kind.into(),
                subject,
                magnitude: 1.0,
            });
        }
        if lost > 0 {
            self.caveats
                .push(format!("The kernel trace dropped {lost} events under load"));
        }
        events.sort_by_key(|e| e.qpc);
        (events, self.caveats)
    }
}
