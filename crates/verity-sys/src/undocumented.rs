//! Undocumented Windows APIs (CLAUDE.md): each behind the `undocumented`
//! feature, looked up at run time, with a documented fallback when missing.

use verity_core::OsError;
use windows::core::{s, w, GUID};
use windows::Win32::Foundation::{FARPROC, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};

use crate::hresult;

/// The power mode slider (Settings > System > Power). Verified on Windows 11
/// 25H2 (build 26200); see `docs/notes/decisions.md`.
///
/// Fallback when the functions are missing: `power.session_overlay` reports
/// "not applicable" and changes nothing.
pub struct PowerOverlay {
    get: unsafe extern "system" fn(*mut GUID) -> u32,
    set: unsafe extern "system" fn(GUID) -> u32,
}

fn lookup(module: HMODULE, name: windows::core::PCSTR) -> FARPROC {
    // SAFETY: `module` is a loaded module and `name` a NUL-terminated ASCII name.
    unsafe { GetProcAddress(module, name) }
}

impl PowerOverlay {
    /// None when this Windows has no power-mode API (or the feature is off).
    pub fn load() -> Result<Option<Self>, OsError> {
        if !cfg!(feature = "undocumented") {
            return Ok(None);
        }
        // SAFETY: loads the system copy of powrprof.dll only; the module stays
        // loaded for the life of the process, so the function pointers stay valid.
        let module =
            unsafe { LoadLibraryExW(w!("powrprof.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
                .map_err(hresult("LoadLibraryExW"))?;
        let (Some(get), Some(set)) = (
            lookup(module, s!("PowerGetActualOverlayScheme")),
            lookup(module, s!("PowerSetActiveOverlayScheme")),
        ) else {
            return Ok(None);
        };
        // SAFETY: the signatures are `DWORD (GUID*)` and `DWORD (GUID)`, checked
        // by the `actions` probe; on x64 a GUID argument is passed by reference.
        Ok(Some(unsafe {
            Self {
                get: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(*mut GUID) -> u32,
                >(get),
                set: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(GUID) -> u32,
                >(set),
            }
        }))
    }

    /// The overlay the user chose for the current power source.
    pub fn get(&self) -> Result<GUID, OsError> {
        let mut g = GUID::zeroed();
        // SAFETY: `g` is a valid out-parameter.
        let e = unsafe { (self.get)(&mut g) };
        crate::check(
            "PowerGetActualOverlayScheme",
            windows::Win32::Foundation::WIN32_ERROR(e),
        )?;
        Ok(g)
    }

    pub fn set(&self, g: GUID) -> Result<(), OsError> {
        // SAFETY: plain value argument.
        let e = unsafe { (self.set)(g) };
        crate::check(
            "PowerSetActiveOverlayScheme",
            windows::Win32::Foundation::WIN32_ERROR(e),
        )
    }
}
