//! Verity desktop app: the window side. Every privileged action goes through the
//! service over the same named pipe the CLI uses.

// A GUI app: never open a console window, in debug builds too.
#![windows_subsystem = "windows"]

mod changes;
mod storage;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::Manager;
use verity_core::metrics::{write_csv, Frame, CSV_HEADER};
use verity_core::telemetry::{parse_gpu_engine, CounterApi, ProcessApi};
use verity_core::SystemInfo;
use verity_ipc::{Client, ClientError, Role, PIPE_NAME};

type Res<T> = Result<T, String>;

async fn call(method: &str, params: Value) -> Res<Value> {
    let mut client = Client::connect(PIPE_NAME, Role::Ui)
        .await
        .map_err(|e| e.to_string())?;
    client.call(method, params).await.map_err(|e| e.to_string())
}

/// `verity.exe` ships beside this app, in both the dev target folder and an install.
fn cli_path() -> Res<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("verity.exe");
    if exe.is_file() {
        Ok(exe)
    } else {
        Err(format!(
            "{} is missing; build the workspace first",
            exe.display()
        ))
    }
}

#[tauri::command]
async fn service_status() -> Value {
    match Client::connect(PIPE_NAME, Role::Ui).await {
        Ok(_) => json!({ "state": "running" }),
        Err(ClientError::ServiceNotRunning) => {
            let installed = verity_sys::paths::install_dir()
                .join("verity-service.exe")
                .is_file();
            json!({ "state": if installed { "stopped" } else { "missing" } })
        }
        Err(e) => json!({ "state": "error", "message": e.to_string() }),
    }
}

/// Installs (if needed) and starts the service through one UAC prompt.
#[tauri::command]
async fn service_setup(install: bool) -> Res<()> {
    let exe = cli_path()?;
    let args = if install {
        "service install --start"
    } else {
        "service start"
    };
    let code =
        tauri::async_runtime::spawn_blocking(move || verity_sys::elevate::run_elevated(&exe, args))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| {
                // 0x800704C7 = ERROR_CANCELLED: the user said no to the admin prompt.
                if e.code == 0x8007_04C7 {
                    "The admin prompt was declined, so nothing changed.".to_string()
                } else {
                    e.to_string()
                }
            })?;
    if code == 0 {
        Ok(())
    } else {
        Err(format!("Setup did not finish (exit code {code})."))
    }
}

#[tauri::command]
async fn system_info() -> Res<SystemInfo> {
    let mut info: SystemInfo =
        serde_json::from_value(call("system.info", json!({})).await?).map_err(|e| e.to_string())?;
    // The service sits in session 0; displays are read from this user session.
    info.displays = verity_sys::displays();
    Ok(info)
}

