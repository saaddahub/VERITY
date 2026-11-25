//! The action catalog (docs/05-actions.md). Only these run; a client can name
//! an id and parameters, never a mechanism.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use verity_core::actions::{
    ActionSys, Executor, ProcKey, RegValue, Scope, StartupPlace, Throttle, EXECUTION_SPEED,
};

use crate::{params, snapshot_of, Action, ActionError, ActionInfo, Risk};

/// Session actions in the order a session applies them.
pub const SESSION_ORDER: [&str; 3] = [
    "power.session_overlay",
    "sched.background_ecoqos",
    "sched.background_cpusets",
];

pub fn all() -> [&'static dyn Action; 7] {
    [
        &PowerOverlay,
        &EcoQos,
        &CpuSets,
        &GpuPreference,
        &MaxRefresh,
        &GameMode,
        &StartupDisable,
    ]
}

pub fn find(id: &str) -> Option<&'static dyn Action> {
    all().into_iter().find(|a| a.info().id == id)
}

fn na(why: impl Into<String>) -> ActionError {
    ActionError::NotApplicable(why.into())
}

/* ---------------- power.session_overlay ---------------- */

pub const BEST_PERFORMANCE: &str = "ded574b5-45a0-4f42-8737-46345c09c238";

fn overlay_name(g: &str) -> String {
    match g {
        BEST_PERFORMANCE => "Best performance".into(),
        "00000000-0000-0000-0000-000000000000" => "Balanced".into(),
        "961cc777-2547-4f9d-8174-7d86181b8a7a" => "Best power efficiency".into(),
        other => format!("custom mode {other}"),
    }
}

pub struct PowerOverlay;

static POWER_OVERLAY: ActionInfo = ActionInfo {
    id: "power.session_overlay",
    title: "Best performance power mode while you play",
    scope: Scope::Session,
    executor: Executor::Service,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: true,
    what: "Sets the Windows power mode to Best performance for the session, only while plugged in.",
    why: "Lets the CPU boost sooner and stay boosted.",
    downside: "More heat and fan noise during the session.",
    undo: "Your power mode is put back when the game closes.",
};

#[derive(Serialize, Deserialize)]
struct OverlaySnap {
    overlay: String,
}

fn plugged_in(sys: &dyn ActionSys) -> Result<bool, ActionError> {
    Ok(sys.on_ac()? == Some(true))
}

impl Action for PowerOverlay {
    fn info(&self) -> &'static ActionInfo {
        &POWER_OVERLAY
    }
    fn plan(&self, sys: &dyn ActionSys, _: &Value) -> Result<String, ActionError> {
        if !plugged_in(sys)? {
            return Err(na(
                "on battery: Verity never changes battery power settings",
            ));
        }
        let Some(cur) = sys.power_overlay()? else {
            return Err(na("this version of Windows has no power mode setting"));
        };
        if cur == BEST_PERFORMANCE {
            return Err(na("the power mode is already Best performance"));
        }
        Ok(format!(
            "Power mode (plugged in): {} -> Best performance",
            overlay_name(&cur)
        ))
    }
    fn snapshot(&self, sys: &dyn ActionSys, _: &Value) -> Result<Value, ActionError> {
        let overlay = sys
            .power_overlay()?
            .ok_or_else(|| na("this version of Windows has no power mode setting"))?;
        Ok(json!(OverlaySnap { overlay }))
    }
    fn apply(&self, sys: &dyn ActionSys, _: &Value) -> Result<(), ActionError> {
        Ok(sys.set_power_overlay(BEST_PERFORMANCE)?)
    }
    fn applied(&self, sys: &dyn ActionSys, _: &Value) -> Result<bool, ActionError> {
        // On battery Windows reports the battery mode, which says nothing about
        // the plugged-in one Verity changed.
        if !plugged_in(sys)? {
            return Err(deferred());
        }
        Ok(sys.power_overlay()?.as_deref() == Some(BEST_PERFORMANCE))
    }
    fn revert(&self, sys: &dyn ActionSys, _: &Value, snap: &Value) -> Result<(), ActionError> {
        if !plugged_in(sys)? {
            return Err(deferred());
        }
        let s: OverlaySnap = snapshot_of(snap)?;
        Ok(sys.set_power_overlay(&s.overlay)?)
    }
}

fn deferred() -> ActionError {
    ActionError::Deferred(
        "the power mode is put back once the PC is plugged in, so the battery setting is never touched"
            .into(),
    )
}

/* ---------------- process-targeted: shared ---------------- */

/// A process, by identity. A client may send only the pid; `prepare` fills
/// in the rest from Windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcParams {
    pub pid: u32,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub name: String,
}

