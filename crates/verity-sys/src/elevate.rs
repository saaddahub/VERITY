use std::path::Path;

use verity_core::OsError;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::{hresult, wide};

/// Runs `exe` with administrator rights through the standard UAC prompt, with
/// no console window, and returns its exit code once it finishes. The caller
/// passes a fixed path and fixed arguments, never user input.
pub fn run_elevated(exe: &Path, args: &str) -> Result<u32, OsError> {
    let file = wide(exe);
    let params = wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialized; `file` and `params` outlive the call.
    // A declined UAC prompt comes back as an error (ERROR_CANCELLED).
    unsafe { ShellExecuteExW(&mut info) }.map_err(hresult("ShellExecuteExW"))?;
    if info.hProcess.is_invalid() {
        return Err(OsError {
            api: "ShellExecuteExW (no process)",
            code: 0,
        });
    }
    // SAFETY: `hProcess` is a live handle we own because of SEE_MASK_NOCLOSEPROCESS;
    // it is closed exactly once below.
    unsafe {
        let waited = WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let got = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited != WAIT_OBJECT_0 {
            return Err(OsError {
                api: "WaitForSingleObject",
                code: waited.0,
            });
        }
        got.map_err(hresult("GetExitCodeProcess"))?;
        Ok(code)
    }
}
