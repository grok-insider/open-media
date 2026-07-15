//! Best-effort episode-count discovery from release titles.
//!
//! When metadata APIs cannot enumerate a season (airing OVAs, null episode
//! counts), index results still carry `- 12` / `S01E12` / batch ranges. This
//! pure scanner extracts a ceiling so the UI can list playable episode numbers.
//!
//! Intentionally lightweight (no season-pack graph): the sources crate has a
//! richer parser for filtering; this only answers "how high does the numbering
//! go for this season?".

/// Hard cap against false positives (years, bitrates).
const EP_MAX: u32 = 500;

/// Scan release titles and return the highest plausible episode number for
/// `season` (1-based), or `None` if nothing parseable was found.
pub fn discover_max_episode<'a, I>(titles: I, season: u32) -> Option<u32>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut max = 0u32;
    for title in titles {
        if looks_like_wrong_season(title, season) {
            continue;
        }
        for n in extract_episode_numbers(title) {
            if (1..=EP_MAX).contains(&n) {
                max = max.max(n);
            }
        }
    }
    (max > 0).then_some(max)
}

/// Explicit markers for a *different* season than the one requested.
fn looks_like_wrong_season(title: &str, season: u32) -> bool {
    let t = title.to_ascii_lowercase();
    // Multi-season complete packs are unusable as a per-season ceiling.
    if t.contains("s01-s") || t.contains("seasons 1-") || t.contains("s1-s") {
        return true;
    }
    // Look for S{N} / Season N / Nth Season that is not our season.
    for n in 1u32..=20 {
        if n == season {
            continue;
        }
        let s_tag = format!("s{n:02}");
        let s_tag2 = format!("s{n}");
        let season_word = format!("season {n}");
        let nth = match n {
            1 => "1st season",
            2 => "2nd season",
            3 => "3rd season",
            k => {
                // only check common ordinals above
                if k > 5 {
                    continue;
                }
                ""
            }
        };
        if t.contains(&s_tag)
            || t.contains(&format!(" {s_tag2} "))
            || t.contains(&format!(" {s_tag2}-"))
            || t.contains(&season_word)
            || (!nth.is_empty() && t.contains(nth))
        {
            // Allow if our season is also marked (rare dual tag).
            let ours = format!("s{season:02}");
            let ours2 = format!("season {season}");
            if t.contains(&ours) || t.contains(&ours2) {
                continue;
            }
            return true;
        }
    }
    // Roman II/III for season 1 requests.
    if season == 1
        && (t.contains(" ii ")
            || t.contains(" ii-")
            || t.contains(" iii ")
            || t.contains(" 2nd season")
            || t.contains(" second season"))
    {
        return true;
    }
    false
}

fn extract_episode_numbers(title: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let bytes = title.as_bytes();
    let lower = title.to_ascii_lowercase();
    let b = lower.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        // SxxEyy / s01e05
        if i + 5 < n && b[i] == b's' {
            let mut j = i + 1;
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < n && b[j] == b'e' {
                let mut k = j + 1;
                while k < n && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k > j + 1 {
                    if let Ok(ep) = lower[j + 1..k].parse::<u32>() {
                        out.push(ep);
                    }
                    i = k;
                    continue;
                }
            }
        }
        // " - 12" / " - 01 ~ 28" / " - 01-12"
        if i + 2 < n && b[i] == b'-' && b[i + 1] == b' ' {
            let mut j = i + 2;
            while j < n && b[j] == b'0' {
                j += 1;
            }
            let start = j;
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                if let Ok(a) = lower[start..j].parse::<u32>() {
                    out.push(a);
                    // optional range end
                    let mut k = j;
                    while k < n && b[k] == b' ' {
                        k += 1;
                    }
                    if k < n && (b[k] == b'~' || b[k] == b'-') {
                        k += 1;
                        while k < n && b[k] == b' ' {
                            k += 1;
                        }
                        let end_start = k;
                        while k < n && b[k].is_ascii_digit() {
                            k += 1;
                        }
                        if k > end_start {
                            if let Ok(end) = lower[end_start..k].parse::<u32>() {
                                out.push(end);
                            }
                        }
                    }
                    // "to" / "a" range after space
                    let rest = &lower[j..];
                    if let Some(end) = parse_word_range_end(rest) {
                        out.push(end);
                    }
                }
                i = j;
                continue;
            }
        }
        // "#12" / "Ep 12" / "Episode 12"
        if i + 2 < n && b[i] == b'#' && b[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            if let Ok(ep) = lower[i + 1..j].parse::<u32>() {
                out.push(ep);
            }
            i = j;
            continue;
        }
        if lower[i..].starts_with("ep ") || lower[i..].starts_with("episode ") {
            let skip = if lower[i..].starts_with("episode ") {
                8
            } else {
                3
            };
            let mut j = i + skip;
            while j < n && b[j] == b' ' {
                j += 1;
            }
            let start = j;
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                if let Ok(ep) = lower[start..j].parse::<u32>() {
                    out.push(ep);
                }
            }
            i = j;
            continue;
        }
        // CJK 第N話 — byte-scan for UTF-8 第 (E7 AC AC)
        if title[i..].starts_with('第') {
            let rest = &title[i + '第'.len_utf8()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                if let Ok(ep) = digits.parse::<u32>() {
                    let after = &rest[digits.len()..];
                    if after.starts_with('話') || after.starts_with('话') || after.starts_with('集')
                    {
                        out.push(ep);
                    }
                }
            }
        }
        let _ = bytes; // silence unused if any
        i += 1;
    }
    out
}

fn parse_word_range_end(rest: &str) -> Option<u32> {
    let rest = rest.trim_start();
    for sep in ["to ", "a ", "& ", "+ "] {
        if let Some(tail) = rest.strip_prefix(sep) {
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_from_single_and_batch() {
        let titles = [
            "[SubsPlease] Frieren - 01 (1080p)",
            "[SubsPlease] Frieren - 12 (1080p)",
            "[Batch] Frieren - 01 ~ 28 (1080p)",
        ];
        assert_eq!(discover_max_episode(titles, 1), Some(28));
    }

    #[test]
    fn skips_wrong_season_releases() {
        let titles = [
            "[X] Show S2 - 12 (1080p)",
            "[X] Show 2nd Season - 05",
            "[X] Show - 03 (1080p)",
        ];
        assert_eq!(discover_max_episode(titles, 1), Some(3));
    }

    #[test]
    fn sxxeyy_and_ep_prefix() {
        assert_eq!(
            discover_max_episode(["Show.S01E07.1080p", "Show Ep 09"], 1),
            Some(9)
        );
    }
}
