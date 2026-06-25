//! Progress, skip-times, list status, and rich-presence value types.
//!
//! These are consumed by the [`Tracker`], [`Enricher`], [`HistoryStore`], and
//! [`PresenceReporter`] ports. Pure data — no I/O, no policy.
//!
//! [`Tracker`]: crate::ports::Tracker
//! [`Enricher`]: crate::ports::Enricher
//! [`HistoryStore`]: crate::ports::HistoryStore
//! [`PresenceReporter`]: crate::ports::PresenceReporter

use serde::{Deserialize, Serialize};

/// A half-open time interval in whole seconds, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub start: u32,
    pub end: u32,
}

impl Interval {
    pub fn contains(&self, t: u32) -> bool {
        t >= self.start && t < self.end
    }

    /// Non-empty means we actually have skip data (start != end).
    pub fn is_meaningful(&self) -> bool {
        self.end > self.start
    }
}

/// Opening/ending skip windows for an episode (from AniSkip etc.).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SkipTimes {
    pub opening: Option<Interval>,
    pub ending: Option<Interval>,
}

impl SkipTimes {
    pub fn is_empty(&self) -> bool {
        self.opening.is_none() && self.ending.is_none()
    }
}

/// A user's watch state for a media item, mapped onto each tracker's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListStatus {
    Watching,
    Completed,
    Planning,
    Paused,
    Dropped,
    Repeating,
}

/// A persisted watch position used for resume and progress sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchProgress {
    /// Stable media key (see [`IdSet::primary_key`]).
    ///
    /// [`IdSet::primary_key`]: crate::model::IdSet::primary_key
    pub media_key: String,
    pub season: u32,
    pub episode: u32,
    /// Last known playback position, seconds.
    pub position_secs: u32,
    /// Total runtime, seconds (`0` if unknown).
    pub duration_secs: u32,
    /// Unix epoch seconds of last update.
    pub updated_at: i64,
}

impl WatchProgress {
    /// Fraction watched in `[0.0, 1.0]`.
    pub fn fraction(&self) -> f32 {
        if self.duration_secs == 0 {
            0.0
        } else {
            (self.position_secs as f32 / self.duration_secs as f32).clamp(0.0, 1.0)
        }
    }

    /// Whether this counts as "finished" for the given completion threshold
    /// (e.g. `0.85`). Mirrors curd's percentage-to-mark-complete rule.
    pub fn is_complete(&self, threshold: f32) -> bool {
        self.fraction() >= threshold
    }
}

/// A snapshot handed to the [`PresenceReporter`] (Discord rich presence).
///
/// [`PresenceReporter`]: crate::ports::PresenceReporter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub title: String,
    /// The episode line, e.g. `"S01E12 - Title"`, `"S01E12"`, or `"Movie"` (built
    /// by [`crate::title::episode_detail`]).
    pub detail: String,
    pub paused: bool,
    pub position_secs: u32,
    pub duration_secs: u32,
    pub image_url: Option<String>,
}
