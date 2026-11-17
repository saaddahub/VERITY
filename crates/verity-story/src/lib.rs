//! Frame Story (docs/07-frame-story.md): for every hitch, what the system was
//! doing at that moment, and whether that is more than coincidence.

pub mod analyze;
pub mod html;
pub mod stats;

pub use analyze::{analyze, Cause, Input, Interval, Story, StoryEvent};
