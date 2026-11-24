//! The folder tree of one scan, built by a parallel walk.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::{attr, FsApi, FsError, RawEntry};

/// Index of the scanned folder itself.
pub const ROOT: u32 = 0;

/// The account could not read this folder; its size is unknown.
pub const UNREADABLE: u8 = 1;
/// A cloud placeholder: counted as zero local space.
pub const ONLINE_ONLY: u8 = 2;
/// A second name of a file already counted (hard link): counted as zero.
pub const LINK_DUP: u8 = 4;
/// A junction or symbolic link: shown, never followed.
pub const REPARSE: u8 = 8;
/// Has the System attribute.
pub const SYSTEM: u8 = 16;
/// Deleted since the scan.
pub const REMOVED: u8 = 32;

const NONE: u32 = u32::MAX;

/// One entry. Kept small: a scan of a system drive holds over a million.
/// Names live in the tree's shared string; children are a linked list.
#[derive(Debug, Clone)]
pub struct Node {
    name_at: u32,
    name_len: u16,
    pub is_dir: bool,
    pub flags: u8,
    pub parent: u32,
    first_child: u32,
    next_sibling: u32,
    /// Files under it (1 for a file).
    pub files: u32,
    /// Space this entry itself uses (0 for links already counted and placeholders).
    pub alloc: u64,
    /// Logical length of a file.
    pub size: u64,
    /// Space used by the entry and everything under it.
    pub total: u64,
    /// Newest last-write time under it, FILETIME.
    pub newest: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub done: bool,
}

pub struct Tree {
    root: PathBuf,
    nodes: Vec<Node>,
    names: String,
    pub unreadable: u64,
    pub cancelled: bool,
}

/// Scans `root` with `threads` workers. `on_progress` runs on the calling
/// thread at most every 150 ms and once at the end.
pub fn scan(
    fs: &dyn FsApi,
    root: &Path,
    threads: usize,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(&Progress),
) -> Tree {
    let mut tree = Tree {
        root: root.to_path_buf(),
        nodes: Vec::new(),
        names: String::new(),
        unreadable: 0,
        cancelled: false,
    };
    tree.push(&root.to_string_lossy(), ROOT, true, 0, 0, 0, 0);
    let queue: Mutex<(VecDeque<(u32, PathBuf)>, bool)> =
        Mutex::new((VecDeque::from([(ROOT, root.to_path_buf())]), false));
    let wake = Condvar::new();
    let (tx, rx) = mpsc::channel::<(u32, PathBuf, Result<Vec<RawEntry>, FsError>)>();
    let mut seen_ids: HashSet<u64> = HashSet::new();
    let mut progress = Progress::default();

    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            let tx = tx.clone();
            let (queue, wake) = (&queue, &wake);
            s.spawn(move || loop {
                let job = {
                    let Ok(mut q) = queue.lock() else { return };
                    loop {
                        if q.1 {
                            return;
                        }
                        if let Some(job) = q.0.pop_front() {
                            break job;
                        }
                        q = match wake.wait(q) {
                            Ok(q) => q,
                            Err(_) => return,
                        };
                    }
                };
                let listed = fs.list_dir(&job.1);
                if tx.send((job.0, job.1, listed)).is_err() {
                    return;
                }
            });
        }
        drop(tx);

        let mut outstanding = 1u64;
        let mut last = Instant::now();
        while outstanding > 0 {
            if cancel.load(Ordering::Relaxed) {
                tree.cancelled = true;
                break;
            }
            let Ok((id, path, listed)) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            outstanding -= 1;
            progress.dirs += 1;
            match listed {
                Ok(entries) => {
                    let mut jobs = Vec::new();
                    for e in entries {
                        let child = tree.add(id, e, &mut seen_ids, &mut progress);
                        let node = &tree.nodes[child as usize];
                        if node.is_dir && node.flags & REPARSE == 0 {
                            jobs.push((child, path.join(tree.name(child))));
                        }
                    }
                    if !jobs.is_empty() {
                        outstanding += jobs.len() as u64;
                        if let Ok(mut q) = queue.lock() {
                            q.0.extend(jobs);
                        }
                        wake.notify_all();
                    }
                }
                Err(_) => {
                    tree.nodes[id as usize].flags |= UNREADABLE;
                    tree.unreadable += 1;
                }
            }
            if last.elapsed() >= Duration::from_millis(150) {
                on_progress(&progress);
                last = Instant::now();
            }
        }
        if let Ok(mut q) = queue.lock() {
            q.1 = true;
            q.0.clear();
        }
        wake.notify_all();
    });

    tree.sum_up();
    progress.done = true;
    on_progress(&progress);
    tree
}

