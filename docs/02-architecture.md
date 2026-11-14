# 02 — Architecture

## Process model

```
┌───────────────────────────── user session ─────────────────────────────┐
│  verity-ui (Tauri)            verity-cli                               │
│  - windows, tray icon         - scripting, testing                     │
│  - user agent duties:                                                  │
│    foreground tracking, HKCU writes, display mode,                     │
│    audio session enumeration, notifications, hotkey                    │
└───────────────┬───────────────────────┬────────────────────────────────┘
                │   JSON-RPC 2.0 over named pipe  \\.\pipe\verity-v1
┌───────────────┴───────────────────────┴──────── session 0 ─────────────┐
│  verity-service (Windows service, LocalSystem)                         │
│  - telemetry capture (ETW, PDH, PresentMon child process)              │
│  - action execution + journal                                          │
│  - session state machine, doctor, frame story, lab                     │
│  - SQLite database owner                                               │
└────────────────────────────────────────────────────────────────────────┘
```

### Why two sides

A service in session 0 can hold kernel trace sessions, open other users' processes and survive logoff, but it cannot see the desktop: no foreground window, no per-user registry hive, no display mode changes, no audio sessions. Those run in the user agent, which is part of the tray app.

**Rule:** every *session-scoped* action must be executable by the service alone. Crash recovery must never depend on a user being logged in. Agent-side actions are persistent-scope only (GPU preference, refresh rate, Game Mode).

If the agent is not connected, agent-side work is skipped and reported as deferred.

## Workspace layout

```
verity/
  CLAUDE.md
  Cargo.toml                  # workspace
  docs/
  crates/
    verity-sys/               # all unsafe + Win32 wrappers, trait impls
    verity-core/              # domain types, traits, journal, errors, config
    verity-telemetry/         # frame capture, PDH sampler, ETW consumers, event logs
    verity-doctor/            # diagnostic rules
    verity-actions/           # concrete Action implementations
    verity-session/           # game detection, profiles, session state machine
    verity-story/             # stutter detection and cause attribution
    verity-lab/               # experiment protocols and statistics
    verity-store/             # SQLite schema, migrations, queries
    verity-ipc/               # protocol types, client, server
  apps/
    verity-service/
    verity-cli/
    verity-ui/                # Tauri app (src-tauri + React front end)
  tools/
    presentmon/               # third-party binary + license
    probes/                   # small binaries that verify OS behavior
    framegen/                 # synthetic "game" for tests (see 11-roadmap)
    stressors/                # cpu / disk / memory load generators for tests
```

Dependency direction: `sys ← core ← {telemetry, actions, store} ← {doctor, session, story, lab} ← ipc ← apps`. `verity-core` defines the OS traits; `verity-sys` implements them. Nothing above `verity-sys` contains `unsafe`.

## IPC

- Transport: named pipe `\\.\pipe\verity-v1`, byte mode, each message a 4-byte little-endian length followed by UTF-8 JSON.
- Protocol: JSON-RPC 2.0, bidirectional. The client calls the service; the service can also call the connected agent for user-session work.
- Versioning: first call is `hello { protocol: 1, role: "ui" | "cli" }`. Mismatch returns a clear error.

### Pipe security

- DACL: SYSTEM and Administrators full control; the pipe owner full control; the Interactive group read plus `FILE_WRITE_DATA` only (not generic write, which would allow creating pipe instances). Clients open with `GENERIC_READ | FILE_WRITE_DATA`; see `docs/notes/decisions.md`. No network access (`PIPE_REJECT_REMOTE_CLIENTS`).
- The service treats every client as untrusted. See "Security rules for the service" in `CLAUDE.md`.
- Persistent-scope HKLM actions require the request to come from an elevated client. Check the client token via `ImpersonateNamedPipeClient` and test for Administrators membership, then revert impersonation immediately.

### Methods (initial set)

| Area | Methods |
|---|---|
| System | `system.info`, `system.capabilities` |
| Doctor | `doctor.run`, `doctor.findings`, `doctor.dismiss` |
| Actions | `actions.catalog`, `actions.plan` (dry-run), `actions.apply`, `actions.revert` |
| Journal | `journal.list`, `journal.revert_session`, `journal.revert_all` |
| Session | `session.status`, `session.start`, `session.stop`, `session.panic` |
| History | `sessions.list`, `sessions.get`, `sessions.frames`, `sessions.events`, `sessions.delete` |
| Story | `story.get`, `story.causes` |
| Games | `games.list`, `games.scan`, `games.add`, `games.remove` |
| Profiles | `profiles.get`, `profiles.set`, `profiles.reset` |
| Lab | `lab.create`, `lab.next_step`, `lab.status`, `lab.result`, `lab.abort` |
| Events | `subscribe { topics }` → notifications `session.state`, `telemetry.live`, `finding.new`, `journal.changed`, `lab.prompt` |
| Agent (service → agent) | `agent.foreground`, `agent.apply`, `agent.revert`, `agent.audio_sessions`, `agent.notify` |

## Storage

- Location: `%ProgramData%\Verity\` → `verity.db`, `logs\`, `exports\`. Directory ACL: SYSTEM and Administrators full, Users read.
- SQLite in WAL mode, owned by the service. Clients never open the database directly.
- Migrations are numbered SQL files embedded in `verity-store`; `PRAGMA user_version` tracks the level.

### Schema sketch

```sql
CREATE TABLE games (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, exe_path TEXT NOT NULL UNIQUE,
  source TEXT NOT NULL,            -- steam | epic | gog | xbox | manual | heuristic
  launcher_id TEXT, added_utc TEXT NOT NULL
);

