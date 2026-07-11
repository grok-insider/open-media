//! Candidate scoring — pure, deterministic, unit-tested.
//!
//! After every [`SourceProvider`] has returned its [`SourceCandidate`]s, the
//! application layer merges them and ranks them here. Keeping this logic in core
//! (not in an adapter) means the ranking is identical regardless of which
//! providers were used, and it can be tested without any network.
//!
//! The weighting reflects real streaming priorities, in order of dominance:
//! 1. **Cached** on the debrid service (instant playback) — when the user wants it.
//! 2. **Quality** — rank, with a bonus for hitting the exact target.
//! 3. **Health** — seeders (matters for P2P / uncached) and language preference.
//! 4. Tie-breakers: codec efficiency, then seeders, then smaller size.
//!
//! [`SourceProvider`]: crate::ports::SourceProvider

use crate::stream::{CacheState, Quality, SourceCandidate};

/// User preferences that shape ranking.
#[derive(Debug, Clone)]
pub struct ScoringPrefs {
    /// Strongly prefer debrid-cached candidates (instant HTTPS start when available).
    pub prefer_cached: bool,
    /// Desired quality tier; an exact match earns a bonus.
    pub target_quality: Option<Quality>,
    /// Reward HEVC/AV1 (smaller for the same quality).
    pub prefer_efficient_codec: bool,
    /// Lowercased language names to reward when present.
    pub preferred_languages: Vec<String>,
    /// Soft floor on seeders; candidates below are penalized, not removed.
    pub min_seeders: Option<u32>,
}

impl Default for ScoringPrefs {
    fn default() -> Self {
        Self {
            prefer_cached: true,
            target_quality: Some(Quality::P1080),
            prefer_efficient_codec: false,
            preferred_languages: Vec::new(),
            min_seeders: None,
        }
    }
}

/// Score a single candidate. Higher is better. Pure function of its inputs.
pub fn score_candidate(c: &SourceCandidate, prefs: &ScoringPrefs) -> i64 {
    let mut score: i64 = 0;

    // 1. Cache state dominates when the user prefers cached playback.
    match c.cache {
        CacheState::Cached if prefs.prefer_cached => score += 100_000,
        CacheState::Uncached if prefs.prefer_cached => score -= 5_000,
        _ => {}
    }
    // Unconditional cached tiebreak: even when the user does not strongly prefer
    // cached, an instantly-available source should edge out an otherwise-equal
    // uncached one. Kept tiny (< every other weight, and far below the
    // prefer_cached bonus) so it only decides genuine ties.
    if c.cache == CacheState::Cached {
        score += 50;
    }

    // 2. Quality rank, plus an exact-target bonus.
    score += (c.quality.rank() as i64) * 1_000;
    if let Some(tq) = prefs.target_quality {
        if c.quality == tq {
            score += 5_000;
        }
    }

    // 3. Health + language.
    if let Some(seeders) = c.seeders {
        score += seeders.min(2_000) as i64;
    }
    if let Some(min) = prefs.min_seeders {
        if c.seeders.unwrap_or(0) < min {
            score -= 2_000;
        }
    }
    if !prefs.preferred_languages.is_empty()
        && c.tags.languages.iter().any(|l| {
            prefs
                .preferred_languages
                .iter()
                .any(|p| p.eq_ignore_ascii_case(l))
        })
    {
        score += 800;
    }

    // 4. Tie-breakers.
    if prefs.prefer_efficient_codec {
        if let Some(v) = &c.tags.video_codec {
            if v.eq_ignore_ascii_case("HEVC") || v.eq_ignore_ascii_case("AV1") {
                score += 300;
            }
        }
    }

    score
}

