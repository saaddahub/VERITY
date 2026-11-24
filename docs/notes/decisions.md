# Decisions

Changes from the spec and why. Newest first.

## 2026-11-24 — M12 Storage, from the probes

Dev laptop: Windows 11 25H2 build 26200, Micron 2400 NVMe SSD.

- **Scan speed and memory** (`tools/probes` `storage`, release): 1.6 million
  entries on C: in 14 to 40 s with 12 threads, 171 s single-threaded; about
  155 MB peak. The tree keeps names in one shared string and children as
  sibling links to stay that small.
- **Coverage:** the user's account counts 306 to 323 GB of about 373 GB used;
  the rest is in places it cannot open. The "within 2%" acceptance criterion
  could not be tested without admin rights, so it was rewritten: the screen
  shows the remainder as "Not visible to your account".
- **Reliability counters** are reached only through each physical disk's
  association (`ASSOCIATORS OF {disk} WHERE ResultClass =
  MSFT_StorageReliabilityCounter`); a plain `SELECT` returns no rows even
  elevated. The laptop's SSD reports temperature (60 °C) but 0 for wear and
  power-on hours.
- **Optimize Drives:** `MSFT_Volume.Optimize` with no arguments fails with
  `WBEM_E_INVALID_PARAMETER`; it needs an operation (`ReTrim` or `Defrag`) and
  the namespace-relative `__RELPATH`. Verity picks the operation from the drive
  type (volume, partition `DiskNumber`, physical disk `MediaType`); ReTrim on
  C: took 2.6 s.
- **Deleting** goes through `IFileOperation` with Recycle Bin and
  `FOF_WANTNUKEWARNING`, so Windows' own "delete permanently?" prompt is the
  second confirmation for items the Recycle Bin cannot take. Verified on the
  laptop: a test file landed in the Recycle Bin; Windows, another profile,
  Downloads itself and `pagefile.sys` were refused before any shell call.
- Dependency added: `sha2` in `verity-storage`, to confirm duplicates by
  content (already used by `verity-telemetry`).

## 2026-11-23 — Four added milestones (owner request)

The owner asked for Process Lasso-style CPU rules, NetLimiter-style network
control, a storage cleaner and a malware scan. Researched and specified as
M10–M13 (`12-process-rules.md` to `15-malware-scan.md`, roadmap section
"Added milestones").

- **No kernel driver for the network limiter.** NetLimiter's limits and
  priorities come from its own kernel driver; `09-safety.md` §2 forbids one.
  M11 uses Windows' Policy-based QoS (upload limits, DSCP tags), the Windows
  Firewall and Delivery Optimization limits. Download limits for arbitrary apps
  are not possible without a driver, and the screen says so.
- **No home-made antivirus.** M13 drives Microsoft Defender through its WMI
  interface and shows its results; Verity never changes Defender settings.
- **Storage scans and deletes in the app, not the service**, so a standard user
  never sees file names through SYSTEM, and the service never writes to a client
  path. Deletion goes to the Recycle Bin.
- **Order:** M12 and M13 change no system settings and are built next, beside
  the co-developer's M2/M3 (exception to "one milestone at a time", by the
  owner's request). M10 and M11 need M3's action framework and wait for it.
- **Approved by the owner the same day** and written into `09-safety.md`:
  Network uses Windows tools only (no driver); deletion goes to the Recycle Bin,
  with permanent deletion for items it cannot take after a second confirmation;
  CPU rules may open non-game processes, games get only CPU-set steering and
  never in strict passive mode, no High/Realtime priority; gateway-only ICMP
  during a user-started network test; the HKLM writes listed in
  `13-network.md`. Build order: M12 and M13 now, M10 and M11 after M3.

## 2026-11-17 — M4 Session Mode and M5 Frame Story

All probes ran on the dev laptop: Windows 11 25H2 build 26200, i5-11400H,
RTX 3050 + Intel UHD.

**Raw ETW in `verity-sys`, not ferrisetw.** ferrisetw always converts event
times to system time (no `PROCESS_TRACE_MODE_RAW_TIMESTAMP`), but a DPC's
duration is its event time minus its `InitialTime` field, which is a raw QPC
value, and frames from PresentMon are QPC too. Decoding by TDH per event is
also too slow at DPC rates (over 10 000 a second in the probe). Verity decodes
the few kernel layouts it uses by hand, bounds-checked, against payloads
captured by `tools/probes/src/bin/etw.rs`. ferrisetw is not a dependency.

