//! Real-time ETW sessions: start, consume on a worker thread, stop.
//!
//! Raw `StartTraceW`/`ProcessTrace` rather than ferrisetw (see
//! docs/notes/decisions.md, M4/M5): Verity needs `PROCESS_TRACE_MODE_RAW_TIMESTAMP`
//! so event times are QPC ticks on the same clock as PresentMon's frames and as
//! the `InitialTime` field inside DPC/ISR events, and it decodes the few kernel
//! layouts it uses by hand because TDH per event is too slow at DPC rates.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use verity_core::OsError;
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_WMI_INSTANCE_NOT_FOUND,
};
use windows::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW,
    CONTROLTRACE_HANDLE, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_HEADER_FLAG_64_BIT_HEADER,
    EVENT_RECORD, EVENT_TRACE_CONTROL, EVENT_TRACE_CONTROL_QUERY, EVENT_TRACE_CONTROL_STOP,
    EVENT_TRACE_CONTROL_UPDATE, EVENT_TRACE_FLAG, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES,
    EVENT_TRACE_REAL_TIME_MODE, EVENT_TRACE_SYSTEM_LOGGER_MODE, PROCESSTRACE_HANDLE,
    PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_RAW_TIMESTAMP,
    PROCESS_TRACE_MODE_REAL_TIME, WNODE_FLAG_TRACED_GUID,
};

use crate::{check, wide};

/// One event, borrowed for the duration of the callback.
#[derive(Debug, Clone, Copy)]
pub struct EtwEvent<'a> {
    pub provider: u128,
    pub id: u16,
    pub version: u8,
    pub opcode: u8,
    pub pid: u32,
    pub tid: u32,
    /// QPC ticks (raw timestamp mode with the QPC clock).
    pub qpc: u64,
    pub data: &'a [u8],
    /// Pointer-sized fields in `data` are 8 bytes.
    pub is_64bit: bool,
}

pub type Callback = Box<dyn FnMut(&EtwEvent) + Send>;

/// A provider to enable on a non-kernel session: (GUID, keywords, level).
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    pub guid: u128,
    pub keywords: u64,
    pub level: u8,
}

pub struct Config {
    pub name: &'static str,
    /// Session GUID; any fixed unique value.
    pub guid: u128,
    /// Kernel `EVENT_TRACE_FLAG_*` bits. Non-zero makes this a private
    /// system-logger session (Windows 8+), which does not fight other tools for
    /// the single "NT Kernel Logger".
    pub kernel_flags: u32,
    pub providers: Vec<Provider>,
}

/// Room for the logger name after the properties block.
const NAME_BYTES: usize = 1024 * 2;

/// `EVENT_TRACE_PROPERTIES` followed by its name buffer, 8-byte aligned.
struct Props {
    buf: Vec<u64>,
}

impl Props {
    fn new(cfg_guid: u128, mode: u32, flags: u32) -> Self {
        let size = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + NAME_BYTES;
        let mut p = Self {
            buf: vec![0u64; size.div_ceil(8)],
        };
        let props = p.get();
        props.Wnode.BufferSize = size as u32;
        props.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        props.Wnode.Guid = GUID::from_u128(cfg_guid);
        props.Wnode.ClientContext = 1; // QPC clock
        props.BufferSize = 64; // KB
        props.MinimumBuffers = 16;
        props.MaximumBuffers = 128;
        props.FlushTimer = 1; // seconds
        props.LogFileMode = mode;
        props.EnableFlags = EVENT_TRACE_FLAG(flags);
        props.LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        p
    }

    fn get(&mut self) -> &mut EVENT_TRACE_PROPERTIES {
        // SAFETY: `buf` is 8-aligned, zero-initialised and at least
        // size_of::<EVENT_TRACE_PROPERTIES>() bytes; a zeroed struct is valid.
        unsafe { &mut *(self.buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) }
    }

    fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES
    }
}

fn control(name: &str, guid: u128, code: EVENT_TRACE_CONTROL) -> Result<Props, OsError> {
    let w = wide(name);
    let mut p = Props::new(guid, 0, 0);
    // SAFETY: `w` is NUL-terminated; `p` is a correctly sized properties block.
    let e = unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(w.as_ptr()),
            p.ptr(),
            code,
        )
    };
    check("ControlTraceW", e)?;
    Ok(p)
}

/// Stops a session by name (cleans up one orphaned by a crash). Not running is fine.
pub fn stop_by_name(name: &str) -> Result<(), OsError> {
    match control(name, 0, EVENT_TRACE_CONTROL_STOP) {
        Err(e) if e.code == ERROR_WMI_INSTANCE_NOT_FOUND.0 => Ok(()),
        r => r.map(|_| ()),
    }
}

