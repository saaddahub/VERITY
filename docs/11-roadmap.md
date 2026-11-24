# 11 — Roadmap and test strategy

Build in order. A milestone is finished only when every acceptance criterion passes and the definition of done holds.

## Definition of done (every milestone)

- `cargo build`, `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check` all pass
- Admin integration tests for the milestone pass in an elevated terminal
- New OS behavior that was marked **(verify)** in a spec has a probe in `tools/probes/` and a note in `docs/notes/`
- Specs updated where reality differed; `docs/notes/decisions.md` has an entry
- `docs/notes/STATUS.md` updated: what works, what is stubbed, known issues
- The human has been walked through each acceptance criterion with evidence

## Test tooling (built in M1, used throughout)

### `tools/framegen` — a synthetic game

A small Direct3D 11 windowed app that presents frames, so the pipeline can be tested deterministically without a real game.

- `--fps <n>` target frame rate via a precise frame limiter; `--uncapped`
- `--cost-ms <x>` simulated per-frame CPU work; `--gpu-load <level>` simulated GPU work
- `--hitch-at <t1,t2,...> --hitch-ms <x>`: planted hitches at known times
- `--hitch-on-event <name>`: hitch whenever a named Windows event object is signaled (lets a harness plant a hitch at the same moment as another process's activity)
- `--duration <s>`
- Writes a ground-truth log: QPC of every planted hitch

### `tools/stressors`

- `stress cpu --threads <n> --ms <x>`
- `stress disk --path <p> --mb <n>`
- `stress mem --gb <n> --hold-s <x>`
- `stress decoy --period-s <n>`: a process that starts and exits on a fixed schedule doing nothing, used as a false-positive control

### Test levels

| Level | Runs where | What |
|---|---|---|
| Unit | everywhere, no privileges | logic against fake OS traits; statistics; parsers; rules |
| Integration (ignored by default) | elevated terminal on Windows | real Win32 calls: apply/revert/compare for each action, trace sessions, PresentMon |
| End-to-end | elevated, with framegen and stressors | whole pipeline against planted ground truth |
| Simulation | everywhere | Lab false-positive and power checks on synthetic runs |
| Manual | the human, real games | a short checklist per milestone |

---

## M0 — Skeleton

Workspace and crate stubs as in `02-architecture.md`. Service installs, starts, stops, uninstalls. IPC works. `tools/check.ps1` runs the full check suite.

**Acceptance**

- `verity service install|start|stop|uninstall` work from an elevated terminal
- `verity system info` (through IPC) prints: Windows build, CPU model, logical processors with performance/efficiency class, GPUs with VRAM and vendor, displays with current and maximum refresh, power source and battery state, active power scheme
- A standard-user CLI can call `system.info`; the pipe rejects remote clients
- Service logs to the data directory with rotation

## M1 — Telemetry v1

Spec: `03-telemetry.md` sections 1–3, 6–8. Build `framegen` and `stressors`.

**Acceptance**

- `verity record --exe framegen.exe` produces a stored session with frames, samples and a summary
- framegen at a 60 FPS cap: reported average FPS within 1% of 60
- 20 planted 50 ms hitches: at least 19 detected, at most 1 false hitch, each within 20 ms of ground truth
- GPU adapter attribution: the summary names the adapter framegen rendered on
- Overhead in Light mode is within budget and reported in the summary
- PresentMon killed mid-capture: capture resumes, the gap is recorded, no false hitch
- `verity sessions export --csv` round-trips frame data
- Manual: record 10 minutes of a real game; the numbers are plausible against another frame-rate tool

## M2 — Doctor v1

Spec: `04-doctor.md`. Implement the rule framework and rules D001–D003, D010–D013, D020, D024–D026, D040, D050–D052. Agent-side reads are served by a headless agent mode in the CLI (`verity agent run`) until the UI exists.

**Acceptance**

- Every implemented rule has fire / near-miss / missing-data unit tests
- With framegen forced onto the integrated GPU (on a hybrid machine), D001 fires with both adapter names in evidence
- With `stress cpu` running during a framegen session, D020 names the stressor with its CPU share
- With the display set to 60 Hz on a higher-refresh panel, D003 fires
- With framegen configured GPU-heavy, D050 fires; CPU-heavy, D051; capped, D052
- `verity doctor --json` output matches a documented schema

## M3 — Actions and journal

Spec: `05-actions.md`, `09-safety.md` section 1. Implement the framework, journal protocol, crash recovery, and: `power.session_overlay`, `sched.background_ecoqos`, `sched.background_cpusets`, `gpu.preference`, `display.max_refresh`, `gamemode.enable`, `startup.disable_item`.

**Acceptance**

- Each action: an integration test applies, verifies, reverts, and compares state with the snapshot
- Each action: revert-after-drift leaves the user's value and marks `superseded`
- `verity actions plan` prints the exact change and makes none
- Kill the service between `pending` and `applied`, and between `applied` and revert; on restart crash recovery restores state and writes a report
- Exclusion list: a process playing audio, the foreground process and a session-0 process are untouched (tested with fakes and one real integration test)
- A standard-user client cannot apply a machine-wide action
- A restore point is created (or the user is told why not) before the first persistent change

## M4 — Session Mode

Spec: `06-session-mode.md`.

**Acceptance**

- Library scan finds installed Steam and Epic games on the test machine with correct main executables (spot-check five)
- Starting framegen (registered as a game) moves Idle → Arming → Active; exit moves to Idle with all session entries reverted
- A process that exits within 10 seconds never triggers a profile
- A background process started mid-session receives the throttling treatment and is restored at the end
- Panic hotkey reverts within 1 second and disables Session Mode
- `taskkill /f` on the service mid-session, then restart: everything reverted
- Suspend/resume mid-session: session ended and reverted before suspend
- Manual: play a real game for 15 minutes with Session Mode on; audio, voice chat and capture software behave normally

## M5 — Frame Story

Spec: `07-frame-story.md`, `03-telemetry.md` sections 4–5. Add kernel tracing, event-log watchers, attribution, HTML export, and Doctor rules D030–D034.

**Acceptance**

- Kernel trace session starts, stops and is cleaned up after a service crash
- DPC/ISR events are attributed to driver file names
- Planted cause: a harness signals framegen's hitch event at the same moment it starts `stress cpu` 20 times → Frame Story labels the stressor a likely cause with lift ≥ 3
- Control: `stress decoy` on a fixed period with independent random hitches is not reported
- Hitches with no events are classified "in-game" using the frame's CPU/GPU busy split
- Standard-mode overhead within budget
- `verity story export --html` produces a self-contained file with no paths or user name
- Manual: a real session's top cause is plausible on inspection of the timeline

## M6 — UI

Spec: `10-ui.md`. Tauri app, tray, agent duties move from the CLI into the app.

**Acceptance**

- All screens except Lab are functional against the real service
- Timeline stays smooth (60 FPS interaction) on a 3-hour session
- First-run flow completes and records the Session Mode consent
- Closing the window leaves the tray agent running; panic works from tray and hotkey
- Usable at 1366×768; keyboard-navigable; passes a contrast check
- TypeScript API types are generated from Rust

## M7 — The Lab

Spec: `08-lab.md`.

**Acceptance**

- Statistics unit tests and the simulation tests pass (false-positive rate ≤ 5%, power ≥ 80% at 3T)
- A/A experiment with framegen does not return improves or hurts
- Experiment where arm B gives framegen a 10% lower frame cost returns Improves
- An interrupted experiment resumes after a service restart
- Invalid runs are detected (pull the charger mid-run) and repeated
- "Keep for this game" writes `proven_by` into the profile; a non-improving verdict stops Doctor from re-suggesting
- Manual: run one real rigorous experiment on a real game end to end

## M8 — Depth

- Remaining Doctor rules and catalog actions (`power.session_scheme`, `services.session_pause`, `memory.standby_purge`, `sched.game_priority`, `sched.game_priority_ifeo`, `dx.global_settings`)
- Strict passive mode with automatic anti-cheat detection
- Cross-session pooling and "verified fixed"
- Long-form Lab protocol
- Optional: native ETW frame capture replacing the PresentMon child process, validated frame-for-frame against PresentMon on the same session
- Optional: live monitor window

**Acceptance:** each item has its own tests in the style above; a handle-audit test proves no handle to the game is opened in strict passive mode.

## M9 — Release

- Installer (service registration, ACLs, startup entry, license notices) and uninstaller with revert-all
- Code signing
- User guide: what Verity does, what it never does, how to undo everything
- The full checklist in `09-safety.md` section 8 passes

**Acceptance:** clean install → first run → session → uninstall on a fresh Windows 11 and a fresh Windows 10 22H2 machine or VM leaves no trace beyond the optional data folder.

---

## Added milestones (owner request, 2026-11-23)

Four milestones requested by the owner. Their numbers say what they are, not
when they run. Order, because of what each depends on:

- **M12 Storage** and **M13 Malware Scan** change no system settings, so they need
  neither M2 nor M3. They are built next, alongside the co-developer's M2/M3,
  as an owner-approved exception to "one milestone at a time".
- **M10 Process Rules** and **M11 Network Control** are Actions and need M3's
  framework and journal. They start once M3 is merged.

Each runs under the safety amendments listed in its spec, once the owner has
approved them.

## M10 — Process Rules

Spec: `12-process-rules.md`. Process Lasso-style per-app CPU rules.

**Acceptance**

- `topology.get` on the dev laptop names 6 cores / 12 threads, no efficiency classes and one L3 domain; unit tests with fake topologies produce the hybrid and X3D presets
- A rule (CPU set, priority, efficiency, memory priority) on framegen applies when framegen starts, shows in `processes.live`, and is reverted when the rule is disabled: the integration test compares every value with the snapshot
- High and Realtime priority are rejected by the rule schema
- Revert after drift: another tool changes framegen's affinity; disabling the rule leaves it and marks `superseded`
- Kill the service while a rule is applied; on restart crash recovery restores the prior values
- A rule on a strict-passive game is shown as paused and opens no handle to the game (handle audit)
- Restraint: `stress cpu` in the background while framegen runs gets Below normal and Efficiency Mode within 3 s and is restored after; the foreground app and an audio-playing process are never restrained
- "Prove it" on a framegen CPU set rule returns a Lab verdict
- Manual: the owner builds a rule for a real app on the Processes screen and undoes it

## M11 — Network Control

Spec: `13-network.md`. Driver-free per-app network view, priorities and limits.

**Acceptance**

- Probe: Microsoft-Windows-Kernel-Network payloads decoded; per-app totals in `net.live` within 5% of the bytes a test tool sent and received
- `net.app_upload_limit` at 1 Mbit/s on a test uploader holds its measured upload within 10% of the limit; revert removes exactly Verity's policy and leaves a user-made policy alone
- `net.app_dscp`: a packet capture shows the DSCP value on the app's outbound packets (with `net.qos_nla` applied if the probe shows it is needed)
- `net.app_block` blocks the app's connections; revert removes only Verity's rule
- `net.do_background_limit` is applied at Session Mode start and reverted at the end
- The network test measures gateway latency idle and under load and contacts no other host (firewall log)
- No address, port or host name appears in the database or logs (grep test)
- Manual: the owner sets a real app to Low and to Blocked on the Network screen and undoes both

## M12 — Storage

Spec: `14-storage.md`. What fills the drives, safe clean-up, drive health.

**Acceptance**

- Scan of the dev laptop's system drive finishes under 60 s (release build); the screen shows what it counted plus "not visible to your account", which together are the drive's used space (the account cannot measure what it cannot open, so no 2% comparison is possible without admin rights)
- Hard links counted once, junctions not followed, online-only OneDrive files shown as zero local space (unit tests on fake trees, one real check)
- Duplicates are confirmed by content hash; two files with the same name and size but different content are not reported
- Deleting ticked items moves them to the Recycle Bin; restoring them brings them back byte for byte
- Protected locations (Windows, Program Files, ProgramData, other profiles, System-attribute files, Verity data) are never offered and are refused if requested (unit tests)
- The service never receives a file path; no file name reaches Verity's database or logs (grep test)
- `storage.health` reports health, wear (or "not reported") and temperature for the laptop's drive; `storage.optimize` runs Optimize Drives and is refused during Session Mode
- Manual: the owner frees space with the clean-up suggestions and restores one file from the Recycle Bin

## M13 — Malware Scan

Spec: `15-malware-scan.md`. Defender-powered status and scans.

**Acceptance**

- `security.status` matches Windows Security on the dev laptop (protecting product, real-time protection, definition age, last scans)
- Quick scan from the app completes and reports Defender's result; the EICAR test file in a custom scan is detected and shown with Defender's action
- `scan.start` is refused while Session Mode is active; the service accepts no path
- With another antivirus registered (or simulated through fakes), Verity names it and offers no Defender scan
- No Defender setting changes across a full test run (snapshot of `Get-MpPreference` before and after)
- Detection file paths are never stored (grep test)
- Manual: the owner runs a quick scan and opens Protection history from Verity

---

## Risks to watch

| Risk | Mitigation |
|---|---|
| PresentMon output format changes between versions | Pin the version, parse by header name, verify hash |
| Kernel trace session conflicts with other tools | Private system-logger session; clear error and degraded capture if unavailable |
| Undocumented power overlay API changes | Capability check and fallback to documented scheme switching |
| Background throttling breaks an app the user needs | Conservative exclusion list, per-user exclusions, panic path, drift-safe revert |
| Benchmark noise makes the Lab frustrating | Calibration, block-count estimate up front, Quick mode, honest "inconclusive" |
| Anti-cheat reaction | Passive design, strict passive mode, clear user messaging |
| Laptop-vendor quirks | Capability report; guide-only for anything vendor-specific |
| Users expect NetLimiter-style download limits | The Network screen says plainly what Windows can and cannot do without a driver |
| A process rule hurts the app it targets | Every rule is reversible, drift-safe and labelled "not proven" until the Lab gives a verdict |
| Deleting the wrong file | Recycle Bin only, protected-location refusal list, confirmation listing every item |
| Defender unavailable (another antivirus active) | Detect through Security Center; hand off to the active product |