CREATE TABLE profiles (
  id INTEGER PRIMARY KEY, game_id INTEGER REFERENCES games(id),  -- NULL = default profile
  json TEXT NOT NULL
);

CREATE TABLE sessions (
  id INTEGER PRIMARY KEY, game_id INTEGER REFERENCES games(id),
  pid INTEGER NOT NULL, process_start_filetime INTEGER NOT NULL,
  started_qpc INTEGER NOT NULL, ended_qpc INTEGER, qpc_freq INTEGER NOT NULL,
  started_utc TEXT NOT NULL, capture_mode TEXT NOT NULL,   -- light | standard | deep
  profile_json TEXT, summary_json TEXT, lab_run_id INTEGER
);

CREATE TABLE frame_chunks (           -- ~10 s of frames per row
  session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL, start_qpc INTEGER NOT NULL, frame_count INTEGER NOT NULL,
  data BLOB NOT NULL,                 -- zstd of columnar f32/u64 arrays, format versioned
  PRIMARY KEY (session_id, seq)
);

CREATE TABLE sample_chunks (          -- periodic counters, columnar per metric
  session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  metric TEXT NOT NULL, seq INTEGER NOT NULL, start_qpc INTEGER NOT NULL,
  data BLOB NOT NULL, PRIMARY KEY (session_id, metric, seq)
);

CREATE TABLE events (                 -- discrete things that happened
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  qpc INTEGER NOT NULL, duration_ticks INTEGER NOT NULL DEFAULT 0,
  kind TEXT NOT NULL,                 -- dpc | isr | hard_fault | disk_io | proc_start | scan | ...
  subject TEXT,                       -- driver, process or task name
  pid INTEGER, magnitude REAL, detail_json TEXT
);
CREATE INDEX events_session_qpc ON events(session_id, qpc);

CREATE TABLE journal (
  id INTEGER PRIMARY KEY, txn_id TEXT NOT NULL, action_id TEXT NOT NULL,
  scope TEXT NOT NULL,                -- session | persistent
  executor TEXT NOT NULL,             -- service | agent
  params_json TEXT NOT NULL, snapshot_json TEXT NOT NULL,
  state TEXT NOT NULL,                -- pending | applied | reverted | revert_failed | superseded
  session_id INTEGER, created_utc TEXT NOT NULL, applied_utc TEXT, reverted_utc TEXT, error TEXT
);

CREATE TABLE findings (
  id INTEGER PRIMARY KEY, session_id INTEGER, rule_id TEXT NOT NULL,
  severity TEXT NOT NULL, confidence TEXT NOT NULL,
  evidence_json TEXT NOT NULL, created_utc TEXT NOT NULL, dismissed INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE experiments (
  id INTEGER PRIMARY KEY, game_id INTEGER NOT NULL REFERENCES games(id),
  name TEXT NOT NULL, action_id TEXT NOT NULL, params_json TEXT NOT NULL,
  protocol_json TEXT NOT NULL, seed INTEGER NOT NULL,
  status TEXT NOT NULL, result_json TEXT, created_utc TEXT NOT NULL
);

CREATE TABLE runs (
  id INTEGER PRIMARY KEY, experiment_id INTEGER NOT NULL REFERENCES experiments(id) ON DELETE CASCADE,
  block INTEGER NOT NULL, arm TEXT NOT NULL, order_idx INTEGER NOT NULL,
  session_id INTEGER REFERENCES sessions(id),
  valid INTEGER, invalid_reason TEXT, metrics_json TEXT
);
```

Retention: keep summaries forever; delete frame and sample chunks of sessions older than a configurable limit (default 60 days) unless the session belongs to an experiment or is pinned.

## Time

Everything is on the QueryPerformanceCounter timeline.

- ETW sessions are created with the QPC clock (`Wnode.ClientContext = 1`).
- PresentMon is asked for QPC timestamps.
- PDH samples are stamped with `QueryPerformanceCounter` at collection.
- Each session stores `qpc_freq` and one `(qpc, utc)` anchor pair so wall-clock time can be reconstructed.

## Threads and tasks in the service

- One tokio runtime for IPC, timers, state machines and database writes (database writes go through a single writer task with a bounded channel).
- ETW consumers run on dedicated OS threads (`ProcessTrace` blocks). They do minimal work in the callback: filter, convert to a compact struct, push to a bounded lock-free queue. If the queue is full, drop and count the drop; report drops in the session summary.
- PresentMon runs as a child process; a task reads its stdout line by line.
- Backpressure never blocks a kernel trace callback.

## Configuration

`%ProgramData%\Verity\config.toml`, loaded at start and reloadable through IPC. Holds capture defaults, retention, exclusion lists, thresholds for Doctor and Frame Story. Every threshold named in the specs is a config value with the spec's number as its default.

## Install and uninstall

- Install registers the service (auto-start, delayed), creates the data directory with its ACL, and adds the tray app to the user's startup.
- Uninstall first runs `journal.revert_all`, then stops trace sessions, removes the service and startup entry, and asks whether to delete data.
- On every service start: run **crash recovery** (see `05-actions.md`), and stop any orphaned trace session named `Verity*`.