/// Whether a session with this name exists.
pub fn is_running(name: &str) -> bool {
    control(name, 0, EVENT_TRACE_CONTROL_QUERY).is_ok()
}

/// Running session plus its consumer thread. Dropping it stops both.
pub struct Trace {
    name: &'static str,
    guid: u128,
    control: CONTROLTRACE_HANDLE,
    consumer: PROCESSTRACE_HANDLE,
    thread: Option<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
}

struct Ctx {
    cb: Callback,
    stopped: Arc<AtomicBool>,
}

unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the call.
    let rec = unsafe { &*rec };
    if rec.UserContext.is_null() {
        return;
    }
    // SAFETY: UserContext is the `Ctx` boxed in `Trace::start`, alive until the
    // consumer thread returns from ProcessTrace, and only this thread uses it.
    let ctx = unsafe { &mut *(rec.UserContext as *mut Ctx) };
    if ctx.stopped.load(Ordering::Relaxed) {
        return;
    }
    let data = if rec.UserData.is_null() || rec.UserDataLength == 0 {
        &[][..]
    } else {
        // SAFETY: UserData points at UserDataLength readable bytes for this call.
        unsafe {
            std::slice::from_raw_parts(rec.UserData as *const u8, rec.UserDataLength as usize)
        }
    };
    let h = &rec.EventHeader;
    let ev = EtwEvent {
        provider: h.ProviderId.to_u128(),
        id: h.EventDescriptor.Id,
        version: h.EventDescriptor.Version,
        opcode: h.EventDescriptor.Opcode,
        pid: h.ProcessId,
        tid: h.ThreadId,
        qpc: h.TimeStamp as u64,
        data,
        is_64bit: (h.Flags as u32 & EVENT_HEADER_FLAG_64_BIT_HEADER) != 0
            || cfg!(target_pointer_width = "64") && (h.Flags as u32 & 0x20) == 0,
    };
    // A panic must not unwind into ETW.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (ctx.cb)(&ev)));
}

