//! One-click junk cleanup (docs/14-storage.md, "Junk"): a fixed catalog of
//! categories that Windows and apps rebuild on their own, so they are deleted
//! for good after one confirmation. Machine-wide categories run in the
//! service through Windows' own Disk Cleanup handlers; the user's own run in
//! the app. Nothing outside this catalog can be named by a client.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::FsApi;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runs {
    /// Under the user's account, in the app.
    App,
    /// In the service (admin rights), for machine-wide files.
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// A Windows Disk Cleanup handler, by its registry key name.
    Handler(&'static str),
    /// Browser cache folders under the user's local app data.
    BrowserCaches,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Category {
    pub id: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    pub runs: Runs,
    pub how: How,
    /// Ticked by default.
    pub preselected: bool,
    /// Measuring takes minutes (Windows scans its component store).
    pub slow: bool,
}

const fn cat(
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    runs: Runs,
    how: How,
    preselected: bool,
) -> Category {
    Category {
        id,
        title,
        detail,
        runs,
        how,
        preselected,
        slow: false,
    }
}

/// The catalog. Windows handlers that can remove a way back (previous
/// installations, reset files, old drivers, file versions), touch a security
/// product, or empty a user folder (Downloads) are deliberately absent.
pub const CATALOG: [Category; 14] = [
    cat("temp", "Temporary files", "Files apps left in your temp folder and not used for a week.", Runs::App, How::Handler("Temporary Files"), true),
    cat("thumbnails", "Thumbnail cache", "Picture previews File Explorer redraws when you next open a folder.", Runs::App, How::Handler("Thumbnail Cache"), true),
    cat("browser", "Browser caches", "Saved copies of web pages in Chrome, Edge, Brave and Firefox. Close the browser first; open files are skipped. You stay signed in.", Runs::App, How::BrowserCaches, true),
    cat("internet", "Temporary internet files", "The older Windows web cache some apps still use.", Runs::App, How::Handler("Internet Cache Files"), true),
    cat("shaders", "DirectX shader cache", "Games rebuild these, so the first launch of each game can stutter for a while afterwards.", Runs::App, How::Handler("D3D Shader Cache"), false),
    cat("delivery", "Delivery Optimization files", "Update pieces Windows keeps to share with other PCs.", Runs::Service, How::Handler("Delivery Optimization Files"), true),
    cat("error_reports", "Error reports", "Crash reports already sent to, or never needed by, Microsoft.", Runs::Service, How::Handler("Windows Error Reporting Files"), true),
    cat("memory_dumps", "Crash memory dumps", "Large files Windows saved after a crash. Only useful to diagnose that crash.", Runs::Service, How::Handler("System error memory dump files"), true),
    cat("minidumps", "Crash minidumps", "Small crash records, only useful to diagnose an old crash.", Runs::Service, How::Handler("System error minidump files"), true),
    cat("setup_logs", "Setup logs", "Logs left by Windows setup.", Runs::Service, How::Handler("Setup Log Files"), true),
    cat("setup_temp", "Temporary setup files", "Files installers left behind.", Runs::Service, How::Handler("Temporary Setup Files"), true),
    cat("upgrade_logs", "Upgrade logs", "Logs from past Windows upgrades.", Runs::Service, How::Handler("Windows Upgrade Log Files"), true),
    cat("chkdsk", "Old disk-check fragments", "Pieces saved by old disk checks.", Runs::Service, How::Handler("Old ChkDsk Files"), true),
    Category {
        slow: true,
        ..cat("updates", "Windows Update leftovers", "Old versions of updated Windows files. Afterwards, installed updates cannot be uninstalled. Checking can take a few minutes.", Runs::Service, How::Handler("Update Cleanup"), false)
    },
];

/// The catalog entry for an id a client sent; None for anything else.
pub fn find(id: &str) -> Option<&'static Category> {
    CATALOG.iter().find(|c| c.id == id)
}

/// Cache folders of Chromium browsers and Firefox under `local_app_data`.
/// Only these folders are ever emptied; profile data and logins are elsewhere.
pub fn browser_cache_dirs(fs: &dyn FsApi, local_app_data: &Path) -> Vec<PathBuf> {
    const CHROMIUM: [&str; 3] = [
        r"Google\Chrome\User Data",
        r"Microsoft\Edge\User Data",
        r"BraveSoftware\Brave-Browser\User Data",
    ];
    const CACHE: [&str; 3] = [r"Cache\Cache_Data", "Code Cache", "GPUCache"];
    let mut out = Vec::new();
    let subdirs = |p: &Path| -> Vec<PathBuf> {
        fs.list_dir(p)
            .map(|e| {
                e.into_iter()
                    .filter(|x| x.is_dir)
                    .map(|x| p.join(x.name))
                    .collect()
            })
            .unwrap_or_default()
    };
    for browser in CHROMIUM {
        for profile in subdirs(&local_app_data.join(browser)) {
            for c in CACHE {
                let dir = profile.join(c);
                if fs.list_dir(&dir).is_ok() {
                    out.push(dir);
                }
            }
        }
    }
    for profile in subdirs(&local_app_data.join(r"Mozilla\Firefox\Profiles")) {
        let dir = profile.join("cache2");
        if fs.list_dir(&dir).is_ok() {
            out.push(dir);
        }
    }
    out
}

