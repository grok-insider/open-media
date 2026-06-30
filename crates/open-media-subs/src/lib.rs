//! `open-media-subs` — a [`SubtitleProvider`] adapter backed by the **open-subtitle**
//! engine.
//!
//! open-subtitle is a sibling Rust workspace that already knows how to talk to
//! OpenSubtitles.com/.org, SubDL, Jimaku, … score the candidates, and decode them
//! to UTF-8. This crate is a thin boundary: it builds an [`os_engine::Engine`]
//! once, translates open-media's [`SubtitleQuery`] into the engine's
//! [`os_core::Media`] + language list, calls `download_best`, and maps the
//! resulting [`os_core::SubtitleFile`]s back into open-media-core [`SubtitleTrack`]s. All
//! open-subtitle types are confined here — the rest of open-media depends only on
//! the [`SubtitleProvider`] port.
//!
//! [`SubtitleProvider`]: open_media_core::ports::SubtitleProvider

use async_trait::async_trait;

use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::MediaKind;
use open_media_core::ports::SubtitleProvider;
use open_media_core::subtitle::{SubtitleQuery, SubtitleTrack};

/// Subtitle provider that wraps the open-subtitle [`os_engine::Engine`].
///
/// The engine is built once in [`OpenSubtitleAdapter::new`] from the preferred
/// languages and reused for every [`fetch`](SubtitleProvider::fetch); it is cheap
/// to share. No credentials are required for the keyless providers the engine
/// wires by default.
pub struct OpenSubtitleAdapter {
    engine: os_engine::Engine,
}

impl OpenSubtitleAdapter {
    /// Build the underlying open-subtitle engine for the given preferred
    /// languages (priority order, e.g. `["en", "ja"]`).
    ///
    /// The languages are only a default; each [`fetch`](SubtitleProvider::fetch)
    /// uses the languages carried by its [`SubtitleQuery`]. A build failure
    /// (misconfiguration) is surfaced as [`CoreError::Config`].
    pub fn new(languages: Vec<String>) -> CoreResult<Self> {
        let cfg = os_config::Config {
            languages,
            ..Default::default()
        };
        let engine = os_compose::build_engine(&cfg).map_err(map_os_error)?;
        Ok(Self { engine })
    }
}

#[async_trait]
impl SubtitleProvider for OpenSubtitleAdapter {
    fn name(&self) -> &str {
        "open-subtitle"
    }

    async fn fetch(&self, query: &SubtitleQuery) -> CoreResult<Vec<SubtitleTrack>> {
        let os_media = to_os_media(query);
        let langs = parse_languages(&query.languages);

        if langs.is_empty() {
            // Nothing parseable to search for — not an error, just no tracks.
            tracing::debug!(
                requested = ?query.languages,
                "no parseable subtitle languages; returning empty"
            );
            return Ok(Vec::new());
        }

        let result = download_best(self.engine.clone(), os_media, langs).await?;

        match result {
            Ok(files) => Ok(files.into_iter().map(to_track).collect()),
            // No subtitle found is a soft miss for us, not a failure: the engine
            // ran fine, there just was nothing to return.
            Err(os_core::CoreError::NotFound) => Ok(Vec::new()),
            Err(e) => Err(map_os_error(e)),
        }
    }
}

/// Drive the engine's `download_best` to completion on a dedicated current-thread
/// runtime inside `spawn_blocking`, and hand back the result.
///
/// Why not just `engine.download_best(..).await` inline? The engine's internal
/// provider fan-out (`StreamExt::map` over a stream of `Arc<dyn Provider>` fed to
/// `buffer_unordered`) trips a rustc higher-ranked-lifetime inference limitation
/// ("`FnOnce` is not general enough") whenever that future is embedded in the
/// boxed, `'static`-bounded future that `#[async_trait]` desugars to. open-subtitle
/// itself only ever drives the engine from a plain `Runtime::block_on(async { … })`
/// (see its FFI/CLI), where inference succeeds. We reproduce exactly that shape:
/// the future is created and awaited entirely inside `block_on`, never crossing
/// the `async_trait` boundary. The engine is `Clone` (`Arc`-backed) and the inputs
/// are owned, so moving them into the blocking task is cheap and sound.
async fn download_best(
    engine: os_engine::Engine,
    media: os_core::Media,
    langs: Vec<os_core::Language>,
) -> CoreResult<os_core::CoreResult<Vec<os_core::SubtitleFile>>> {
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| CoreError::Other(format!("subtitle runtime build failed: {e}")))?;
        let opts = os_core::ProcessOpts::default();
        Ok(rt.block_on(engine.download_best(&media, &langs, &opts)))
    })
    .await
    .map_err(|e| CoreError::Other(format!("subtitle task failed: {e}")))?
}