impl ProcParams {
    fn key(&self) -> ProcKey {
        ProcKey {
            pid: self.pid,
            created: self.created,
        }
    }
}

/// Fills in a process's identity from Windows when only its pid is given. A
/// full identity is kept as is: every later call opens the process by pid and
/// creation time and fails with `Gone` if they do not match.
fn prepare_proc(sys: &dyn ActionSys, v: Value) -> Result<Value, ActionError> {
    let mut p: ProcParams = params(&v)?;
    if p.name.len() > 260 {
        return Err(ActionError::Params("name too long".into()));
    }
    if p.created != 0 && !p.name.is_empty() {
        return Ok(json!(p));
    }
    let procs = sys.processes()?;
    let found = procs
        .iter()
        .find(|i| i.key.pid == p.pid && (p.created == 0 || i.key.created == p.created))
        .ok_or_else(|| na(format!("no running process with pid {}", p.pid)))?;
    if found.key.created == 0 {
        return Err(na("Windows does not let Verity see this process"));
    }
    p.created = found.key.created;
    p.name = found.name.clone();
    Ok(json!(p))
}

/* ---------------- sched.background_ecoqos ---------------- */

pub struct EcoQos;

static ECOQOS: ActionInfo = ActionInfo {
    id: "sched.background_ecoqos",
    title: "Efficiency mode for background apps",
    scope: Scope::Session,
    executor: Executor::Service,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: true,
    what: "Marks background apps as efficiency-class (EcoQoS), the same as Task Manager's efficiency mode.",
    why: "Background work gets slower, lower-power cores and clocks, leaving more headroom for the game.",
    downside: "Background apps (downloads, builds, browser tabs) run slower during the session.",
    undo: "Each app is put back when the game closes, unless it changed the setting itself.",
};

fn eco_on(t: Throttle) -> bool {
    t.control & EXECUTION_SPEED != 0 && t.state & EXECUTION_SPEED != 0
}

impl Action for EcoQos {
    fn info(&self) -> &'static ActionInfo {
        &ECOQOS
    }
    fn prepare(&self, sys: &dyn ActionSys, v: Value) -> Result<Value, ActionError> {
        prepare_proc(sys, v)
    }
    fn plan(&self, sys: &dyn ActionSys, v: &Value) -> Result<String, ActionError> {
        let p: ProcParams = params(v)?;
        let t = sys.throttling(p.key())?;
        if eco_on(t) {
            return Err(na("already in efficiency mode"));
        }
        if t.control & EXECUTION_SPEED != 0 {
            return Err(na("the app asked Windows not to slow it down"));
        }
        Ok(format!("{} (pid {}): efficiency mode on", p.name, p.pid))
    }
    fn snapshot(&self, sys: &dyn ActionSys, v: &Value) -> Result<Value, ActionError> {
        let p: ProcParams = params(v)?;
        Ok(json!(sys.throttling(p.key())?))
    }
    fn apply(&self, sys: &dyn ActionSys, v: &Value) -> Result<(), ActionError> {
        let p: ProcParams = params(v)?;
        let t = sys.throttling(p.key())?;
        let on = Throttle {
            control: t.control | EXECUTION_SPEED,
            state: t.state | EXECUTION_SPEED,
        };
        Ok(sys.set_throttling(p.key(), on)?)
    }
    fn applied(&self, sys: &dyn ActionSys, v: &Value) -> Result<bool, ActionError> {
        let p: ProcParams = params(v)?;
        Ok(eco_on(sys.throttling(p.key())?))
    }
    fn revert(&self, sys: &dyn ActionSys, v: &Value, snap: &Value) -> Result<(), ActionError> {
        let p: ProcParams = params(v)?;
        let before: Throttle = snapshot_of(snap)?;
        Ok(sys.set_throttling(p.key(), before)?)
    }
}

/* ---------------- sched.background_cpusets ---------------- */

pub struct CpuSets;

static CPUSETS: ActionInfo = ActionInfo {
    id: "sched.background_cpusets",
    title: "Background apps on efficiency cores",
    scope: Scope::Session,
    executor: Executor::Service,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: true,
    what: "On CPUs with efficiency cores, asks Windows to run background apps on those cores.",
    why: "Keeps the performance cores free for the game.",
    downside: "Background apps run slower. Does nothing on CPUs without efficiency cores.",
    undo: "Each app is put back when the game closes, unless it chose its own cores since.",
};

#[derive(Serialize, Deserialize)]
struct SetsSnap {
    sets: Vec<u32>,
}

fn sorted(mut v: Vec<u32>) -> Vec<u32> {
    v.sort_unstable();
    v
}

