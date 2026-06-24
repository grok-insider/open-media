//! # om-history
//!
//! [`HistoryStore`] implementation: persists [`WatchProgress`] so the engine can
//! resume an episode at its last position and show a "continue watching" list.
//!
//! - [`SqliteHistory`] — a single SQLite file under the user's data dir. Chosen
//!   over curd's full-file-rewrite CSV because per-row upserts are concurrency-safe
//!   and cheap on every position tick.
//!
//! Scaffold stub: the [`HistoryStore`] contract is implemented; bodies return
//! [`CoreError::NotImplemented`] until Phase 7 (see `docs/ROADMAP.md`).
//!
//! [`HistoryStore`]: om_core::ports::HistoryStore
//! [`WatchProgress`]: om_core::tracking::WatchProgress

use std::path::PathBuf;

use om_core::error::{CoreError, CoreResult};
use om_core::ports::HistoryStore;
use om_core::tracking::WatchProgress;

/// SQLite-backed watch history.
pub struct SqliteHistory {
    #[allow(dead_code)]
    db_path: PathBuf,
}

impl SqliteHistory {
    /// Open (and later, in Phase 7, migrate) the history DB at `db_path`.
    pub fn open(db_path: PathBuf) -> CoreResult<Self> {
        Ok(Self { db_path })
    }

    /// Default path: `~/.local/share/open-media/history.db`.
    pub fn default_path() -> PathBuf {
        dirs_data_dir().join("open-media").join("history.db")
    }
}

impl HistoryStore for SqliteHistory {
    fn save(&self, _progress: &WatchProgress) -> CoreResult<()> {
        Err(CoreError::NotImplemented("history.save"))
    }

    fn resume(
        &self,
        _media_key: &str,
        _season: u32,
        _episode: u32,
    ) -> CoreResult<Option<WatchProgress>> {
        Err(CoreError::NotImplemented("history.resume"))
    }

    fn recent(&self, _limit: usize) -> CoreResult<Vec<WatchProgress>> {
        Err(CoreError::NotImplemented("history.recent"))
    }
}

/// Minimal XDG data dir without pulling `dirs` into this crate.
fn dirs_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
}
