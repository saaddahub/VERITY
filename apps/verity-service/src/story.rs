//! Frame Story for a stored session, computed on demand (`story.get`,
//! `story.export`) and at session end.

use verity_core::metrics::{Frame, Thresholds};
use verity_core::telemetry::{SessionSummary, SysEvent};
use verity_store::{SessionBounds, Store};
use verity_story::analyze::CANDIDATE_KINDS;
use verity_story::{analyze, Input, Interval, Story};

pub struct StoryOut {
    pub story: Story,
    pub frames: Vec<Frame>,
    /// Candidate events only: what the timeline lanes show.
    pub events: Vec<SysEvent>,
    pub bounds: SessionBounds,
    pub summary: SessionSummary,
    pub started_utc: String,
}

pub fn for_session(store: &Store, id: i64) -> Result<StoryOut, verity_store::StoreError> {
    let row = store.get(id)?;
    let bounds = store.bounds(id)?;
    let frames = store.all_frames(id)?;
    let all = store.events(id)?;
    let excluded: Vec<Interval> = all
        .iter()
        .filter(|e| e.kind == "capture_gap" || e.kind == "background")
        .map(|e| Interval {
            start: e.qpc,
            end: e.qpc + e.duration_ticks,
        })
        .collect();
    let caveats: Vec<String> = all
        .iter()
        .filter(|e| e.kind == "caveat")
        .map(|e| e.subject.clone())
        .collect();
    let events: Vec<SysEvent> = all
        .into_iter()
        .filter(|e| CANDIDATE_KINDS.contains(&e.kind.as_str()))
        .collect();
    let mut story = analyze(&Input {
        frames: &frames,
        qpc_freq: bounds.qpc_freq,
        capture_start: bounds.started_qpc,
        capture_end: bounds.ended_qpc,
        excluded: &excluded,
        events: &events,
        capture_mode: &row.summary.capture_mode,
        thresholds: &Thresholds::default(),
    });
    story.caveats.extend(caveats);
    Ok(StoryOut {
        story,
        frames,
        events,
        bounds,
        summary: row.summary,
        started_utc: row.started_utc,
    })
}

/// The shareable HTML report.
pub fn html(out: &StoryOut) -> String {
    verity_story::html::render(&verity_story::html::Report {
        exe: &out.summary.exe,
        started_utc: &out.started_utc,
        avg_fps: out.summary.stats.avg_fps,
        low_1pct_fps: out.summary.stats.low_1pct_fps,
        story: &out.story,
        frames: &out.frames,
        events: &out.events,
        qpc_freq: out.bounds.qpc_freq,
        capture_start: out.bounds.started_qpc,
        capture_end: out.bounds.ended_qpc,
    })
}