impl Action for CpuSets {
    fn info(&self) -> &'static ActionInfo {
        &CPUSETS
    }
    fn prepare(&self, sys: &dyn ActionSys, v: Value) -> Result<Value, ActionError> {
        if sys.efficiency_cpu_sets()?.is_none() {
            return Err(na("this CPU has no efficiency cores"));
        }
        prepare_proc(sys, v)
    }
    fn plan(&self, sys: &dyn ActionSys, v: &Value) -> Result<String, ActionError> {
        let p: ProcParams = params(v)?;
        let eff = sys
            .efficiency_cpu_sets()?
            .ok_or_else(|| na("this CPU has no efficiency cores"))?;
        if !sys.default_cpu_sets(p.key())?.is_empty() {
            return Err(na("the app already chose its own cores"));
        }
        Ok(format!(
            "{} (pid {}): runs on efficiency cores (CPU sets {:?})",
            p.name, p.pid, eff
        ))
    }
    fn snapshot(&self, sys: &dyn ActionSys, v: &Value) -> Result<Value, ActionError> {
        let p: ProcParams = params(v)?;
        Ok(json!(SetsSnap {
            sets: sys.default_cpu_sets(p.key())?
        }))
    }
    fn apply(&self, sys: &dyn ActionSys, v: &Value) -> Result<(), ActionError> {
        let p: ProcParams = params(v)?;
        let eff = sys
            .efficiency_cpu_sets()?
            .ok_or_else(|| na("this CPU has no efficiency cores"))?;
        Ok(sys.set_default_cpu_sets(p.key(), &eff)?)
    }
    fn applied(&self, sys: &dyn ActionSys, v: &Value) -> Result<bool, ActionError> {
        let p: ProcParams = params(v)?;
        let eff = sys.efficiency_cpu_sets()?.unwrap_or_default();
        Ok(sorted(sys.default_cpu_sets(p.key())?) == sorted(eff))
    }
    fn revert(&self, sys: &dyn ActionSys, v: &Value, snap: &Value) -> Result<(), ActionError> {
        let p: ProcParams = params(v)?;
        let s: SetsSnap = snapshot_of(snap)?;
        Ok(sys.set_default_cpu_sets(p.key(), &s.sets)?)
    }
}

/* ---------------- gpu.preference ---------------- */

const GPU_KEY: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";

pub struct GpuPreference;

static GPU_PREFERENCE: ActionInfo = ActionInfo {
    id: "gpu.preference",
    title: "Run a game on the high-performance GPU",
    scope: Scope::Persistent,
    executor: Executor::Agent,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: false,
    what: "Sets the game to the high-performance GPU, the same as Settings > Display > Graphics.",
    why: "On laptops with two GPUs a game can end up on the slower built-in one.",
    downside: "More power use and heat when the game runs on battery.",
    undo: "Verity can put the previous setting back at any time.",
};

#[derive(Serialize, Deserialize)]
struct ExeParams {
    exe: String,
}

#[derive(Serialize, Deserialize)]
struct StrSnap {
    value: Option<String>,
}

/// `C:\...\game.exe`, the form Windows uses for the value name.
fn valid_exe(s: &str) -> Result<(), ActionError> {
    let b = s.as_bytes();
    let drive = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    if !drive || s.len() > 1024 || s.contains('\0') || !s.to_ascii_lowercase().ends_with(".exe") {
        return Err(ActionError::Params(
            "exe must be a full path like C:\\Games\\game.exe".into(),
        ));
    }
    Ok(())
}

/// Sets one `name=value;` token in a DirectX preference string, keeping the others.
fn set_token(current: Option<&str>, name: &str, value: &str) -> String {
    let mut out: Vec<String> = current
        .unwrap_or("")
        .split(';')
        .filter(|t| !t.is_empty() && t.split('=').next() != Some(name))
        .map(str::to_string)
        .collect();
    out.push(format!("{name}={value}"));
    out.join(";") + ";"
}

fn token<'a>(current: Option<&'a str>, name: &str) -> Option<&'a str> {
    current?
        .split(';')
        .find_map(|t| t.strip_prefix(name)?.strip_prefix('='))
}

fn reg_str(sys: &dyn ActionSys, key: &str, name: &str) -> Result<Option<String>, ActionError> {
    Ok(match sys.reg_get(key, name)? {
        Some(RegValue::Str(s)) => Some(s),
        _ => None,
    })
}

