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

/// Writes a session's events inside the caller's transaction.
pub(crate) fn insert(
    tx: &rusqlite::Transaction,
    id: i64,
    events: &[SysEvent],
) -> Result<(), StoreError> {
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
    Ok(())
}

impl Store {
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
        let game = s
            .add_game(&crate::NewGame {
                name: "G",
                exe_path: r"C:\g.exe",
                source: "manual",
                launcher_id: None,
            })
            .unwrap();
        let ev = |q: u64, k: &str| SysEvent {
            qpc: q,
            duration_ticks: 5,
            kind: k.into(),
            subject: "a.sys".into(),
            magnitude: 1.5,
        };
        let summary: verity_core::telemetry::SessionSummary =
            serde_json::from_value(serde_json::json!({
                "exe": "g.exe", "pid": 1, "capture_mode": "standard", "capture_s": 80.0,
                "warmup_excluded_s": 5.0, "total_frames": 0,
                "stats": { "frame_count": 0, "duration_s": 0.0, "avg_fps": 0.0,
                    "low_1pct_fps": 0.0, "low_01pct_fps": 0.0, "pacing_spread_ms": 0.0,
                    "hitch_groups": [], "hitch_rate_per_min": 0.0 },
                "gpu_adapter": null, "capture_gaps": [], "unparsed_rows": 0, "samples": 0,
                "overhead_cpu_pct": 0.0, "overhead_budget_pct": 1.0
            }))
            .unwrap();
        let id = s
            .insert_session(&crate::NewSession {
                pid: 1,
                started_qpc: 100,
                ended_qpc: 900,
                qpc_freq: 10,
                started_utc: "x",
                summary: &summary,
                frames: &[],
                series: &[],
                gaps: &[],
                process_start_filetime: 1234,
                game_id: Some(game),
                profile_json: Some("{}"),
                events: &[ev(300, "dpc"), ev(200, "disk_io")],
            })
            .unwrap();
        let back = s.events(id).unwrap();
        assert_eq!(back, [ev(200, "disk_io"), ev(300, "dpc")]);
        assert!(s.events(id + 1).unwrap().is_empty());

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
        let start: i64 = s
            .conn
            .query_row(
                "SELECT process_start_filetime FROM sessions WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(start, 1234);
        assert!(matches!(s.bounds(99), Err(StoreError::NotFound(_))));
        assert!(s.all_frames(id).unwrap().is_empty());
    }
}
