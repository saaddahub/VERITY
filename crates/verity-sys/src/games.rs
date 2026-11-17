//! Where game launchers keep their libraries: registry reads and volume
//! listing for the library scan (docs/06-session-mode.md).

use std::path::PathBuf;

use verity_core::OsError;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDriveStringsW};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
};

use crate::system::reg_string;
use crate::{check, from_wide, wide};

const STEAM_KEY: &str = r"SOFTWARE\WOW6432Node\Valve\Steam";
const GOG_KEY: &str = r"SOFTWARE\WOW6432Node\GOG.com\Games";
/// `DRIVE_FIXED` from WinBase.h.
const DRIVE_FIXED: u32 = 3;

/// Steam's install folder from HKLM (the service is SYSTEM, so HKCU is not the user's).
pub fn steam_install_dir() -> Option<PathBuf> {
    reg_string(STEAM_KEY, "InstallPath").ok().map(PathBuf::from)
}

/// One installed GOG game: (id, name, folder, listed exe).
pub struct GogGame {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub exe: Option<PathBuf>,
}

/// GOG Galaxy's per-game keys. Missing key means no GOG games.
pub fn gog_games() -> Result<Vec<GogGame>, OsError> {
    let ids = match subkeys(GOG_KEY) {
        Ok(ids) => ids,
        Err(e) if e.code == 2 => return Ok(Vec::new()), // ERROR_FILE_NOT_FOUND
        Err(e) => return Err(e),
    };
    Ok(ids
        .into_iter()
        .filter_map(|id| {
            let key = format!(r"{GOG_KEY}\{id}");
            let name = reg_string(&key, "gameName").ok()?;
            let path = PathBuf::from(reg_string(&key, "path").ok()?);
            let exe = reg_string(&key, "exe").ok().map(PathBuf::from);
            Some(GogGame {
                id,
                name,
                path,
                exe,
            })
        })
        .collect())
}

fn subkeys(path: &str) -> Result<Vec<String>, OsError> {
    let p = wide(path);
    let mut key = HKEY::default();
    // SAFETY: `p` is NUL-terminated; `key` receives the opened handle, closed below.
    let e = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(p.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    check("RegOpenKeyExW", e)?;
    let mut out = Vec::new();
    let mut result = Ok(());
    for i in 0.. {
        // Registry key names are at most 255 characters.
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `key` is open; `name`/`len` describe a writable buffer of `len` chars.
        let e = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if e == ERROR_NO_MORE_ITEMS {
            break;
        }
        if let Err(err) = check("RegEnumKeyExW", e) {
            result = Err(err);
            break;
        }
        out.push(from_wide(&name[..len as usize]));
    }
    // SAFETY: `key` was opened above and is closed once.
    let _ = unsafe { RegCloseKey(key) };
    result.map(|_| out)
}

/// Root of each fixed drive, e.g. `C:\`.
pub fn fixed_volumes() -> Vec<PathBuf> {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is a writable buffer; the return is the length written.
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return Vec::new();
    }
    buf[..n]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .filter(|root| {
            let w: Vec<u16> = root.iter().copied().chain(Some(0)).collect();
            // SAFETY: `w` is a NUL-terminated root path.
            let kind = unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) };
            kind == DRIVE_FIXED
        })
        .map(|root| PathBuf::from(String::from_utf16_lossy(root)))
        .collect()
}