impl Action for GpuPreference {
    fn info(&self) -> &'static ActionInfo {
        &GPU_PREFERENCE
    }
    fn prepare(&self, _: &dyn ActionSys, v: Value) -> Result<Value, ActionError> {
        let p: ExeParams = params(&v)?;
        valid_exe(&p.exe)?;
        Ok(json!(p))
    }
    fn plan(&self, sys: &dyn ActionSys, v: &Value) -> Result<String, ActionError> {
        let p: ExeParams = params(v)?;
        let cur = reg_str(sys, GPU_KEY, &p.exe)?;
        if token(cur.as_deref(), "GpuPreference") == Some("2") {
            return Err(na("already set to the high-performance GPU"));
        }
        Ok(format!(
            "HKCU\\{GPU_KEY}: \"{}\" = \"{}\" (was {})",
            p.exe,
            set_token(cur.as_deref(), "GpuPreference", "2"),
            cur.map_or("not set".into(), |c| format!("\"{c}\""))
        ))
    }
    fn snapshot(&self, sys: &dyn ActionSys, v: &Value) -> Result<Value, ActionError> {
        let p: ExeParams = params(v)?;
        Ok(json!(StrSnap {
            value: reg_str(sys, GPU_KEY, &p.exe)?
        }))
    }
    fn apply(&self, sys: &dyn ActionSys, v: &Value) -> Result<(), ActionError> {
        let p: ExeParams = params(v)?;
        let cur = reg_str(sys, GPU_KEY, &p.exe)?;
        let new = set_token(cur.as_deref(), "GpuPreference", "2");
        Ok(sys.reg_set(GPU_KEY, &p.exe, &RegValue::Str(new))?)
    }
    fn applied(&self, sys: &dyn ActionSys, v: &Value) -> Result<bool, ActionError> {
        let p: ExeParams = params(v)?;
        Ok(token(reg_str(sys, GPU_KEY, &p.exe)?.as_deref(), "GpuPreference") == Some("2"))
    }
    fn revert(&self, sys: &dyn ActionSys, v: &Value, snap: &Value) -> Result<(), ActionError> {
        let p: ExeParams = params(v)?;
        let s: StrSnap = snapshot_of(snap)?;
        match s.value {
            Some(old) => sys.reg_set(GPU_KEY, &p.exe, &RegValue::Str(old))?,
            None => sys.reg_delete(GPU_KEY, &p.exe)?,
        }
        Ok(())
    }
}

/* ---------------- display.max_refresh ---------------- */

pub struct MaxRefresh;

static MAX_REFRESH: ActionInfo = ActionInfo {
    id: "display.max_refresh",
    title: "Highest refresh rate for the display",
    scope: Scope::Persistent,
    executor: Executor::Agent,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: false,
    what: "Sets the display to its highest refresh rate at the current resolution.",
    why: "Windows often leaves high-refresh screens at 60 Hz, so games feel less smooth.",
    downside: "Slightly more power use; the screen blinks once while it switches.",
    undo: "Verity can put the previous refresh rate back at any time.",
};

#[derive(Serialize, Deserialize)]
struct DisplayParams {
    #[serde(default)]
    device: String,
    /// The target rate, filled in by `prepare`.
    #[serde(default)]
    hz: u32,
}

#[derive(Serialize, Deserialize)]
struct HzSnap {
    hz: u32,
}

impl Action for MaxRefresh {
    fn info(&self) -> &'static ActionInfo {
        &MAX_REFRESH
    }
    fn prepare(&self, sys: &dyn ActionSys, v: Value) -> Result<Value, ActionError> {
        let mut p: DisplayParams = params(&v)?;
        if p.device.is_empty() {
            p.device = sys.primary_display()?;
        }
        if !p.device.starts_with(r"\\.\DISPLAY") || p.device.len() > 32 {
            return Err(ActionError::Params(
                "device must look like \\\\.\\DISPLAY1".into(),
            ));
        }
        let cur = sys.display_mode(&p.device)?;
        p.hz = sys
            .display_modes(&p.device)?
            .into_iter()
            .filter(|m| m.width == cur.width && m.height == cur.height && m.bits == cur.bits)
            .map(|m| m.hz)
            .max()
            .unwrap_or(cur.hz);
        Ok(json!(p))
    }
    fn plan(&self, sys: &dyn ActionSys, v: &Value) -> Result<String, ActionError> {
        let p: DisplayParams = params(v)?;
        let cur = sys.display_mode(&p.device)?;
        if p.hz <= cur.hz {
            return Err(na(format!(
                "already at its highest refresh rate ({} Hz)",
                cur.hz
            )));
        }
        Ok(format!(
            "{}: {} Hz -> {} Hz at {}x{}",
            p.device, cur.hz, p.hz, cur.width, cur.height
        ))
    }
    fn snapshot(&self, sys: &dyn ActionSys, v: &Value) -> Result<Value, ActionError> {
        let p: DisplayParams = params(v)?;
        Ok(json!(HzSnap {
            hz: sys.display_mode(&p.device)?.hz
        }))
    }
    fn apply(&self, sys: &dyn ActionSys, v: &Value) -> Result<(), ActionError> {
        let p: DisplayParams = params(v)?;
        Ok(sys.set_display_hz(&p.device, p.hz)?)
    }
    fn applied(&self, sys: &dyn ActionSys, v: &Value) -> Result<bool, ActionError> {
        let p: DisplayParams = params(v)?;
        Ok(sys.display_mode(&p.device)?.hz == p.hz)
    }
    fn revert(&self, sys: &dyn ActionSys, v: &Value, snap: &Value) -> Result<(), ActionError> {
        let p: DisplayParams = params(v)?;
        let s: HzSnap = snapshot_of(snap)?;
        Ok(sys.set_display_hz(&p.device, s.hz)?)
    }
}

