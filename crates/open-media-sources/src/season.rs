//! Anime season disambiguation for nyaa.
//!
//! AniList models each season of an anime as a **separate, flat-numbered entry**
//! (S1 episodes 1..20, S2 episodes 1..12 under its own id), and reports a single
//! synthetic "Season 1" for each. So the engine can't tell seasons apart by
//! number — the only reliable signal is the **title** ("2nd Season" / "Season 2"
//! / "S2" / a trailing roman numeral).
//!
//! Without this, a nyaa search for `"<base> 01"` returns episode-1 releases from
//! *every* season (`- 01`, `S2 - 01`, `2nd Season - 01`). These helpers extract a
//! season ordinal from the selected entry's title, and classify each release by
//! the season(s) it covers so the wrong season can be filtered out.
//!
//! Handled: explicit numeric markers (`S2`, `Season 2`, `2nd Season`), multi-season
//! ranges (`S01-S05`, `Seasons 1-5`), and trailing roman numerals (`… II`). Also
//! absolute episode numbering (a sequel numbered `… - 21`): [`release_episode`]
//! parses the episode coordinate so a marker-less release whose number is the
//! franchise-continuous one can be matched, given an offset from AniList relations
//! (`MetadataProvider::episode_offset`, applied in `om-app`).

use std::sync::LazyLock;

use regex::Regex;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

// --- Patterns over a raw *metadata* title (case-insensitive). ---
re!(M_NTH_SEASON, r"(?i)(\d+)(?:st|nd|rd|th)\s+season");
re!(M_SEASON_N, r"(?i)season\s+0*(\d+)");
re!(M_S_N, r"(?i)\bs0*(\d+)\b");
re!(M_ROMAN_END, r"(?i)\s+(viii|vii|vi|iv|ix|iii|ii|x|v)\s*$");

// --- Patterns over a *normalized* release decoration (already lowercased; `-`
// and `~` kept so ranges survive). ---
re!(R_S_RANGE, r"s0*(\d+)\s*[-~]\s*s0*(\d+)");
// Plural "seasons" always denotes a range ("Seasons 1-5").
re!(R_SEASON_RANGE, r"seasons\s+0*(\d+)\s*[-~]\s*0*(\d+)");
// Singular "Season N-M" is a range too ("Season 1-5"), but only when the second
// number is a *bare* season number (`5`), not a zero-padded episode coordinate.
// "Season 2 - 01" is *S2 episode 1*: the `01` form excludes it, so it falls
// through to the single-season match below as Single(2).
re!(
    R_SEASON_RANGE_SINGULAR,
    r"season\s+0*(\d+)\s*[-~]\s*([1-9]\d*)\b"
);
re!(R_NTH_SEASON, r"(\d+)(?:st|nd|rd|th)\s+season");
re!(R_SEASON_N, r"season\s+0*(\d+)");
re!(R_S_N, r"\bs0*(\d+)\b");
// A bare ordinal in the decoration ("Kagejitsu 2nd #01-12", "… 2nd Cour") — in an
// anime release the franchise name is the base, so a trailing "2nd"/"3rd" is a
// sequel marker even without the word "Season".
re!(R_NTH_BARE, r"\b(\d+)(?:st|nd|rd|th)\b");
re!(R_ROMAN_START, r"^\s*(viii|vii|vi|iv|ix|iii|ii|x|v)\b");

// Episode coordinate in the decoration: a `-`/`~`/`#`-introduced number, with an
// optional `~`/`-` range end (`- 21`, `- 01 ~ 20`, `#01-12`). Anchored on a
// `-`/`~`/`#` lead so a bare resolution/year token elsewhere isn't read as the
// episode. 1–4 digits covers absolute counts without matching `1080`/`2024`,
// which are not `-`-led in a release name.
re!(R_EPISODE, r"[-~#]\s*0*(\d{1,4})(?:\s*[-~]\s*0*(\d{1,4}))?");

/// Which season(s) a release covers. `None` means no marker → treated as the
/// first season (the bare `- 01` convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeasonMatch {
    None,
    Single(u32),
    Range(u32, u32),
}

impl SeasonMatch {
    /// Whether this release is relevant to the requested season ordinal.
    pub fn covers(&self, ordinal: u32) -> bool {
        match self {
            SeasonMatch::None => ordinal == 1,
            SeasonMatch::Single(n) => *n == ordinal,
            SeasonMatch::Range(a, b) => *a <= ordinal && ordinal <= *b,
        }
    }
}

