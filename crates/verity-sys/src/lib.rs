//! All `unsafe` code and raw Win32 calls, behind safe wrappers.
//! Nothing above this crate contains `unsafe`.

pub mod elevate;
pub mod games;
pub mod paths;
pub mod pdh;
pub mod pipe;
pub mod process;
mod security;
mod system;

pub use system::{displays, WinSystemInfo};

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use verity_core::OsError;
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};

fn check(api: &'static str, e: WIN32_ERROR) -> Result<(), OsError> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(OsError { api, code: e.0 })
    }
}

fn hresult(api: &'static str) -> impl FnOnce(windows::core::Error) -> OsError {
    move |e| OsError {
        api,
        code: e.code().0 as u32,
    }
}

/// NUL-terminated UTF-16 for Win32 string parameters.
fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}
