//! SQLite storage. The service owns the database; clients never open it.

pub mod codec;
pub mod events;
pub mod games;

pub use events::SessionBounds;
pub use games::{GameRow, NewGame, SyncResult};

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use verity_core::metrics::Frame;
use verity_core::telemetry::{CaptureGap, SessionSummary};

/// Numbered migrations; `PRAGMA user_version` records how many have run.
const MIGRATIONS: [&str; 2] = [
    include_str!("migrations/001_initial.sql"),
    include_str!("migrations/002_session_mode.sql"),
];
/// Roughly this much play time per frame chunk.
const CHUNK_SECONDS: u64 = 10;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("stored data is unreadable: {0}")]
    Codec(String),
    #[error("{0} not found")]
    NotFound(String),
}

/// Everything one finished capture stores.
pub struct NewSession<'a> {
    pub pid: u32,
    pub started_qpc: u64,
    pub ended_qpc: u64,
    pub qpc_freq: u64,
    pub started_utc: &'a str,
    pub summary: &'a SessionSummary,
    pub frames: &'a [Frame],
    /// (metric name, points).
    pub series: &'a [(&'a str, Vec<(u64, f32)>)],
    pub gaps: &'a [CaptureGap],
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: i64,
    pub started_utc: String,
    pub summary: SessionSummary,
    pub frame_chunks: u32,
}

pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self, StoreError> {
        // journal_mode returns a row, so it cannot go through execute_batch.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        let done: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(done) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i + 1)?;
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    /// Stores a session with its frames, counters and gaps, all or nothing.
    pub fn insert_session(&mut self, s: &NewSession) -> Result<i64, StoreError> {
        let summary =
            serde_json::to_string(s.summary).map_err(|e| StoreError::Codec(e.to_string()))?;
        let tx = self.conn.transaction()?;
        // shortcut: process_start_filetime is 0 (not recorded) because reading it
        // needs a handle to the game; fill it from kernel process events in M4.
        tx.execute(
            "INSERT INTO sessions (pid, process_start_filetime, started_qpc, ended_qpc,
                 qpc_freq, started_utc, capture_mode, summary_json)
             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                s.pid,
                s.started_qpc as i64,
                s.ended_qpc as i64,
                s.qpc_freq as i64,
                s.started_utc,
                s.summary.capture_mode,
                summary
            ],
        )?;
        let id = tx.last_insert_rowid();

        let span = CHUNK_SECONDS * s.qpc_freq;
        let mut rest = s.frames;
        let mut seq = 0u32;
        while let Some(first) = rest.first() {
            let n = rest.partition_point(|f| f.qpc < first.qpc + span).max(1);
            let (chunk, tail) = rest.split_at(n);
            tx.execute(
                "INSERT INTO frame_chunks (session_id, seq, start_qpc, frame_count, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    seq,
                    first.qpc as i64,
                    n as i64,
                    codec::encode_frames(chunk)?
                ],
            )?;
            rest = tail;
            seq += 1;
        }
        for (metric, points) in s.series {
            if let Some(first) = points.first() {
                tx.execute(
                    "INSERT INTO sample_chunks (session_id, metric, seq, start_qpc, data)
                     VALUES (?1, ?2, 0, ?3, ?4)",
                    params![id, metric, first.0 as i64, codec::encode_series(points)?],
                )?;
            }
        }
        for gap in s.gaps {
            tx.execute(
                "INSERT INTO events (session_id, qpc, duration_ticks, kind)
                 VALUES (?1, ?2, ?3, 'capture_gap')",
                params![id, gap.qpc as i64, gap.duration_ticks as i64],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    fn rows(&self, filter: &str, id: Option<i64>) -> Result<Vec<SessionRow>, StoreError> {
        let sql = format!(
            "SELECT s.id, s.started_utc, s.summary_json,
                    (SELECT COUNT(*) FROM frame_chunks c WHERE c.session_id = s.id)
             FROM sessions s {filter} ORDER BY s.id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let raw = stmt
            .query_map(rusqlite::params_from_iter(id), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, u32>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raw.into_iter()
            .map(|(id, started_utc, json, frame_chunks)| {
                Ok(SessionRow {
                    id,
                    started_utc,
                    summary: serde_json::from_str(&json)
                        .map_err(|e| StoreError::Codec(e.to_string()))?,
                    frame_chunks,
                })
            })
            .collect()
    }

    /// Newest first.
    pub fn list(&self) -> Result<Vec<SessionRow>, StoreError> {
        self.rows("", None)
    }

    pub fn get(&self, id: i64) -> Result<SessionRow, StoreError> {
        self.rows("WHERE s.id = ?1", Some(id))?
            .pop()
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))
    }

    /// One chunk (about ten seconds) of a session's frames.
    pub fn frames(&self, id: i64, seq: u32) -> Result<Vec<Frame>, StoreError> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT data FROM frame_chunks WHERE session_id = ?1 AND seq = ?2",
                params![id, seq],
                |r| r.get(0),
            )
            .optional()?;
        codec::decode_frames(
            &blob.ok_or_else(|| StoreError::NotFound(format!("session {id} chunk {seq}")))?,
        )
    }

    pub fn series(&self, id: i64, metric: &str) -> Result<Vec<(u64, f32)>, StoreError> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT data FROM sample_chunks WHERE session_id = ?1 AND metric = ?2 AND seq = 0",
                params![id, metric],
                |r| r.get(0),
            )
            .optional()?;
        blob.map_or(Ok(Vec::new()), |b| codec::decode_series(&b))
    }
}

