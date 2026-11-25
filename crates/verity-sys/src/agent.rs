//! What the user-session agent (the desktop app) needs from Windows: the
//! foreground process and the global panic hotkey (docs/06-session-mode.md).

use std::sync::mpsc;
use std::thread::JoinHandle;

use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, MessageBoxW, PostThreadMessageW,
    MB_ICONERROR, MB_OK, MSG, WM_HOTKEY, WM_QUIT,
};

use crate::{hresult, wide};

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

/// Shows a modal error message: the only way a GUI app, which has no
/// console, can report a failure before its window exists.
pub fn error_box(title: &str, text: &str) -> Result<(), OsError> {
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let r = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        )
    };
    if r.0 == 0 {
        // SAFETY: reads the calling thread's last error.
        return Err(OsError {
            api: "MessageBoxW",
            code: unsafe { GetLastError() }.0,
        });
    }
    Ok(())
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

/// Pids of processes with an active audio session on any speaker or
/// microphone, so background actions leave them alone (docs/05-actions.md).
/// Reads no names or titles.
pub fn audio_pids() -> Result<Vec<u32>, OsError> {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        eAll, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: balances the successful CoInitializeEx below.
            unsafe { CoUninitialize() };
        }
    }
    // SAFETY: no preconditions; balanced by `Com` (also on S_FALSE).
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(hresult("CoInitializeEx"))?;
    let _com = Com;
    let mut out = Vec::new();
    // SAFETY: COM is initialized on this thread; every interface below is
    // used on this thread and released before `_com` drops.
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(hresult("CoCreateInstance(MMDeviceEnumerator)"))?;
        let devices = en
            .EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)
            .map_err(hresult("IMMDeviceEnumerator::EnumAudioEndpoints"))?;
        for i in 0..devices
            .GetCount()
            .map_err(hresult("IMMDeviceCollection::GetCount"))?
        {
            let Ok(device) = devices.Item(i) else {
                continue;
            };
            let Ok(mgr) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = mgr.GetSessionEnumerator() else {
                continue;
            };
            for k in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(control) = sessions.GetSession(k) else {
                    continue;
                };
                let Ok(c2) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if c2.GetState().ok() == Some(AudioSessionStateActive) {
                    if let Ok(pid) = c2.GetProcessId() {
                        if pid != 0 && !out.contains(&pid) {
                            out.push(pid);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}
