//! Session events, session-to-game links and capture bounds (M4/M5).

use rusqlite::{params, OptionalExtension};
use verity_core::telemetry::SysEvent;

use crate::{Store, StoreError};

/// Capture bounds of a stored session, for Frame Story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBounds {
    pub started_qpc: u64,
    pub ended_qpc: u64,
    pub qpc_freq: u64,
    pub game_id: Option<i64>,
}

impl Store {
    /// Records which game a session belongs to, the game process's creation
    /// time (its identity with the pid) and the profile in force.
    pub fn link_session(
        &mut self,
        id: i64,
        game_id: Option<i64>,
        process_start_filetime: u64,
        profile_json: Option<&str>,
    ) -> Result<(), StoreError> {
        let n = self.conn.execute(
            "UPDATE sessions SET game_id = ?2, process_start_filetime = ?3, profile_json = ?4
             WHERE id = ?1",
            params![id, game_id, process_start_filetime as i64, profile_json],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("session {id}")));
        }
        Ok(())
    }

    /// Appends events to a session in one transaction.
    pub fn add_events(&mut self, id: i64, events: &[SysEvent]) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO events (session_id, qpc, duration_ticks, kind, subject, magnitude)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for e in events {
                stmt.execute(params![
                    id,
                    e.qpc as i64,
                    e.duration_ticks as i64,
                    e.kind,
                    e.subject,
                    e.magnitude
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every event of a session, in time order (capture gaps included).
    pub fn events(&self, id: i64) -> Result<Vec<SysEvent>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT qpc, duration_ticks, kind, ifnull(subject, ''), ifnull(magnitude, 0)
             FROM events WHERE session_id = ?1 ORDER BY qpc, id",
        )?;
        let rows = stmt
            .query_map([id], |r| {
                Ok(SysEvent {
                    qpc: r.get::<_, i64>(0)? as u64,
                    duration_ticks: r.get::<_, i64>(1)? as u64,
                    kind: r.get(2)?,
                    subject: r.get(3)?,
                    magnitude: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn bounds(&self, id: i64) -> Result<SessionBounds, StoreError> {
        self.conn
            .query_row(
                "SELECT started_qpc, ifnull(ended_qpc, started_qpc), qpc_freq, game_id
                 FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    Ok(SessionBounds {
                        started_qpc: r.get::<_, i64>(0)? as u64,
                        ended_qpc: r.get::<_, i64>(1)? as u64,
                        qpc_freq: r.get::<_, i64>(2)? as u64,
                        game_id: r.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))
    }

    /// All frames of a session, chunk by chunk.
    pub fn all_frames(&self, id: i64) -> Result<Vec<verity_core::metrics::Frame>, StoreError> {
        let chunks: u32 = self.conn.query_row(
            "SELECT COUNT(*) FROM frame_chunks WHERE session_id = ?1",
            [id],
            |r| r.get(0),
        )?;
        let mut out = Vec::new();
        for seq in 0..chunks {
            out.extend(self.frames(id, seq)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_links_and_bounds() {
        let mut s = Store::open_in_memory().unwrap();
        s.conn
            .execute(
                "INSERT INTO sessions (pid, process_start_filetime, started_qpc, ended_qpc,
                   qpc_freq, started_utc, capture_mode) VALUES (1, 0, 100, 900, 10, 'x', 'standard')",
                [],
            )
            .unwrap();
        let id = s.conn.last_insert_rowid();
        let ev = |q: u64, k: &str| SysEvent {
            qpc: q,
            duration_ticks: 5,
            kind: k.into(),
            subject: "a.sys".into(),
            magnitude: 1.5,
        };
        s.add_events(id, &[ev(300, "dpc"), ev(200, "disk_io")])
            .unwrap();
        let back = s.events(id).unwrap();
        assert_eq!(back, [ev(200, "disk_io"), ev(300, "dpc")]);
        assert!(s.events(id + 1).unwrap().is_empty());

        let game = s
            .add_game(&crate::NewGame {
                name: "G",
                exe_path: r"C:\g.exe",
                source: "manual",
                launcher_id: None,
            })
            .unwrap();
        s.link_session(id, Some(game), 1234, Some("{}")).unwrap();
        let b = s.bounds(id).unwrap();
        assert_eq!(
            b,
            SessionBounds {
                started_qpc: 100,
                ended_qpc: 900,
                qpc_freq: 10,
                game_id: Some(game)
            }
        );
        assert!(matches!(
            s.link_session(99, None, 0, None),
            Err(StoreError::NotFound(_))
        ));
        assert!(matches!(s.bounds(99), Err(StoreError::NotFound(_))));
        assert!(s.all_frames(id).unwrap().is_empty());
    }
}
