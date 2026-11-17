//! Game library, per-game profiles, answers and settings (M4).

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::{utc_now, Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameRow {
    pub id: i64,
    pub name: String,
    pub exe_path: String,
    pub source: String,
    pub launcher_id: Option<String>,
    pub added_utc: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGame<'a> {
    pub name: &'a str,
    pub exe_path: &'a str,
    pub source: &'a str,
    pub launcher_id: Option<&'a str>,
}

/// What a library sync changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SyncResult {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

impl Store {
    /// Makes the stored games from `sources` match `found`. A game that is gone
    /// from disk but has recorded sessions is kept, so its history stays named.
    pub fn sync_games(
        &mut self,
        sources: &[&str],
        found: &[NewGame],
    ) -> Result<SyncResult, StoreError> {
        let tx = self.conn.transaction()?;
        let now = utc_now();
        let mut r = SyncResult {
            added: 0,
            updated: 0,
            removed: 0,
        };
        for g in found {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM games WHERE exe_path = ?1 COLLATE NOCASE",
                    [g.exe_path],
                    |row| row.get(0),
                )
                .optional()?;
            match existing {
                Some(id) => {
                    r.updated += tx.execute(
                        "UPDATE games SET name = ?2, source = ?3, launcher_id = ?4
                         WHERE id = ?1 AND (name, source, ifnull(launcher_id, ''))
                               IS NOT (?2, ?3, ifnull(?4, ''))",
                        params![id, g.name, g.source, g.launcher_id],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO games (name, exe_path, source, launcher_id, added_utc)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![g.name, g.exe_path, g.source, g.launcher_id, now],
                    )?;
                    r.added += 1;
                }
            }
        }
        let stale: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id, exe_path, source FROM games")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .filter(|(_, path, source)| {
                    sources.contains(&source.as_str())
                        && !found.iter().any(|g| g.exe_path.eq_ignore_ascii_case(path))
                })
                .map(|(id, _, _)| id)
                .collect()
        };
        for id in stale {
            let used: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM sessions WHERE game_id = ?1)",
                [id],
                |row| row.get(0),
            )?;
            if !used {
                tx.execute("DELETE FROM profiles WHERE game_id = ?1", [id])?;
                r.removed += tx.execute("DELETE FROM games WHERE id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(r)
    }

    /// Adds one game; an exe already in the library returns its existing id.
    pub fn add_game(&mut self, g: &NewGame) -> Result<i64, StoreError> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM games WHERE exe_path = ?1 COLLATE NOCASE",
                [g.exe_path],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO games (name, exe_path, source, launcher_id, added_utc)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![g.name, g.exe_path, g.source, g.launcher_id, utc_now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Removes a game. Its recorded sessions stay, no longer linked to it.
    pub fn remove_game(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET game_id = NULL WHERE game_id = ?1",
            [id],
        )?;
        tx.execute("DELETE FROM profiles WHERE game_id = ?1", [id])?;
        if tx.execute("DELETE FROM games WHERE id = ?1", [id])? == 0 {
            return Err(StoreError::NotFound(format!("game {id}")));
        }
        tx.commit()?;
        Ok(())
    }

    /// Sorted by name.
    pub fn games(&self) -> Result<Vec<GameRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, exe_path, source, launcher_id, added_utc
             FROM games ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(GameRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    exe_path: r.get(2)?,
                    source: r.get(3)?,
                    launcher_id: r.get(4)?,
                    added_utc: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn game(&self, id: i64) -> Result<GameRow, StoreError> {
        self.games()?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| StoreError::NotFound(format!("game {id}")))
    }

    /// The profile JSON for a game, or the default profile for `None`.
    pub fn profile(&self, game_id: Option<i64>) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT json FROM profiles WHERE ifnull(game_id, 0) = ifnull(?1, 0)",
                [game_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_profile(&mut self, game_id: Option<i64>, json: &str) -> Result<(), StoreError> {
        if let Some(id) = game_id {
            self.game(id)?;
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM profiles WHERE ifnull(game_id, 0) = ifnull(?1, 0)",
            [game_id],
        )?;
        tx.execute(
            "INSERT INTO profiles (game_id, json) VALUES (?1, ?2)",
            params![game_id, json],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The user's answer for an exe (file name, any case), if any.
    pub fn game_answer(&self, exe_name: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT answer FROM game_answers WHERE exe_name = ?1",
                [exe_name.to_lowercase()],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_game_answer(&mut self, exe_name: &str, answer: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO game_answers (exe_name, answer, answered_utc) VALUES (?1, ?2, ?3)
             ON CONFLICT(exe_name) DO UPDATE SET answer = excluded.answer,
                 answered_utc = excluded.answered_utc",
            params![exe_name.to_lowercase(), answer, utc_now()],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g<'a>(name: &'a str, exe: &'a str, source: &'a str) -> NewGame<'a> {
        NewGame {
            name,
            exe_path: exe,
            source,
            launcher_id: None,
        }
    }

    #[test]
    fn sync_adds_updates_and_removes() {
        let mut s = Store::open_in_memory().unwrap();
        let r = s
            .sync_games(
                &["steam"],
                &[g("A", r"C:\a.exe", "steam"), g("B", r"C:\b.exe", "steam")],
            )
            .unwrap();
        assert_eq!((r.added, r.updated, r.removed), (2, 0, 0));
        let manual = s.add_game(&g("M", r"C:\m.exe", "manual")).unwrap();
        // Same exe, different case: no duplicate.
        assert_eq!(s.add_game(&g("M2", r"c:\M.EXE", "manual")).unwrap(), manual);

        // A renamed, B uninstalled; the manual game is not touched by a steam sync.
        let r = s
            .sync_games(&["steam"], &[g("A2", r"C:\A.exe", "steam")])
            .unwrap();
        assert_eq!((r.added, r.updated, r.removed), (0, 1, 1));
        let names: Vec<String> = s.games().unwrap().into_iter().map(|g| g.name).collect();
        assert_eq!(names, ["A2", "M"]);

        // Unchanged rescan reports no updates.
        let r = s
            .sync_games(&["steam"], &[g("A2", r"C:\A.exe", "steam")])
            .unwrap();
        assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
    }

    #[test]
    fn a_game_with_sessions_survives_uninstall_and_removal_unlinks() {
        let mut s = Store::open_in_memory().unwrap();
        s.sync_games(&["steam"], &[g("A", r"C:\a.exe", "steam")])
            .unwrap();
        let id = s.games().unwrap()[0].id;
        s.conn
            .execute(
                "INSERT INTO sessions (game_id, pid, process_start_filetime, started_qpc,
                   qpc_freq, started_utc, capture_mode) VALUES (?1, 1, 0, 0, 1, 'x', 'light')",
                [id],
            )
            .unwrap();
        let r = s.sync_games(&["steam"], &[]).unwrap();
        assert_eq!(r.removed, 0);
        s.remove_game(id).unwrap();
        assert!(s.games().unwrap().is_empty());
        let linked: Option<i64> = s
            .conn
            .query_row("SELECT game_id FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(linked, None);
        assert!(matches!(s.remove_game(id), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn profiles_answers_settings() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.profile(None).unwrap(), None);
        s.set_profile(None, "{\"d\":1}").unwrap();
        s.set_profile(None, "{\"d\":2}").unwrap();
        assert_eq!(s.profile(None).unwrap().as_deref(), Some("{\"d\":2}"));
        assert!(matches!(
            s.set_profile(Some(9), "{}"),
            Err(StoreError::NotFound(_))
        ));
        let id = s.add_game(&g("A", r"C:\a.exe", "manual")).unwrap();
        s.set_profile(Some(id), "{\"g\":1}").unwrap();
        assert_eq!(s.profile(Some(id)).unwrap().as_deref(), Some("{\"g\":1}"));
        assert_eq!(s.profile(None).unwrap().as_deref(), Some("{\"d\":2}"));

        assert_eq!(s.game_answer("X.exe").unwrap(), None);
        s.set_game_answer("X.exe", "never").unwrap();
        assert_eq!(s.game_answer("x.EXE").unwrap().as_deref(), Some("never"));
        assert!(s.set_game_answer("y.exe", "maybe").is_err());

        assert_eq!(s.setting("k").unwrap(), None);
        s.set_setting("k", "1").unwrap();
        s.set_setting("k", "2").unwrap();
        assert_eq!(s.setting("k").unwrap().as_deref(), Some("2"));
    }
}