/// Space used by the files under `dirs`.
pub fn measure_dirs(fs: &dyn FsApi, dirs: &[PathBuf]) -> u64 {
    let mut total = 0;
    walk_files(fs, dirs, &mut |_, alloc| total += alloc);
    total
}

/// Deletes every file under `dirs` for good, leaving the folders, and returns
/// the space freed. Files in use (an open browser) are skipped.
pub fn clear_dirs(fs: &dyn FsApi, dirs: &[PathBuf]) -> u64 {
    let mut files = Vec::new();
    walk_files(fs, dirs, &mut |p, alloc| files.push((p, alloc)));
    files
        .into_iter()
        .filter(|(p, _)| fs.remove_file(p).is_ok())
        .map(|(_, alloc)| alloc)
        .sum()
}

fn walk_files(fs: &dyn FsApi, dirs: &[PathBuf], f: &mut dyn FnMut(PathBuf, u64)) {
    let mut stack: Vec<PathBuf> = dirs.to_vec();
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs.list_dir(&d) else {
            continue;
        };
        for e in entries {
            let p = d.join(&e.name);
            // Links inside a cache are never followed.
            if e.attrs & crate::attr::REPARSE_POINT != 0 {
                continue;
            }
            if e.is_dir {
                stack.push(p);
            } else {
                f(p, e.alloc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{dir, file, FakeFs};

    #[test]
    fn clears_files_inside_and_skips_locked_ones() {
        let mut fs = FakeFs::default();
        fs.add(
            r"C:\cache",
            vec![file("a", 100, 0), file("b", 50, 0), dir("sub")],
        )
        .add(r"C:\cache\sub", vec![file("c", 7, 0)]);
        fs.locked.push(PathBuf::from(r"C:\cache\b"));
        let dirs = [PathBuf::from(r"C:\cache")];
        assert_eq!(measure_dirs(&fs, &dirs), 157);
        assert_eq!(clear_dirs(&fs, &dirs), 107);
        let mut removed = fs.removed.lock().unwrap().clone();
        removed.sort();
        assert_eq!(
            removed,
            [
                PathBuf::from(r"C:\cache\a"),
                PathBuf::from(r"C:\cache\sub\c")
            ]
        );
    }

    #[test]
    fn catalog_ids_are_unique_and_risky_handlers_absent() {
        let mut ids: Vec<&str> = CATALOG.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        for banned in [
            "Previous Installations",
            "Windows ESD installation files",
            "Upgrade Discarded Files",
            "Device Driver Packages",
            "User file versions",
            "Windows Defender",
            "DownloadsFolder",
            "Language Pack",
        ] {
            assert!(
                !CATALOG.iter().any(|c| c.how == How::Handler(banned)),
                "{banned}"
            );
        }
        assert!(find("updates").is_some_and(|c| c.slow && !c.preselected));
        assert!(find(r"..\Run").is_none());
    }

    #[test]
    fn finds_only_browser_cache_folders() {
        let mut fs = FakeFs::default();
        fs.add(
            r"L:\Google\Chrome\User Data",
            vec![dir("Default"), dir("Profile 1"), file("Local State", 1, 0)],
        )
        .add(
            r"L:\Google\Chrome\User Data\Default\Cache\Cache_Data",
            vec![file("data_1", 10, 0)],
        )
        .add(r"L:\Google\Chrome\User Data\Default\Code Cache", vec![])
        .add(r"L:\Google\Chrome\User Data\Profile 1\GPUCache", vec![])
        .add(r"L:\Mozilla\Firefox\Profiles", vec![dir("abc.default")])
        .add(r"L:\Mozilla\Firefox\Profiles\abc.default\cache2", vec![]);
        let got = browser_cache_dirs(&fs, Path::new(r"L:\"));
        let want: Vec<PathBuf> = [
            r"L:\Google\Chrome\User Data\Default\Cache\Cache_Data",
            r"L:\Google\Chrome\User Data\Default\Code Cache",
            r"L:\Google\Chrome\User Data\Profile 1\GPUCache",
            r"L:\Mozilla\Firefox\Profiles\abc.default\cache2",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
    }
}
