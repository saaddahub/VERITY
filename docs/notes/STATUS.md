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
- framegen has no `--gpu-load` or `--hitch-on-event` yet