impl Trace {
    /// Starts the session (replacing an orphan of the same name), enables the
    /// providers and begins delivering events to `cb` on a worker thread.
    pub fn start(cfg: Config, cb: Callback) -> Result<Self, OsError> {
        let mode = EVENT_TRACE_REAL_TIME_MODE
            | if cfg.kernel_flags != 0 {
                EVENT_TRACE_SYSTEM_LOGGER_MODE
            } else {
                0
            };
        let name = wide(cfg.name);
        let mut handle = CONTROLTRACE_HANDLE::default();
        let mut started = false;
        for _ in 0..2 {
            let mut p = Props::new(cfg.guid, mode, cfg.kernel_flags);
            // SAFETY: `name` is NUL-terminated and fits NAME_BYTES; `p` is sized for it.
            let e = unsafe { StartTraceW(&mut handle, PCWSTR(name.as_ptr()), p.ptr()) };
            if e == ERROR_ALREADY_EXISTS {
                stop_by_name(cfg.name)?;
                continue;
            }
            check("StartTraceW", e)?;
            started = true;
            break;
        }
        if !started {
            return Err(OsError {
                api: "StartTraceW",
                code: ERROR_ALREADY_EXISTS.0,
            });
        }
        let stop_now = |e: OsError| {
            let _ = stop_by_name(cfg.name);
            e
        };
        for p in &cfg.providers {
            let guid = GUID::from_u128(p.guid);
            // SAFETY: `handle` is the session just started; `guid` outlives the call.
            let e = unsafe {
                EnableTraceEx2(
                    handle,
                    &guid,
                    EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
                    p.level,
                    p.keywords,
                    0,
                    0,
                    None,
                )
            };
            check("EnableTraceEx2", e).map_err(stop_now)?;
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let ctx = Box::into_raw(Box::new(Ctx {
            cb,
            stopped: stopped.clone(),
        }));
        let mut logger_name = wide(cfg.name);
        let mut logfile = EVENT_TRACE_LOGFILEW {
            LoggerName: PWSTR(logger_name.as_mut_ptr()),
            Context: ctx as *mut _,
            ..Default::default()
        };
        logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME
            | PROCESS_TRACE_MODE_EVENT_RECORD
            | PROCESS_TRACE_MODE_RAW_TIMESTAMP;
        logfile.Anonymous2.EventRecordCallback = Some(on_event);
        // SAFETY: `logfile` and the name it points at are valid for the call.
        let consumer = unsafe { OpenTraceW(&mut logfile) };
        if consumer.Value == u64::MAX {
            // SAFETY: reads the calling thread's last error.
            let code = unsafe { GetLastError() }.0;
            // SAFETY: `ctx` came from Box::into_raw above and was never handed out.
            drop(unsafe { Box::from_raw(ctx) });
            return Err(stop_now(OsError {
                api: "OpenTraceW",
                code,
            }));
        }
        let ctx_addr = ctx as usize;
        let thread = std::thread::Builder::new()
            .name(format!("etw-{}", cfg.name))
            .spawn(move || {
                // SAFETY: `consumer` is open; ProcessTrace blocks until CloseTrace.
                let _ = unsafe { ProcessTrace(&[consumer], None, None) };
                // SAFETY: ProcessTrace has returned, so no callback can still use ctx.
                drop(unsafe { Box::from_raw(ctx_addr as *mut Ctx) });
            })
            .map_err(|_| {
                stop_now(OsError {
                    api: "CreateThread",
                    code: 0,
                })
            })?;
        Ok(Self {
            name: cfg.name,
            guid: cfg.guid,
            control: handle,
            consumer,
            thread: Some(thread),
            stopped,
        })
    }

    /// Changes the kernel flags of a running system-logger session in place.
    pub fn update_flags(&self, flags: u32) -> Result<(), OsError> {
        let mode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_SYSTEM_LOGGER_MODE;
        let mut p = Props::new(self.guid, mode, flags);
        // SAFETY: `control` is this session's handle; `p` is a sized properties block.
        let e = unsafe {
            ControlTraceW(
                self.control,
                PCWSTR::null(),
                p.ptr(),
                EVENT_TRACE_CONTROL_UPDATE,
            )
        };
        check("ControlTraceW(update)", e)
    }

    /// Events the session dropped so far (buffers full).
    pub fn events_lost(&self) -> u32 {
        control(self.name, self.guid, EVENT_TRACE_CONTROL_QUERY)
            .map(|mut p| p.get().EventsLost + p.get().RealTimeBuffersLost)
            .unwrap_or(0)
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let mut p = Props::new(self.guid, 0, 0);
        // SAFETY: `control` is this session's handle; `p` is a sized properties block.
        let _ = unsafe {
            ControlTraceW(
                self.control,
                PCWSTR::null(),
                p.ptr(),
                EVENT_TRACE_CONTROL_STOP,
            )
        };
        // SAFETY: `consumer` was opened in `start` and is closed exactly once here.
        let _ = unsafe { CloseTrace(self.consumer) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.shutdown();
        }
    }
}

/// Little-endian readers for hand-decoded event payloads.
pub mod read {
    pub fn u8_at(d: &[u8], off: usize) -> Option<u8> {
        d.get(off).copied()
    }
    pub fn u16_at(d: &[u8], off: usize) -> Option<u16> {
        Some(u16::from_le_bytes(d.get(off..off + 2)?.try_into().ok()?))
    }
    pub fn u32_at(d: &[u8], off: usize) -> Option<u32> {
        Some(u32::from_le_bytes(d.get(off..off + 4)?.try_into().ok()?))
    }
    pub fn u64_at(d: &[u8], off: usize) -> Option<u64> {
        Some(u64::from_le_bytes(d.get(off..off + 8)?.try_into().ok()?))
    }
    /// A pointer-sized value.
    pub fn ptr_at(d: &[u8], off: usize, is_64bit: bool) -> Option<u64> {
        if is_64bit {
            u64_at(d, off)
        } else {
            u32_at(d, off).map(u64::from)
        }
    }
    /// A NUL-terminated UTF-16 string at `off`: (text, bytes consumed incl. NUL).
    pub fn wstr_at(d: &[u8], off: usize) -> Option<(String, usize)> {
        let rest = d.get(off..)?;
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        let used = (units.len() + 1) * 2;
        Some((String::from_utf16_lossy(&units), used.min(rest.len())))
    }
    /// A NUL-terminated ANSI string at `off`.
    pub fn astr_at(d: &[u8], off: usize) -> Option<(String, usize)> {
        let rest = d.get(off..)?;
        let n = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Some((
            String::from_utf8_lossy(&rest[..n]).into_owned(),
            (n + 1).min(rest.len()),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn readers_bounds_check() {
            let d = [1u8, 0, 0, 0, 0x41, 0, 0x42, 0, 0, 0, 0xFF];
            assert_eq!(u32_at(&d, 0), Some(1));
            assert_eq!(u32_at(&d, 8), None);
            assert_eq!(wstr_at(&d, 4), Some(("AB".into(), 6)));
            assert_eq!(astr_at(b"ab\0c", 0), Some(("ab".into(), 3)));
            assert_eq!(u64_at(&d, 100), None);
            assert_eq!(ptr_at(&d, 0, false), Some(1));
            // Unterminated string reads to the end without panicking.
            assert_eq!(wstr_at(&[0x41, 0], 0), Some(("A".into(), 2)));
        }
    }
}