/// Current UTC time as `YYYY-MM-DDThh:mm:ssZ`.
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_utc(secs)
}

fn format_utc(unix_secs: u64) -> String {
    // Days-to-civil conversion (Howard Hinnant's algorithm).
    let days = unix_secs / 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (
        doy - (153 * mp + 2) / 5 + 1,
        if mp < 10 { mp + 3 } else { mp - 9 },
    );
    let year = yoe + era * 400 + u64::from(month <= 2);
    let t = unix_secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use verity_core::metrics::{summarize, Thresholds};

    const FREQ: u64 = 10_000_000;

    fn frames(n: u64) -> Vec<Frame> {
        (0..n)
            .map(|i| Frame {
                qpc: 1_000 + i * FREQ / 60,
                frame_time_ms: 1000.0 / 60.0,
                cpu_busy_ms: 2.0,
                gpu_busy_ms: 1.0,
                displayed_time_ms: f32::NAN,
                display_latency_ms: f32::NAN,
                present_mode: 3,
                flags: 1,
            })
            .collect()
    }

    fn summary(frames: &[Frame]) -> SessionSummary {
        SessionSummary {
            exe: "game.exe".into(),
            pid: 77,
            capture_mode: "light".into(),
            capture_s: 25.0,
            warmup_excluded_s: 5.0,
            total_frames: frames.len() as u64,
            stats: summarize(frames, FREQ, &Thresholds::default()),
            gpu_adapter: Some("Test GPU".into()),
            capture_gaps: vec![],
            unparsed_rows: 0,
            samples: 2,
            overhead_cpu_pct: 0.05,
            overhead_budget_pct: 0.3,
        }
    }

    #[test]
    fn session_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(store.list().unwrap().is_empty());

        let frames = frames(60 * 25); // 25 s at 60 FPS: three chunks
        let summary = summary(&frames);
        let gaps = [CaptureGap {
            qpc: 5,
            duration_ticks: 9,
        }];
        let series = [
            ("cpu_total_pct", vec![(10u64, 12.5f32), (20, 13.0)]),
            ("empty", vec![]),
        ];
        let id = store
            .insert_session(&NewSession {
                pid: 77,
                started_qpc: 1_000,
                ended_qpc: 1_000 + 25 * FREQ,
                qpc_freq: FREQ,
                started_utc: "2026-11-14T10:00:00Z",
                summary: &summary,
                frames: &frames,
                series: &series,
                gaps: &gaps,
            })
            .unwrap();

        let row = store.get(id).unwrap();
        assert_eq!(row.summary, summary);
        assert_eq!(row.frame_chunks, 3);
        assert_eq!(store.list().unwrap(), [row]);

        let mut back = Vec::new();
        for seq in 0..3 {
            back.extend(store.frames(id, seq).unwrap());
        }
        assert_eq!(back.len(), frames.len());
        assert!(back
            .iter()
            .zip(&frames)
            .all(|(a, b)| a.qpc == b.qpc && a.flags == b.flags));

        assert_eq!(store.series(id, "cpu_total_pct").unwrap(), series[0].1);
        assert!(store.series(id, "empty").unwrap().is_empty());
        let gap_rows: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'capture_gap'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gap_rows, 1);
    }

    #[test]
    fn missing_things_are_not_found() {
        let store = Store::open_in_memory().unwrap();
        assert!(matches!(store.get(1), Err(StoreError::NotFound(_))));
        assert!(matches!(store.frames(1, 0), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn reopening_does_not_rerun_migrations() {
        let dir = std::env::temp_dir().join(format!("verity-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("verity.db");
        drop(Store::open(&path).unwrap());
        let store = Store::open(&path).unwrap(); // would fail with "table exists"
        assert!(store.list().unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_794_650_096), "2026-11-14T09:54:56Z");
    }
}
