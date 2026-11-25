# 05 — Actions

An Action is one reversible change to the system. Nothing in Verity changes system state except through an Action.

Items marked **(verify)** must be confirmed with a probe before being relied on.

## Action interface

```rust
pub trait Action: Send + Sync {
    fn info(&self) -> ActionInfo;
    /// Is this applicable here, and what is the current state?
    fn probe(&self, sys: &dyn Sys) -> Result<Probe>;
    /// Capture everything needed to undo. Must be serializable and self-contained.
    fn snapshot(&self, sys: &dyn Sys, params: &Params) -> Result<Snapshot>;
    /// Describe the exact change without making it.
    fn plan(&self, sys: &dyn Sys, params: &Params) -> Result<Plan>;
    fn apply(&self, sys: &dyn Sys, params: &Params) -> Result<()>;
    fn revert(&self, sys: &dyn Sys, snapshot: &Snapshot) -> Result<RevertOutcome>;
    /// Is the applied state still in effect? (Detects drift caused by the user or other tools.)
    fn verify(&self, sys: &dyn Sys, params: &Params) -> Result<bool>;
}

pub struct ActionInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub scope: Scope,            // Session | Persistent
    pub executor: Executor,      // Service | Agent
    pub risk: Risk,              // Low | Medium | High
    pub needs_reboot: bool,
    pub default_enabled: bool,
    pub lab_testable: bool,
    pub what: &'static str, pub why: &'static str, pub downside: &'static str,
}
```

## The journal protocol

For every apply:

1. `snapshot()` → write a journal row with state `pending`, including params and snapshot. Commit to disk.
2. `apply()`.
3. On success mark `applied`. On failure call `revert()` with the snapshot and mark `reverted` or `revert_failed` with the error.

For every revert:

1. Load the row. If the current state no longer matches what Verity applied (the user or another tool changed it since), **do not overwrite it**: mark `superseded` and report it.
2. Otherwise `revert()` and mark `reverted`.

Session end reverts that session's rows in reverse order of application.

### Crash recovery

At every service start, before anything else: find all rows with scope `session` in state `pending` or `applied`, and revert them in reverse order. Process-targeted snapshots are skipped if the process identity `(pid, creation time)` no longer exists. Log a recovery report and surface it in the UI.

### Agent presence

Background actions also depend on what only the agent can see (the
foreground window and audio sessions). When the agent has not reported for
15 seconds, background actions are skipped for that moment and the reason is
logged; session-wide actions such as the power mode still apply.

### Process-targeted actions

Snapshots store `(pid, creation time, image name)` and the prior value per process. Revert only touches a process whose identity still matches.

## Exclusion list for background actions

Background actions never touch:

- The game process, its children, and its launcher
- Anything in session 0 (services, system processes)
- Anti-cheat processes and services, matched by a maintained name list (EasyAntiCheat, BattlEye, Vanguard, and others)
- Processes the OS refuses to open, including protected processes. Access denied is a normal result, not an error.
- Processes with an active audio session (reported by the agent)
- The foreground process and any process whose window was foreground in the last 2 minutes
- Voice chat, capture and streaming software by default (configurable name list)
- Security software
- Verity itself
- Anything on the user's own exclusion list

## Catalog

### Session scope, executed by the service

| ID | What it does | Mechanism | Risk | Default |
|---|---|---|---|---|
| `power.session_overlay` | Sets the Windows power mode to best performance while on AC | Undocumented `PowerGetActualOverlayScheme(GUID*)` / `PowerSetActiveOverlayScheme(GUID)` in `powrprof.dll`, looked up at run time (`verity-sys::undocumented`). Best performance is `ded574b5-45a0-4f42-8737-46345c09c238`, Balanced `00000000-…`, Best power efficiency `961cc777-2547-4f9d-8174-7d86181b8a7a`. Verified on build 26200: works unelevated and as SYSTEM, changes only `ActiveOverlayAcPowerScheme` while plugged in. Applied only on AC; a revert while on battery waits until AC (it would otherwise write the battery setting). Fallback when the functions are missing: not applicable (no change). | Low | On |
| `power.session_scheme` | Activates a Verity-owned power scheme for the session | `PowerDuplicateScheme` from the user's scheme once; tune processor settings by their powercfg aliases (energy performance preference, boost mode, minimum unparked cores) with `PowerWriteACValueIndex`; `PowerSetActiveScheme`. Resolve setting GUIDs from `powercfg /aliases` and hard-code them only after **(verify)**. AC values only; never change DC values. | Medium | Off; Lab first |
| `sched.background_ecoqos` | Marks eligible background processes as efficiency-class | `SetProcessInformation(ProcessPowerThrottling)` with `PROCESS_POWER_THROTTLING_EXECUTION_SPEED`; prior state read with `GetProcessInformation` (verified). A process that set the control bit with the state off (asked not to be slowed) is left alone. | Low | On |
| `sched.background_cpusets` | On hybrid CPUs, steers eligible background processes to efficiency cores | `GetSystemCpuSetInformation` to find the lowest `EfficiencyClass` set; `SetProcessDefaultCpuSets`. Not applicable on non-hybrid CPUs. Newly started background processes are handled by watching process-start events. | Low | On (hybrid only) |
| `sched.game_priority` | Raises the game's priority class one step | `SetPriorityClass(ABOVE_NORMAL_PRIORITY_CLASS)` with a handle opened for `PROCESS_SET_INFORMATION` only. If access is denied, skip silently. Never `HIGH`, never `REALTIME`. Not applied to games on the anti-cheat caution list (see `09-safety.md`). | Medium | Off; Lab first |
| `services.session_pause` | Stops a small allowlisted set of services for the session and restarts them after | Service Control Manager. Allowlist v1: Windows Search indexer only. Snapshot records prior running state and start type; start type is never changed. | Medium | Off |
| `memory.standby_purge` | Clears the standby cache once, at session start, only when gated | `NtSetSystemInformation` memory-list command **(verify)**; needs `SeProfileSingleProcessPrivilege`. Gate: available memory < 15% and standby cache > 25% of total. One-shot; nothing to revert. | Medium | Off |