/* ---------------- gamemode.enable ---------------- */

const GAMEBAR_KEY: &str = r"Software\Microsoft\GameBar";
/// Windows 11 reads `AutoGameModeEnabled`; older builds wrote
/// `AllowAutoGameMode`. Either at 0 turns Game Mode off; absent means on.
const GAME_MODE_VALUES: [&str; 2] = ["AutoGameModeEnabled", "AllowAutoGameMode"];

pub struct GameMode;

static GAME_MODE: ActionInfo = ActionInfo {
    id: "gamemode.enable",
    title: "Turn on Game Mode",
    scope: Scope::Persistent,
    executor: Executor::Agent,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: false,
    what: "Turns on Windows Game Mode, the same as Settings > Gaming > Game Mode.",
    why: "Windows holds back updates and background work while a game runs.",
    downside: "Rarely, a game or overlay behaves worse with it on.",
    undo: "Verity can put the previous setting back at any time.",
};

#[derive(Serialize, Deserialize)]
struct ValuesSnap {
    values: Vec<(String, Option<RegValue>)>,
}

fn game_mode_on(sys: &dyn ActionSys) -> Result<bool, ActionError> {
    for name in GAME_MODE_VALUES {
        if sys.reg_get(GAMEBAR_KEY, name)? == Some(RegValue::Dword(0)) {
            return Ok(false);
        }
    }
    Ok(true)
}

impl Action for GameMode {
    fn info(&self) -> &'static ActionInfo {
        &GAME_MODE
    }
    fn plan(&self, sys: &dyn ActionSys, _: &Value) -> Result<String, ActionError> {
        if game_mode_on(sys)? {
            return Err(na("Game Mode is already on"));
        }
        Ok(format!(
            "HKCU\\{GAMEBAR_KEY}: AutoGameModeEnabled = 1 (and AllowAutoGameMode = 1 if present)"
        ))
    }
    fn snapshot(&self, sys: &dyn ActionSys, _: &Value) -> Result<Value, ActionError> {
        let mut values = Vec::new();
        for name in GAME_MODE_VALUES {
            values.push((name.to_string(), sys.reg_get(GAMEBAR_KEY, name)?));
        }
        Ok(json!(ValuesSnap { values }))
    }
    fn apply(&self, sys: &dyn ActionSys, _: &Value) -> Result<(), ActionError> {
        sys.reg_set(GAMEBAR_KEY, GAME_MODE_VALUES[0], &RegValue::Dword(1))?;
        if sys.reg_get(GAMEBAR_KEY, GAME_MODE_VALUES[1])?.is_some() {
            sys.reg_set(GAMEBAR_KEY, GAME_MODE_VALUES[1], &RegValue::Dword(1))?;
        }
        Ok(())
    }
    fn applied(&self, sys: &dyn ActionSys, _: &Value) -> Result<bool, ActionError> {
        game_mode_on(sys)
    }
    fn revert(&self, sys: &dyn ActionSys, _: &Value, snap: &Value) -> Result<(), ActionError> {
        let s: ValuesSnap = snapshot_of(snap)?;
        for (name, v) in s.values {
            if !GAME_MODE_VALUES.contains(&name.as_str()) {
                continue;
            }
            match v {
                Some(v) => sys.reg_set(GAMEBAR_KEY, &name, &v)?,
                None => sys.reg_delete(GAMEBAR_KEY, &name)?,
            }
        }
        Ok(())
    }
}

/* ---------------- startup.disable_item ---------------- */

const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

pub struct StartupDisable;

