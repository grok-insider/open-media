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
    /// Strongly prefer debrid-cached candidates (instant start, no seeding).
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
}
