//! # om-history
//!
//! [`HistoryStore`] implementation backed by SQLite. Persists [`WatchProgress`]
//! so the engine can resume an episode at its last position and show a "continue
//! watching" list.
//!
//! Chosen over curd's full-file-rewrite CSV because per-row upserts are
//! concurrency-safe and cheap on every position tick. The `rusqlite` connection
//! is `Send` but not `Sync`, so it lives behind a `Mutex` to satisfy the
//! `Send + Sync` port bound; writes are sub-millisecond so the lock is not a
//! contention concern on the ~1 Hz progress path.
//!
//! [`HistoryStore`]: om_core::ports::HistoryStore
//! [`WatchProgress`]: om_core::tracking::WatchProgress

use std::path::PathBuf;
use std::sync::Mutex;

use om_core::error::{CoreError, CoreResult};
use om_core::ports::HistoryStore;
use om_core::tracking::WatchProgress;
use rusqlite::{params, Connection};

/// SQLite-backed watch history.
pub struct SqliteHistory {
    conn: Mutex<Connection>,
}

impl SqliteHistory {
    /// Open (creating + migrating) the history DB at `db_path`.
    pub fn open(db_path: PathBuf) -> CoreResult<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Storage(format!("creating {}: {e}", parent.display())))?;
        }
        let conn = Connection::open(&db_path).map_err(storage)?;
        Self::from_connection(conn)
    }

    /// In-memory store (tests).
    pub fn in_memory() -> CoreResult<Self> {
        let conn = Connection::open_in_memory().map_err(storage)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> CoreResult<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS watch_progress (
                 media_key     TEXT    NOT NULL,
                 season        INTEGER NOT NULL,
                 episode       INTEGER NOT NULL,
                 position_secs INTEGER NOT NULL,
                 duration_secs INTEGER NOT NULL,
                 updated_at    INTEGER NOT NULL,
                 PRIMARY KEY (media_key, season, episode)
             );",
        )
        .map_err(storage)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Default path: `~/.local/share/open-media/history.db`.
    pub fn default_path() -> PathBuf {
        dirs_data_dir().join("open-media").join("history.db")
    }
}

impl HistoryStore for SqliteHistory {
    fn save(&self, p: &WatchProgress) -> CoreResult<()> {
        let conn = self.conn.lock().map_err(poisoned)?;
        conn.execute(
            "INSERT INTO watch_progress
                 (media_key, season, episode, position_secs, duration_secs, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(media_key, season, episode) DO UPDATE SET
                 position_secs = excluded.position_secs,
                 duration_secs = excluded.duration_secs,
                 updated_at    = excluded.updated_at",
            params![
                p.media_key,
                p.season,
                p.episode,
                p.position_secs,
                p.duration_secs,
                p.updated_at
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    fn resume(
        &self,
        media_key: &str,
        season: u32,
        episode: u32,
    ) -> CoreResult<Option<WatchProgress>> {
        let conn = self.conn.lock().map_err(poisoned)?;
        let mut stmt = conn
            .prepare(
                "SELECT media_key, season, episode, position_secs, duration_secs, updated_at
                 FROM watch_progress WHERE media_key = ?1 AND season = ?2 AND episode = ?3",
            )
            .map_err(storage)?;
        let mut rows = stmt
            .query_map(params![media_key, season, episode], row_to_progress)
            .map_err(storage)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(storage)?)),
            None => Ok(None),
        }
    }

    fn recent(&self, limit: usize) -> CoreResult<Vec<WatchProgress>> {
        let conn = self.conn.lock().map_err(poisoned)?;
        let mut stmt = conn
            .prepare(
                "SELECT media_key, season, episode, position_secs, duration_secs, updated_at
                 FROM watch_progress ORDER BY updated_at DESC LIMIT ?1",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_progress)
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)
    }
}

fn row_to_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<WatchProgress> {
    Ok(WatchProgress {
        media_key: row.get(0)?,
        season: row.get(1)?,
        episode: row.get(2)?,
        position_secs: row.get(3)?,
        duration_secs: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn storage(e: rusqlite::Error) -> CoreError {
    CoreError::Storage(format!("sqlite: {e}"))
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> CoreError {
    CoreError::Storage("history mutex poisoned".into())
}

fn dirs_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(key: &str, ep: u32, pos: u32, t: i64) -> WatchProgress {
        WatchProgress {
            media_key: key.into(),
            season: 1,
            episode: ep,
            position_secs: pos,
            duration_secs: 1440,
            updated_at: t,
        }
    }

    #[test]
    fn save_then_resume_roundtrips() {
        let store = SqliteHistory::in_memory().unwrap();
        store.save(&progress("imdb:tt1", 1, 615, 100)).unwrap();
        let got = store.resume("imdb:tt1", 1, 1).unwrap().unwrap();
        assert_eq!(got.position_secs, 615);
        assert_eq!(got.duration_secs, 1440);
        assert!(got.is_complete(0.4)); // 615/1440 ~ 0.43
    }

    #[test]
    fn save_upserts_on_conflict() {
        let store = SqliteHistory::in_memory().unwrap();
        store.save(&progress("imdb:tt1", 1, 100, 1)).unwrap();
        store.save(&progress("imdb:tt1", 1, 900, 2)).unwrap();
        let got = store.resume("imdb:tt1", 1, 1).unwrap().unwrap();
        assert_eq!(got.position_secs, 900, "second save should overwrite");
    }

    #[test]
    fn resume_missing_is_none() {
        let store = SqliteHistory::in_memory().unwrap();
        assert!(store.resume("nope", 1, 1).unwrap().is_none());
    }

    #[test]
    fn recent_orders_by_updated_at_desc() {
        let store = SqliteHistory::in_memory().unwrap();
        store.save(&progress("a", 1, 10, 100)).unwrap();
        store.save(&progress("b", 2, 20, 300)).unwrap();
        store.save(&progress("c", 3, 30, 200)).unwrap();
        let recent = store.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].media_key, "b"); // newest
        assert_eq!(recent[1].media_key, "c");
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        {
            let store = SqliteHistory::open(path.clone()).unwrap();
            store.save(&progress("imdb:tt9", 1, 500, 1)).unwrap();
        }
        let store = SqliteHistory::open(path).unwrap();
        assert_eq!(
            store
                .resume("imdb:tt9", 1, 1)
                .unwrap()
                .unwrap()
                .position_secs,
            500
        );
    }
}