/// Rank candidates best-first, in place. Stable on ties via seeders then size.
pub fn rank(candidates: &mut [SourceCandidate], prefs: &ScoringPrefs) {
    candidates.sort_by(|a, b| {
        let sa = score_candidate(a, prefs);
        let sb = score_candidate(b, prefs);
        sb.cmp(&sa)
            .then_with(|| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
            .then_with(|| a.size_bytes.cmp(&b.size_bytes))
    });
}

/// Drop candidates whose release title shares **no** significant tokens with
/// `media_title`.
///
/// Torrentio (and similar id-keyed scrapers) occasionally return multi-file or
/// mis-tagged torrents under the right IMDB id — e.g. a *Handmaid's Tale* pack
/// under Mushoku Tensei's `tt…` path. Those can still be **debrid-cached**, and
/// [`score_candidate`]'s huge cached bonus would rank them above real uncached
/// releases. Filtering by title tokens before rank closes that hole.
///
/// Returns how many candidates were removed. If `media_title` has no usable
/// tokens (empty / only noise / only very short words), nothing is dropped.
pub fn filter_title_mismatch(candidates: &mut Vec<SourceCandidate>, media_title: &str) -> usize {
    let media_tokens = significant_title_tokens(media_title);
    if media_tokens.is_empty() {
        return 0;
    }
    let before = candidates.len();
    candidates.retain(|c| title_matches_media(media_title, &c.title));
    before.saturating_sub(candidates.len())
}

/// Whether the candidate release name is acceptable for `media_title`.
///
/// Only drops candidates that look like a **real release title** with zero
/// token overlap. Sparse stubs (`ep=1`, quality-only tags) are kept so we do
/// not over-filter legitimately terse provider output.
pub fn title_matches_media(media_title: &str, candidate_title: &str) -> bool {
    let media_tokens = significant_title_tokens(media_title);
    if media_tokens.is_empty() {
        return true;
    }
    // No significant tokens in the release name → nothing to contradict the
    // media title (coordinate/quality-only labels).
    if significant_title_tokens(candidate_title).is_empty() {
        return true;
    }
    title_shares_media_token(&media_tokens, candidate_title)
}

fn title_shares_media_token(media_tokens: &[String], candidate_title: &str) -> bool {
    let haystack = normalize_title(candidate_title);
    media_tokens.iter().any(|t| haystack.contains(t.as_str()))
}

/// Significant tokens from a human media title (for matching release names).
fn significant_title_tokens(title: &str) -> Vec<String> {
    normalize_title(title)
        .split_whitespace()
        .filter(|t| t.len() >= 3 && !is_noise_token(t))
        .map(str::to_string)
        .collect()
}

/// Lowercase, replace non-alphanumerics with spaces (keeps `s03e01`-style tokens
/// intact as single tokens when digits/letters mix).
fn normalize_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out
}

fn is_noise_token(t: &str) -> bool {
    matches!(
        t,
        // Articles / conjunctions
        "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "into"
            // Release-quality / container noise common in torrent names
            | "1080p"
            | "2160p"
            | "720p"
            | "480p"
            | "360p"
            | "web"
            | "webdl"
            | "webrip"
            | "bluray"
            | "bdrip"
            | "hdtv"
            | "hdrip"
            | "proper"
            | "repack"
            | "extended"
            | "remux"
            | "multi"
            | "subs"
            | "multisubs"
            | "dual"
            | "audio"
            | "hevc"
            | "x264"
            | "x265"
            | "h264"
            | "h265"
            | "avc"
            | "av1"
            | "aac"
            | "ac3"
            | "dts"
            | "truehd"
            | "atmos"
            | "hdr"
            | "sdr"
            | "10bit"
            | "8bit"
            | "nf"
            | "amzn"
            | "dsnp"
            | "hulu"
            | "cr"
            | "bili"
            | "batch"
            | "complete"
            | "season"
            | "episode"
            | "series"
            | "movie"
            | "mkv"
            | "mp4"
            | "avi"
    ) || is_episode_coordinate_token(t)
}

