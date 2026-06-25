//! # om-core
//!
//! The **domain core** of open-media: value types, the port traits every adapter
//! implements, and pure ranking logic. This crate has **no I/O** — no HTTP, no
//! disk, no process spawning — and therefore no heavy dependencies. Everything
//! that talks to the outside world lives in an adapter crate behind a port
//! defined here.
//!
//! ## Layout
//! - [`error`] — the unified [`CoreError`]/[`CoreResult`] returned by all ports.
//! - [`model`] — identity ([`MediaId`], [`IdSet`]) and media ([`Media`],
//!   [`Season`], [`Episode`]).
//! - [`stream`] — [`SourceCandidate`] (a found file) and [`Playback`] (a resolved,
//!   player-openable URL), plus [`Quality`]/[`CacheState`].
//! - [`tracking`] — [`SkipTimes`], [`WatchProgress`], [`ListStatus`], [`Activity`].
//! - [`title`] — pure human-facing title formatting (mpv media-title + presence).
//! - [`ports`] — the trait boundaries: [`MetadataProvider`], [`SourceProvider`],
//!   [`DebridProvider`], [`StreamResolver`], [`Player`]/[`PlaybackControl`],
//!   [`Tracker`], [`Enricher`], [`HistoryStore`], [`PresenceReporter`].
//! - [`scoring`] — pure, tested candidate ranking.
//!
//! See `docs/ARCHITECTURE.md` for how these compose into the playback pipeline.
//!
//! [`CoreError`]: error::CoreError
//! [`CoreResult`]: error::CoreResult
//! [`MediaId`]: model::MediaId
//! [`IdSet`]: model::IdSet
//! [`Media`]: model::Media
//! [`Season`]: model::Season
//! [`Episode`]: model::Episode
//! [`SourceCandidate`]: stream::SourceCandidate
//! [`Playback`]: stream::Playback
//! [`Quality`]: stream::Quality
//! [`CacheState`]: stream::CacheState
//! [`SkipTimes`]: tracking::SkipTimes
//! [`WatchProgress`]: tracking::WatchProgress
//! [`ListStatus`]: tracking::ListStatus
//! [`Activity`]: tracking::Activity
//! [`MetadataProvider`]: ports::MetadataProvider
//! [`SourceProvider`]: ports::SourceProvider
//! [`DebridProvider`]: ports::DebridProvider
//! [`StreamResolver`]: ports::StreamResolver
//! [`Player`]: ports::Player
//! [`PlaybackControl`]: ports::PlaybackControl
//! [`Tracker`]: ports::Tracker
//! [`Enricher`]: ports::Enricher
//! [`HistoryStore`]: ports::HistoryStore
//! [`PresenceReporter`]: ports::PresenceReporter

pub mod error;
pub mod model;
pub mod ports;
pub mod scoring;
pub mod stream;
pub mod title;
pub mod tracking;

pub use error::{CoreError, CoreResult};