/// Split a metadata title into `(base name, season ordinal)`.
///
/// The base is the title up to the first season marker (so the nyaa query targets
/// the franchise, not one season's naming); the ordinal is which sequel it is
/// (1 when unmarked). Examples:
/// - `"Kage no Jitsuryokusha ni Naritakute! 2nd Season"` → `("Kage no Jitsuryokusha ni Naritakute!", 2)`
/// - `"The Eminence in Shadow Season 2"` → `("The Eminence in Shadow", 2)`
/// - `"Mob Psycho 100 II"` → `("Mob Psycho 100", 2)`
/// - `"Mob Psycho 100"` → `("Mob Psycho 100", 1)` (the `100` is not a season)
pub fn parse_title_season(title: &str) -> (String, u32) {
    let mut best: Option<(usize, u32)> = None;
    let mut consider = |start: usize, ord: u32| {
        if best.is_none_or(|(b, _)| start < b) {
            best = Some((start, ord));
        }
    };
    for re in [&M_NTH_SEASON, &M_SEASON_N, &M_S_N] {
        if let Some(c) = re.captures(title) {
            let start = c.get(0).unwrap().start();
            let ord = c[1].parse().unwrap_or(1);
            consider(start, ord);
        }
    }
    if let Some(c) = M_ROMAN_END.captures(title) {
        let start = c.get(0).unwrap().start();
        consider(start, roman_to_u32(&c[1]));
    }
    match best {
        Some((start, ord)) => (title[..start].trim().to_string(), ord),
        None => (title.trim().to_string(), 1),
    }
}

/// Classify a release title by the season(s) it covers, anchored on the known
/// `base` name so tokens inside the franchise name (e.g. the `100` in
/// `Mob Psycho 100`) and trailing roman numerals are interpreted correctly.
pub fn release_season(release_title: &str, base: &str) -> SeasonMatch {
    let rel = normalize(release_title);
    let base_n = normalize(base);
    // Look only at what follows the base name (the "decoration"); fall back to the
    // whole string if the base can't be located (e.g. an English-named release).
    let decoration = match base_n.is_empty() {
        true => rel.as_str(),
        false => match rel.find(&base_n) {
            Some(pos) => &rel[pos + base_n.len()..],
            None => rel.as_str(),
        },
    };

    // Ranges first (a single-season regex would otherwise match the low end).
    for re in [&R_S_RANGE, &R_SEASON_RANGE, &R_SEASON_RANGE_SINGULAR] {
        if let Some(c) = re.captures(decoration) {
            if let (Ok(a), Ok(b)) = (c[1].parse::<u32>(), c[2].parse::<u32>()) {
                return SeasonMatch::Range(a.min(b), a.max(b));
            }
        }
    }
    for re in [&R_NTH_SEASON, &R_SEASON_N, &R_S_N, &R_NTH_BARE] {
        if let Some(c) = re.captures(decoration) {
            if let Ok(n) = c[1].parse::<u32>() {
                return SeasonMatch::Single(n);
            }
        }
    }
    if let Some(c) = R_ROMAN_START.captures(decoration) {
        return SeasonMatch::Single(roman_to_u32(&c[1]));
    }
    SeasonMatch::None
}

/// Which episode(s) a release covers, parsed from its decoration. Used only to
/// recognize an *absolute-numbered* sequel release (a no-season-marker entry
/// whose episode number is the franchise-continuous one, e.g. `… - 21`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeMatch {
    /// No episode coordinate found (e.g. a season-pack named only `Complete`).
    None,
    /// A single episode (`… - 21`).
    Single(u32),
    /// An inclusive episode batch (`… - 01 ~ 20`).
    Range(u32, u32),
}

impl EpisodeMatch {
    /// Whether this release's episode coverage includes `n`.
    pub fn covers(&self, n: u32) -> bool {
        match self {
            EpisodeMatch::None => false,
            EpisodeMatch::Single(e) => *e == n,
            EpisodeMatch::Range(a, b) => *a <= n && n <= *b,
        }
    }
}

/// Extract the episode coordinate from a release title, anchored on the known
/// `base` name (so a number inside the franchise name isn't read as the episode).
///
/// This exists for absolute-numbering: when a sequel is numbered continuously
/// (`[SubsPlease] <base> - 21`), the episode token `21` is the only signal that
/// it's the requested absolute episode, since it carries no season marker. The
/// caller pairs this with [`release_season`] returning [`SeasonMatch::None`].
pub fn release_episode(release_title: &str, base: &str) -> EpisodeMatch {
    let rel = normalize(release_title);
    let base_n = normalize(base);
    let decoration = match base_n.is_empty() {
        true => rel.as_str(),
        false => match rel.find(&base_n) {
            Some(pos) => &rel[pos + base_n.len()..],
            None => rel.as_str(),
        },
    };
    match R_EPISODE.captures(decoration) {
        Some(c) => {
            let a: u32 = match c[1].parse() {
                Ok(v) => v,
                Err(_) => return EpisodeMatch::None,
            };
            match c.get(2).and_then(|m| m.as_str().parse::<u32>().ok()) {
                Some(b) => EpisodeMatch::Range(a.min(b), a.max(b)),
                None => EpisodeMatch::Single(a),
            }
        }
        None => EpisodeMatch::None,
    }
}