impl Tree {
    fn add(
        &mut self,
        parent: u32,
        e: RawEntry,
        seen_ids: &mut HashSet<u64>,
        progress: &mut Progress,
    ) -> u32 {
        let mut flags = 0;
        let mut alloc = e.alloc;
        if e.attrs & attr::REPARSE_POINT != 0 && e.is_dir {
            flags |= REPARSE;
        }
        if e.attrs & attr::SYSTEM != 0 {
            flags |= SYSTEM;
        }
        if !e.is_dir {
            if e.attrs & attr::ONLINE_ONLY != 0 {
                flags |= ONLINE_ONLY;
                alloc = 0;
            } else if e.file_id != 0 && !seen_ids.insert(e.file_id) {
                flags |= LINK_DUP;
                alloc = 0;
            }
            progress.files += 1;
            progress.bytes += alloc;
        }
        let newest = if e.is_dir { 0 } else { e.modified };
        self.push(&e.name, parent, e.is_dir, flags, alloc, e.size, newest)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        name: &str,
        parent: u32,
        is_dir: bool,
        flags: u8,
        alloc: u64,
        size: u64,
        newest: u64,
    ) -> u32 {
        // Windows names are at most 255 UTF-16 units, far under u16::MAX bytes.
        let name = &name[..name.floor_char_boundary(usize::from(u16::MAX))];
        let id = self.nodes.len() as u32;
        let next_sibling = if id == ROOT {
            NONE
        } else {
            std::mem::replace(&mut self.nodes[parent as usize].first_child, id)
        };
        self.nodes.push(Node {
            name_at: self.names.len() as u32,
            name_len: name.len() as u16,
            is_dir,
            flags,
            parent,
            first_child: NONE,
            next_sibling,
            files: u32::from(!is_dir),
            alloc,
            size,
            total: alloc,
            newest,
        });
        self.names.push_str(name);
        id
    }

    /// The name of a node.
    pub fn name(&self, id: u32) -> &str {
        let Some(n) = self.nodes.get(id as usize) else {
            return "";
        };
        let at = n.name_at as usize;
        self.names
            .get(at..at + usize::from(n.name_len))
            .unwrap_or("")
    }

    fn kids(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let first = self.nodes.get(id as usize).map_or(NONE, |n| n.first_child);
        std::iter::successors((first != NONE).then_some(first), move |&c| {
            let next = self.nodes[c as usize].next_sibling;
            (next != NONE).then_some(next)
        })
    }

    /// Children are always added after their parent, so one backwards pass
    /// totals every folder.
    fn sum_up(&mut self) {
        for i in (1..self.nodes.len()).rev() {
            let (total, files, newest, parent) = {
                let n = &self.nodes[i];
                (n.total, n.files, n.newest, n.parent as usize)
            };
            let p = &mut self.nodes[parent];
            p.total += total;
            p.files += files;
            p.newest = p.newest.max(newest);
        }
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.get(id as usize)
    }