/// Map an open-media-core [`SubtitleQuery`] into an open-subtitle [`os_core::Media`].
///
/// Pure (no engine, no network) so the mapping is unit-testable in isolation:
/// - [`MediaKind::Movie`], or any kind without episode coordinates, → `movie`.
/// - [`MediaKind::Anime`] with an episode → `anime` (flat-numbered, season 1).
/// - otherwise (series/anime with season+episode) → `episode`.
fn to_os_media(query: &SubtitleQuery) -> os_core::Media {
    let title = query.media.title.clone();
    match (query.media.kind, query.season, query.episode) {
        // Anime is flat-numbered: the engine's `anime` constructor pins season 1
        // and takes only the (absolute) episode number.
        (MediaKind::Anime, _, Some(episode)) => os_core::Media::anime(title, episode),
        // Episodic series with full coordinates.
        (_, Some(season), Some(episode)) => os_core::Media::episode(title, season, episode),
        // Episode number but no season → treat as flat-numbered episode 1-season.
        (_, None, Some(episode)) => os_core::Media::episode(title, 1, episode),
        // No episode coordinates → a movie (also the right shape for a whole-film
        // request regardless of declared kind).
        (_, _, None) => os_core::Media::movie(title),
    }
}

/// Parse open-media-core language tags into open-subtitle [`os_core::Language`]s,
/// dropping any the engine can't understand. Pure and order-preserving.
fn parse_languages(tags: &[String]) -> Vec<os_core::Language> {
    tags.iter()
        .filter_map(|t| os_core::Language::parse(t))
        .collect()
}

/// Map a delivered [`os_core::SubtitleFile`] into an open-media-core [`SubtitleTrack`].
///
/// The language is reported as the alpha-3 code (e.g. `eng`), the stable tag the
/// engine carries; `title` is left `None` because open-media labels tracks from
/// its own metadata, not the provider's release string.
fn to_track(file: os_core::SubtitleFile) -> SubtitleTrack {
    SubtitleTrack {
        language: file.language.alpha3,
        format: file.format,
        text: file.text,
        title: None,
    }
}

