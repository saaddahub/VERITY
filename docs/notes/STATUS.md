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
- Not recorded whether the service (session 0) sees the displays; the CLI uses
  its own view either way and prints a note when the two differ.
- The hybrid-CPU path (performance/efficiency split) is unit-tested only; the dev
  laptop's CPU is not hybrid.
- "Pipe rejects remote clients" rests on `PIPE_REJECT_REMOTE_CLIENTS` being set;
  there is no test from a second machine.
- Log rotation is configured, not observed across a day boundary.
- `%ProgramData%\Verity` is left behind after uninstall (by design until M9 asks).
