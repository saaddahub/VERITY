# 09 — Safety

These rules are binding. Code that breaks one is a bug even if it improves performance.

## 1. Reversibility guarantees

1. Every change is an Action with a snapshot journaled before the change (`05-actions.md`).
2. Session-scoped changes are reverted at session end, on panic, on service stop, on suspend/shutdown, and by crash recovery at next service start.
3. A revert never overwrites a value the user or another tool changed after Verity applied it. It reports `superseded`.
4. Before the first persistent change on a machine, create a System Restore point (`SRSetRestorePointW`) if System Restore is enabled. If it is disabled or rate-limited, tell the user and proceed only with their confirmation.
5. Uninstall reverts everything still applied.
6. `verity journal revert-all` and the tray's "Undo everything" always exist and always work without the UI window.

## 2. Hands off the game

Verity observes games **passively** and never interacts with a game process beyond what is listed here.

Never:

- Read or write game process memory
- Inject or load any code into a game process; no hooks, no overlays inside the game
- Install, load or bundle a kernel driver (this includes third-party sensor drivers such as WinRing0)
- Enumerate a game's modules or threads by opening the process. Module information comes from kernel image-load events only.
- Suspend or debug a game process
- Modify game files, configuration or launch options
- Simulate input

Allowed:

- Consuming system-wide ETW events (frame presentation, process and image events)
- Reading performance counters
- Opening the game process with `PROCESS_SET_INFORMATION` only, solely for `sched.game_priority`, and only when the rule below permits

### Anti-cheat caution

Maintain a name list of anti-cheat services, drivers and modules (Vanguard, EasyAntiCheat, BattlEye, FACEIT, ESEA and others). A game is in **strict passive mode** when any of these is true:

- an anti-cheat module appears in its image-load events
- an anti-cheat service associated with it is running
- it is on the built-in list of titles known to use kernel anti-cheat
- the user ticked "strict passive" for it

In strict passive mode Verity never opens a handle to the game process at all and does not apply `sched.game_priority`, `sched.game_priority_ifeo` or `sched.game_cpusets`. Capture, Doctor, Frame Story and background-process actions continue to work.

The UI states plainly: Verity is designed to stay out of the way of anti-cheat by being passive, but no third-party tool can guarantee how an anti-cheat vendor will treat it.

## 3. Never-do list

Verity never, under any setting:

- Disables, weakens or adds exclusions to Windows Defender or any security product
- Disables security mitigations (speculative-execution mitigations, memory integrity, driver signature enforcement, UAC)
- Disables Windows Update or its services
- Edits boot configuration (BCD)
- Deletes user files, caches, prefetch data, restore points or event logs
- Removes or disables Windows components or Store apps
- Kills processes or stops services outside the session-pause allowlist
- Changes power settings for battery (DC) operation
- Sets any process to realtime priority
- Changes paging file configuration
- Makes network connections. Verity v1 is fully offline: no update checks, no telemetry upload.

## 4. Snake-oil blacklist

These are not to be implemented as actions, and the Lab will not test them. Each is on the list because it is ineffective on current Windows, risky, or both. If Doctor is asked about one, it explains why.

| Tweak | Why it is excluded |
|---|---|
| "RAM cleaners" that trim working sets on a timer | Forces pages out that are immediately faulted back in; causes the stutter it claims to fix |
| Disabling the paging file | Causes crashes under memory pressure; no speed benefit |
| Mass-disabling services from a list | Breaks features unpredictably for negligible gain |
| Global TCP registry tweaks (Nagle, ACK frequency) for "lower ping" | Most games use UDP; settings are per-interface and largely irrelevant |
| Forcing or disabling the platform timer in boot configuration | Outdated folklore; can worsen timing; needs BCD edits |
| Multimedia scheduler registry tweaks (`SystemResponsiveness`, `NetworkThrottlingIndex`) | No reliable measured benefit on current builds |
| Magic `Win32PrioritySeparation` values | Folklore values with no robust evidence |
| Core "unparking" utilities | Modern power plans already manage this; use the power actions instead |
| Disabling CPU security mitigations | Security cost; excluded on principle |
| Realtime or high priority for the game | Starves input, audio and drivers; can freeze the machine |
| Deleting temp files / prefetch "for FPS" | No effect on frame rate |
| Disabling SysMain, indexing or telemetry wholesale and permanently | Use session-scoped pause for the indexer; the rest has no measured benefit |
| "Debloat" scripts | Out of scope and frequently destructive |
| Registry "GPU priority" keys for games | Read only by the multimedia scheduler for registered tasks; not what they are claimed to be |

If evidence emerges that an item here has a real effect, the route is: propose it to the human, move it to "experimental, Lab only" with a documented mechanism and revert, and record the decision in `docs/notes/decisions.md`.

## 5. Consent model

| Change type | Consent |
|---|---|
| Session-scoped, low risk, default-on | Covered by enabling Session Mode once, with the list shown |
| Session-scoped, medium risk | Explicit opt-in per profile |
| Persistent, per-user | Explicit confirmation each time, showing the exact change and how to undo |
| Persistent, machine-wide | Explicit confirmation plus an elevated client |
| Needs reboot | Explicit confirmation; never reboot for the user |

Every confirmation shows: what changes, why, the downside, and how to undo.

## 6. Service hardening

- Catalog-only execution, schema-validated parameters, no client-supplied paths, commands or registry keys (`CLAUDE.md`)
- Pipe DACL and elevation check for machine-wide changes (`02-architecture.md`)
- Data directory writable only by SYSTEM and Administrators
- The PresentMon binary is launched by absolute path from the install directory, whose ACL prevents modification by standard users. Verify its hash against the value recorded at build time before launching.
- Trace sessions are stopped on service stop and orphaned ones cleaned at start
- Fuzz the IPC message parser

## 7. Privacy

- All data stays in `%ProgramData%\Verity`.
- Stored: process names and image paths, driver names, counters, frame timings.
- Never stored: window titles, command lines, file names from disk I/O events, network addresses, anything typed.
- Exports contain names only, no paths and no user name.

## 8. Release safety checklist (M9 gate)

- [ ] Power-loss test: cut power mid-session; after boot, all session actions are reverted
- [ ] Kill the service mid-session; restart; all session actions are reverted
- [ ] Revert-after-drift test for every action
- [ ] Uninstall leaves no applied actions, no trace sessions, no service
- [ ] Standard-user client cannot trigger a machine-wide change
- [ ] IPC fuzzing ran without a crash
- [ ] No outbound network connection from any Verity process (verified with a firewall log)
- [ ] Strict passive mode verified with a handle-audit: no handle to the game process is opened
- [ ] Blacklist grep: none of the excluded registry values or commands appear in the codebase
- [ ] Binaries are code-signed