    /// Full path of a node.
    pub fn path(&self, id: u32) -> PathBuf {
        let mut parts = Vec::new();
        let mut at = id;
        while at != ROOT {
            let Some(n) = self.nodes.get(at as usize) else {
                break;
            };
            parts.push(self.name(at));
            at = n.parent;
        }
        let mut p = self.root.clone();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    /// The node at `path`, if it was scanned.
    pub fn find(&self, path: &Path) -> Option<u32> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let mut at = ROOT;
        for part in rel.components() {
            let name = part.as_os_str().to_string_lossy();
            at = self
                .kids(at)
                .find(|&c| self.name(c).eq_ignore_ascii_case(&name))?;
        }
        Some(at)
    }

    /// Children of a folder, largest first, without deleted ones.
    pub fn children(&self, id: u32) -> Vec<u32> {
        let mut c: Vec<u32> = self
            .kids(id)
            .filter(|&c| self.nodes[c as usize].flags & REMOVED == 0)
            .collect();
        c.sort_by(|&a, &b| {
            self.nodes[b as usize]
                .total
                .cmp(&self.nodes[a as usize].total)
        });
        c
    }

    /// Files using at least `min` bytes, largest first, at most `n`.
    pub fn largest_files(&self, n: usize, min: u64) -> Vec<u32> {
        let mut f: Vec<u32> = (1..self.nodes.len() as u32)
            .filter(|&i| {
                let x = &self.nodes[i as usize];
                !x.is_dir && x.alloc >= min && x.flags & REMOVED == 0
            })
            .collect();
        f.sort_by(|&a, &b| {
            self.nodes[b as usize]
                .alloc
                .cmp(&self.nodes[a as usize].alloc)
        });
        f.truncate(n);
        f
    }

    /// Every file node that still exists.
    pub fn files(&self) -> impl Iterator<Item = u32> + '_ {
        (1..self.nodes.len() as u32).filter(|&i| {
            let x = &self.nodes[i as usize];
            !x.is_dir && x.flags & REMOVED == 0
        })
    }

    /// Space by kind of file, largest first: (group, bytes, files).
    pub fn types(&self) -> Vec<(&'static str, u64, u64)> {
        let mut groups: Vec<(&'static str, u64, u64)> = Vec::new();
        for i in self.files() {
            let n = &self.nodes[i as usize];
            let g = type_group(self.name(i));
            match groups.iter_mut().find(|x| x.0 == g) {
                Some(x) => {
                    x.1 += n.alloc;
                    x.2 += 1;
                }
                None => groups.push((g, n.alloc, 1)),
            }
        }
        groups.sort_by_key(|g| std::cmp::Reverse(g.1));
        groups
    }

    /// Marks a deleted node and takes its space off every folder above it.
    pub fn remove(&mut self, id: u32) {
        if id == ROOT || id as usize >= self.nodes.len() {
            return;
        }
        let (total, files): (u64, u32) = {
            let n = &mut self.nodes[id as usize];
            if n.flags & REMOVED != 0 {
                return;
            }
            n.flags |= REMOVED;
            (n.total, n.files)
        };
        let mut at = self.nodes[id as usize].parent;
        loop {
            let p = &mut self.nodes[at as usize];
            p.total = p.total.saturating_sub(total);
            p.files = p.files.saturating_sub(files);
            if at == ROOT {
                break;
            }
            at = p.parent;
        }
    }
}

