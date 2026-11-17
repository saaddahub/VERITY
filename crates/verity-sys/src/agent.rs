//! What the user-session agent (the desktop app) needs from Windows: the
//! foreground process and the global panic hotkey (docs/06-session-mode.md).

use std::sync::mpsc;
use std::thread::JoinHandle;

use verity_core::OsError;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW, MSG, WM_HOTKEY,
    WM_QUIT,
};

use crate::hresult;

/// Pid of the process that owns the foreground window; None on the secure
/// desktop or when no window has focus. Reads no window title.
pub fn foreground_pid() -> Option<u32> {
    // SAFETY: no preconditions.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `hwnd` came from GetForegroundWindow; `pid` is an out-parameter.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0).then_some(pid)
}

/// A registered global hotkey. `on_press` runs on the hotkey thread.
pub struct Hotkey {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// Ctrl+Alt+Shift+V, the default panic hotkey.
pub const PANIC_VK: u32 = b'V' as u32;

impl Hotkey {
    pub fn register(vk: u32, on_press: Box<dyn Fn() + Send>) -> Result<Self, OsError> {
        let (tx, rx) = mpsc::channel::<Result<u32, OsError>>();
        let thread = std::thread::Builder::new()
            .name("panic-hotkey".into())
            .spawn(move || {
                let mods: HOT_KEY_MODIFIERS = MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT;
                // SAFETY: a null HWND binds the hotkey to this thread's queue.
                if let Err(e) = unsafe { RegisterHotKey(None, 1, mods, vk) } {
                    let _ = tx.send(Err(hresult("RegisterHotKey")(e)));
                    return;
                }
                // SAFETY: no preconditions.
                let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
                let mut msg = MSG::default();
                // SAFETY: `msg` is an out-parameter; GetMessageW returns 0 on WM_QUIT.
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                    if msg.message == WM_HOTKEY {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&on_press));
                    }
                }
                // SAFETY: unregisters the id registered above on this thread.
                let _ = unsafe { UnregisterHotKey(None, 1) };
            })
            .map_err(|_| OsError {
                api: "CreateThread",
                code: 0,
            })?;
        let thread_id = rx.recv().map_err(|_| OsError {
            api: "RegisterHotKey",
            code: 0,
        })??;
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        // SAFETY: posts WM_QUIT to the hotkey thread's queue, ending its loop.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
