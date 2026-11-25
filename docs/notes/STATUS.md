# Status

## M0 — Skeleton: complete (2026-11-14)

Verified on the dev laptop (Windows 11 25H2 build 26200, i5-11400H, RTX 3050 + Intel UHD).

**Works**
- `tools/check.ps1` passes build, test, clippy, fmt
- `verity service install|start|stop|uninstall` from an elevated terminal
  (`apps/verity-cli/tests/service_lifecycle.rs`, run elevated: 1 passed)
- `verity system info` through IPC, as admin and as a standard user
  (`runas /trustlevel:0x20000`)
- Service logs to `%ProgramData%\Verity\logs`, rotated daily, 7 files kept
- IPC over a real named pipe with the production DACL
  (`crates/verity-ipc/tests/pipe_roundtrip.rs`)
- Probe: `cargo run -p verity-probes --bin sysinfo`

**Stubbed**
- `verity-telemetry`, `-doctor`, `-actions`, `-session`, `-story`, `-lab`, `-store`
- `apps/verity-ui` does not exist until M6

**Known issues / open questions**
- Verified 2026-11-16: the service (session 0) sees 0 displays on the dev
  laptop, so the CLI and app read displays from the user session, as designed.
- The hybrid-CPU path (performance/efficiency split) is unit-tested only; the dev
  laptop's CPU is not hybrid.
- "Pipe rejects remote clients" rests on `PIPE_REJECT_REMOTE_CLIENTS` being set;
  there is no test from a second machine.
- Log rotation observed 2026-11-16: `service.2026-11-16.log` was created beside
  `service.2026-11-14.log`. Deleting files past the 7-file limit is not yet observed.
- `%ProgramData%\Verity` is left behind after uninstall (by design until M9 asks).

## M1 — Telemetry v1: complete (2026-11-16)

Verified on the dev laptop by `apps/verity-cli/tests/record_e2e.rs` (elevated).

**Works**
- `verity record --exe framegen.exe` stores a session with frames, counter
  samples and a summary
- 60 FPS cap reads 60.00 FPS average
- 20 planted 50 ms hitches: 20 found, 0 false, each inside its long frame
- GPU attribution: summary names "Intel(R) UHD Graphics", the adapter framegen
  rendered on
- Overhead reported in the summary: 0.14% of all cores on the 36 s run, budget 0.30%
- PresentMon killed mid-capture: restarted, one `capture_gap` event stored,
  no hitch reported around it
- `verity sessions list | show | export --csv`; the CSV round-trips bit for bit
- `tools/framegen`, `tools/stressors`, probes `sysinfo` and `counters`

**Open**
- Manual criterion passed 2026-11-16: Forza Horizon 5, session 7, 8.5 min
  (the game was closed before the 10 minutes; the capture ended with it, no gaps).
  Verity: 68.0 FPS average, 1% low 40.1, 76 hitch groups, worst 212 ms, rendered on
  the RTX 3050, overhead 0.16%. The owner's in-game counter showed about 68 average
  with lows around 40.
- Overhead on short captures runs close to the budget (0.24% on a 14 s run):
  start-up cost is spread over little time.
- Only `--mode light` exists; standard and deep need kernel tracing (M5).

**Moved to M2**
- Bottleneck classification, and the counters and hardware reads the Doctor
  rules consume (see `decisions.md`)

**Known shortcuts** (each marked `shortcut:` in code)
- `process_start_filetime` stored as 0
- primary swap chain chosen over the whole capture
- GPU Engine counter instances fixed when the capture starts
- database calls run inline on the async runtime
- framegen has no `--gpu-load` yet (`--hitch-on-event` added in M5)

## M3 — Actions and journal: complete (2026-11-26)

Built here (see `docs/notes/decisions.md`). Crates: `verity-actions` (catalog,
journal protocol, exclusion list, session runner), `verity-core::actions`
(OS traits and journal types), `verity-sys::actions` / `undocumented` /
`restore`, `verity-store::journal`, `verity-ipc::agent`.

**Acceptance, all verified on the dev laptop**
- Each of the 7 actions: a real integration test applies, verifies, reverts
  and compares with the snapshot, then repeats with drift and gets
  `superseded` (`cargo test -p verity-actions --test real -- --ignored
  --test-threads=1`: 10 passed). `sched.background_cpusets` is "not
  applicable" on this non-hybrid CPU, as designed; `display.max_refresh` was
  run 60 -> 144 Hz and back.
- Unit tests against fakes for apply, revert, revert-after-drift, dry-run and
  failed apply for every action (18 tests).
- `verity actions plan` prints the exact change and makes none (CLI and
  `plan_changes_nothing`).
- Crash between `pending` and `applied`: recovered from the database file
  (`crash_recovery_from_the_database_file`). Crash after `applied`:
  `taskkill /f` on the service, restart, power mode and efficiency mode put
  back and a notice shown (`killed_service_after_apply_is_recovered_at_start`).
- Exclusion list: a process playing sound, the foreground process and every
  session-0 process are never eligible (fakes and `exclusions_on_real_processes`).
- A standard-user CLI is refused `actions.apply power.session_overlay`
  (error -32005); the power mode stayed Balanced.
- Restore point: asked before the first persistent change; Windows had
  already made one that day, so Verity stopped and asked; with `--yes` Game
  Mode was turned on, and `verity actions revert` turned it back off.

**Not done**
- `verity journal list --session` filter, and `journal.changed` notifications.
- Machine-wide startup items (HKLM).
- The power mode is not re-applied when the charger is plugged in mid-session.