static STARTUP_DISABLE: ActionInfo = ActionInfo {
    id: "startup.disable_item",
    title: "Stop a program starting with Windows",
    scope: Scope::Persistent,
    executor: Executor::Agent,
    risk: Risk::Low,
    needs_reboot: false,
    default_enabled: false,
    what:
        "Disables one startup program the same way Task Manager does. The program is not removed.",
    why: "Fewer programs starting with Windows means a faster start and less background work.",
    downside: "The program no longer starts on its own; start it yourself when you need it.",
    undo: "Verity can turn it back on at any time.",
};

#[derive(Serialize, Deserialize)]
struct StartupParams {
    name: String,
    place: StartupPlace,
}

#[derive(Serialize, Deserialize)]
struct BytesSnap {
    value: Option<Vec<u8>>,
}

fn approved_key(place: StartupPlace) -> String {
    let sub = match place {
        StartupPlace::Run => "Run",
        StartupPlace::StartupFolder => "StartupFolder",
    };
    format!(r"{APPROVED_KEY}\{sub}")
}

fn approved(sys: &dyn ActionSys, p: &StartupParams) -> Result<Option<Vec<u8>>, ActionError> {
    Ok(match sys.reg_get(&approved_key(p.place), &p.name)? {
        Some(RegValue::Binary(b)) => Some(b),
        _ => None,
    })
}

/// Task Manager's flag: an odd first byte means disabled.
fn is_disabled(v: Option<&[u8]>) -> bool {
    v.and_then(|b| b.first()).is_some_and(|f| f & 1 == 1)
}

/// FILETIME now, from the Unix clock.
fn filetime_now() -> u64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() / 100) as u64;
    unix + 116_444_736_000_000_000
}

