//! Domain types and the OS traits every other crate talks through.
//! Real implementations live in `verity-sys`; tests use fakes.

pub mod system;

pub use system::*;

/// A failed OS call: the API name and its Win32 error code or HRESULT.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{api} failed with code {code:#010x}")]
pub struct OsError {
    pub api: &'static str,
    pub code: u32,
}

impl From<OsError> for std::io::Error {
    fn from(e: OsError) -> Self {
        std::io::Error::other(e)
    }
}