/// A plain-language group from the file extension.
pub fn type_group(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "m4v" | "ts" | "flv" => "Videos",
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "raw" | "cr2" | "nef"
        | "psd" | "tif" | "tiff" => "Pictures",
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" => "Music",
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" => "Archives",
        "exe" | "msi" | "msix" | "appx" | "iso" | "img" | "cab" => "Programs and installers",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "odt" => {
            "Documents"
        }
        "pak" | "vpk" | "ucas" | "utoc" | "bundle" | "assets" | "bsa" | "ba2" | "forge" | "rpf"
        | "wad" | "bk2" => "Game data",
        "dll" | "sys" | "mui" | "dat" | "db" | "log" | "etl" | "tmp" => "System and app data",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{dir, file, FakeFs};

    fn tree(fs: &FakeFs) -> Tree {
        scan(
            fs,
            Path::new(r"C:\"),
            3,
            &AtomicBool::new(false),
            &mut |_| {},
        )
    }

    #[test]
    fn totals_add_up_through_folders() {
        let mut fs = FakeFs::default();
        fs.add(
            r"C:\",
            vec![dir("Games"), dir("Users"), file("pagefile.sys", 8000, 1)],
        )
        .add(r"C:\Games", vec![dir("Forza"), file("readme.txt", 10, 5)])
        .add(
            r"C:\Games\Forza",
            vec![file("data.pak", 5000, 9), file("game.exe", 300, 2)],
        )
        .add(r"C:\Users", vec![]);
        let t = tree(&fs);
        assert_eq!(t.node(ROOT).unwrap().total, 13_310);
        assert_eq!(t.node(ROOT).unwrap().files, 4);
        assert_eq!(t.node(ROOT).unwrap().newest, 9);
        let games = t.find(Path::new(r"C:\Games")).unwrap();
        assert_eq!(t.node(games).unwrap().total, 5310);
        let order: Vec<String> = t
            .children(ROOT)
            .iter()
            .map(|&c| t.name(c).to_string())
            .collect();
        assert_eq!(order, ["pagefile.sys", "Games", "Users"]);
        let pak = t.find(Path::new(r"C:\Games\Forza\data.pak")).unwrap();
        assert_eq!(t.path(pak), PathBuf::from(r"C:\Games\Forza\data.pak"));
        assert_eq!(t.largest_files(2, 0).len(), 2);
        assert_eq!(t.types()[0].0, "System and app data");
    }

    #[test]
    fn links_placeholders_and_unreadable_folders() {
        let mut fs = FakeFs::default();
        let mut a = file("a.bin", 1000, 0);
        a.file_id = 42;
        let mut b = file("b.bin", 1000, 0);
        b.file_id = 42; // a hard link to the same file
        let mut cloud = file("cloud.mp4", 900, 0);
        cloud.attrs = crate::attr::RECALL_ON_DATA_ACCESS;
        let mut junction = dir("Link");
        junction.attrs |= crate::attr::REPARSE_POINT;
        fs.add(r"C:\", vec![a, b, cloud, junction, dir("Secret")]);
        fs.unreadable.push(PathBuf::from(r"C:\Secret"));
        let t = tree(&fs);
        assert_eq!(t.node(ROOT).unwrap().total, 1000);
        assert_eq!(t.unreadable, 1);
        let secret = t.find(Path::new(r"C:\Secret")).unwrap();
        assert_ne!(t.node(secret).unwrap().flags & UNREADABLE, 0);
        let link = t.find(Path::new(r"C:\Link")).unwrap();
        assert_ne!(t.node(link).unwrap().flags & REPARSE, 0);
    }

    #[test]
    fn removing_takes_space_off_parents() {
        let mut fs = FakeFs::default();
        fs.add(r"C:\", vec![dir("D")])
            .add(r"C:\D", vec![file("x", 70, 0), file("y", 30, 0)]);
        let mut t = tree(&fs);
        let x = t.find(Path::new(r"C:\D\x")).unwrap();
        t.remove(x);
        t.remove(x);
        assert_eq!(t.node(ROOT).unwrap().total, 30);
        let d = t.find(Path::new(r"C:\D")).unwrap();
        assert_eq!(t.children(d).len(), 1);
        assert_eq!(t.node(d).unwrap().files, 1);
    }

    #[test]
    fn cancel_stops_early() {
        let mut fs = FakeFs::default();
        fs.add(r"C:\", vec![dir("A")]).add(r"C:\A", vec![]);
        let t = scan(
            &fs,
            Path::new(r"C:\"),
            2,
            &AtomicBool::new(true),
            &mut |_| {},
        );
        assert!(t.cancelled);
    }
}