/// Apps whose GPU engines are busy right now, busiest first: the Record picker.
#[tauri::command]
async fn gpu_apps() -> Res<Value> {
    tauri::async_runtime::spawn_blocking(|| -> Res<Value> {
        let mut counters = verity_sys::pdh::PdhCounters::new().map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(1000));
        let sample = counters.sample().map_err(|e| e.to_string())?;
        let mut busy: HashMap<u32, f32> = HashMap::new();
        for (instance, pct) in &sample.gpu_engines {
            if let Some((pid, _)) = parse_gpu_engine(instance) {
                *busy.entry(pid).or_default() += pct;
            }
        }
        let names: HashMap<u32, String> = verity_sys::process::WinProcesses
            .processes()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|p| (p.pid, p.name))
            .collect();
        let own = std::process::id();
        let mut apps: Vec<(u32, String, f32)> = busy
            .into_iter()
            .filter(|(pid, pct)| *pid != own && *pct >= 0.5)
            .filter_map(|(pid, pct)| names.get(&pid).map(|n| (pid, n.clone(), pct)))
            .filter(|(_, name, _)| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "dwm.exe" | "system" | "csrss.exe"
                )
            })
            .collect();
        apps.sort_by(|a, b| b.2.total_cmp(&a.2));
        apps.truncate(12);
        Ok(json!(apps
            .into_iter()
            .map(|(pid, name, gpu)| json!({ "pid": pid, "name": name, "gpu_pct": gpu }))
            .collect::<Vec<_>>()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn record(pid: u32, duration_s: Option<f64>) -> Res<Value> {
    call(
        "record.run",
        json!({ "pid": pid, "duration_s": duration_s }),
    )
    .await
}

#[tauri::command]
async fn sessions_list() -> Res<Value> {
    call("sessions.list", json!({})).await
}

#[tauri::command]
async fn session_get(id: i64) -> Res<Value> {
    call("sessions.get", json!({ "id": id })).await
}

async fn all_frames(id: i64, chunks: u32) -> Res<Vec<Frame>> {
    let mut frames = Vec::new();
    for seq in 0..chunks {
        let part: Vec<Frame> =
            serde_json::from_value(call("sessions.frames", json!({ "id": id, "seq": seq })).await?)
                .map_err(|e| e.to_string())?;
        frames.extend(part);
    }
    Ok(frames)
}

/// Frame times for the trace, as two parallel arrays (QPC ticks, milliseconds).
#[tauri::command]
async fn session_trace(id: i64, chunks: u32) -> Res<Value> {
    let frames = all_frames(id, chunks).await?;
    let qpc: Vec<u64> = frames.iter().map(|f| f.qpc).collect();
    let ms: Vec<Option<f32>> = frames
        .iter()
        .map(|f| f.frame_time_ms.is_finite().then_some(f.frame_time_ms))
        .collect();
    Ok(json!({ "qpc": qpc, "ms": ms }))
}

/// Writes the session's frames to the Downloads folder and returns the path.
#[tauri::command]
async fn export_csv(app: tauri::AppHandle, id: i64, chunks: u32) -> Res<String> {
    let frames = all_frames(id, chunks).await?;
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    let path = dir.join(format!("verity-session-{id}.csv"));
    let mut out = format!("{CSV_HEADER}\n").into_bytes();
    write_csv(&mut out, &frames)
        .and_then(|()| std::fs::write(&path, &out))
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Why the panic hotkey could not be registered, if it could not.
static HOTKEY_ERROR: OnceLock<String> = OnceLock::new();

/// A Session Mode status from the service, plus what this app cannot do
/// (shown with the service's own capability notes).
async fn status_call(method: &str, params: Value) -> Res<Value> {
    let mut v = call(method, params).await?;
    if let (Some(e), Some(caps)) = (HOTKEY_ERROR.get(), v["capabilities"].as_array_mut()) {
        caps.push(json!(format!(
            "The panic hotkey Ctrl+Alt+Shift+V is unavailable ({e}). Use Undo now or the tray menu."
        )));
    }
    Ok(v)
}

#[tauri::command]
async fn session_status(since: u64) -> Res<Value> {
    status_call("session.status", json!({ "since": since })).await
}

#[tauri::command]
async fn session_enable(on: bool) -> Res<Value> {
    status_call("session.enable", json!({ "on": on })).await
}

#[tauri::command]
async fn session_stop() -> Res<Value> {
    status_call("session.stop", json!({})).await
}

#[tauri::command]
async fn session_panic() -> Res<Value> {
    status_call("session.panic", json!({})).await
}

#[tauri::command]
async fn games_list() -> Res<Value> {
    call("games.list", json!({})).await
}

#[tauri::command]
async fn games_scan() -> Res<Value> {
    call("games.scan", json!({})).await
}

#[tauri::command]
async fn games_add(exe_path: String) -> Res<Value> {
    let path = exe_path.trim().trim_matches('"').to_string();
    call("games.add", json!({ "exe_path": path })).await
}

#[tauri::command]
async fn games_remove(id: i64) -> Res<Value> {
    call("games.remove", json!({ "id": id })).await
}

#[tauri::command]
async fn games_answer(exe: String, answer: String) -> Res<Value> {
    call("games.answer", json!({ "exe": exe, "answer": answer })).await
}

#[tauri::command]
async fn story_get(id: i64) -> Res<Value> {
    call("story.get", json!({ "id": id })).await
}

/// Writes the Frame Story report to the Downloads folder and returns the path.
#[tauri::command]
async fn story_export(app: tauri::AppHandle, id: i64) -> Res<String> {
    let v = call("story.export", json!({ "id": id })).await?;
    let html = v["html"].as_str().ok_or("no report returned")?;
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    let path = dir.join(format!("verity-frame-story-{id}.html"));
    std::fs::write(&path, html).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Agent duties (docs/06-session-mode.md): report the foreground process to the
/// service, so Session Mode knows when the game is in front. Pid only, never a
/// window title.
fn report_foreground() {
    std::thread::spawn(|| {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return,
        };
        let (mut last, mut last_audio) = (None, None);
        let mut tick = 0u64;
        loop {
            tick = tick.wrapping_add(1);
            std::thread::sleep(Duration::from_millis(500));
            let now = verity_sys::agent::foreground_pid();
            // Audio sessions every 2 s: background actions skip apps playing sound.
            let audio = if tick.is_multiple_of(4) {
                verity_sys::agent::audio_pids().ok().map(|mut a| {
                    a.sort_unstable();
                    a
                })
            } else {
                last_audio.clone()
            };
            let Some(pid) = now else { continue };
            // A report at least every 5 s tells the service the app is running.
            if now != last || audio != last_audio || tick.is_multiple_of(10) {
                let msg = json!({ "pid": pid, "audio": audio });
                if rt.block_on(call("agent.foreground", msg)).is_ok() {
                    (last, last_audio) = (now, audio);
                }
            }
        }
    });
}

/// Panic from anywhere (hotkey or tray): revert the session and turn Session Mode off.
fn panic_now() {
    std::thread::spawn(|| {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            let _ = rt.block_on(call("session.panic", json!({})));
        }
    });
}

fn tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    let open = MenuItem::with_id(app, "open", "Open Verity", true, None::<&str>)?;
    let panic_label = if HOTKEY_ERROR.get().is_some() {
        "Undo session changes now"
    } else {
        "Undo session changes now (Ctrl+Alt+Shift+V)"
    };
    let panic = MenuItem::with_id(app, "panic", panic_label, true, None::<&str>)?;
    let undo = MenuItem::with_id(
        app,
        "undo_all",
        "Undo all Verity changes",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Verity", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &panic, &undo, &quit])?;
    let mut builder = TrayIconBuilder::with_id("verity")
        .tooltip("Verity")
        .menu(&menu)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show(app),
            "panic" => panic_now(),
            "undo_all" => changes::undo_all_now(),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn show(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    // The hotkey lives as long as the app; dropping it unregisters.
    let hotkey =
        verity_sys::agent::Hotkey::register(verity_sys::agent::PANIC_VK, Box::new(panic_now));
    if let Err(e) = &hotkey {
        // Usually another app owns the shortcut; the app says so on its Games screen.
        let _ = HOTKEY_ERROR.set(e.to_string());
    }
    report_foreground();
    tauri::Builder::default()
        .manage(storage::Storage::default())
        .setup(|app| {
            tray(app)?;
            Ok(())
        })
        // Closing the window keeps Verity in the tray, so the panic hotkey and
        // tray command still work. Quit from the tray menu.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            changes::changes_catalog,
            changes::changes_list,
            changes::changes_startup_items,
            changes::changes_plan,
            changes::changes_apply,
            changes::changes_revert,
            changes::changes_undo_all,
            session_status,
            session_enable,
            session_stop,
            session_panic,
            games_list,
            games_scan,
            games_add,
            games_remove,
            games_answer,
            story_get,
            story_export,
            service_status,
            service_setup,
            system_info,
            gpu_apps,
            record,
            sessions_list,
            session_get,
            session_trace,
            export_csv,
            storage::storage_drives,
            storage::storage_scan,
            storage::storage_cancel,
            storage::storage_children,
            storage::storage_largest,
            storage::storage_types,
            storage::storage_duplicates,
            storage::storage_suggestions,
            storage::storage_delete,
            storage::storage_empty_recycle_bin,
            storage::storage_junk_catalog,
            storage::storage_junk_measure,
            storage::storage_junk_clean,
            storage::storage_health,
            storage::storage_optimize
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // No console in a GUI app; the message box is the only place to say it.
            let _ = verity_sys::agent::error_box("Verity", &format!("Verity could not start: {e}"));
            std::process::exit(1);
        });
}
