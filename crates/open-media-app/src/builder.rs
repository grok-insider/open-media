use std::sync::Arc;

use open_media_core::ports::{
    Enricher, HistoryStore, IdBridge, LibraryStore, MetadataProvider, Player, PresenceReporter,
    SourceProvider, StreamResolver, SubtitleProvider, Tracker,
};
use open_media_core::scoring::ScoringPrefs;

use crate::Engine;

/// Builder for [`Engine`]. The composition root adds whichever adapters the
/// user's config selects; unset capabilities simply disable their features.
#[derive(Default)]
pub struct EngineBuilder {
    metadata: Vec<Arc<dyn MetadataProvider>>,
    sources: Vec<Arc<dyn SourceProvider>>,
    resolver: Option<Arc<dyn StreamResolver>>,
    player: Option<Arc<dyn Player>>,
    tracker: Option<Arc<dyn Tracker>>,
    enricher: Option<Arc<dyn Enricher>>,
    history: Option<Arc<dyn HistoryStore>>,
    library: Option<Arc<dyn LibraryStore>>,
    presence: Option<Arc<dyn PresenceReporter>>,
    subtitles: Option<Arc<dyn SubtitleProvider>>,
    subtitle_languages: Vec<String>,
    id_bridge: Option<Arc<dyn IdBridge>>,
    prefs: ScoringPrefs,
    complete_threshold: f32,
    skip_filler: bool,
    autoplay_next: bool,
    resume: Option<bool>,
}

impl EngineBuilder {
    pub fn add_metadata(mut self, p: Arc<dyn MetadataProvider>) -> Self {
        self.metadata.push(p);
        self
    }
    pub fn add_source(mut self, p: Arc<dyn SourceProvider>) -> Self {
        self.sources.push(p);
        self
    }
    pub fn resolver(mut self, r: Arc<dyn StreamResolver>) -> Self {
        self.resolver = Some(r);
        self
    }
    pub fn player(mut self, p: Arc<dyn Player>) -> Self {
        self.player = Some(p);
        self
    }
    pub fn tracker(mut self, t: Arc<dyn Tracker>) -> Self {
        self.tracker = Some(t);
        self
    }
    pub fn enricher(mut self, e: Arc<dyn Enricher>) -> Self {
        self.enricher = Some(e);
        self
    }
    pub fn history(mut self, h: Arc<dyn HistoryStore>) -> Self {
        self.history = Some(h);
        self
    }
    pub fn library(mut self, l: Arc<dyn LibraryStore>) -> Self {
        self.library = Some(l);
        self
    }
    pub fn presence(mut self, p: Arc<dyn PresenceReporter>) -> Self {
        self.presence = Some(p);
        self
    }
    /// External subtitle provider (optional). When set, [`Engine::play`] fetches
    /// subtitles before launching the player and passes them as `--sub-file=`.
    pub fn subtitles(mut self, s: Arc<dyn SubtitleProvider>) -> Self {
        self.subtitles = Some(s);
        self
    }
    /// Preferred subtitle languages (most-wanted first, e.g. `["en", "ja"]`) used
    /// for the subtitle search. Default empty; only meaningful with a
    /// [`subtitles`](Self::subtitles) provider wired.
    pub fn subtitle_languages(mut self, languages: Vec<String>) -> Self {
        self.subtitle_languages = languages;
        self
    }
    /// AniList/MAL → IMDB bridge (optional). When set, [`Engine::find_sources`]
    /// fills a missing `ids.imdb` for anime before querying sources, so the
    /// IMDB-keyed providers (Torrentio → debrid) can serve them. Without it, anime
    /// keep their anime-native (nyaa) sources.
    pub fn id_bridge(mut self, bridge: Arc<dyn IdBridge>) -> Self {
        self.id_bridge = Some(bridge);
        self
    }
    pub fn scoring_prefs(mut self, prefs: ScoringPrefs) -> Self {
        self.prefs = prefs;
        self
    }
    /// Fraction watched at which an episode is marked complete (default 0.85).
    pub fn complete_threshold(mut self, threshold: f32) -> Self {
        self.complete_threshold = threshold;
        self
    }
    /// Skip filler/recap episodes when auto-advancing (default false). Only has an
    /// effect when an [`Enricher`] is also wired.
    pub fn skip_filler(mut self, skip: bool) -> Self {
        self.skip_filler = skip;
        self
    }
    /// Auto-advance to the next episode after a completed episodic playback
    /// (binge mode; default false).
    pub fn autoplay_next(mut self, enabled: bool) -> Self {
        self.autoplay_next = enabled;
        self
    }
    /// Seek to the saved resume position when starting playback (default true).
    /// When `false`, playback always starts from the beginning, but progress is
    /// still recorded to the [`HistoryStore`] for later sessions.
    pub fn resume(mut self, enabled: bool) -> Self {
        self.resume = Some(enabled);
        self
    }

    pub fn build(self) -> Engine {
        Engine {
            metadata: self.metadata,
            sources: self.sources,
            resolver: self.resolver,
            player: self.player,
            tracker: self.tracker,
            enricher: self.enricher,
            history: self.history,
            library: self.library,
            presence: self.presence,
            subtitles: self.subtitles,
            subtitle_languages: self.subtitle_languages,
            id_bridge: self.id_bridge,
            prefs: self.prefs,
            complete_threshold: if self.complete_threshold > 0.0 {
                self.complete_threshold
            } else {
                0.85
            },
            skip_filler: self.skip_filler,
            autoplay_next: self.autoplay_next,
            resume: self.resume.unwrap_or(true),
        }
    }
}
