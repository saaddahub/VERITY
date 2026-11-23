use std::path::{Path, PathBuf};

use crate::library::LibraryFs;

/// Most directory entries one exe search visits, so a game folder with
/// hundreds of thousands of files cannot stall a scan.
const MAX_ENTRIES: usize = 50_000;

/// The real [`LibraryFs`]: read-only `std::fs` calls.
pub struct StdFs;

impl LibraryFs for StdFs {
    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn files(&self, dir: &Path, ext: &str) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(ext))
            })
            .collect()
    }

    fn exes(&self, dir: &Path, depth: usize) -> Vec<(PathBuf, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![(dir.to_path_buf(), 0usize)];
        let mut visited = 0usize;
        while let Some((d, level)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                visited += 1;
                if visited > MAX_ENTRIES {
                    return out;
                }
                // file_type does not follow links, so junction loops are not walked.
                let Ok(t) = e.file_type() else { continue };
                let p = e.path();
                if t.is_dir() && level < depth {
                    stack.push((p, level + 1));
                } else if t.is_file()
                    && p.extension()
                        .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("exe"))
                {
                    if let Ok(m) = e.metadata() {
                        out.push((p, m.len()));
                    }
                }
            }
        }
        out
    }

    fn subdirs(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect()
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
}

/// The launcher locations on this machine.
pub fn real_roots() -> crate::library::Roots {
    let program_data = std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    let gog = match verity_sys::games::gog_games() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("GOG registry unreadable: {e}");
            Vec::new()
        }
    };
    crate::library::Roots {
        steam: verity_sys::games::steam_install_dir(),
        epic_manifests: Some(program_data.join(r"Epic\EpicGamesLauncher\Data\Manifests")),
        gog: gog
            .into_iter()
            .map(|g| crate::library::GogEntry {
                id: g.id,
                name: g.name,
                path: g.path,
                exe: g.exe,
            })
            .collect(),
        volumes: verity_sys::games::fixed_volumes(),
    }
}
