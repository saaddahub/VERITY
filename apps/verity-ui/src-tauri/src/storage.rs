//! The Storage screen's commands (docs/14-storage.md). Scanning and deleting
//! run here, in the app, under the user's own account; the service is only
//! asked for drive health, which contains no file names.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use verity_storage::safety::{self, Protected};
use verity_storage::tree::{REPARSE, SYSTEM};
use verity_storage::{attr, cleanup, dupes, junk, RealFs, Tree, ROOT};
use verity_sys::shellops::{self, Known};

type Res<T> = Result<T, String>;

/// Largest files shown, and the smallest size that counts as large.
const LARGEST: usize = 200;
const LARGE_BYTES: u64 = 100 << 20;
/// Rows shown per folder; the rest are summed into one line.
const MAX_ROWS: usize = 300;
/// Duplicates smaller than this are not worth the reading time.
const DUP_MIN: u64 = 1 << 20;
const MAX_DUP_GROUPS: usize = 100;

#[derive(Default)]
pub struct Storage {
    tree: Mutex<Option<Tree>>,
    cancel: AtomicBool,
    busy: AtomicBool,
}

/// Clears the busy flag however the work ends.
struct Busy<'a>(&'a AtomicBool);

impl<'a> Busy<'a> {
    fn take(flag: &'a AtomicBool) -> Res<Self> {
        if flag.swap(true, Ordering::SeqCst) {
            return Err("Verity is already working on your drives; wait for it to finish.".into());
        }
        Ok(Self(flag))
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn hwnd(window: &WebviewWindow) -> Option<isize> {
    window.hwnd().ok().map(|h| h.0 as isize)
}

/// The locations deletion always refuses (docs/14-storage.md, "Deleting").
fn protected() -> Protected {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut system: Vec<PathBuf> = [
        "SystemRoot",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramData",
    ]
    .into_iter()
    .filter_map(env)
    .collect();
    system.push(verity_sys::paths::data_dir());
    let profile = shellops::known_folder(Known::Profile).or_else(|| env("USERPROFILE"));
    let users = profile
        .as_ref()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let mut keep: Vec<PathBuf> = [
        Known::Profile,
        Known::Desktop,
        Known::Documents,
        Known::Downloads,
        Known::Pictures,
        Known::Videos,
        Known::Music,
    ]
    .into_iter()
    .filter_map(shellops::known_folder)
    .collect();
    keep.push(std::env::temp_dir());
    keep.extend(env("OneDrive"));
    Protected {
        system,
        users,
        profile,
        keep,
    }
}

/// Attribute bits the safety check needs, from what the scan recorded.
fn attrs_of(flags: u8) -> u32 {
    let mut a = 0;
    if flags & SYSTEM != 0 {
        a |= attr::SYSTEM;
    }
    if flags & REPARSE != 0 {
        a |= attr::REPARSE_POINT;
    }
    a
}

fn row(t: &Tree, id: u32, p: &Protected) -> Option<Value> {
    let n = t.node(id)?;
    let path = t.path(id);
    Some(json!({
        "id": id,
        "name": t.name(id),
        "path": path,
        "is_dir": n.is_dir,
        "bytes": n.total,
        "files": n.files,
        "newest": n.newest,
        "unreadable": n.flags & verity_storage::tree::UNREADABLE != 0,
        "online_only": n.flags & verity_storage::tree::ONLINE_ONLY != 0,
        "link": n.flags & REPARSE != 0,
        "deletable": safety::check(&path, Some(attrs_of(n.flags)), p).is_ok(),
    }))
}

fn with_tree<T>(app: &AppHandle, f: impl FnOnce(&Tree) -> Res<T>) -> Res<T> {
    let st = app.state::<Storage>();
    let guard = st
        .tree
        .lock()
        .map_err(|_| "storage state poisoned".to_string())?;
    let t = guard.as_ref().ok_or("Scan a drive first.")?;
    f(t)
}

#[tauri::command]
pub async fn storage_drives() -> Res<Value> {
    let drives = verity_sys::files::drives().map_err(|e| e.to_string())?;
    Ok(json!(drives
        .into_iter()
        .map(|d| json!({ "root": d.root, "total": d.total, "free": d.free }))
        .collect::<Vec<_>>()))
}

/// Scans one fixed drive, sending `storage-progress` events, and keeps the
/// result for the other commands.
#[tauri::command]
pub async fn storage_scan(app: AppHandle, root: String) -> Res<Value> {
    let drive = verity_sys::files::drives()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|d| d.root.to_string_lossy().eq_ignore_ascii_case(&root))
        .ok_or("Pick one of this PC's drives.")?;
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Storage>();
        let _busy = Busy::take(&st.busy)?;
        st.cancel.store(false, Ordering::SeqCst);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let started = std::time::Instant::now();
        let tree = verity_storage::scan(&RealFs, &drive.root, threads, &st.cancel, &mut |p| {
            let _ = app.emit("storage-progress", p);
        });
        let r = tree.node(ROOT).ok_or("the scan produced no result")?;
        let used = drive.total.saturating_sub(drive.free);
        let summary = json!({
            "root": drive.root,
            "counted": r.total,
            "files": r.files,
            "unreadable_folders": tree.unreadable,
            "drive_total": drive.total,
            "drive_used": used,
            "not_visible": used.saturating_sub(r.total),
            "seconds": started.elapsed().as_secs_f64(),
            "cancelled": tree.cancelled,
        });
        *st.tree.lock().map_err(|_| "storage state poisoned")? = Some(tree);
        Ok(summary)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn storage_cancel(app: AppHandle) {
    app.state::<Storage>().cancel.store(true, Ordering::SeqCst);
}

/// One folder: its place in the tree and its children, largest first.
#[tauri::command]
pub async fn storage_children(app: AppHandle, id: u32) -> Res<Value> {
    let p = protected();
    with_tree(&app, |t| {
        let n = t
            .node(id)
            .filter(|n| n.flags & verity_storage::tree::REMOVED == 0)
            .ok_or("That folder is no longer in the scan.")?;
        let mut crumbs = Vec::new();
        let mut at = id;
        loop {
            crumbs.push(json!({ "id": at, "name": t.name(at) }));
            if at == ROOT {
                break;
            }
            at = t.node(at).map_or(ROOT, |x| x.parent);
        }
        crumbs.reverse();
        let kids = t.children(id);
        let rest: u64 = kids
            .iter()
            .skip(MAX_ROWS)
            .filter_map(|&c| t.node(c))
            .map(|c| c.total)
            .sum();
        Ok(json!({
            "id": id,
            "path": t.path(id),
            "bytes": n.total,
            "files": n.files,
            "crumbs": crumbs,
            "items": kids.iter().take(MAX_ROWS).filter_map(|&c| row(t, c, &p)).collect::<Vec<_>>(),
            "more": { "count": kids.len().saturating_sub(MAX_ROWS), "bytes": rest },
        }))
    })
}

#[tauri::command]
pub async fn storage_largest(app: AppHandle) -> Res<Value> {
    let p = protected();
    with_tree(&app, |t| {
        Ok(json!(t
            .largest_files(LARGEST, LARGE_BYTES)
            .into_iter()
            .filter_map(|id| row(t, id, &p))
            .collect::<Vec<_>>()))
    })
}

#[tauri::command]
pub async fn storage_types(app: AppHandle) -> Res<Value> {
    with_tree(&app, |t| {
        Ok(json!(t
            .types()
            .into_iter()
            .map(|(group, bytes, files)| json!({ "group": group, "bytes": bytes, "files": files }))
            .collect::<Vec<_>>()))
    })
}

/// Identical files in the scan, read and compared by content, with
/// `storage-dupes-progress` events.
#[tauri::command]
pub async fn storage_duplicates(app: AppHandle) -> Res<Value> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Storage>();
        let _busy = Busy::take(&st.busy)?;
        st.cancel.store(false, Ordering::SeqCst);
        let p = protected();
        let guard = st.tree.lock().map_err(|_| "storage state poisoned")?;
        let t = guard.as_ref().ok_or("Scan a drive first.")?;
        let groups = dupes::find(t, &RealFs, DUP_MIN, &st.cancel, &mut |done, total| {
            if done % 25 == 0 || done == total {
                let _ = app.emit(
                    "storage-dupes-progress",
                    json!({ "done": done, "total": total }),
                );
            }
        });
        Ok(json!(groups
            .iter()
            .take(MAX_DUP_GROUPS)
            .map(|g| json!({
                "size": g.size,
                "wasted": g.wasted(),
                "items": g.nodes.iter().filter_map(|&id| row(t, id, &p)).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn storage_suggestions() -> Res<Value> {
    tauri::async_runtime::spawn_blocking(|| {
        let places = cleanup::Places {
            downloads: shellops::known_folder(Known::Downloads),
            recycle_bin: shellops::recycle_bin_size().ok(),
        };
        let p = protected();
        let mut out = cleanup::suggest(&RealFs, &places, verity_sys::process::filetime_now());
        // A suggestion never offers a protected item.
        for s in &mut out {
            s.items.retain(|i| safety::check(&i.path, None, &p).is_ok());
        }
        Ok(json!(out))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The junk catalog: what one-click cleanup can clear, and where each runs.
#[tauri::command]
pub fn storage_junk_catalog() -> Value {
    json!(junk::CATALOG)
}

fn browser_dirs() -> Vec<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|l| junk::browser_cache_dirs(&RealFs, Path::new(&l)))
        .unwrap_or_default()
}

/// Runs the app-side categories (the user's own junk) on a fresh thread each,
/// since Disk Cleanup handlers need their own COM apartment.
fn app_side(ids: &[&'static junk::Category], clean: bool) -> Vec<Value> {
    let volume = format!(
        "{}\\",
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    );
    ids.iter()
        .map(|c| {
            let id = c.id;
            let r: Result<u64, String> = match c.how {
                junk::How::BrowserCaches => {
                    let dirs = browser_dirs();
                    Ok(if clean {
                        junk::clear_dirs(&RealFs, &dirs)
                    } else {
                        junk::measure_dirs(&RealFs, &dirs)
                    })
                }
                junk::How::Handler(h) => {
                    let v = volume.clone();
                    std::thread::spawn(move || {
                        if clean {
                            verity_sys::cleanup::purge(h, &v)
                        } else {
                            verity_sys::cleanup::measure(h, &v).map(|m| m.bytes)
                        }
                    })
                    .join()
                    .map_err(|_| "the cleanup handler stopped unexpectedly".to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()))
                }
            };
            match r {
                Ok(bytes) => json!({ "id": id, "bytes": bytes }),
                Err(e) => json!({ "id": id, "error": e }),
            }
        })
        .collect()
}

/// Measures (`clean` false) or clears junk categories: the user's own here,
/// Windows' machine-wide ones in the service. One result per id.
async fn junk_run(app: AppHandle, ids: Vec<String>, clean: bool) -> Res<Value> {
    let mut mine = Vec::new();
    let mut service = Vec::new();
    for id in &ids {
        let c = junk::find(id).ok_or_else(|| format!("{id} is not a cleanup category"))?;
        match c.runs {
            junk::Runs::App => mine.push(c),
            junk::Runs::Service => service.push(c.id),
        }
    }
    let local = tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Storage>();
        // Measuring may overlap other work; deleting may not.
        let _busy = if clean {
            Some(Busy::take(&st.busy)?)
        } else {
            None
        };
        Ok::<_, String>(app_side(&mine, clean))
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut out = local;
    if !service.is_empty() {
        let method = if clean { "junk.clean" } else { "junk.measure" };
        match crate::call(method, json!({ "ids": service })).await {
            Ok(Value::Array(rows)) => out.extend(rows),
            Ok(_) => {}
            Err(e) => out.extend(service.iter().map(|id| json!({ "id": id, "error": e }))),
        }
    }
    Ok(json!(out))
}

#[tauri::command]
pub async fn storage_junk_measure(app: AppHandle, ids: Vec<String>) -> Res<Value> {
    junk_run(app, ids, false).await
}

/// Deletes the chosen junk for good. The screen has already asked.
#[tauri::command]
pub async fn storage_junk_clean(app: AppHandle, ids: Vec<String>) -> Res<Value> {
    junk_run(app, ids, true).await
}

/// Moves the given items to the Recycle Bin after checking each one again.
/// Windows itself asks before deleting anything the Recycle Bin cannot take.
#[tauri::command]
pub async fn storage_delete(
    app: AppHandle,
    window: WebviewWindow,
    paths: Vec<String>,
) -> Res<Value> {
    let owner = hwnd(&window);
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<Storage>();
        let _busy = Busy::take(&st.busy)?;
        let p = protected();
        let mut allowed = Vec::new();
        let mut kept = Vec::new();
        for raw in paths {
            let path = PathBuf::from(&raw);
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                kept.push(json!({ "path": raw, "reason": "Already gone" }));
                continue;
            };
            match safety::check(&path, Some(meta.file_attributes()), &p) {
                Ok(()) => allowed.push(path),
                Err(r) => kept.push(json!({ "path": raw, "reason": r.reason() })),
            }
        }
        if !allowed.is_empty() {
            // The shell needs a single-threaded COM apartment: a fresh thread has none.
            let batch = allowed.clone();
            std::thread::spawn(move || shellops::recycle(&batch, owner))
                .join()
                .map_err(|_| "the delete thread stopped unexpectedly".to_string())?
                .map_err(|e| e.to_string())?;
        }
        let mut deleted = Vec::new();
        let mut freed = 0u64;
        let mut guard = st.tree.lock().map_err(|_| "storage state poisoned")?;
        for path in allowed {
            if std::fs::symlink_metadata(&path).is_ok() {
                kept.push(json!({ "path": path, "reason": "Still there: in use, or the delete was cancelled" }));
                continue;
            }
            if let Some(t) = guard.as_mut() {
                if let Some(id) = t.find(&path) {
                    freed += t.node(id).map_or(0, |n| n.total);
                    t.remove(id);
                }
            }
            deleted.push(path);
        }
        Ok(json!({ "deleted": deleted, "kept": kept, "freed": freed }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Empties every Recycle Bin. The screen has already asked; this cannot be undone.
#[tauri::command]
pub async fn storage_empty_recycle_bin(window: WebviewWindow) -> Res<()> {
    let owner = hwnd(&window);
    std::thread::spawn(move || shellops::empty_recycle_bin(owner))
        .join()
        .map_err(|_| "the Recycle Bin thread stopped unexpectedly".to_string())?
        .map_err(|e| e.to_string())
}

/// Drive health from the service (it needs admin rights; no file names involved).
#[tauri::command]
pub async fn storage_health() -> Res<Value> {
    crate::call("storage.health", json!({})).await
}

#[tauri::command]
pub async fn storage_optimize(drive: String) -> Res<Value> {
    crate::call("storage.optimize", json!({ "drive": drive })).await
}
