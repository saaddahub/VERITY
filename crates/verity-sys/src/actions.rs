//! The real [`ActionSys`]: what reversible actions change in Windows
//! (docs/05-actions.md). Processes are always opened by identity (pid plus
//! creation time) and only with the rights each change needs.

use std::path::PathBuf;
use std::sync::OnceLock;

use verity_core::actions::{
    ActionSys, DisplayMode, ProcError, ProcInfo, ProcKey, RegValue, StartupItem, StartupPlace,
    Throttle,
};
use verity_core::OsError;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
    ERROR_INVALID_PARAMETER, FILETIME, HANDLE,
};
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplaySettingsW, CDS_TEST,
    CDS_UPDATEREGISTRY, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE,
    DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegEnumValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY,
    REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessDefaultCpuSets, GetProcessInformation, GetProcessTimes,
    OpenProcess, ProcessPowerThrottling, SetProcessDefaultCpuSets, SetProcessInformation,
    PROCESS_ACCESS_RIGHTS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SET_LIMITED_INFORMATION,
};

use crate::system::{devmode, display_device, power};
use crate::undocumented::PowerOverlay;
use crate::{check, from_wide, hresult, wide};

const STILL_ACTIVE: u32 = 259;
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

pub struct WinActionSys;

/// A process handle, closed on drop.
struct Proc(HANDLE);

impl Drop for Proc {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn filetime(t: FILETIME) -> u64 {
    (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
}

fn created(h: HANDLE) -> Result<u64, OsError> {
    let (mut c, mut e, mut k, mut u) = Default::default();
    // SAFETY: `h` is a live handle with query rights; out-pointers are valid.
    unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) }
        .map_err(hresult("GetProcessTimes"))?;
    Ok(filetime(c))
}

/// Opens a process with `access` (plus limited query, to check identity).
/// Refuses a process whose creation time differs: the pid was reused.
fn open(p: ProcKey, access: PROCESS_ACCESS_RIGHTS) -> Result<Proc, ProcError> {
    // SAFETY: plain call; the handle is owned by `Proc`.
    let h = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, false, p.pid) }
        .map_err(|e| match e.code() {
            c if c == ERROR_ACCESS_DENIED.to_hresult() => ProcError::Denied,
            c if c == ERROR_INVALID_PARAMETER.to_hresult() => ProcError::Gone,
            c => ProcError::Os(OsError {
                api: "OpenProcess",
                code: c.0 as u32,
            }),
        })?;
    let proc = Proc(h);
    let mut code = 0u32;
    // SAFETY: live handle; `code` is an out-parameter.
    unsafe { GetExitCodeProcess(proc.0, &mut code) }.map_err(hresult("GetExitCodeProcess"))?;
    if code != STILL_ACTIVE || created(proc.0)? != p.created {
        return Err(ProcError::Gone);
    }
    Ok(proc)
}

/// ERROR_ACCESS_DENIED from a set call is a normal "Windows said no".
fn set_err(api: &'static str) -> impl FnOnce(windows::core::Error) -> ProcError {
    move |e| {
        if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
            ProcError::Denied
        } else {
            ProcError::Os(OsError {
                api,
                code: e.code().0 as u32,
            })
        }
    }
}

fn last_error(api: &'static str) -> OsError {
    hresult(api)(windows::core::Error::from_win32())
}

fn overlay() -> Result<Option<&'static PowerOverlay>, OsError> {
    static OVERLAY: OnceLock<Option<PowerOverlay>> = OnceLock::new();
    if let Some(o) = OVERLAY.get() {
        return Ok(o.as_ref());
    }
    let loaded = PowerOverlay::load()?;
    Ok(OVERLAY.get_or_init(|| loaded).as_ref())
}

fn guid_string(g: GUID) -> String {
    format!("{g:?}").to_ascii_lowercase()
}

/// An open `HKEY_CURRENT_USER` subkey, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: opened by RegOpenKeyExW/RegCreateKeyExW, closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// None when the key does not exist.
fn open_key(path: &str) -> Result<Option<Key>, OsError> {
    let w = wide(path);
    let mut k = HKEY::default();
    // SAFETY: `w` is NUL-terminated; `k` is an out-parameter owned by `Key`.
    let e = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(w.as_ptr()),
            None,
            KEY_READ,
            &mut k,
        )
    };
    if e == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check("RegOpenKeyExW", e)?;
    Ok(Some(Key(k)))
}

