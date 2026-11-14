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