/// Map an open-subtitle [`os_core::CoreError`] into the open-media-core [`CoreError`].
///
/// Note `NotFound` is handled at the call site (it means "no subtitles", which is
/// `Ok(vec![])` for us) and so is intentionally not special-cased here.
fn map_os_error(e: os_core::CoreError) -> CoreError {
    use os_core::CoreError as Os;
    match e {
        Os::Network(m) => CoreError::Network(m),
        Os::Config(m) => CoreError::Config(m),
        Os::Parse(m) => CoreError::Parse {
            what: "open-subtitle response".to_string(),
            message: m,
        },
        Os::AuthRequired(m) => CoreError::Auth(m),
        Os::NotFound => CoreError::NotFound("no subtitles".to_string()),
        // RateLimited / DownloadLimit / Throttled / Unsupported / Io / Provider:
        // a remote/service-level failure from the engine's perspective.
        other => CoreError::Remote {
            service: "open-subtitle".to_string(),
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_media_core::model::{IdSet, Media, MediaKind};

    fn media(kind: MediaKind, title: &str) -> Media {
        Media {
            kind,
            ids: IdSet::default(),
            title: title.to_string(),
            original_title: None,
            year: None,
            score: None,
            overview: None,
            poster: None,
            genres: Vec::new(),
            status: None,
            episode_count: None,
            season_count: None,
        }
    }

    fn query(
        kind: MediaKind,
        title: &str,
        season: Option<u32>,
        episode: Option<u32>,
    ) -> SubtitleQuery {
        SubtitleQuery {
            media: media(kind, title),
            season,
            episode,
            languages: vec!["en".to_string()],
        }
    }

    #[test]
    fn movie_maps_to_movie() {
        let q = query(MediaKind::Movie, "Inception", None, None);
        let m = to_os_media(&q);
        assert_eq!(m.kind, os_core::MediaKind::Movie);
        assert_eq!(m.title, "Inception");
        assert!(m.season.is_none());
        assert!(m.episodes.is_empty());
    }

    #[test]
    fn series_with_coordinates_maps_to_episode() {
        let q = query(MediaKind::Series, "Severance", Some(2), Some(5));
        let m = to_os_media(&q);
        assert_eq!(m.kind, os_core::MediaKind::Series);
        assert_eq!(m.title, "Severance");
        assert_eq!(m.season, Some(2));
        assert_eq!(m.episodes, vec![5]);
        assert_eq!(m.coordinate().as_deref(), Some("S02E05"));
    }

    #[test]
    fn anime_with_episode_maps_to_anime_season_one() {
        let q = query(MediaKind::Anime, "Frieren", None, Some(12));
        let m = to_os_media(&q);
        assert_eq!(m.kind, os_core::MediaKind::Anime);
        assert_eq!(m.title, "Frieren");
        // anime() pins season 1 and flat-numbers the episode.
        assert_eq!(m.season, Some(1));
        assert_eq!(m.episodes, vec![12]);
    }

    #[test]
    fn anime_with_season_and_episode_still_uses_anime_constructor() {
        // Anime kind routes to the flat-numbered anime constructor even when a
        // season is present (the engine pins season 1 for anime).
        let q = query(MediaKind::Anime, "Bleach", Some(2), Some(3));
        let m = to_os_media(&q);
        assert_eq!(m.kind, os_core::MediaKind::Anime);
        assert_eq!(m.season, Some(1));
        assert_eq!(m.episodes, vec![3]);
    }

    #[test]
    fn episode_without_season_defaults_to_season_one() {
        let q = query(MediaKind::Series, "Show", None, Some(7));
        let m = to_os_media(&q);
        assert_eq!(m.kind, os_core::MediaKind::Series);
        assert_eq!(m.season, Some(1));
        assert_eq!(m.episodes, vec![7]);
    }

    #[test]
    fn parse_languages_keeps_valid_drops_invalid_and_preserves_order() {
        let tags = vec![
            "en".to_string(),
            "zzzzz".to_string(), // not a language → dropped
            "ja".to_string(),
        ];
        let langs = parse_languages(&tags);
        assert_eq!(langs.len(), 2);
        assert_eq!(langs[0].alpha3, "eng");
        assert_eq!(langs[1].alpha3, "jpn");
    }

    #[test]
    fn parse_languages_empty_when_all_invalid() {
        let tags = vec!["??".to_string(), "".to_string()];
        assert!(parse_languages(&tags).is_empty());
    }

    #[test]
    fn not_found_maps_to_core_not_found() {
        let mapped = map_os_error(os_core::CoreError::NotFound);
        assert!(matches!(mapped, CoreError::NotFound(_)));
    }

    #[test]
    fn network_error_maps_to_network() {
        let mapped = map_os_error(os_core::CoreError::Network("dns".to_string()));
        assert!(matches!(mapped, CoreError::Network(m) if m == "dns"));
    }

    #[test]
    fn provider_error_maps_to_remote() {
        let mapped = map_os_error(os_core::CoreError::Provider("boom".to_string()));
        match mapped {
            CoreError::Remote { service, message } => {
                assert_eq!(service, "open-subtitle");
                assert!(message.contains("boom"));
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn subtitle_file_maps_to_track_with_alpha3_language() {
        let file = os_core::SubtitleFile {
            language: os_core::Language::parse("en").unwrap(),
            format: "srt".to_string(),
            text: "1\n00:00:01,000 --> 00:00:02,000\nHi\n".to_string(),
            provider: "opensubtitles".to_string(),
            release: Some("Some.Release.1080p".to_string()),
            hi: false,
            forced: false,
        };
        let track = to_track(file);
        assert_eq!(track.language, "eng");
        assert_eq!(track.format, "srt");
        assert!(track.text.contains("Hi"));
        assert!(track.title.is_none());
    }
}
