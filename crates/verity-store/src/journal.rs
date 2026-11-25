//! The action journal (docs/05-actions.md). Every write is its own committed
//! statement, so a row is on disk before the change it records is made.

use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use verity_core::actions::{
    Executor, JournalFilter, JournalRow, JournalState, JournalStore, NewJournalRow, Scope,
};

use crate::{utc_now, Store};

fn text<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(s.into())).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn json(s: &str) -> rusqlite::Result<serde_json::Value> {
    serde_json::from_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

const COLUMNS: &str = "id, txn_id, action_id, scope, executor, params_json, snapshot_json, \
                       owner, state, created_utc, applied_utc, reverted_utc, error";

fn row(r: &Row) -> rusqlite::Result<JournalRow> {
    let state: String = r.get(8)?;
    Ok(JournalRow {
        id: r.get(0)?,
        row: NewJournalRow {
            txn_id: r.get(1)?,
            action_id: r.get(2)?,
            scope: parse::<Scope>(&r.get::<_, String>(3)?)?,
            executor: parse::<Executor>(&r.get::<_, String>(4)?)?,
            params: json(&r.get::<_, String>(5)?)?,
            snapshot: json(&r.get::<_, String>(6)?)?,
            owner: r.get(7)?,
        },
        state: JournalState::parse(&state).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, state.into())
        })?,
        created_utc: r.get(9)?,
        applied_utc: r.get(10)?,
        reverted_utc: r.get(11)?,
        error: r.get(12)?,
    })
}

impl JournalStore for Store {
    fn journal_insert(&self, n: &NewJournalRow) -> Result<i64, String> {
        self.conn
            .execute(
                "INSERT INTO journal (txn_id, action_id, scope, executor, params_json, \
                 snapshot_json, owner, state, created_utc) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
                params![
                    n.txn_id,
                    n.action_id,
                    text(&n.scope),
                    text(&n.executor),
                    n.params.to_string(),
                    n.snapshot.to_string(),
                    n.owner,
                    utc_now()
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(self.conn.last_insert_rowid())
    }

    fn journal_set_state(
        &self,
        id: i64,
        state: JournalState,
        error: Option<&str>,
    ) -> Result<(), String> {
        let now = utc_now();
        let n = self
            .conn
            .execute(
                "UPDATE journal SET state = ?2, error = COALESCE(?3, error), \
                 applied_utc = CASE WHEN ?2 = 'applied' THEN ?4 ELSE applied_utc END, \
                 reverted_utc = CASE WHEN ?2 IN ('reverted', 'superseded', 'revert_failed') \
                                THEN ?4 ELSE reverted_utc END \
                 WHERE id = ?1",
                params![id, state.as_str(), error, now],
            )
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!("journal entry {id} not found"));
        }
        Ok(())
    }

    fn journal_get(&self, id: i64) -> Result<Option<JournalRow>, String> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM journal WHERE id = ?1"),
                [id],
                row,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    fn journal_list(&self, f: &JournalFilter) -> Result<Vec<JournalRow>, String> {
        let scope = f.scope.map(|s| text(&s));
        let executor = f.executor.map(|e| text(&e));
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM journal \
                 WHERE (?1 IS NULL OR txn_id = ?1) AND (?2 IS NULL OR scope = ?2) \
                 AND (?3 IS NULL OR executor = ?3) AND (?4 IS NULL OR owner = ?4) \
                 AND (?5 = 0 OR state IN ('pending', 'applied')) ORDER BY id"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                params![f.txn_id, scope, executor, f.owner, f.open_only],
                row,
            )
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rows_round_trip_and_filter() {
        let s = Store::open_in_memory().unwrap();
        let new = |txn: &str, scope, executor, owner: &str| NewJournalRow {
            txn_id: txn.into(),
            action_id: "sched.background_ecoqos".into(),
            scope,
            executor,
            params: json!({"pid": 4}),
            snapshot: json!({"control": 0, "state": 0}),
            owner: owner.into(),
        };
        let a = s
            .journal_insert(&new("s1", Scope::Session, Executor::Service, ""))
            .unwrap();
        let b = s
            .journal_insert(&new("u1", Scope::Persistent, Executor::Agent, "S-1-5-21-x"))
            .unwrap();
        s.journal_set_state(a, JournalState::Applied, None).unwrap();
        let got = s.journal_get(a).unwrap().unwrap();
        assert_eq!(got.state, JournalState::Applied);
        assert!(got.applied_utc.is_some());
        assert_eq!(got.row.params, json!({"pid": 4}));

        let open_session = s
            .journal_list(&JournalFilter {
                scope: Some(Scope::Session),
                open_only: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            open_session.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![a]
        );
        let mine = s
            .journal_list(&JournalFilter {
                owner: Some("S-1-5-21-x"),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, b);

        s.journal_set_state(a, JournalState::Superseded, Some("changed by the user"))
            .unwrap();
        let got = s.journal_get(a).unwrap().unwrap();
        assert!(got.reverted_utc.is_some());
        assert_eq!(got.error.as_deref(), Some("changed by the user"));
        assert!(s
            .journal_set_state(99, JournalState::Applied, None)
            .is_err());
    }
}
