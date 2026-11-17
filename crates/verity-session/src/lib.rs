//! Session Mode (docs/06-session-mode.md): game library, detection and the
//! session state machine. OS access goes through traits so it runs on fakes.

pub mod library;
pub mod machine;
mod stdfs;

pub use stdfs::{real_roots, StdFs};
