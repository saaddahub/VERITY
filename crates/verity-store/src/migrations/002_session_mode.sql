-- M4 Session Mode (docs/06-session-mode.md).
-- The user's answer to "Is <name> a game?", keyed by lowercase exe file name.
CREATE TABLE game_answers (
  exe_name TEXT PRIMARY KEY,
  answer TEXT NOT NULL CHECK (answer IN ('game', 'not_game', 'never')),
  answered_utc TEXT NOT NULL
);
-- Key/value settings the service keeps (Session Mode on/off, panic state).
CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
-- One default profile (game_id NULL) and at most one per game.
CREATE UNIQUE INDEX profiles_game ON profiles(ifnull(game_id, 0));