### Persistent scope, executed by the agent (per-user settings)

| ID | What it does | Mechanism | Risk | Default |
|---|---|---|---|---|
| `gpu.preference` | Sets a game exe to the high-performance GPU | `HKCU\Software\Microsoft\DirectX\UserGpuPreferences`, value named by the full exe path, data `GpuPreference=2;` | Low | Suggested |
| `display.max_refresh` | Sets the display to its highest refresh rate at the current resolution | `EnumDisplaySettingsExW`, `ChangeDisplaySettingsExW` with test first, then update registry | Low | Suggested |
| `dx.global_settings` | Enables optimizations for windowed games and variable refresh | Same key, value `DirectXUserGlobalSettings` **(verify)** exact token names | Low | Suggested |
| `gamemode.enable` | Turns on Game Mode | `HKCU\Software\Microsoft\GameBar`: `AutoGameModeEnabled` = 1, and `AllowAutoGameMode` = 1 when present (older builds); either at 0 means off, absent means on. Both present on the dev laptop. | Low | Suggested |
| `startup.disable_item` | Disables one startup entry the same way Task Manager does | `HKCU\...\Explorer\StartupApproved\Run` and `\StartupFolder`, 12-byte value: first byte odd = disabled (`03 00 00 00` + FILETIME), even = enabled (`02` + zeros); verified against the entries Task Manager wrote on the dev laptop. Current-user items only in v1 (machine-wide items need an HKLM write). Never deletes the Run entry itself. | Low | Per item, user picks |

### Persistent scope, executed by the service (machine settings, elevated client required)

| ID | What it does | Mechanism | Risk | Default |
|---|---|---|---|---|
| `sched.game_priority_ifeo` | Sets a per-exe default priority without ever opening the game process | `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\<exe>\PerfOptions`, `CpuPriorityClass` **(verify)** value mapping | Medium | Off; Lab first |
| `gpu.hags_toggle` | Toggles hardware-accelerated GPU scheduling | `HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers`, `HwSchMode`; needs reboot | Medium | Off; Lab long-form only; ask the human |

### Experimental (off, hidden behind an "experimental" setting, Lab only)

| ID | Notes |
|---|---|
| `timer.resolution` | On current Windows, a timer-resolution request from another process no longer affects the game unless a machine-wide registry switch is set and the system rebooted **(verify)**. Costs battery. Only expose as a Lab experiment with a clear warning. |
| `sched.game_cpusets` | Restricting the *game* to performance cores. The OS scheduler usually does this well already; can hurt. Lab only. |

### Added with M10 and M11 (approved by the owner, 2026-11-23)

| ID | Spec | Scope |
|---|---|---|
| `rule.apply` (CPU set, affinity, priority up to Above normal, efficiency, memory priority, I/O priority) | `12-process-rules.md` | Session, process-targeted, applied per matching process |
| `sched.restraint` | `12-process-rules.md` | Session, background, off until proven |
| `net.app_upload_limit`, `net.app_dscp` | `13-network.md` | Persistent or session, machine |
| `net.app_block` | `13-network.md` | Persistent, machine |
| `net.do_background_limit` | `13-network.md` | Session, machine |
| `net.qos_nla` | `13-network.md` | Persistent, machine; may need a reboot (ask first) |

M12 Storage and M13 Malware Scan add no actions: deleting user-picked files
runs in the app and goes to the Recycle Bin, and starting a Defender scan
changes no setting.

### Notify-only (Verity detects and guides, never changes)

Windows Update activity, Defender scans, cloud-sync uploads, vendor performance profiles, BIOS memory profile, discrete-only display mode, driver updates, memory integrity.

## Adding an action

An action may be added to the catalog only if all are true:

1. Its mechanism is a documented API or a registry value the OS itself reads, or it is isolated as undocumented with a fallback.
2. `snapshot` and `revert` restore the exact prior state.
3. It has a stated downside.
4. It is not on the blacklist in `09-safety.md`.
5. It has unit tests against fakes for apply, revert, revert-after-drift and dry-run, and an ignored admin integration test that applies and reverts for real and compares state.

## CLI

```
verity actions list
verity actions plan <id> [--param k=v ...]       # dry run
verity actions apply <id> [--param k=v ...]
verity actions revert <journal-id>
verity journal list [--session <id>]
verity journal revert-all
```