/// Lowercase; keep alphanumerics plus `-`/`~` (so season/episode ranges survive);
/// collapse everything else to single spaces.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '~' {
            out.extend(ch.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn roman_to_u32(s: &str) -> u32 {
    match s.to_ascii_lowercase().as_str() {
        "ii" => 2,
        "iii" => 3,
        "iv" => 4,
        "v" => 5,
        "vi" => 6,
        "vii" => 7,
        "viii" => 8,
        "ix" => 9,
        "x" => 10,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_season_numeric_and_words() {
        assert_eq!(
            parse_title_season("Kage no Jitsuryokusha ni Naritakute! 2nd Season"),
            ("Kage no Jitsuryokusha ni Naritakute!".to_string(), 2)
        );
        assert_eq!(
            parse_title_season("The Eminence in Shadow Season 2"),
            ("The Eminence in Shadow".to_string(), 2)
        );
        assert_eq!(
            parse_title_season("Some Anime S2"),
            ("Some Anime".to_string(), 2)
        );
    }

    #[test]
    fn title_season_roman_suffix() {
        assert_eq!(
            parse_title_season("Mob Psycho 100 II"),
            ("Mob Psycho 100".to_string(), 2)
        );
        assert_eq!(
            parse_title_season("Sword Art Online III"),
            ("Sword Art Online".to_string(), 3)
        );
    }

    #[test]
    fn title_season_defaults_to_one_and_keeps_base_numbers() {
        // Numbers that are part of the name must NOT be read as seasons.
        assert_eq!(
            parse_title_season("Mob Psycho 100"),
            ("Mob Psycho 100".to_string(), 1)
        );
        assert_eq!(parse_title_season("86"), ("86".to_string(), 1));
        assert_eq!(
            parse_title_season("Steins;Gate 0"),
            ("Steins;Gate 0".to_string(), 1)
        );
        assert_eq!(
            parse_title_season("Kage no Jitsuryokusha ni Naritakute!"),
            ("Kage no Jitsuryokusha ni Naritakute!".to_string(), 1)
        );
    }

    const BASE: &str = "Kage no Jitsuryokusha ni Naritakute!";

    #[test]
    fn release_no_marker_is_season_one() {
        let m = release_season(
            "[SubsPlease] Kage no Jitsuryokusha ni Naritakute! - 01 (1080p)",
            BASE,
        );
        assert_eq!(m, SeasonMatch::None);
        assert!(m.covers(1));
        assert!(!m.covers(2));
    }

    #[test]
    fn release_s2_variants_are_season_two() {
        for t in [
            "[SubsPlease] Kage no Jitsuryokusha ni Naritakute! S2 - 01 (1080p)",
            "[Erai-raws] Kage no Jitsuryokusha ni Naritakute! 2nd Season - 01 ~ 12 [1080p][BATCH]",
            "Kage no Jitsuryokusha ni Naritakute! Season 2 - 01",
        ] {
            let m = release_season(t, BASE);
            assert!(m.covers(2), "{t} should cover S2, got {m:?}");
            assert!(!m.covers(1), "{t} should not cover S1, got {m:?}");
        }
    }

    #[test]
    fn release_explicit_season_one_marker() {
        let m = release_season(
            "[Anime Time] Kage no Jitsuryokusha ni Naritakute! (Season 01) [1080p] The Eminence in Shadow",
            BASE,
        );
        assert_eq!(m, SeasonMatch::Single(1));
        assert!(m.covers(1));
    }

    #[test]
    fn release_multi_season_batch_covers_range() {
        let m = release_season(
            "[Group] Kage no Jitsuryokusha ni Naritakute! S01-S05 1080p Complete",
            BASE,
        );
        assert_eq!(m, SeasonMatch::Range(1, 5));
        assert!(m.covers(1));
        assert!(m.covers(2));
        assert!(!m.covers(6));
    }

    #[test]
    fn release_singular_season_range_covers_range() {
        // Singular "Season N-M" (with or without spaces around the dash) is a
        // multi-season batch, same as "Seasons 1-5" / "S01-S05".
        for t in [
            "[Group] Kage no Jitsuryokusha ni Naritakute! Season 1-5 1080p Complete",
            "[Group] Kage no Jitsuryokusha ni Naritakute! Season 1 - 5 1080p Complete",
        ] {
            let m = release_season(t, BASE);
            assert_eq!(m, SeasonMatch::Range(1, 5), "{t}");
            assert!(m.covers(1));
            assert!(m.covers(5));
            assert!(!m.covers(6));
        }
    }

    #[test]
    fn release_singular_season_then_episode_stays_single() {
        // "Season 2 - 01": the zero-padded `01` is an episode coordinate, not a
        // range end, so this must remain Single(2) — not a "2 to 1" range.
        let m = release_season(
            "[SubsPlease] Kage no Jitsuryokusha ni Naritakute! Season 2 - 01 (1080p)",
            BASE,
        );
        assert_eq!(m, SeasonMatch::Single(2));
        assert!(m.covers(2));
        assert!(!m.covers(1));
    }

    #[test]
    fn release_plural_seasons_range_covers_range() {
        let m = release_season(
            "[Group] Kage no Jitsuryokusha ni Naritakute! Seasons 1-5 1080p Complete",
            BASE,
        );
        assert_eq!(m, SeasonMatch::Range(1, 5));
        assert!(m.covers(3));
        assert!(!m.covers(6));
    }

    #[test]
    fn release_roman_only_after_base() {
        // Roman numeral counts only in the decoration, not inside the base name.
        let m2 = release_season("[X] Mob Psycho 100 II - 01 [1080p]", "Mob Psycho 100");
        assert_eq!(m2, SeasonMatch::Single(2));
        let m1 = release_season("[X] Mob Psycho 100 - 01 [1080p]", "Mob Psycho 100");
        assert_eq!(m1, SeasonMatch::None);
        assert!(m1.covers(1));
    }

    #[test]
    fn release_bare_ordinal_shorthand_is_season_two() {
        // "Kagejitsu 2nd #01-12" — a sequel batch with a nickname + bare ordinal,
        // no "Season" word.
        let m = release_season(
            "[Anime-Releases] Kage no Jitsuryokusha ni Naritakute! - Kagejitsu 2nd #01-12 [complete]",
            BASE,
        );
        assert_eq!(m, SeasonMatch::Single(2));
        assert!(!m.covers(1));
        assert!(m.covers(2));
    }

    #[test]
    fn release_episode_coordinate_not_misread_as_season() {
        // A bare "- 01 ~ 20" episode batch is season-less, not a season range.
        let m = release_season(
            "[Erai-raws] Kage no Jitsuryokusha ni Naritakute! - 01 ~ 20 [1080p]",
            BASE,
        );
        assert_eq!(m, SeasonMatch::None);
        assert!(m.covers(1));
    }

    #[test]
    fn release_episode_parses_single_and_range() {
        // Single absolute episode of a continuously-numbered sequel.
        assert_eq!(
            release_episode("[SubsPlease] Frieren - 21 (1080p) [ABCD].mkv", "Frieren"),
            EpisodeMatch::Single(21)
        );
        // A batch range.
        assert_eq!(
            release_episode("[Erai-raws] Frieren - 01 ~ 20 [1080p][Batch]", "Frieren"),
            EpisodeMatch::Range(1, 20)
        );
        // Leading zeros are stripped, not octal-parsed.
        assert_eq!(
            release_episode("[X] Frieren - 09 (1080p)", "Frieren"),
            EpisodeMatch::Single(9)
        );
    }

    #[test]
    fn release_episode_ignores_numbers_in_base_and_resolution() {
        // The `100` is inside the base; the only episode token is `- 21`. A bare
        // `1080p` (not `-`-led) must not be read as the episode.
        assert_eq!(
            release_episode("[X] Mob Psycho 100 - 21 (1080p)", "Mob Psycho 100"),
            EpisodeMatch::Single(21)
        );
    }

    #[test]
    fn release_episode_none_when_no_coordinate() {
        assert_eq!(
            release_episode("[Group] Frieren Complete Batch [1080p]", "Frieren"),
            EpisodeMatch::None
        );
    }

    #[test]
    fn episode_match_covers() {
        assert!(EpisodeMatch::Single(21).covers(21));
        assert!(!EpisodeMatch::Single(21).covers(20));
        assert!(EpisodeMatch::Range(1, 20).covers(20));
        assert!(!EpisodeMatch::Range(1, 20).covers(21));
        assert!(!EpisodeMatch::None.covers(1));
    }
}
