//! Human-facing title formatting.
//!
//! Pure string builders shared by the playback path (mpv `--force-media-title`)
//! and Discord rich presence, so both render an episode identically and degrade
//! the same way when data is missing. Living in `open-media-core` keeps the format in one
//! place without either consumer depending on the other (DIP).
//!
//! Formats:
//! - episodic + episode title → `Series - S01E01 - The Hated Classmate`
//! - episodic, no episode title → `Series - S01E01`
//! - movie (with year) → `Series (2022)`
//! - movie (no year) → `Series`

use crate::model::Media;

/// The full title for the player window / mpv `media-title`.
///
/// `season`/`episode` are the resolved coordinates (anime is flat-numbered with
/// `season == 1`). `episode_title` is the episode name when the metadata provider
/// supplied one (often `None` for AniList anime). Movies never get a coordinate.
pub fn media_title(
    media: &Media,
    season: u32,
    episode: u32,
    episode_title: Option<&str>,
) -> String {
    let base = media.display_title();
    if media.kind.is_episodic() {
        match episode_title {
            Some(t) if !t.is_empty() => format!("{base} - {} - {t}", coordinate(season, episode)),
            _ => format!("{base} - {}", coordinate(season, episode)),
        }
    } else if let Some(year) = media.year {
        format!("{base} ({year})")
    } else {
        base.to_string()
    }
}

/// The shorter "what am I watching" line for Discord presence `state`, paired
/// with the series name shown separately as `details`.
///
/// - episodic + title → `S01E01 - The Hated Classmate`
/// - episodic, no title → `S01E01`
/// - movie → `Movie`
pub fn episode_detail(
    media: &Media,
    season: u32,
    episode: u32,
    episode_title: Option<&str>,
) -> String {
    if media.kind.is_episodic() {
        match episode_title {
            Some(t) if !t.is_empty() => format!("{} - {t}", coordinate(season, episode)),
            _ => coordinate(season, episode),
        }
    } else {
        "Movie".to_string()
    }
}

/// `S{season:02}E{episode:02}` — the same coordinate form sources use.
fn coordinate(season: u32, episode: u32) -> String {
    format!("S{season:02}E{episode:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{IdSet, MediaKind};

    fn media(kind: MediaKind, title: &str, year: Option<i32>) -> Media {
        Media {
            kind,
            ids: IdSet::default(),
            title: title.to_string(),
            original_title: None,
            year,
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
    fn episodic_with_episode_title() {
        let m = media(MediaKind::Series, "The Eminence in Shadow", Some(2022));
        assert_eq!(
            media_title(&m, 1, 1, Some("The Hated Classmate")),
            "The Eminence in Shadow - S01E01 - The Hated Classmate"
        );
    }

    #[test]
    fn episodic_without_episode_title() {
        let m = media(MediaKind::Series, "The Eminence in Shadow", Some(2022));
        assert_eq!(
            media_title(&m, 1, 1, None),
            "The Eminence in Shadow - S01E01"
        );
        // Empty string is treated as missing, not appended.
        assert_eq!(
            media_title(&m, 1, 1, Some("")),
            "The Eminence in Shadow - S01E01"
        );
    }

    #[test]
    fn anime_is_flat_numbered_but_still_coordinated() {
        let m = media(MediaKind::Anime, "Frieren", Some(2023));
        assert_eq!(media_title(&m, 1, 5, None), "Frieren - S01E05");
        assert_eq!(
            media_title(&m, 1, 5, Some("Privilege of the Brave")),
            "Frieren - S01E05 - Privilege of the Brave"
        );
    }

    #[test]
    fn movie_with_year_has_no_coordinate() {
        let m = media(MediaKind::Movie, "Interstellar", Some(2014));
        assert_eq!(media_title(&m, 1, 1, None), "Interstellar (2014)");
        // Even if a stray episode title leaks in, a movie never gets S01E01.
        assert_eq!(
            media_title(&m, 1, 1, Some("ignored")),
            "Interstellar (2014)"
        );
    }

    #[test]
    fn movie_without_year_is_bare_title() {
        let m = media(MediaKind::Movie, "Untitled Film", None);
        assert_eq!(media_title(&m, 1, 1, None), "Untitled Film");
    }

    #[test]
    fn double_digit_coordinates_pad_correctly() {
        let m = media(MediaKind::Series, "Show", None);
        assert_eq!(media_title(&m, 12, 24, None), "Show - S12E24");
    }

    #[test]
    fn detail_forms() {
        let series = media(MediaKind::Series, "Show", None);
        assert_eq!(
            episode_detail(&series, 1, 1, Some("Pilot")),
            "S01E01 - Pilot"
        );
        assert_eq!(episode_detail(&series, 2, 3, None), "S02E03");
        let movie = media(MediaKind::Movie, "Film", Some(2000));
        assert_eq!(episode_detail(&movie, 1, 1, None), "Movie");
    }
}
