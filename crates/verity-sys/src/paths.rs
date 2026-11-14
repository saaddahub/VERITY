use std::io;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION};

use crate::security::SecurityDescriptor;
use crate::{check, wide};

/// SYSTEM and Administrators full control, Users read; inherited by children.
const DATA_DIR_SDDL: &str = "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;BU)";

fn known_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.into())
        .join("Verity")
}

/// `%ProgramData%\Verity`
pub fn data_dir() -> PathBuf {
    known_dir("ProgramData", r"C:\ProgramData")
}

/// `%ProgramFiles%\Verity`; standard users cannot write here, so the SYSTEM
/// service binary cannot be swapped by a non-admin.
pub fn install_dir() -> PathBuf {
    known_dir("ProgramFiles", r"C:\Program Files")
}

/// Creates `dir` and `dir\logs`, then applies the data-directory DACL to the tree.
pub fn ensure_data_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir.join("logs"))?;
    let sd = SecurityDescriptor::from_sddl(DATA_DIR_SDDL)?;
    let dacl = sd.dacl()?;
    let path = wide(dir);
    // SAFETY: `path` is NUL-terminated; `dacl` points into `sd`, which outlives the call.
    let e = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(dacl),
            None,
        )
    };
    Ok(check("SetNamedSecurityInfoW", e)?)
}
