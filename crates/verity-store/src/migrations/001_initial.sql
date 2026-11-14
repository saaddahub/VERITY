-- Schema from docs/02-architecture.md.
CREATE TABLE games (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, exe_path TEXT NOT NULL UNIQUE,
  source TEXT NOT NULL,
  launcher_id TEXT, added_utc TEXT NOT NULL
);

CREATE TABLE profiles (
  id INTEGER PRIMARY KEY, game_id INTEGER REFERENCES games(id),
  json TEXT NOT NULL
);

CREATE TABLE sessions (
  id INTEGER PRIMARY KEY, game_id INTEGER REFERENCES games(id),
  pid INTEGER NOT NULL, process_start_filetime INTEGER NOT NULL,
  started_qpc INTEGER NOT NULL, ended_qpc INTEGER, qpc_freq INTEGER NOT NULL,
  started_utc TEXT NOT NULL, capture_mode TEXT NOT NULL,
  profile_json TEXT, summary_json TEXT, lab_run_id INTEGER
);

CREATE TABLE frame_chunks (
  session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL, start_qpc INTEGER NOT NULL, frame_count INTEGER NOT NULL,
  data BLOB NOT NULL,
  PRIMARY KEY (session_id, seq)
);

CREATE TABLE sample_chunks (
  session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  metric TEXT NOT NULL, seq INTEGER NOT NULL, start_qpc INTEGER NOT NULL,
  data BLOB NOT NULL, PRIMARY KEY (session_id, metric, seq)
);

CREATE TABLE events (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  qpc INTEGER NOT NULL, duration_ticks INTEGER NOT NULL DEFAULT 0,
  kind TEXT NOT NULL,
  subject TEXT,
  pid INTEGER, magnitude REAL, detail_json TEXT
);
CREATE INDEX events_session_qpc ON events(session_id, qpc);

CREATE TABLE journal (
  id INTEGER PRIMARY KEY, txn_id TEXT NOT NULL, action_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  executor TEXT NOT NULL,
  params_json TEXT NOT NULL, snapshot_json TEXT NOT NULL,
  state TEXT NOT NULL,
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
