//! File-system reads for the Storage screen (docs/14-storage.md). These run in
//! the desktop app under the user's own account, never in the service.

use std::path::{Path, PathBuf};

use verity_core::OsError;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_HANDLE_EOF, ERROR_NO_MORE_FILES, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetDiskFreeSpaceExW,
    GetDriveTypeW, GetFileInformationByHandleEx, GetLogicalDriveStringsW,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::{from_wide, wide};

/// One directory entry, as the file system reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Space the file occupies on disk (cluster-rounded, compressed or sparse).
    pub alloc: u64,
    /// Logical length.
    pub size: u64,
    /// `FILE_ATTRIBUTE_*` bits.
    pub attrs: u32,
    /// NTFS file ID; equal for hard links of one file. 0 where unsupported.
    pub file_id: u64,
    /// Last write, FILETIME.
    pub modified: u64,
}

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// Closes a handle on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Lists one directory in large batches. Access denied comes back as an
/// error carrying `ERROR_ACCESS_DENIED`, which callers treat as "not readable".
pub fn list_dir(path: &Path) -> Result<Vec<DirEntry>, OsError> {
    let w = wide(path);
    // SAFETY: `w` is NUL-terminated and outlives the call; the handle is owned below.
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(|e| OsError {
        api: "CreateFileW",
        code: (e.code().0 & 0xFFFF) as u32,
    })?;
    let h = Handle(h);

    let mut out = Vec::new();
    // 64 KiB, 8-byte aligned, as FILE_ID_BOTH_DIR_INFO requires.
    let mut buf = vec![0u64; 8192];
    let bytes = buf.len() * 8;
    let mut class = FileIdBothDirectoryRestartInfo;
    loop {
        // SAFETY: `buf` is writable for `bytes` bytes and suitably aligned.
        let r = unsafe {
            GetFileInformationByHandleEx(h.0, class, buf.as_mut_ptr().cast(), bytes as u32)
        };
        if r.is_err() {
            // SAFETY: reads the calling thread's last error.
            let code = unsafe { GetLastError() };
            if code == ERROR_NO_MORE_FILES || code == ERROR_HANDLE_EOF {
                break;
            }
            return Err(OsError {
                api: "GetFileInformationByHandleEx",
                code: code.0,
            });
        }
        class = FileIdBothDirectoryInfo;
        parse_batch(buf_bytes(&buf), &mut out);
    }
    Ok(out)
}

fn buf_bytes(buf: &[u64]) -> &[u8] {
    // SAFETY: any u64 buffer is a valid byte buffer of eight times the length.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast(), buf.len() * 8) }
}

/// Walks the chain of FILE_ID_BOTH_DIR_INFO records in one batch, bounds-checked.
fn parse_batch(b: &[u8], out: &mut Vec<DirEntry>) {
    let fixed = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
    let mut off = 0usize;
    while off + fixed <= b.len() {
        // SAFETY: `off + fixed` is inside `b`; read_unaligned copes with any alignment.
        let rec: FILE_ID_BOTH_DIR_INFO =
            unsafe { std::ptr::read_unaligned(b[off..].as_ptr().cast()) };
        let name_len = rec.FileNameLength as usize;
        let name_at = off + fixed;
        if name_at + name_len > b.len() {
            break;
        }
        let units: Vec<u16> = b[name_at..name_at + name_len]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let name = String::from_utf16_lossy(&units);
        if name != "." && name != ".." {
            out.push(DirEntry {
                is_dir: rec.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                alloc: rec.AllocationSize.max(0) as u64,
                size: rec.EndOfFile.max(0) as u64,
                attrs: rec.FileAttributes,
                file_id: rec.FileId as u64,
                modified: rec.LastWriteTime.max(0) as u64,
                name,
            });
        }
        if rec.NextEntryOffset == 0 {
            break;
        }
        off += rec.NextEntryOffset as usize;
    }
}

/// A fixed local drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// `C:\`
    pub root: PathBuf,
    pub total: u64,
    pub free: u64,
}

const DRIVE_FIXED: u32 = 3;

/// Fixed drives with their size and free space (free as the caller sees it).
pub fn drives() -> Result<Vec<Drive>, OsError> {
    let mut buf = [0u16; 512];
    // SAFETY: `buf` is writable; the return is the length written.
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        // SAFETY: reads the calling thread's last error.
        return Err(OsError {
            api: "GetLogicalDriveStringsW",
            code: unsafe { GetLastError() }.0,
        });
    }
    let mut out = Vec::new();
    for root in buf[..n].split(|&c| c == 0).filter(|s| !s.is_empty()) {
        let root_w: Vec<u16> = root.iter().copied().chain(Some(0)).collect();
        // SAFETY: `root_w` is NUL-terminated.
        if unsafe { GetDriveTypeW(PCWSTR(root_w.as_ptr())) } != DRIVE_FIXED {
            continue;
        }
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: out-pointers are valid locals; `root_w` is NUL-terminated.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root_w.as_ptr()),
                Some(&mut free),
                Some(&mut total),
                None,
            )
        };
        if ok.is_ok() {
            out.push(Drive {
                root: PathBuf::from(from_wide(&root_w)),
                total,
                free,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_a_real_directory() {
        let dir = std::env::temp_dir().join(format!("verity-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), vec![7u8; 10_000]).unwrap();
        let mut got = list_dir(&dir).unwrap();
        got.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].name.as_str(), got[0].is_dir), ("a.txt", false));
        assert_eq!(got[0].size, 10_000);
        assert!(got[0].alloc >= 10_000);
        assert_ne!(got[0].file_id, 0);
        assert!(got[1].is_dir);
        let missing = list_dir(&dir.join("nope")).unwrap_err();
        assert_eq!(missing.code, 2); // ERROR_FILE_NOT_FOUND
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!drives().unwrap().is_empty());
    }
}