fn read_value(key: &Key, name: &str) -> Result<Option<RegValue>, OsError> {
    let n = wide(name);
    let mut ty = REG_VALUE_TYPE::default();
    let mut len = 0u32;
    // SAFETY: size query; all pointers are valid locals.
    let e = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(n.as_ptr()),
            None,
            Some(&mut ty),
            None,
            Some(&mut len),
        )
    };
    if e == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check("RegQueryValueExW", e)?;
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` holds `len` bytes.
    check("RegQueryValueExW", unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(n.as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut len),
        )
    })?;
    buf.truncate(len as usize);
    Ok(Some(match ty {
        REG_DWORD if buf.len() == 4 => {
            RegValue::Dword(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
        }
        REG_SZ => {
            let units: Vec<u16> = buf
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            RegValue::Str(from_wide(&units))
        }
        _ => RegValue::Binary(buf),
    }))
}

fn value_names(key: &Key) -> Result<Vec<String>, OsError> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = [0u16; 16384];
        let mut len = name.len() as u32;
        // SAFETY: `name` holds `len` UTF-16 units; other outputs are unused.
        let e = unsafe {
            RegEnumValueW(
                key.0,
                i,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if e == windows::Win32::Foundation::ERROR_NO_MORE_ITEMS {
            break;
        }
        check("RegEnumValueW", e)?;
        out.push(String::from_utf16_lossy(&name[..len as usize]));
    }
    Ok(out)
}

/// The user's Startup folder.
fn startup_folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
}

fn mode(dm: &windows::Win32::Graphics::Gdi::DEVMODEW) -> DisplayMode {
    DisplayMode {
        width: dm.dmPelsWidth,
        height: dm.dmPelsHeight,
        bits: dm.dmBitsPerPel,
        hz: dm.dmDisplayFrequency,
    }
}

fn current_devmode(device: &str) -> Result<windows::Win32::Graphics::Gdi::DEVMODEW, OsError> {
    let name = wide(device);
    let mut dm = devmode();
    // SAFETY: `name` is NUL-terminated; `dm.dmSize` is set.
    if !unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }
        .as_bool()
    {
        return Err(OsError {
            api: "EnumDisplaySettingsW",
            code: ERROR_FILE_NOT_FOUND.0,
        });
    }
    Ok(dm)
}

impl ActionSys for WinActionSys {
    fn processes(&self) -> Result<Vec<ProcInfo>, OsError> {
        // SAFETY: no preconditions; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(hresult("CreateToolhelp32Snapshot"))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut out = Vec::new();
        // SAFETY: valid snapshot and sized entry; an error ends the walk.
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while more {
            let pid = entry.th32ProcessID;
            let mut session = 0u32;
            // SAFETY: plain call with an out-parameter. Failure leaves 0, which
            // means "system", so the process is never touched.
            let _ = unsafe { ProcessIdToSessionId(pid, &mut session) };
            // Creation time needs a limited-query handle; 0 when Windows refuses.
            // SAFETY: plain call; the handle is closed by `Proc`.
            let created = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
                .ok()
                .and_then(|h| created(Proc(h).0).ok())
                .unwrap_or(0);
            out.push(ProcInfo {
                key: ProcKey { pid, created },
                parent: entry.th32ParentProcessID,
                session,
                name: from_wide(&entry.szExeFile),
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        // SAFETY: closed once.
        let _ = unsafe { CloseHandle(snapshot) };
        Ok(out)
    }

    fn throttling(&self, p: ProcKey) -> Result<Throttle, ProcError> {
        let proc = open(p, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut s = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ..Default::default()
        };
        // SAFETY: `s` is a correctly sized, versioned out-buffer.
        unsafe {
            GetProcessInformation(
                proc.0,
                ProcessPowerThrottling,
                &mut s as *mut _ as *mut _,
                std::mem::size_of_val(&s) as u32,
            )
        }
        .map_err(set_err("GetProcessInformation"))?;
        Ok(Throttle {
            control: s.ControlMask,
            state: s.StateMask,
        })
    }

    fn set_throttling(&self, p: ProcKey, t: Throttle) -> Result<(), ProcError> {
        let proc = open(p, PROCESS_SET_INFORMATION)?;
        let s = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: t.control,
            StateMask: t.state,
        };
        // SAFETY: `s` is a correctly sized, versioned in-buffer.
        unsafe {
            SetProcessInformation(
                proc.0,
                ProcessPowerThrottling,
                &s as *const _ as *const _,
                std::mem::size_of_val(&s) as u32,
            )
        }
        .map_err(set_err("SetProcessInformation"))
    }

    fn default_cpu_sets(&self, p: ProcKey) -> Result<Vec<u32>, ProcError> {
        let proc = open(p, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut needed = 0u32;
        // SAFETY: size query with no buffer.
        if unsafe { GetProcessDefaultCpuSets(proc.0, None, &mut needed) }.as_bool() {
            return Ok(Vec::new());
        }
        let mut ids = vec![0u32; needed as usize];
        // SAFETY: `ids` holds `needed` entries.
        if !unsafe { GetProcessDefaultCpuSets(proc.0, Some(&mut ids), &mut needed) }.as_bool() {
            return Err(ProcError::Os(last_error("GetProcessDefaultCpuSets")));
        }
        ids.truncate(needed as usize);
        Ok(ids)
    }

    fn set_default_cpu_sets(&self, p: ProcKey, sets: &[u32]) -> Result<(), ProcError> {
        let proc = open(p, PROCESS_SET_LIMITED_INFORMATION)?;
        let arg = (!sets.is_empty()).then_some(sets);
        // SAFETY: `arg` is None (clear) or a live slice of CPU set ids.
        if unsafe { SetProcessDefaultCpuSets(proc.0, arg) }.as_bool() {
            Ok(())
        } else {
            Err(set_err("SetProcessDefaultCpuSets")(
                windows::core::Error::from_win32(),
            ))
        }
    }

    fn efficiency_cpu_sets(&self) -> Result<Option<Vec<u32>>, OsError> {
        let mut len = 0u32;
        // SAFETY: size query; fails with ERROR_INSUFFICIENT_BUFFER.
        let ok = unsafe { GetSystemCpuSetInformation(None, 0, &mut len, None, None) }.as_bool();
        if !ok {
            let e = windows::core::Error::from_win32();
            if e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
                return Err(hresult("GetSystemCpuSetInformation")(e));
            }
        }
        // u64 storage keeps the entries 8-byte aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` holds at least `len` bytes.
        if !unsafe {
            GetSystemCpuSetInformation(
                Some(buf.as_mut_ptr().cast::<SYSTEM_CPU_SET_INFORMATION>()),
                len,
                &mut len,
                None,
                None,
            )
        }
        .as_bool()
        {
            return Err(last_error("GetSystemCpuSetInformation"));
        }
        let bytes = buf.as_ptr().cast::<u8>();
        let mut sets: Vec<(u8, u32)> = Vec::new();
        let mut off = 0usize;
        while off + std::mem::size_of::<SYSTEM_CPU_SET_INFORMATION>() <= len as usize {
            // SAFETY: `off` stays inside the `len` bytes Windows filled; each
            // entry starts 8-byte aligned and begins with its own size.
            let info = unsafe { &*bytes.add(off).cast::<SYSTEM_CPU_SET_INFORMATION>() };
            if info.Size == 0 {
                break;
            }
            if info.Type == CpuSetInformation {
                // SAFETY: `Type` says the union holds a CPU set.
                let cs = unsafe { info.Anonymous.CpuSet };
                sets.push((cs.EfficiencyClass, cs.Id));
            }
            off += info.Size as usize;
        }
        let (Some(lo), Some(hi)) = (
            sets.iter().map(|s| s.0).min(),
            sets.iter().map(|s| s.0).max(),
        ) else {
            return Ok(None);
        };
        if lo == hi {
            return Ok(None);
        }
        Ok(Some(
            sets.iter().filter(|s| s.0 == lo).map(|s| s.1).collect(),
        ))
    }

    fn on_ac(&self) -> Result<Option<bool>, OsError> {
        Ok(power()?.ac_online)
    }

    fn power_overlay(&self) -> Result<Option<String>, OsError> {
        match overlay()? {
            Some(o) => Ok(Some(guid_string(o.get()?))),
            None => Ok(None),
        }
    }

    fn set_power_overlay(&self, guid: &str) -> Result<(), OsError> {
        let g = GUID::try_from(guid).map_err(|_| OsError {
            api: "PowerSetActiveOverlayScheme",
            code: ERROR_INVALID_PARAMETER.0,
        })?;
        match overlay()? {
            Some(o) => o.set(g),
            None => Err(OsError {
                api: "PowerSetActiveOverlayScheme",
                code: windows::Win32::Foundation::ERROR_PROC_NOT_FOUND.0,
            }),
        }
    }

    fn reg_get(&self, key: &str, name: &str) -> Result<Option<RegValue>, OsError> {
        match open_key(key)? {
            Some(k) => read_value(&k, name),
            None => Ok(None),
        }
    }

    fn reg_set(&self, key: &str, name: &str, v: &RegValue) -> Result<(), OsError> {
        let w = wide(key);
        let mut k = HKEY::default();
        // SAFETY: `w` is NUL-terminated; `k` is owned by `Key` below.
        check("RegCreateKeyExW", unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(w.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut k,
                None,
            )
        })?;
        let k = Key(k);
        let (ty, data): (REG_VALUE_TYPE, Vec<u8>) = match v {
            RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
            RegValue::Str(s) => (
                REG_SZ,
                wide(s).iter().flat_map(|u| u.to_le_bytes()).collect(),
            ),
            RegValue::Binary(b) => (REG_BINARY, b.clone()),
        };
        let n = wide(name);
        // SAFETY: `n` is NUL-terminated and `data` is a live byte slice.
        check("RegSetValueExW", unsafe {
            RegSetValueExW(k.0, PCWSTR(n.as_ptr()), None, ty, Some(&data))
        })
    }

    fn reg_delete(&self, key: &str, name: &str) -> Result<(), OsError> {
        let (k, n) = (wide(key), wide(name));
        // SAFETY: both strings are NUL-terminated.
        let e = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()))
        };
        if e == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        check("RegDeleteKeyValueW", e)
    }

    fn startup_items(&self) -> Result<Vec<StartupItem>, OsError> {
        let mut out = Vec::new();
        let mut add = |names: Vec<String>,
                       place: StartupPlace,
                       approved: &str|
         -> Result<(), OsError> {
            let flags = open_key(&format!(r"{APPROVED_KEY}\{approved}"))?;
            for name in names {
                let enabled = match &flags {
                    Some(k) => {
                        !matches!(read_value(k, &name)?, Some(RegValue::Binary(b)) if b.first().is_some_and(|f| f & 1 == 1))
                    }
                    None => true,
                };
                out.push(StartupItem {
                    name,
                    place,
                    enabled,
                });
            }
            Ok(())
        };
        let run = match open_key(RUN_KEY)? {
            Some(k) => value_names(&k)?,
            None => Vec::new(),
        };
        add(run, StartupPlace::Run, "Run")?;
        let mut links: Vec<String> = startup_folder()
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.eq_ignore_ascii_case("desktop.ini"))
            .collect();
        links.sort();
        add(links, StartupPlace::StartupFolder, "StartupFolder")?;
        Ok(out)
    }

    fn primary_display(&self) -> Result<String, OsError> {
        for i in 0.. {
            let mut dd = display_device();
            // SAFETY: `dd.cb` is set; a null device name enumerates outputs.
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0) }.as_bool() {
                break;
            }
            let flags = dd.StateFlags;
            if (flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP).0 != 0
                && (flags & DISPLAY_DEVICE_PRIMARY_DEVICE).0 != 0
            {
                return Ok(from_wide(&dd.DeviceName));
            }
        }
        Err(OsError {
            api: "EnumDisplayDevicesW",
            code: ERROR_FILE_NOT_FOUND.0,
        })
    }

    fn display_mode(&self, device: &str) -> Result<DisplayMode, OsError> {
        Ok(mode(&current_devmode(device)?))
    }

    fn display_modes(&self, device: &str) -> Result<Vec<DisplayMode>, OsError> {
        let name = wide(device);
        let mut out = Vec::new();
        for i in 0.. {
            let mut dm = devmode();
            // SAFETY: as in `current_devmode`; false ends the list.
            if !unsafe {
                EnumDisplaySettingsW(
                    PCWSTR(name.as_ptr()),
                    ENUM_DISPLAY_SETTINGS_MODE(i),
                    &mut dm,
                )
            }
            .as_bool()
            {
                break;
            }
            out.push(mode(&dm));
        }
        Ok(out)
    }

    fn set_display_hz(&self, device: &str, hz: u32) -> Result<(), OsError> {
        let mut dm = current_devmode(device)?;
        dm.dmDisplayFrequency = hz;
        dm.dmFields = DM_DISPLAYFREQUENCY;
        let name = wide(device);
        for flags in [CDS_TEST, CDS_UPDATEREGISTRY] {
            // SAFETY: `name` is NUL-terminated and `dm` a valid DEVMODEW.
            let r = unsafe {
                ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&dm), None, flags, None)
            };
            if r != DISP_CHANGE_SUCCESSFUL {
                return Err(OsError {
                    api: "ChangeDisplaySettingsExW",
                    code: r.0 as u32,
                });
            }
        }
        Ok(())
    }
}
