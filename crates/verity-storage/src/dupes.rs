//! Duplicate files, confirmed by content: same size, then the same hash of
//! the first MiB, then the same hash of the whole file. Never by name alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::tree::{Tree, LINK_DUP, ONLINE_ONLY};
use crate::FsApi;

const HEAD: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupGroup {
    /// Size of each copy.
    pub size: u64,
    /// Node ids of the copies, newest first.
    pub nodes: Vec<u32>,
}

impl DupGroup {
    /// Space freed by keeping one copy.
    pub fn wasted(&self) -> u64 {
        self.size * (self.nodes.len() as u64).saturating_sub(1)
    }
}

/// Groups of identical files of at least `min_size` bytes, most wasted space
/// first. `on_progress(hashed, to_hash)` reports the files read so far.
pub fn find(
    tree: &Tree,
    fs: &dyn FsApi,
    min_size: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Vec<DupGroup> {
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for id in tree.files() {
        let Some(n) = tree.node(id) else { continue };
        if n.size >= min_size.max(1) && n.flags & (LINK_DUP | ONLINE_ONLY) == 0 {
            by_size.entry(n.size).or_default().push(id);
        }
    }
    let candidates: Vec<(u64, Vec<u32>)> =
        by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    let to_hash: u64 = candidates.iter().map(|(_, v)| v.len() as u64).sum();
    let mut hashed = 0u64;

    let mut groups = Vec::new();
    for (size, ids) in candidates {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // Stage 1: the first MiB (the whole file when smaller).
        let mut by_head: HashMap<[u8; 32], Vec<u32>> = HashMap::new();
        for id in ids {
            hashed += 1;
            on_progress(hashed, to_hash);
            if let Ok(h) = fs.hash(&tree.path(id), Some(HEAD)) {
                by_head.entry(h).or_default().push(id);
            }
        }
        for same_head in by_head.into_values().filter(|v| v.len() > 1) {
            if size <= HEAD {
                groups.push(group(tree, size, same_head));
                continue;
            }
            // Stage 2: the whole file.
            let mut by_full: HashMap<[u8; 32], Vec<u32>> = HashMap::new();
            for id in same_head {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(h) = fs.hash(&tree.path(id), None) {
                    by_full.entry(h).or_default().push(id);
                }
            }
            groups.extend(
                by_full
                    .into_values()
                    .filter(|v| v.len() > 1)
                    .map(|v| group(tree, size, v)),
            );
        }
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.wasted()));
    groups
}

fn group(tree: &Tree, size: u64, mut nodes: Vec<u32>) -> DupGroup {
    nodes.sort_by_key(|&id| std::cmp::Reverse(tree.node(id).map_or(0, |n| n.newest)));
    DupGroup { size, nodes }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::fake::{dir, file, FakeFs};
    use crate::scan;

    #[test]
    fn same_content_only() {
        let mut fs = FakeFs::default();
        let big = 3 << 20;
        fs.add(r"C:\", vec![dir("A"), dir("B")])
            .add(
                r"C:\A",
                vec![file("movie.mp4", big, 5), file("note.txt", 10, 1)],
            )
            .add(
                r"C:\B",
                vec![
                    file("movie copy.mp4", big, 9),
                    file("other.mp4", big, 3),
                    file("note.txt", 10, 1),
                ],
            );
        let same = vec![1u8; big as usize];
        let mut tail_differs = same.clone();
        *tail_differs.last_mut().unwrap() = 2; // same first MiB, different end
        fs.contents
            .insert(PathBuf::from(r"C:\A\movie.mp4"), same.clone());
        fs.contents
            .insert(PathBuf::from(r"C:\B\movie copy.mp4"), same);
        fs.contents
            .insert(PathBuf::from(r"C:\B\other.mp4"), tail_differs);
        // Same name and size, different content.
        fs.contents
            .insert(PathBuf::from(r"C:\A\note.txt"), b"aaaaaaaaaa".to_vec());
        fs.contents
            .insert(PathBuf::from(r"C:\B\note.txt"), b"bbbbbbbbbb".to_vec());

        let t = scan(
            &fs,
            Path::new(r"C:\"),
            2,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        let groups = find(&t, &fs, 1, &AtomicBool::new(false), &mut |_, _| {});
        assert_eq!(groups.len(), 1);
        let names: Vec<PathBuf> = groups[0].nodes.iter().map(|&n| t.path(n)).collect();
        assert_eq!(
            names,
            [
                PathBuf::from(r"C:\B\movie copy.mp4"),
                PathBuf::from(r"C:\A\movie.mp4")
            ]
        );
        assert_eq!(groups[0].wasted(), big);
    }
}