**Process starts come from Microsoft-Windows-Kernel-Process, not the kernel
`PROCESS` flag.** The kernel class carries a 15-character image name; the
provider carries the full NT path and the creation time, which is the
process identity with the pid. ProcessStart is version 4 here: pid, sequence
number, creation time, ..., a mandatory-label SID of variable length, then the
image name; the decoder finds the name by its `\` prefix so older versions
(creation time at offset 4) also decode.

**`VerityKernel` runs only during Standard captures.** Nothing needs it between
sessions now that process starts come from the provider. In-place flag updates
were verified anyway (`update_flags -> Ok`, DPC events appeared only after it).

**DPC/ISR attribution uses the image rundown, not `EnumDeviceDrivers`.** The
rundown at enable time delivered every kernel module (pid 0, base, size, path).
Header process ids are -1 for PerfInfo, DiskIo and PageFault events, so disk
I/O and hard faults are attributed through their thread id and the `THREAD`
rundown.

**Per-process CPU comes from the `Process V2` counter set** (instances
`name:pid`). Wildcard counters pick up processes started after the query
opened on the next collection (probe `story_sampler`), so no re-expansion.

**Arming waits up to 120 s for the first frame**, then gives up. The spec said
10 s alive with frames, else Idle; games often compile shaders for longer than
10 s before their first frame. A process that exits within 10 s still never
applies a profile (tested).

**A game's session ends 15 s after it exits**, because a matching process in
that window continues the same session (the spec's relaunch rule). Relaunch
segments are joined with a capture gap between them.

**Session actions are a trait with a no-op implementation until M3 lands.**
`apply`, `on_new_process` (the background watcher), `pause`/`resume` (focus
lost for 60 s) and `revert_all` are called at the spec's moments. The M4
criteria about throttling background processes and reverting them wait for M3.

**The library does not read Steam's launch config** (binary `appinfo.vdf`). The
main exe is the largest one that is not an installer, crash handler, helper or
redistributable; an Unreal `*-Shipping.exe` wins over its bootstrapper. Steam
tool app ids (Steamworks redistributables, runtimes, SteamVR, Proton) are
skipped, and an app must have `StateFlags` bit 4 (fully installed). Verified on
three Steam games. Epic, GOG and Xbox scans are unit-tested only: none are
installed on the dev laptop.

**Notices go through the app.** The service runs in session 0 and cannot show
anything; the app polls `session.status` every 2 s and shows session start/end
notices and the "Is X a game?" question. Only processes seen starting after the
service started can be asked about, because their path is known without
opening a handle.

**Panic hotkey and tray live in the app** (`RegisterHotKey` in
`verity-sys::agent`, tray from tauri's `tray-icon` feature). Closing the window
hides it to the tray so both keep working. The app also reports the foreground
pid to the service (`agent.foreground`), never a window title.

**The Task Scheduler log is off on the dev laptop.** Verity reports "scheduled
task visibility unavailable" as a caveat and does not enable it. Windows Update
ids 44/41 (download) and 43/19/20 (install) are from documentation; only 26, 41
and 31 were seen on this machine.

**Cross-session pooling and "verified fixed"** (Frame Story steps 6 and the end
of 7) are M8 items in the roadmap and are not built.

**Dependencies:** none added to the product crates. tauri gains its
`tray-icon` feature. The test tool `stressors` gains the `windows` crate for
`stress hitch`, which signals framegen's named event.

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

## 2026-11-14 — Scope: games and everyday use

**Verity covers everyday PC use, not only games** (decided by the project owner).
`docs/01-vision.md` gains pillar 6 with what is measured when no game runs. No
safety rule changes. Roadmap fit, so no milestone is derailed:
- right after M1: `verity record --desktop` (frame pacing of `dwm.exe`)
- M2: Doctor rules are written for any PC, plus rules for app hangs and boot time
- M4: Session Mode profiles can be triggered by any app the user picks
These are added to each milestone's acceptance criteria when that milestone starts.
