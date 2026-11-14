# Decisions

Changes from the spec and why. Newest first.

## 2026-11-14 — M0

**Pipe uses byte mode, not message mode** (`02-architecture.md` said message mode).
The 4-byte length prefix already delimits messages. Message read mode makes partial
reads fail with `ERROR_MORE_DATA`, which adds handling for no benefit.

**Pipe DACL grants interactive users `0x0012008B`, not generic write.**
`FILE_GENERIC_WRITE` includes `FILE_APPEND_DATA`, which on a named pipe is
`FILE_CREATE_PIPE_INSTANCE`: a standard user could create server instances and
impersonate the service. Granted instead: `FILE_GENERIC_READ | FILE_WRITE_DATA`.
The first instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE` so a squatter
that grabbed the name first makes the service fail loudly instead of sharing it.

**Displays come from the CLI, not the service.** M0 asked `system.info` (served by
the service) to list displays, but the service runs in session 0 and per
`02-architecture.md` cannot see the desktop. The service still reports what it sees;
the CLI replaces that with its own view and prints a note when they differ. That
note is the probe: record on this machine whether the service sees the displays.

**The service binary is copied to `%ProgramFiles%\Verity` before registration.**
Registering it from `target\debug` would run a user-writable exe as SYSTEM.

**`verity-sys` depends on `verity-core`.** Core defines the OS traits and types;
sys implements them, so the dependency points from sys to core.

**`tracing-appender`** added for daily log rotation with a retention limit.

**Pipe clients open with `GENERIC_READ | FILE_WRITE_DATA`, not `GENERIC_WRITE`.**
Found by the round-trip test: with the strict DACL above, a `GENERIC_WRITE` open
(tokio's default) is denied, because it also requests `FILE_CREATE_PIPE_INSTANCE`.
`verity_sys::pipe::open_client` opens with the exact rights and identification-level
impersonation. Any future client (the Tauri app) must open the pipe the same way.

**Pipe DACL also grants the owner (`OW`) full control.** The owner is whoever
created the pipe: SYSTEM in production. It lets a non-admin test run a server
with the production DACL; it grants nothing to other users.

**GPU list is de-duplicated by hardware ID.** Probe result on the dev laptop
(Windows 11 25H2 build 26200, i5-11400H, RTX 3050 + Intel UHD): DXGI listed
"Intel(R) UHD Graphics" twice with different LUIDs. The second entry is the
Parsec Virtual Display Adapter, which DXGI reports with the IDs of the GPU that
renders for it. Entries with the same vendor/device/subsystem/revision are
collapsed. M1 GPU attribution and Doctor D001 must expect such virtual adapters.

**`Display` has no monitor name.** `EnumDisplayDevicesW` for the monitor returned
nothing on the dev laptop, and M0 does not need the name. Dropped.