/// `s01e02`, `s03e001`, bare `e01`, `ep01`.
fn is_episode_coordinate_token(t: &str) -> bool {
    let b = t.as_bytes();
    if b.len() >= 4 && b[0] == b's' {
        // sNN…eNN…
        if let Some(epos) = t.find('e') {
            if epos > 1
                && t[1..epos].chars().all(|c| c.is_ascii_digit())
                && !t[epos + 1..].is_empty()
                && t[epos + 1..].chars().all(|c| c.is_ascii_digit())
            {
                return true;
            }
        }
    }
    if (t.starts_with("ep") || t.starts_with('e')) && t.len() <= 5 {
        let rest = t.trim_start_matches("ep").trim_start_matches('e');
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Index of the recommended candidate (best score), or `None` if empty.
pub fn recommended_index(candidates: &[SourceCandidate], prefs: &ScoringPrefs) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .max_by_key(|(_, c)| score_candidate(c, prefs))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::ReleaseTags;

    fn candidate(quality: Quality, cache: CacheState, seeders: Option<u32>) -> SourceCandidate {
        SourceCandidate {
            provider: "test".into(),
            title: "Test.Release".into(),
            quality,
            size_bytes: 1_000_000_000,
            seeders,
            info_hash: Some("0".repeat(40)),
            magnet: None,
            direct_url: None,
            file_index: None,
            cache,
            tags: ReleaseTags::default(),
        }
    }

    #[test]
    fn cached_beats_uncached_when_preferred() {
        let prefs = ScoringPrefs::default();
        let cached = candidate(Quality::P720, CacheState::Cached, Some(1));
        let uncached = candidate(Quality::P2160, CacheState::Uncached, Some(9999));
        assert!(score_candidate(&cached, &prefs) > score_candidate(&uncached, &prefs));
    }

    #[test]
    fn target_quality_bonus_applied() {
        let prefs = ScoringPrefs {
            prefer_cached: false,
            target_quality: Some(Quality::P1080),
            ..Default::default()
        };
        let target = candidate(Quality::P1080, CacheState::Unknown, Some(10));
        let higher = candidate(Quality::P2160, CacheState::Unknown, Some(10));
        // Exact target (1080p) beats 2160p here because the +5000 target bonus
        // outweighs the single-rank difference.
        assert!(score_candidate(&target, &prefs) > score_candidate(&higher, &prefs));
    }

    #[test]
    fn rank_orders_best_first() {
        let prefs = ScoringPrefs::default();
        let mut list = vec![
            candidate(Quality::P480, CacheState::Uncached, Some(5)),
            candidate(Quality::P1080, CacheState::Cached, Some(2)),
            candidate(Quality::P720, CacheState::Cached, Some(50)),
        ];
        rank(&mut list, &prefs);
        // Both cached; 1080p target bonus puts it first.
        assert_eq!(list[0].quality, Quality::P1080);
        assert_eq!(list[2].cache, CacheState::Uncached);
        assert_eq!(recommended_index(&list, &prefs), Some(0));
    }

    #[test]
    fn cached_breaks_ties_when_not_preferred() {
        let prefs = ScoringPrefs {
            prefer_cached: false,
            target_quality: None,
            ..Default::default()
        };
        // Two otherwise-identical candidates: only the cache state differs.
        let cached = candidate(Quality::P1080, CacheState::Cached, Some(10));
        let uncached = candidate(Quality::P1080, CacheState::Uncached, Some(10));
        assert!(score_candidate(&cached, &prefs) > score_candidate(&uncached, &prefs));

        let mut list = vec![uncached, cached];
        rank(&mut list, &prefs);
        assert_eq!(list[0].cache, CacheState::Cached);
        assert_eq!(recommended_index(&list, &prefs), Some(0));
    }

    #[test]
    fn min_seeders_penalty() {
        let prefs = ScoringPrefs {
            prefer_cached: false,
            min_seeders: Some(10),
            target_quality: None,
            ..Default::default()
        };
        let healthy = candidate(Quality::P1080, CacheState::Unknown, Some(100));
        let starved = candidate(Quality::P1080, CacheState::Unknown, Some(1));
        assert!(score_candidate(&healthy, &prefs) > score_candidate(&starved, &prefs));
    }

    fn titled(title: &str, cache: CacheState) -> SourceCandidate {
        let mut c = candidate(Quality::P1080, cache, Some(1));
        c.title = title.into();
        c
    }

    #[test]
    fn title_match_keeps_matching_release() {
        assert!(title_matches_media(
            "Mushoku Tensei: Jobless Reincarnation",
            "[Erai-raws] Mushoku Tensei S03E01 [1080p CR WEB-DL AVC AAC][MultiSub]"
        ));
        assert!(title_matches_media(
            "Rick and Morty",
            "Rick.and.Morty.S09E07.1080p.WEB.h264"
        ));
    }

    #[test]
    fn title_match_rejects_handmaids_under_mushoku() {
        // Live Torrentio pollution: multi-file Handmaid pack under Mushoku's tt id.
        assert!(!title_matches_media(
            "Mushoku Tensei: Jobless Reincarnation",
            "The.Handmaids.Tale.1080p.Multi.Subs.AAC.x265-DLKING\n\
             The.Handmaids.Tale.S03E001.1080p.Multi.Subs.AAC.x265-DLKING.mkv\n\
             👤 1 💾 1.84 GB ⚙️ ThePirateBay"
        ));
    }

    #[test]
    fn filter_drops_cached_title_mismatch_before_rank() {
        let media = "Mushoku Tensei: Jobless Reincarnation";
        let mut list = vec![
            titled(
                "The.Handmaids.Tale.1080p.Multi.Subs.AAC.x265-DLKING",
                CacheState::Cached,
            ),
            titled(
                "[ToonsHub] Mushoku Tensei Jobless Reincarnation S03E01 1080p",
                CacheState::Uncached,
            ),
            titled(
                "[Erai-raws] Mushoku Tensei S03E01 [1080p]",
                CacheState::Uncached,
            ),
        ];
        let dropped = filter_title_mismatch(&mut list, media);
        assert_eq!(dropped, 1);
        assert_eq!(list.len(), 2);
        assert!(list
            .iter()
            .all(|c| c.title.to_ascii_lowercase().contains("mushoku")));

        // After filter, rank must not resurrect the junk — and uncached Mushoku
        // rows remain available (cached junk is gone, so prefer_cached cannot
        // promote Handmaid's over real releases).
        let prefs = ScoringPrefs::default();
        rank(&mut list, &prefs);
        assert!(list[0].title.to_ascii_lowercase().contains("mushoku"));
    }

    #[test]
    fn filter_keeps_all_when_media_title_has_no_tokens() {
        let mut list = vec![titled("Anything.At.All.1080p", CacheState::Cached)];
        assert_eq!(filter_title_mismatch(&mut list, ""), 0);
        assert_eq!(filter_title_mismatch(&mut list, "Up"), 0); // too short
        assert_eq!(list.len(), 1);
    }
}
