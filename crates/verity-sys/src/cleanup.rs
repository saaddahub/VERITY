//! Windows' own Disk Cleanup handlers (`IEmptyVolumeCache`), the code behind
//! Disk Cleanup and Storage Sense (docs/14-storage.md). Callers pass a handler
//! name from their compiled-in allowlist; this module only adds the fixed
//! registry prefix, never a client string.

use verity_core::OsError;
use windows::core::{implement, Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::System::Com::{
    CLSIDFromString, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::LegacyWindowsEnvironmentFeatures::{
    IEmptyVolumeCache, IEmptyVolumeCache2, IEmptyVolumeCacheCallBack,
    IEmptyVolumeCacheCallBack_Impl, EMPTY_VOLUME_CACHE_FLAGS,
};

use crate::{check, hresult, wide};

const ROOT: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\VolumeCaches";

/// A progress sink. Some handlers (D3D Shader Cache) crash without one, so one
/// is always passed; Verity needs no progress from it.
#[implement(IEmptyVolumeCacheCallBack)]
struct Quiet;

impl IEmptyVolumeCacheCallBack_Impl for Quiet_Impl {
    fn ScanProgress(&self, _used: u64, _flags: u32, _status: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn PurgeProgress(
        &self,
        _freed: u64,
        _to_free: u64,
        _flags: u32,
        _status: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// COM in a single-threaded apartment on this thread, as the handlers expect.
struct Apartment;

impl Apartment {
    fn enter() -> Result<Self, OsError> {
        // SAFETY: no preconditions; balanced by CoUninitialize in Drop.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
            .ok()
            .map_err(hresult("CoInitializeEx"))?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `enter`.
        unsafe { CoUninitialize() };
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key came from RegOpenKeyExW and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: a non-null string from the handler, NUL-terminated.
    let s = unsafe { p.to_string() }.unwrap_or_default();
    // SAFETY: handlers allocate these with CoTaskMemAlloc; freed once here.
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// One initialized handler, deactivated on drop.
struct Handler {
    cache: IEmptyVolumeCache,
    _key: Key,
    pub name: String,
    pub description: String,
}

impl Drop for Handler {
    fn drop(&mut self) {
        // SAFETY: the handler was initialized; Deactivate ends its use.
        let _ = unsafe { self.cache.Deactivate() };
    }
}

fn open(handler: &str, volume: &str) -> Result<Handler, OsError> {
    if handler.contains(['\\', '/']) {
        return Err(OsError {
            api: "cleanup handler name",
            code: 87, // ERROR_INVALID_PARAMETER
        });
    }
    let path = wide(format!(r"{ROOT}\{handler}"));
    let mut hkey = HKEY::default();
    // SAFETY: `path` is NUL-terminated; `hkey` receives a key closed by `Key`.
    check("RegOpenKeyExW", unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
    })?;
    let key = Key(hkey);

    let mut buf = [0u16; 64];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: reads the key's default value into `buf`, at most `len` bytes.
    check("RegGetValueW", unsafe {
        RegGetValueW(
            key.0,
            PCWSTR::null(),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    })?;
    // SAFETY: `buf` holds a NUL-terminated CLSID string.
    let clsid: GUID =
        unsafe { CLSIDFromString(PCWSTR(buf.as_ptr())) }.map_err(hresult("CLSIDFromString"))?;
    // SAFETY: COM is initialized on this thread by the caller.
    let cache: IEmptyVolumeCache = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }
        .map_err(hresult("CoCreateInstance(cleanup handler)"))?;

    let vol = wide(volume);
    let name_w = wide(handler);
    let (mut display, mut desc, mut button) = (PWSTR::null(), PWSTR::null(), PWSTR::null());
    let mut flags = EMPTY_VOLUME_CACHE_FLAGS(0);
    // SAFETY: strings are NUL-terminated and outlive the call; out-pointers are locals.
    let init = match cache.cast::<IEmptyVolumeCache2>() {
        Ok(c2) => unsafe {
            c2.InitializeEx(
                key.0,
                PCWSTR(vol.as_ptr()),
                PCWSTR(name_w.as_ptr()),
                &mut display,
                &mut desc,
                &mut button,
                &mut flags,
            )
        },
        Err(_) => unsafe {
            cache.Initialize(
                key.0,
                PCWSTR(vol.as_ptr()),
                &mut display,
                &mut desc,
                &mut flags,
            )
        },
    };
    let (name, description) = (take(display), take(desc));
    let _ = take(button);
    init.map_err(hresult("IEmptyVolumeCache::Initialize"))?;
    Ok(Handler {
        cache,
        _key: key,
        name: if name.is_empty() {
            handler.to_string()
        } else {
            name
        },
        description,
    })
}

/// What a handler would free on `volume` (like `C:\`), with Windows' own name
/// and description for it. Deletes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    pub name: String,
    pub description: String,
    pub bytes: u64,
}

/// Measures one handler. Runs its own COM apartment, so call it from a thread
/// that has none yet.
pub fn measure(handler: &str, volume: &str) -> Result<Measured, OsError> {
    let _com = Apartment::enter()?;
    let h = open(handler, volume)?;
    let bytes = space_used(&h)?;
    Ok(Measured {
        name: h.name.clone(),
        description: h.description.clone(),
        bytes,
    })
}

/// HRESULTs for "file or folder not found": the handler's folder does not
/// exist on this PC, so it has nothing to clean (probed: Temporary Setup Files).
const NOT_FOUND: [u32; 2] = [0x8007_0002, 0x8007_0003];

fn space_used(h: &Handler) -> Result<u64, OsError> {
    let mut bytes = 0u64;
    let cb: IEmptyVolumeCacheCallBack = Quiet.into();
    // SAFETY: the handler is initialized; `bytes` and `cb` outlive the call.
    unsafe { h.cache.GetSpaceUsed(&mut bytes, &cb) }.or_else(|e| {
        if NOT_FOUND.contains(&(e.code().0 as u32)) {
            Ok(())
        } else {
            Err(hresult("IEmptyVolumeCache::GetSpaceUsed")(e))
        }
    })?;
    Ok(bytes)
}

/// Lets the handler delete everything it measured, the way Disk Cleanup does,
/// and returns the space freed (measured before minus after). Permanent. Same
/// threading rule as [`measure`].
pub fn purge(handler: &str, volume: &str) -> Result<u64, OsError> {
    let _com = Apartment::enter()?;
    let before = {
        let h = open(handler, volume)?;
        let bytes = space_used(&h)?;
        if bytes == 0 {
            return Ok(0);
        }
        let cb: IEmptyVolumeCacheCallBack = Quiet.into();
        // SAFETY: GetSpaceUsed ran first, as Purge requires; `cb` outlives the call.
        unsafe { h.cache.Purge(bytes, &cb) }.map_err(hresult("IEmptyVolumeCache::Purge"))?;
        bytes
    };
    let after = open(handler, volume)
        .and_then(|h| space_used(&h))
        .unwrap_or(0);
    Ok(before.saturating_sub(after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_handler_and_rejects_paths() {
        let r = std::thread::spawn(|| measure("Thumbnail Cache", "C:\\"))
            .join()
            .unwrap()
            .unwrap();
        assert!(!r.name.is_empty());
        let bad = std::thread::spawn(|| measure(r"..\Run", "C:\\"))
            .join()
            .unwrap();
        assert_eq!(bad.unwrap_err().code, 87);
    }
}
