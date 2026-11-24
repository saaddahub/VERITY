//! Storage (docs/14-storage.md): what fills a drive, duplicates, clean-up
//! suggestions, and which paths may never be deleted. Pure logic over the
//! [`FsApi`] trait; [`RealFs`] is the real file system. Runs in the desktop
//! app under the user's account, never in the service.

pub mod cleanup;
pub mod dupes;
pub mod junk;
pub mod safety;
pub mod tree;

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

pub use tree::{scan, Node, Progress, Tree, ROOT};

/// `FILE_ATTRIBUTE_*` bits the logic looks at.
pub mod attr {
    pub const SYSTEM: u32 = 0x4;
    pub const REPARSE_POINT: u32 = 0x400;
    pub const OFFLINE: u32 = 0x1000;
    pub const RECALL_ON_OPEN: u32 = 0x4_0000;
    pub const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    /// Cloud placeholder: its content is not on this disk.
    pub const ONLINE_ONLY: u32 = OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS;
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub name: String,
    pub is_dir: bool,
    /// Space used on disk.
    pub alloc: u64,
    /// Logical length.
    pub size: u64,
    pub attrs: u32,
    /// Equal for hard links of one file; 0 where the file system has none.
    pub file_id: u64,
    /// Last write, FILETIME.
    pub modified: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    /// The user's account may not read it.
    NotReadable,
    Other(String),
}

/// What the storage logic needs from the file system.
pub trait FsApi: Sync {
    fn list_dir(&self, path: &Path) -> Result<Vec<RawEntry>, FsError>;
    /// SHA-256 of the first `limit` bytes, or of the whole file for `None`.
    fn hash(&self, path: &Path, limit: Option<u64>) -> Result<[u8; 32], FsError>;
    /// Deletes one file for good. Used only for junk-catalog folders.
    fn remove_file(&self, path: &Path) -> Result<(), FsError>;
}

/// The real file system, through `verity-sys`.
pub struct RealFs;

const ERROR_ACCESS_DENIED: u32 = 5;

impl FsApi for RealFs {
    fn list_dir(&self, path: &Path) -> Result<Vec<RawEntry>, FsError> {
        let entries = verity_sys::files::list_dir(path).map_err(|e| {
            if e.code == ERROR_ACCESS_DENIED {
                FsError::NotReadable
            } else {
                FsError::Other(e.to_string())
            }
        })?;
        Ok(entries
            .into_iter()
            .map(|e| RawEntry {
                name: e.name,
                is_dir: e.is_dir,
                alloc: e.alloc,
                size: e.size,
                attrs: e.attrs,
                file_id: e.file_id,
                modified: e.modified,
            })
            .collect())
    }

    fn hash(&self, path: &Path, limit: Option<u64>) -> Result<[u8; 32], FsError> {
        let file = std::fs::File::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => FsError::NotReadable,
            _ => FsError::Other(e.to_string()),
        })?;
        let mut reader: Box<dyn Read> = match limit {
            Some(n) => Box::new(file.take(n)),
            None => Box::new(file),
        };
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| FsError::Other(e.to_string()))?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(h.finalize().into())
    }

    fn remove_file(&self, path: &Path) -> Result<(), FsError> {
        std::fs::remove_file(path).map_err(|e| FsError::Other(e.to_string()))
    }
}

/// An in-memory file system for tests.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct FakeFs {
        pub dirs: HashMap<PathBuf, Vec<RawEntry>>,
        pub contents: HashMap<PathBuf, Vec<u8>>,
        pub unreadable: Vec<PathBuf>,
        /// Files that refuse deletion (in use).
        pub locked: Vec<PathBuf>,
        pub removed: Mutex<Vec<PathBuf>>,
    }

    pub fn file(name: &str, alloc: u64, modified: u64) -> RawEntry {
        RawEntry {
            name: name.into(),
            is_dir: false,
            alloc,
            size: alloc,
            attrs: 0,
            file_id: 0,
            modified,
        }
    }

    pub fn dir(name: &str) -> RawEntry {
        RawEntry {
            name: name.into(),
            is_dir: true,
            alloc: 0,
            size: 0,
            attrs: 0x10,
            file_id: 0,
            modified: 0,
        }
    }

    impl FakeFs {
        pub fn add(&mut self, dir: &str, entries: Vec<RawEntry>) -> &mut Self {
            self.dirs.insert(PathBuf::from(dir), entries);
            self
        }
    }

    impl FsApi for FakeFs {
        fn list_dir(&self, path: &Path) -> Result<Vec<RawEntry>, FsError> {
            if self.unreadable.iter().any(|p| p == path) {
                return Err(FsError::NotReadable);
            }
            self.dirs
                .get(path)
                .cloned()
                .ok_or_else(|| FsError::Other(format!("no such dir {}", path.display())))
        }

        fn hash(&self, path: &Path, limit: Option<u64>) -> Result<[u8; 32], FsError> {
            let data = self.contents.get(path).ok_or(FsError::NotReadable)?;
            let n = limit.map_or(data.len(), |l| data.len().min(l as usize));
            Ok(Sha256::digest(&data[..n]).into())
        }

        fn remove_file(&self, path: &Path) -> Result<(), FsError> {
            if self.locked.iter().any(|p| p == path) {
                return Err(FsError::Other("in use".into()));
            }
            if let Ok(mut r) = self.removed.lock() {
                r.push(path.to_path_buf());
            }
            Ok(())
        }
    }
}
