//! Where deletion is refused, whatever the user ticks (docs/14-storage.md,
//! "Deleting"). Checked again right before every delete.

use std::path::{Component, Path, PathBuf};

use crate::attr;

/// Locations that are never offered and never deleted.
#[derive(Debug, Clone, Default)]
pub struct Protected {
    /// Windows, Program Files (both), ProgramData, Verity's data folder.
    pub system: Vec<PathBuf>,
    /// `C:\Users`: only the user's own profile below it is allowed.
    pub users: Option<PathBuf>,
    /// The user's profile folder.
    pub profile: Option<PathBuf>,
    /// Folders that may be emptied but never deleted themselves (profile,
    /// Desktop, Documents, Downloads, Pictures, Videos, Music, temp).
    pub keep: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotAFullPath,
    DriveRoot,
    SystemLocation,
    OtherUser,
    KeptFolder,
    SystemFile,
    Link,
}

impl Refusal {
    /// Plain words for the UI.
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::NotAFullPath => "Not a full path",
            Refusal::DriveRoot => "A whole drive is never deleted",
            Refusal::SystemLocation => "Windows and installed programs need this",
            Refusal::OtherUser => "Belongs to another user of this PC",
            Refusal::KeptFolder => "A main folder like Documents is kept; its contents can go",
            Refusal::SystemFile => "Windows marks this as a system file",
            Refusal::Link => "A link to another folder; open the folder itself instead",
        }
    }
}

fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

fn under(p: &str, dir: &Path) -> bool {
    let d = norm(dir);
    !d.is_empty() && (p == d || p.starts_with(&format!("{d}\\")))
}

/// Whether `path` (with its attributes, when known) may be deleted.
pub fn check(path: &Path, attrs: Option<u32>, p: &Protected) -> Result<(), Refusal> {
    let mut comps = path.components();
    let full = matches!(comps.next(), Some(Component::Prefix(_)))
        && matches!(comps.next(), Some(Component::RootDir));
    if !full
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Refusal::NotAFullPath);
    }
    if path.components().count() <= 2 {
        return Err(Refusal::DriveRoot);
    }
    let s = norm(path);
    if p.system.iter().any(|d| under(&s, d)) {
        return Err(Refusal::SystemLocation);
    }
    if let Some(users) = &p.users {
        let own = p.profile.as_ref().is_some_and(|pr| under(&s, pr));
        if under(&s, users) && !own {
            return Err(Refusal::OtherUser);
        }
    }
    if p.keep.iter().any(|k| norm(k) == s) {
        return Err(Refusal::KeptFolder);
    }
    if let Some(a) = attrs {
        if a & attr::REPARSE_POINT != 0 {
            return Err(Refusal::Link);
        }
        if a & attr::SYSTEM != 0 {
            return Err(Refusal::SystemFile);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protected() -> Protected {
        Protected {
            system: vec![
                PathBuf::from(r"C:\Windows"),
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Program Files (x86)"),
                PathBuf::from(r"C:\ProgramData"),
            ],
            users: Some(PathBuf::from(r"C:\Users")),
            profile: Some(PathBuf::from(r"C:\Users\sam")),
            keep: vec![
                PathBuf::from(r"C:\Users\sam"),
                PathBuf::from(r"C:\Users\sam\Downloads"),
            ],
        }
    }

    #[test]
    fn refuses_protected_places() {
        let p = protected();
        let no = |s: &str| check(Path::new(s), None, &p).unwrap_err();
        assert_eq!(no(r"C:\"), Refusal::DriveRoot);
        assert_eq!(no(r"D:\"), Refusal::DriveRoot);
        assert_eq!(no(r"C:\Windows\Temp\x.tmp"), Refusal::SystemLocation);
        assert_eq!(no(r"c:\program files\Steam"), Refusal::SystemLocation);
        assert_eq!(no(r"C:\ProgramData"), Refusal::SystemLocation);
        assert_eq!(no(r"C:\Users\alex\Documents\a.doc"), Refusal::OtherUser);
        assert_eq!(no(r"C:\Users\Public\a.doc"), Refusal::OtherUser);
        assert_eq!(no(r"C:\Users\sam\Downloads\"), Refusal::KeptFolder);
        assert_eq!(no(r"C:\Users\sam"), Refusal::KeptFolder);
        assert_eq!(no(r"relative\file.txt"), Refusal::NotAFullPath);
        assert_eq!(no(r"C:\Users\sam\..\alex\x"), Refusal::NotAFullPath);
        let sys = check(Path::new(r"D:\x.bin"), Some(attr::SYSTEM), &p);
        assert_eq!(sys, Err(Refusal::SystemFile));
        let link = check(Path::new(r"D:\Link"), Some(attr::REPARSE_POINT | 0x10), &p);
        assert_eq!(link, Err(Refusal::Link));
    }

    #[test]
    fn allows_the_users_own_files_and_other_drives() {
        let p = protected();
        for ok in [
            r"C:\Users\sam\Downloads\setup.exe",
            r"C:\Users\sam\Videos\clip.mp4",
            r"C:\Users\sam\AppData\Local\Temp\old.tmp",
            r"D:\Games\Old Game",
            r"C:\Windowsfoo\x", // only the real Windows folder is protected
        ] {
            assert_eq!(check(Path::new(ok), Some(0x20), &p), Ok(()), "{ok}");
        }
    }
}
