use std::ffi::c_void;

use verity_core::telemetry::{ProcessApi, ProcessEntry};
use verity_core::OsError;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use crate::{from_wide, hresult};

/// Current QueryPerformanceCounter value.
pub fn qpc() -> u64 {
    let mut t = 0i64;
    // SAFETY: `t` is a valid out-parameter; the call cannot fail on supported Windows.
    let _ = unsafe { QueryPerformanceCounter(&mut t) };
    t as u64
}

/// QPC ticks per second.
pub fn qpc_freq() -> u64 {
    let mut f = 0i64;
    // SAFETY: `f` is a valid out-parameter; the call cannot fail on supported Windows.
    let _ = unsafe { QueryPerformanceFrequency(&mut f) };
    f as u64
}

/// Current UTC time as a FILETIME (100 ns since 1601).
pub fn filetime_now() -> u64 {
    // SAFETY: no preconditions.
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

/// The real `ProcessApi`: a Toolhelp snapshot, which opens no process.
pub struct WinProcesses;

impl ProcessApi for WinProcesses {
    fn processes(&self) -> Result<Vec<ProcessEntry>, OsError> {
        // SAFETY: no preconditions; the returned handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(hresult("CreateToolhelp32Snapshot"))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut out = Vec::new();
        // SAFETY: `snapshot` is valid and `entry.dwSize` is set; an error ends the walk.
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while more {
            out.push(ProcessEntry {
                pid: entry.th32ProcessID,
                name: from_wide(&entry.szExeFile),
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        // SAFETY: `snapshot` came from CreateToolhelp32Snapshot and is closed once.
        let _ = unsafe { CloseHandle(snapshot) };
        Ok(out)
    }
}

fn cpu_time(handle: HANDLE) -> Result<u64, OsError> {
    let (mut created, mut exited) = (FILETIME::default(), FILETIME::default());
    let (mut kernel, mut user) = (FILETIME::default(), FILETIME::default());
    // SAFETY: `handle` is a live process handle; all four out-pointers are valid.
    unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(hresult("GetProcessTimes"))?;
    let ticks = |t: FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
    Ok(ticks(kernel) + ticks(user))
}

/// Kernel plus user CPU time this process has used, in 100 ns units.
pub fn own_cpu_time() -> Result<u64, OsError> {
    // SAFETY: the pseudo-handle for the current process is always valid.
    cpu_time(unsafe { GetCurrentProcess() })
}

/// CPU time of a child process Verity started itself, in 100 ns units.
/// `raw` must be a live handle to that child (never a game's handle).
pub fn child_cpu_time(raw: *mut c_void) -> Result<u64, OsError> {
    cpu_time(HANDLE(raw))
}
