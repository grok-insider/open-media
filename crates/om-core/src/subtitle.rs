//! Subtitle value types: what a subtitle search is asked for and what it returns.
//!
//! These are **metadata-only**. open-media plays a *stream URL* (debrid direct
//! link or local P2P), not a file on disk, so there is no path or content hash to
//! key off — subtitle search is by title + season/episode coordinates, the same
//! dialect the [`SourceProvider`] already speaks. A [`SubtitleProvider`] takes a
//! [`SubtitleQuery`] and returns [`SubtitleTrack`]s carrying the decoded subtitle
//! text, ready to hand to the player.
//!
//! [`SourceProvider`]: crate::ports::SourceProvider
//! [`SubtitleProvider`]: crate::ports::SubtitleProvider

use crate::model::Media;

/// A delivered subtitle, decoded and ready to play.
///
/// `text` is the full UTF-8 subtitle document (e.g. SubRip/WebVTT), already
/// decoded by the adapter — the core stays I/O- and encoding-free. There is
/// deliberately no file path: open-media plays a stream, so a track is identified
/// by its `language`/`format`/`title`, not a location on disk.
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    /// Language tag for the track (e.g. `"en"`, `"eng"`), as the source reports it.
    pub language: String,
    /// Subtitle container/format (e.g. `"srt"`, `"vtt"`, `"ass"`).
    pub format: String,
    /// The full subtitle document as UTF-8 text.
    pub text: String,
    /// Optional human label for the track (release name, "Forced", …).
    pub title: Option<String>,
}

/// What a [`SubtitleProvider`] is being asked to find.
///
/// Metadata-only: the query carries the [`Media`] plus optional season/episode
/// coordinates (`None`/`None` for a movie) and the preferred `languages`. It
/// intentionally has no file path or hash — see the module docs.
///
/// [`SubtitleProvider`]: crate::ports::SubtitleProvider
#[derive(Debug, Clone)]
pub struct SubtitleQuery {
    pub media: Media,
    /// `None` for movies; `Some` for episodic content.
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// Preferred language tags, most-wanted first (e.g. `["en", "ja"]`).
    pub languages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IdSet, MediaKind};

    fn sample_media() -> Media {
        Media {
            kind: MediaKind::Series,
            ids: IdSet::default(),
            title: "Frieren".to_string(),
            original_title: None,
            year: Some(2023),
            score: None,
            overview: None,
            poster: None,
            genres: Vec::new(),
            status: None,
            episode_count: None,
            season_count: None,
        }
    }

    #[test]
    fn constructs_query_and_track() {
        let query = SubtitleQuery {
            media: sample_media(),
            season: Some(1),
            episode: Some(1),
            languages: vec!["en".to_string()],
        };
        assert_eq!(query.season, Some(1));
        assert_eq!(query.languages, vec!["en".to_string()]);

        let track = SubtitleTrack {
            language: "en".to_string(),
            format: "srt".to_string(),
            text: "1\n00:00:01,000 --> 00:00:02,000\nHello\n".to_string(),
            title: Some("Frieren - 01".to_string()),
        };
        assert_eq!(track.language, "en");
        assert_eq!(track.format, "srt");
        assert!(track.text.contains("Hello"));
        assert_eq!(track.title.as_deref(), Some("Frieren - 01"));
    }
}
