# 06 — Session Mode

A session is the period one game is running. Verity applies that game's profile at the start, records telemetry throughout, and puts everything back at the end.

## Game detection

Two sources, combined.

### Library scan

Run at service start and on demand (`games.scan`). Items marked **(verify)** need checking on the machine.

| Source | Where |
|---|---|
| Steam | Install path from the registry; `steamapps\libraryfolders.vdf` for library folders; each `appmanifest_*.acf` for name and install directory. Pick the main exe by: launch config if readable, otherwise the largest exe that is not an installer, crash handler or redistributable. |
| Epic | `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item` (JSON: display name, install location, launch executable) |
| GOG | `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\*` **(verify)** |
| Xbox / Store | Best effort: `XboxGames` folders on each volume **(verify)**. Otherwise rely on the heuristic. |
| Manual | User adds an exe |

Maintain a name list of launchers and helpers that are never games (Steam client, Epic launcher, web helpers, crash reporters, anti-cheat services).

### Heuristic

A process becomes a **candidate** when all hold for 10 seconds:

- It is the foreground window's process (agent)
- It has 3D GPU engine utilization above 20%
- It is not on the never-a-game list and not a browser or video player on the known list

A candidate that is not in the library triggers one notification: "Is `<name>` a game? Yes / No / Never ask". The answer is stored.

Process starts come from the kernel trace `PROCESS` events (a light always-on trace session with only that flag) rather than polling.

## State machine

```
Idle ──process start matches a game──▶ Arming
Arming ──10 s alive and has presented frames──▶ Active      (else back to Idle)
Active ──game process exits──▶ Ending
Active ──panic hotkey / session.stop──▶ Ending
Active ──service stopping / shutdown / suspend──▶ Ending
Ending ──all session actions reverted, summary written──▶ Idle
```

- One session at a time. A second game starting while one is active is recorded as an event and otherwise ignored.
- **Arming** exists so that launchers, splash screens and crash-on-start do not trigger a profile.
- On **Active**: start capture at the profile's mode, apply session actions in catalog order, start the background-process watcher.
- On **Ending**: stop capture, revert in reverse order, compute the summary, run session rules and Frame Story, notify the user with a one-line result.
- System suspend or shutdown notification ends the session and reverts synchronously before acknowledging.
- If the game loses foreground for more than 60 seconds, pause background throttling (the user is doing something else) and resume when the game regains foreground. Record both as events.

### Background-process watcher

While Active, each newly started process in the user's session is checked against the exclusion list (`05-actions.md`) and, if eligible, gets the same EcoQoS and CPU-set treatment, with its own journal entry.

## Profiles

One default profile plus an optional per-game profile that overrides it. Stored as JSON.

```json
{
  "version": 1,
  "capture_mode": "standard",
  "actions": {
    "power.session_overlay":     { "enabled": true },
    "sched.background_ecoqos":   { "enabled": true },
    "sched.background_cpusets":  { "enabled": true },
    "sched.game_priority":       { "enabled": false },
    "power.session_scheme":      { "enabled": false, "params": {} },
    "services.session_pause":    { "enabled": false, "params": { "services": [] } },
    "memory.standby_purge":      { "enabled": false }
  },
  "extra_exclusions": ["obs64.exe"],
  "notes": ""
}
```

- An action enabled in a profile only because a Lab experiment proved it carries `"proven_by": <experiment id>`. The UI shows this as a badge.
- Unknown action ids in a stored profile are ignored with a warning, never an error.

## The panic path

A global hotkey registered by the agent (default Ctrl+Alt+Shift+V, configurable) calls `session.panic`: revert everything session-scoped immediately, stop capture, disable Session Mode until the user turns it back on. The tray menu has the same command. This must work even if the UI window is closed.

## User-visible behavior

- Session start: one quiet notification, "Verity: session started for `<game>`", suppressible.
- Session end: one notification with the headline numbers and, if any, the top Frame Story cause. Clicking opens the session.
- Nothing is shown over the game.

## Edge cases to handle explicitly

- Game restarts itself (launcher → game → relaunch): treat a new matching process within 15 seconds of exit as the same session.
- Game runs elevated or protected: capture still works (passive); process-targeted actions on the game are skipped.
- Power source changes mid-session: record the event; `power.session_overlay` is a no-op on battery and re-evaluated on change.
- User changes the power mode manually mid-session: the revert sees drift and marks the entry `superseded` instead of overwriting the user's choice.
- Fast user switching or logoff mid-session: end the session.

## CLI

```
verity session status
verity session start --exe <path> | --pid <pid>
verity session stop
verity session panic
verity games scan | list | add <exe> | remove <id>
verity profile show [--game <id>]
verity profile set  [--game <id>] --file profile.json
```
