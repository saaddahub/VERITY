//! Clean-up suggestions (docs/14-storage.md). Each lists exactly the items
//! it would remove; nothing is removed until the user ticks and confirms.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use crate::tree::{Tree, ROOT};
use crate::{scan, FsApi};

/// FILETIME ticks per day.
const DAY: u64 = 864_000_000_000;
const TEMP_AGE_DAYS: u64 = 7;
const DOWNLOAD_AGE_DAYS: u64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The user ticks items; they go to the Recycle Bin.
    Delete,
    /// Emptying the Recycle Bin (permanent, confirmed separately).
    EmptyRecycleBin,
    /// Windows' own Storage Sense removes these.
    OpenStorageSense,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub path: PathBuf,
    pub bytes: u64,
    /// Newest last write inside, FILETIME.
    pub modified: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub id: &'static str,
    pub title: &'static str,
    pub detail: &'static str,
    pub kind: Kind,
    /// Total size, or None when Windows does not let the user read it.
    pub bytes: Option<u64>,
    pub items: Vec<Item>,
    /// Ticked by default.
    pub preselected: bool,
}

/// Where to look.
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub temp: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    /// (bytes, items) from the shell, when known.
    pub recycle_bin: Option<(u64, u64)>,
    /// Windows' own leftovers that Storage Sense cleans: previous Windows
    /// installation, crash dumps and the like.
    pub windows_leftovers: Vec<PathBuf>,
}

fn walk(fs: &dyn FsApi, dir: &Path) -> Tree {
    scan(fs, dir, 2, &AtomicBool::new(false), &mut |_| {})
}

fn top_items(t: &Tree, keep: impl Fn(&crate::Node, &str) -> bool) -> Vec<Item> {
    let mut items: Vec<Item> = t
        .children(ROOT)
        .into_iter()
        .filter_map(|c| {
            let n = t.node(c)?;
            keep(n, t.name(c)).then(|| Item {
                path: t.path(c),
                bytes: n.total,
                modified: n.newest,
                is_dir: n.is_dir,
            })
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    items
}

fn installer(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [".exe", ".msi", ".msix", ".iso"]
        .iter()
        .any(|e| n.ends_with(e))
}

pub fn suggest(fs: &dyn FsApi, places: &Places, now: u64) -> Vec<Suggestion> {
    let mut out = Vec::new();

    if let Some((bytes, items)) = places.recycle_bin.filter(|r| r.1 > 0) {
        out.push(Suggestion {
            id: "recycle_bin",
            title: "Recycle Bin",
            detail: if items == 1 {
                "1 deleted item is still taking space. Emptying it cannot be undone."
            } else {
                "Deleted items are still taking space. Emptying it cannot be undone."
            },
            kind: Kind::EmptyRecycleBin,
            bytes: Some(bytes),
            items: Vec::new(),
            preselected: false,
        });
    }

    if let Some(temp) = &places.temp {
        let t = walk(fs, temp);
        let old = now.saturating_sub(TEMP_AGE_DAYS * DAY);
        let items = top_items(&t, |n, _| n.newest > 0 && n.newest < old);
        out.push(Suggestion {
            id: "temp",
            title: "Old temporary files",
            detail: "Left behind by apps and installers, untouched for over a week.",
            kind: Kind::Delete,
            bytes: Some(items.iter().map(|i| i.bytes).sum()),
            items,
            preselected: true,
        });
    }

    if let Some(downloads) = &places.downloads {
        let t = walk(fs, downloads);
        let old = now.saturating_sub(DOWNLOAD_AGE_DAYS * DAY);
        let installers = top_items(&t, |n, name| !n.is_dir && installer(name));
        let stale = top_items(&t, |n, name| {
            n.newest > 0 && n.newest < old && !(!n.is_dir && installer(name))
        });
        out.push(Suggestion {
            id: "installers",
            title: "Installers in Downloads",
            detail: "Setup files are usually not needed once the app is installed.",
            kind: Kind::Delete,
            bytes: Some(installers.iter().map(|i| i.bytes).sum()),
            items: installers,
            preselected: false,
        });
        out.push(Suggestion {
            id: "old_downloads",
            title: "Old downloads",
            detail: "Not opened or changed in over three months. Check before removing.",
            kind: Kind::Delete,
            bytes: Some(stale.iter().map(|i| i.bytes).sum()),
            items: stale,
            preselected: false,
        });
    }

    if !places.windows_leftovers.is_empty() {
        let mut bytes = Some(0u64);
        for dir in &places.windows_leftovers {
            let t = walk(fs, dir);
            match (&mut bytes, t.unreadable) {
                (Some(b), 0) => *b += t.node(ROOT).map_or(0, |n| n.total),
                _ => bytes = None,
            }
        }
        out.push(Suggestion {
            id: "windows",
            title: "Windows leftovers",
            detail: "Update files, a previous Windows installation and crash reports. \
                     Windows' own Storage Sense removes these safely.",
            kind: Kind::OpenStorageSense,
            bytes,
            items: Vec::new(),
            preselected: false,
        });
    }

    out.retain(|s| s.kind != Kind::Delete || !s.items.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{dir, file, FakeFs};

    #[test]
    fn suggestions_list_exactly_their_items() {
        let now = 1000 * DAY;
        let mut fs = FakeFs::default();
        fs.add(
            r"T:\Temp",
            vec![
                file("old.tmp", 500, now - 30 * DAY),
                file("fresh.tmp", 900, now - DAY),
                dir("Setup"),
            ],
        )
        .add(r"T:\Temp\Setup", vec![file("cab", 200, now - 10 * DAY)])
        .add(
            r"T:\Downloads",
            vec![
                file("game-setup.exe", 4000, now - DAY),
                file("old video.mp4", 7000, now - 200 * DAY),
                file("recent.pdf", 50, now - 5 * DAY),
            ],
        )
        .add(r"T:\Windows.old", vec![]);
        fs.unreadable.push(PathBuf::from(r"T:\Windows.old"));
        let places = Places {
            temp: Some(PathBuf::from(r"T:\Temp")),
            downloads: Some(PathBuf::from(r"T:\Downloads")),
            recycle_bin: Some((12_345, 3)),
            windows_leftovers: vec![PathBuf::from(r"T:\Windows.old")],
        };
        let s = suggest(&fs, &places, now);
        let ids: Vec<&str> = s.iter().map(|x| x.id).collect();
        assert_eq!(
            ids,
            [
                "recycle_bin",
                "temp",
                "installers",
                "old_downloads",
                "windows"
            ]
        );

        let temp = &s[1];
        let names: Vec<String> = temp
            .items
            .iter()
            .map(|i| i.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["old.tmp", "Setup"]);
        assert_eq!(temp.bytes, Some(700));
        assert_eq!(s[2].items.len(), 1);
        assert_eq!(
            s[3].items[0].path,
            PathBuf::from(r"T:\Downloads\old video.mp4")
        );
        assert_eq!(s[4].bytes, None, "unreadable leftovers have no size");
        assert_eq!(s[4].kind, Kind::OpenStorageSense);
    }

    #[test]
    fn empty_suggestions_are_left_out() {
        let mut fs = FakeFs::default();
        fs.add(r"T:\Temp", vec![]);
        let places = Places {
            temp: Some(PathBuf::from(r"T:\Temp")),
            recycle_bin: Some((0, 0)),
            ..Default::default()
        };
        assert!(suggest(&fs, &places, DAY).is_empty());
    }
}
