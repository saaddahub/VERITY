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

## 2026-11-14 — M1 (in progress)

**Test tools under `tools/` hold their own Win32 calls.** `CLAUDE.md` puts all
`unsafe` in `verity-sys`; that rule is for the product. `tools/framegen` is a
standalone Direct3D test fixture, so its window and swap-chain code lives with
it, each `unsafe` block still carrying a `// SAFETY:` comment.

**framegen ships without `--gpu-load` and `--hitch-on-event` for now.** Nothing
in M1 uses them; they are added when M2 (rule D050) and M5 (planted causes) do.

**PresentMon pinned to 2.6.0 and probed** (elevated capture of framegen, dev
laptop). Flag names matched the spec's guesses. Two things the spec did not
say: redirected stdout is UTF-16 LE with a BOM, and capturing needs admin
rights. The 2.6.0 column list is now in `docs/03-telemetry.md`; the parser maps
columns by name and fails on a missing one. The binary is not committed
(`tools/presentmon/README.md` has the URL and SHA-256); its licence is.

**A hitch is reported at the start of the long frame.** In the probe each
planted 50 ms hitch appeared as a ~66.7 ms frame whose CPU start was 16.4 ms
(one frame) before framegen's logged time. Tests therefore check that the
planted time lies inside the long frame, with the roadmap's 20 ms as slack,
rather than comparing start times, which would fail below 50 FPS.

**Average FPS is frame count over the sum of frame times**, so warm-up and
capture gaps drop out without separate bookkeeping.

**Bottleneck classification (§6) is deferred to M2**, where rules D050–D052
test it and the refresh rate it needs is available.

**1% and 0.1% lows use frame time, not displayed time** (spec §6 said displayed
time when available). In the first end-to-end run framegen's window was hidden
behind other windows: 1,690 of 2,023 frames were never displayed and the rest
stayed up for up to 6.8 s, so a steady 60 FPS run reported a 0.3 FPS low. Frame
time was correct throughout (p99 17.6 ms). `percentile_source` is gone.

**The capture is stopped by a second PresentMon run** with
`--terminate_existing_session`, PresentMon's own way to end a session cleanly.
The same call at service start clears a `VerityPM` session orphaned by a crash,
so no raw ETW control code is needed yet.

**`record.run` blocks until the capture ends** and returns the summary; there is
no separate start/stop pair until Session Mode (M4) needs one. One capture at a
time: there is one `VerityPM` session.

**`process_start_filetime` is stored as 0 in M1.** Reading a creation time needs
a handle to the game, which `docs/09-safety.md` forbids in strict passive mode.
M4 gets it handle-free from kernel process events. Until then a session is
identified by pid for a capture the user started by hand.

**Frames are fetched one chunk per call** (`sessions.frames {id, seq}`), because a
whole long session does not fit the 1 MiB message limit. The CLI writes the CSV
itself, so the service never writes to a client-supplied path.

**The primary swap chain is the one with the most frames over the whole
capture**, not the last 5 s as the spec says. Marked `shortcut:` in
`capture.rs`.

**M1 samples three counters at 1 Hz**: total CPU, available memory, and the
game's GPU engine use (which gives the adapter). The rest of spec §2 and §3
(per-core and per-process CPU, throttling, disk, network, battery rate, RAM
modules, storage type) moves to M2, where the Doctor rules that read them are
built and tested.

Dependencies added in M1 and why: `rusqlite` (bundled SQLite) and `zstd`, both
named in `CLAUDE.md`; `sha2` to check PresentMon's hash before running it as
SYSTEM (`docs/09-safety.md` §6).