## M4 — Session Mode: built, not complete (2026-11-17)

Branch `m4-m5-session-story`. Verified on the dev laptop by
`apps/verity-cli/tests/session_e2e.rs` (elevated, `tools/m4m5-admin.ps1`): 5 passed.

**Works**
- Library scan: Steam (verified: Forza Horizon 5, RV There Yet? via its
  `Ride-Win64-Shipping.exe`, BOMBANANA! Demo; the Steamworks redistributables
  app and an incomplete demo are skipped), Epic, GOG, Xbox (unit-tested only),
  manual add. Probe: `cargo run -p verity-probes --bin games` (50 ms).
- Game detection from Microsoft-Windows-Kernel-Process (full path, creation
  time); Idle → Arming → Active → Idle, tested with framegen as a game
- A process that exits within 10 s never becomes a session (tested, 7 s run)
- Panic (`session.panic`, Ctrl+Alt+Shift+V in the app, tray menu): answered in
  1.8 ms, session ended, Session Mode off (tested)
- `taskkill /f` on the service mid-session, restart: no kernel trace left
  running, state idle (tested)
- Suspend and logoff end the session before acknowledging (code path; not run)
- Relaunch within 15 s continues the session; focus lost for 60 s pauses the
  throttling hooks; second game recorded and ignored (unit-tested)
- Profiles (default and per game), `verity session|games|profile`, Games screen,
  session start/end notices, "Is X a game?" question

**Done with M3 (2026-11-26)**
- Session actions are real: power mode, efficiency mode for background apps
  (and efficiency cores on hybrid CPUs), applied when the game becomes active
  and put back at exit, pause and panic. Verified by `actions_e2e`
  `session_treats_background_apps_and_puts_them_back`: a background app got
  efficiency mode at session start, one started mid-session too, the game was
  untouched, all put back at exit. `taskkill /f` on the service: everything
  put back at restart.

**Not done**
- Library spot-check of five games: only three are installed; no Epic games
- Manual: suspend/resume mid-session, and 15 minutes of a real game with audio,
  voice chat and capture software

## M5 — Frame Story: built, not complete (2026-11-17)

**Works**
- Private system-logger trace `VerityKernel` during Standard captures
  (DPC, ISR, hard faults, disk I/O, threads, image loads); stopped at service
  start after a crash (tested). Probe: `cargo run -p verity-probes --bin etw`
  (elevated).
- DPC/ISR attributed to driver files from the kernel module rundown
  (unit-tested on probed payloads)
- Event-log watchers: Defender scans, Windows Update; Task Scheduler reported
  as unavailable because its log is off on this machine
- 4 Hz sampler: background CPU (`Process V2`), other apps on the GPU, CPU
  performance limit, charger changes
- Planted cause (20 stressor starts, each planting a hitch): `stress.exe`
  program start labelled likely, 20 of 20 hitches, lift 4.7; its CPU use also
  likely, 8 of 20 (4 Hz sampling misses short bursts) (tested)
- Decoy on a 7 s period with unrelated hitches: not reported (tested)
- Hitches with no cause classified from the frame: framegen's planted spins
  read as "inside the game, CPU side" (tested)
- Standard overhead 0.29% of all cores, budget 1% (tested)
- `verity story show|export --html`, Frame Story panel and event lanes in the
  app; the report contains file names only, no user name or folder (tested)

**Not done**
- Doctor rules D030–D034: they need M2's rule framework (co-developer)
- Cross-session pooling and "verified fixed": M8 in the roadmap
- Deep mode (`CSWITCH`, ready-thread contention)
- Manual: a real session's top cause looks plausible on its timeline

**Known shortcuts** (each marked `shortcut:` in code)
- Steam launch config not read; largest eligible exe is the main exe
- no session actions until M3

## M12 — Storage: built, not complete (2026-11-24)

Verified on the dev laptop in the real app and by probes (`tools/probes`
`storage`, `diskhealth`, `optimize`).

**Works**
- Storage screen: drive space and drive health on the inspection sheet;
  junk cleanup with one button (Windows Disk Cleanup handlers and browser
  caches, sizes measured on open); Recycle Bin, installers and old downloads to the Recycle Bin; folder browser with sizes; largest
  files; file types; duplicates confirmed by content (SHA-256)
- Scan in the app under the user's account: C: (1.3 to 1.6 million entries) in
  14 to 40 s release, 62 s debug build; the remainder shown as "Not visible to
  your account"
- Deleting: a test file went to the Recycle Bin and is restorable; Windows,
  another profile, Downloads itself and `pagefile.sys` refused with reasons
  (real app); protected-location rules unit-tested
- `storage.health` (service): Micron 2400 SSD, Healthy, 0% worn (reported as
  0), 60 °C; `storage.optimize`: ReTrim on C: from the app's button
- Unit tests: tree totals, hard links, placeholders, junctions, unreadable
  folders, removal, duplicates by content, suggestions, protected locations,
  UI helpers (sizes, nested selections, marks)

**Not done**
- Restore from the Recycle Bin byte for byte: not checked by a test
- Duplicates on the real drive: only unit-tested (reading every candidate file
  takes long)
- Optimize refused during Session Mode: code path only, not run
- Grep test that no file name reaches the database or logs: by inspection only
  (the service never receives a path; the app does not log)
- Manual: the owner frees space with the suggestions and restores one file

**Known shortcuts** (each marked `shortcut:` in code)
- none in M12; the session list paging limit from the review fixes stands