impl Action for StartupDisable {
    fn info(&self) -> &'static ActionInfo {
        &STARTUP_DISABLE
    }
    fn prepare(&self, sys: &dyn ActionSys, v: Value) -> Result<Value, ActionError> {
        let p: StartupParams = params(&v)?;
        if p.name.is_empty() || p.name.len() > 260 || p.name.contains(['\0', '\\']) {
            return Err(ActionError::Params(
                "name must be a startup item's name".into(),
            ));
        }
        if !sys
            .startup_items()?
            .iter()
            .any(|i| i.name == p.name && i.place == p.place)
        {
            return Err(ActionError::Params(format!(
                "no startup item named \"{}\"",
                p.name
            )));
        }
        Ok(json!(p))
    }
    fn plan(&self, sys: &dyn ActionSys, v: &Value) -> Result<String, ActionError> {
        let p: StartupParams = params(v)?;
        if is_disabled(approved(sys, &p)?.as_deref()) {
            return Err(na("already disabled"));
        }
        Ok(format!(
            "HKCU\\{}: \"{}\" = disabled (03 00 00 00 + time); the program and its Run entry stay",
            approved_key(p.place),
            p.name
        ))
    }
    fn snapshot(&self, sys: &dyn ActionSys, v: &Value) -> Result<Value, ActionError> {
        let p: StartupParams = params(v)?;
        Ok(json!(BytesSnap {
            value: approved(sys, &p)?
        }))
    }
    fn apply(&self, sys: &dyn ActionSys, v: &Value) -> Result<(), ActionError> {
        let p: StartupParams = params(v)?;
        let mut flag = vec![3u8, 0, 0, 0];
        flag.extend_from_slice(&filetime_now().to_le_bytes());
        Ok(sys.reg_set(&approved_key(p.place), &p.name, &RegValue::Binary(flag))?)
    }
    fn applied(&self, sys: &dyn ActionSys, v: &Value) -> Result<bool, ActionError> {
        let p: StartupParams = params(v)?;
        Ok(is_disabled(approved(sys, &p)?.as_deref()))
    }
    fn revert(&self, sys: &dyn ActionSys, v: &Value, snap: &Value) -> Result<(), ActionError> {
        let p: StartupParams = params(v)?;
        let s: BytesSnap = snapshot_of(snap)?;
        let key = approved_key(p.place);
        match s.value {
            Some(b) => sys.reg_set(&key, &p.name, &RegValue::Binary(b))?,
            None => sys.reg_delete(&key, &p.name)?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeSys, MemJournal};
    use crate::{Engine, RevertOutcome};
    use verity_core::actions::{DisplayMode, JournalStore, StartupItem};

    /// Apply, verify, revert, compare, then the same with drift.
    fn round_trip(
        sys: &FakeSys,
        id: &str,
        p: Value,
        drift: impl Fn(&FakeSys),
        unchanged: impl Fn(&FakeSys) -> bool,
    ) {
        let j = MemJournal::default();
        let e = Engine { sys, journal: &j };
        let a = find(id).unwrap();
        let prepared = a.prepare(sys, p.clone()).unwrap();
        let plan = a.plan(sys, &prepared).unwrap();
        assert!(unchanged(sys), "plan changed something: {plan}");
        let row = e.apply(a, p.clone(), "t", "").unwrap();
        assert!(a
            .applied(sys, &j.journal_get(row).unwrap().unwrap().row.params)
            .unwrap());
        assert!(matches!(
            a.plan(sys, &prepared),
            Err(ActionError::NotApplicable(_))
        ));
        assert_eq!(
            e.revert(&j.journal_get(row).unwrap().unwrap()),
            RevertOutcome::Reverted
        );
        assert!(
            unchanged(sys),
            "{id} revert did not restore the prior state"
        );

        let row = e.apply(a, p, "t", "").unwrap();
        drift(sys);
        assert_eq!(
            e.revert(&j.journal_get(row).unwrap().unwrap()),
            RevertOutcome::Superseded
        );
    }

    #[test]
    fn power_overlay_round_trip_and_battery_rules() {
        let sys = FakeSys::default();
        sys.set_power(Some(true), Some("00000000-0000-0000-0000-000000000000"));
        round_trip(
            &sys,
            "power.session_overlay",
            json!({}),
            |s| s.set_power(Some(true), Some("961cc777-2547-4f9d-8174-7d86181b8a7a")),
            |s| s.overlay().as_deref() == Some("00000000-0000-0000-0000-000000000000"),
        );

        let a = find("power.session_overlay").unwrap();
        sys.set_power(Some(false), Some("00000000-0000-0000-0000-000000000000"));
        assert!(matches!(
            a.plan(&sys, &json!({})),
            Err(ActionError::NotApplicable(_))
        ));
        sys.set_power(Some(true), None);
        assert!(matches!(
            a.plan(&sys, &json!({})),
            Err(ActionError::NotApplicable(_))
        ));

        // Applied while plugged in, then unplugged: the revert waits.
        sys.set_power(Some(true), Some("00000000-0000-0000-0000-000000000000"));
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        let id = e.apply(a, json!({}), "t", "").unwrap();
        sys.set_power(Some(false), Some(BEST_PERFORMANCE));
        let row = j.journal_get(id).unwrap().unwrap();
        assert!(matches!(e.revert(&row), RevertOutcome::Deferred(_)));
        assert!(j.journal_get(id).unwrap().unwrap().state.open());
        sys.set_power(Some(true), Some(BEST_PERFORMANCE));
        assert_eq!(e.revert(&row), RevertOutcome::Reverted);
        assert_eq!(
            sys.overlay().as_deref(),
            Some("00000000-0000-0000-0000-000000000000")
        );
    }

    #[test]
    fn ecoqos_round_trip_and_respects_the_apps_choice() {
        let sys = FakeSys::default();
        let k = ProcKey { pid: 9, created: 3 };
        sys.add_proc(k, 1, "app.exe");
        round_trip(
            &sys,
            "sched.background_ecoqos",
            json!({"pid": 9}),
            |s| s.set_throttle(k, Throttle::default()),
            |s| s.throttle(k) == Throttle::default(),
        );
        let a = find("sched.background_ecoqos").unwrap();
        sys.set_throttle(
            k,
            Throttle {
                control: EXECUTION_SPEED,
                state: 0,
            },
        );
        let p = a.prepare(&sys, json!({"pid": 9})).unwrap();
        assert!(matches!(
            a.plan(&sys, &p),
            Err(ActionError::NotApplicable(_))
        ));
        sys.deny(9);
        assert!(matches!(
            a.plan(&sys, &p),
            Err(ActionError::Proc(verity_core::actions::ProcError::Denied))
        ));
        assert!(a.prepare(&sys, json!({"pid": 99})).is_err());
    }

    #[test]
    fn cpusets_round_trip_only_on_hybrid_cpus() {
        let sys = FakeSys::default();
        let k = ProcKey { pid: 9, created: 3 };
        sys.add_proc(k, 1, "app.exe");
        let a = find("sched.background_cpusets").unwrap();
        assert!(matches!(
            a.prepare(&sys, json!({"pid": 9})),
            Err(ActionError::NotApplicable(_))
        ));
        sys.set_efficiency(Some(vec![264, 265]));
        round_trip(
            &sys,
            "sched.background_cpusets",
            json!({"pid": 9}),
            |s| s.set_default_cpu_sets(k, &[256]).unwrap(),
            |s| s.sets(k).is_empty(),
        );
    }

    #[test]
    fn gpu_preference_keeps_other_tokens() {
        let sys = FakeSys::default();
        let exe = r"C:\Games\g.exe";
        sys.put_reg(
            GPU_KEY,
            exe,
            RegValue::Str("SwapEffectUpgradeEnable=1;GpuPreference=1;".into()),
        );
        round_trip(
            &sys,
            "gpu.preference",
            json!({"exe": exe}),
            |s| s.put_reg(GPU_KEY, exe, RegValue::Str("GpuPreference=1;".into())),
            |s| {
                s.reg(GPU_KEY, exe)
                    == Some(RegValue::Str(
                        "SwapEffectUpgradeEnable=1;GpuPreference=1;".into(),
                    ))
            },
        );
        assert_eq!(
            set_token(
                Some("SwapEffectUpgradeEnable=1;GpuPreference=1;"),
                "GpuPreference",
                "2"
            ),
            "SwapEffectUpgradeEnable=1;GpuPreference=2;"
        );
        let a = find("gpu.preference").unwrap();
        assert!(a.prepare(&sys, json!({"exe": "g.exe"})).is_err());
        // Not set before: the revert removes the value.
        let fresh = r"C:\Games\new.exe";
        let j = MemJournal::default();
        let e = Engine {
            sys: &sys,
            journal: &j,
        };
        let id = e.apply(a, json!({"exe": fresh}), "t", "").unwrap();
        assert_eq!(
            sys.reg(GPU_KEY, fresh),
            Some(RegValue::Str("GpuPreference=2;".into()))
        );
        assert_eq!(
            e.revert(&j.journal_get(id).unwrap().unwrap()),
            RevertOutcome::Reverted
        );
        assert_eq!(sys.reg(GPU_KEY, fresh), None);
    }

    #[test]
    fn max_refresh_round_trip() {
        let sys = FakeSys::default();
        let m = |hz| DisplayMode {
            width: 1920,
            height: 1080,
            bits: 32,
            hz,
        };
        sys.set_display(
            m(60),
            vec![
                m(60),
                m(144),
                DisplayMode {
                    width: 1280,
                    height: 720,
                    bits: 32,
                    hz: 240,
                },
            ],
        );
        round_trip(
            &sys,
            "display.max_refresh",
            json!({}),
            |s| s.set_display_hz("", 60).unwrap(),
            |s| s.hz() == 60,
        );
        let a = find("display.max_refresh").unwrap();
        let p = a.prepare(&sys, json!({})).unwrap();
        assert_eq!(p["hz"], 144);
        assert!(a.prepare(&sys, json!({"device": "C:\\x"})).is_err());
    }

    #[test]
    fn game_mode_round_trip() {
        let sys = FakeSys::default();
        sys.put_reg(GAMEBAR_KEY, "AutoGameModeEnabled", RegValue::Dword(0));
        sys.put_reg(GAMEBAR_KEY, "AllowAutoGameMode", RegValue::Dword(0));
        round_trip(
            &sys,
            "gamemode.enable",
            json!({}),
            |s| s.put_reg(GAMEBAR_KEY, "AutoGameModeEnabled", RegValue::Dword(0)),
            |s| {
                s.reg(GAMEBAR_KEY, "AutoGameModeEnabled") == Some(RegValue::Dword(0))
                    && s.reg(GAMEBAR_KEY, "AllowAutoGameMode") == Some(RegValue::Dword(0))
            },
        );
    }

    #[test]
    fn startup_disable_round_trip() {
        let sys = FakeSys::default();
        sys.set_startup(vec![StartupItem {
            name: "Steam".into(),
            place: StartupPlace::Run,
            enabled: true,
        }]);
        let key = approved_key(StartupPlace::Run);
        sys.put_reg(
            &key,
            "Steam",
            RegValue::Binary(vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
        round_trip(
            &sys,
            "startup.disable_item",
            json!({"name": "Steam", "place": "run"}),
            |s| s.put_reg(&key, "Steam", RegValue::Binary(vec![2; 12])),
            |s| {
                s.reg(&key, "Steam")
                    == Some(RegValue::Binary(vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
            },
        );
        let a = find("startup.disable_item").unwrap();
        assert!(a
            .prepare(&sys, json!({"name": "Nope", "place": "run"}))
            .is_err());
        assert!(a
            .prepare(&sys, json!({"name": "a\\b", "place": "run"}))
            .is_err());
    }

    #[test]
    fn catalog_ids_are_unique_and_described() {
        let ids: Vec<_> = all().iter().map(|a| a.info().id).collect();
        let mut dedup = ids.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(ids.len(), dedup.len());
        for a in all() {
            let i = a.info();
            assert!(
                !i.what.is_empty() && !i.downside.is_empty() && !i.undo.is_empty(),
                "{}",
                i.id
            );
        }
        for id in SESSION_ORDER {
            assert_eq!(find(id).unwrap().info().scope, Scope::Session);
        }
    }
}
