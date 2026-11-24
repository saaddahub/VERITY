//! Shell operations for the Storage screen, run in the desktop app under the
//! user's account (docs/14-storage.md, "Deleting"): Recycle Bin moves,
//! emptying the Recycle Bin and known folders. Nothing here opens another app.

use std::path::PathBuf;

use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_Videos, FileOperation, IFileOperation, IShellItem,
    SHCreateItemFromParsingName, SHEmptyRecycleBinW, SHGetKnownFolderPath, SHQueryRecycleBinW,
    FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_WANTNUKEWARNING, KF_FLAG_DEFAULT,
    SHERB_NOCONFIRMATION, SHQUERYRBINFO,
};

use crate::{hresult, wide};

/// COM for this thread, single-threaded apartment as the shell requires;
/// uninitialized on drop.
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

fn hwnd(owner: isize) -> HWND {
    HWND(owner as *mut _)
}

/// Moves `paths` to the Recycle Bin in one shell operation, with Explorer's
/// own rules for files in use and permissions. Anything the Recycle Bin
/// cannot take (too large, a removable drive) is deleted only after Windows'
/// own "delete permanently?" warning, which is the second confirmation the
/// owner approved. Runs a COM apartment on the calling thread, so call it from
/// a thread that has no COM apartment yet. Callers check afterwards which
/// paths are gone.
pub fn recycle(paths: &[PathBuf], owner: Option<isize>) -> Result<(), OsError> {
    let _com = Apartment::enter()?;
    // SAFETY: COM is initialized on this thread; FileOperation is a shell CLSID.
    let op: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL) }
        .map_err(hresult("CoCreateInstance(FileOperation)"))?;
    // SAFETY: `op` is a live IFileOperation; flags are documented values.
    unsafe {
        op.SetOperationFlags(
            FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOFX_RECYCLEONDELETE,
        )
    }
    .map_err(hresult("IFileOperation::SetOperationFlags"))?;
    if let Some(o) = owner {
        // SAFETY: the window handle belongs to this app and outlives the operation.
        unsafe { op.SetOwnerWindow(hwnd(o)) }.map_err(hresult("IFileOperation::SetOwnerWindow"))?;
    }
    for p in paths {
        let w = wide(p);
        // SAFETY: `w` is NUL-terminated and outlives the call.
        let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }
            .map_err(hresult("SHCreateItemFromParsingName"))?;
        // SAFETY: `item` is a live shell item.
        unsafe { op.DeleteItem(&item, None) }.map_err(hresult("IFileOperation::DeleteItem"))?;
    }
    // SAFETY: the operation was fully queued above. A cancel by the user comes
    // back as an error code, which the caller reports by checking what is gone.
    let _ = unsafe { op.PerformOperations() };
    Ok(())
}

/// Bytes and item count in the Recycle Bins of all drives.
pub fn recycle_bin_size() -> Result<(u64, u64), OsError> {
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: a null root means every drive; `info.cbSize` is set.
    unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info) }
        .map_err(hresult("SHQueryRecycleBinW"))?;
    Ok((info.i64Size.max(0) as u64, info.i64NumItems.max(0) as u64))
}

/// Empties every Recycle Bin. Permanent: callers confirm with the user first.
pub fn empty_recycle_bin(owner: Option<isize>) -> Result<(), OsError> {
    // SAFETY: a null root means every drive. Verity has already asked the user.
    match unsafe { SHEmptyRecycleBinW(owner.map(hwnd), PCWSTR::null(), SHERB_NOCONFIRMATION) } {
        Ok(()) => Ok(()),
        // E_UNEXPECTED: the bin was already empty.
        Err(e) if e.code().0 as u32 == 0x8000_FFFF => Ok(()),
        Err(e) => Err(hresult("SHEmptyRecycleBinW")(e)),
    }
}

/// Folders the Storage screen needs, following redirection (OneDrive, moved Documents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    Profile,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Videos,
    Music,
}

pub fn known_folder(k: Known) -> Option<PathBuf> {
    let id = match k {
        Known::Profile => &FOLDERID_Profile,
        Known::Desktop => &FOLDERID_Desktop,
        Known::Documents => &FOLDERID_Documents,
        Known::Downloads => &FOLDERID_Downloads,
        Known::Pictures => &FOLDERID_Pictures,
        Known::Videos => &FOLDERID_Videos,
        Known::Music => &FOLDERID_Music,
    };
    // SAFETY: `id` is a static KNOWNFOLDERID; the returned buffer is freed below.
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: on success `p` is a valid NUL-terminated string.
    let s = unsafe { p.to_string() }.ok();
    // SAFETY: allocated by the shell with CoTaskMemAlloc, freed once.
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_folders_and_recycle_bin_query() {
        let profile = known_folder(Known::Profile).expect("profile");
        assert!(profile.is_dir());
        assert!(known_folder(Known::Downloads).is_some());
        recycle_bin_size().expect("query the Recycle Bin");
    }

    #[test]
    fn recycling_moves_a_file_out_of_its_folder() {
        let dir = std::env::temp_dir().join(format!("verity-recycle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("throwaway.txt");
        std::fs::write(&f, b"verity test file").unwrap();
        // Runs on its own thread: the test harness thread may already have COM.
        let target = vec![f.clone()];
        std::thread::spawn(move || recycle(&target, None))
            .join()
            .unwrap()
            .unwrap();
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
