//! Release-name parsing: titles/filenames → [`Quality`] + [`ReleaseTags`] +
//! cache/seeders/size. Adapted from miru's well-tested Torrentio parser, but
//! producing `om-core` types so scoring and the UI stay format-agnostic.
//!
//! Torrentio encodes metadata across a two-line `name`/`title` pair with emoji
//! (`👤` seeders, `💾` size, flag emoji for languages, `[RD+]`/`⚡` for cache).
//! nyaa filenames have no emoji, so [`parse_release_name`] handles the
//! quality/codec subset there.

use std::sync::LazyLock;

use open_media_core::stream::{CacheState, Quality, ReleaseTags};
use regex::Regex;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(SEEDERS_RE, r"👤\s*(\d+)");
re!(SIZE_RE, r"💾\s*([\d.]+)\s*(GiB|MiB|TiB|GB|MB|TB)");
re!(QUALITY_RE, r"\b(2160p|4K|1080p|720p|480p|360p)\b");
re!(
    HDR_RE,
    r"(?i)\b(HDR10\+|HDR10|DoVi|DV|Dolby[\s.]?Vision|HDR)\b"
);
re!(
    VIDEO_CODEC_RE,
    r"(?i)\b(HEVC|x265|x264|AVC|AV1|H\.?265|H\.?264|VC-1)\b"
);
re!(
    AUDIO_RE,
    r"(?i)(DTS-HD[\s.]?MA|TrueHD|Atmos|DTS|AAC|FLAC|EAC3|E-AC-3|AC3|DD\+|DD|LPCM)"
);
re!(AUDIO_CHANNELS_RE, r"\b([257]\.[01])\b");
re!(
    SOURCE_RE,
    r"(?i)\b(UHD[\s.]?BluRay|BluRay|Blu-Ray|BDRip|BRRip|WEB-DL|WEBDL|WEBRip|REMUX|HDTV|DVDRip)\b"
);
re!(
    LANG_FLAGS_RE,
    r"(🇬🇧|🇺🇸|🇩🇪|🇫🇷|🇮🇹|🇪🇸|🇯🇵|🇰🇷|🇨🇳|🇧🇷|🇵🇹|🇷🇺|🇳🇱|🇵🇱|🇸🇪|🇳🇴|🇩🇰|🇫🇮|🇬🇷|🇹🇷|🇮🇳|🇹🇭|🇻🇳|🇮🇩|🇲🇽|🇦🇷)"
);

/// Known Torrentio sub-trackers, as `(needle_lowercase, canonical_name)` pairs.
/// Keyless Torrentio streams name the originating tracker only in the release
/// title (not in a `[...]` debrid tag), so we scan for these to give the TUI
/// "Provider" filter something more useful than the literal "torrentio".
/// Order matters: longer / more specific aliases come first so e.g. "tpb"
/// doesn't shadow "thepiratebay" and "kat" is tried alongside the full name.
const KNOWN_TRACKERS: &[(&str, &str)] = &[
    ("1337x", "1337x"),
    ("rarbg", "RARBG"),
    ("yify", "YTS"),
    ("yts", "YTS"),
    ("thepiratebay", "ThePirateBay"),
    ("tpb", "ThePirateBay"),
    ("eztv", "EZTV"),
    ("kickasstorrents", "KickassTorrents"),
    ("kat", "KickassTorrents"),
    ("magnetdl", "MagnetDL"),
    ("horriblesubs", "HorribleSubs"),
    ("nyaasi", "NyaaSi"),
    ("nyaa", "NyaaSi"),
    ("torrentgalaxy", "TorrentGalaxy"),
];

/// Scan free text (a release title, or non-bracket parts of `name`) for a known
/// Torrentio tracker token, returning its canonical name. Case-insensitive,
/// whole-word so an alias like "kat" doesn't match inside another word.
fn detect_tracker(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric();
    for (needle, canonical) in KNOWN_TRACKERS {
        let mut start = 0;
        while let Some(pos) = lower[start..].find(needle) {
            let at = start + pos;
            let before_ok = at == 0 || !is_word(bytes[at - 1]);
            let after = at + needle.len();
            let after_ok = after >= bytes.len() || !is_word(bytes[after]);
            if before_ok && after_ok {
                return Some(canonical);
            }
            start = at + 1;
        }
    }
    None
}

fn flag_to_language(flag: &str) -> &'static str {
    match flag {
        "🇬🇧" | "🇺🇸" => "English",
        "🇩🇪" => "German",
        "🇫🇷" => "French",
        "🇮🇹" => "Italian",
        "🇪🇸" | "🇲🇽" | "🇦🇷" => "Spanish",
        "🇯🇵" => "Japanese",
        "🇰🇷" => "Korean",
        "🇨🇳" => "Chinese",
        "🇧🇷" | "🇵🇹" => "Portuguese",
        "🇷🇺" => "Russian",
        "🇳🇱" => "Dutch",
        "🇵🇱" => "Polish",
        "🇸🇪" => "Swedish",
        "🇳🇴" => "Norwegian",
        "🇩🇰" => "Danish",
        "🇫🇮" => "Finnish",
        "🇬🇷" => "Greek",
        "🇹🇷" => "Turkish",
        "🇮🇳" => "Hindi",
        "🇹🇭" => "Thai",
        "🇻🇳" => "Vietnamese",
        "🇮🇩" => "Indonesian",
        _ => "Unknown",
    }
}

/// Everything parsed out of a Torrentio `name`/`title` pair.
#[derive(Debug, Clone)]
pub struct ParsedRelease {
    pub provider: String,
    pub quality: Quality,
    pub size_bytes: u64,
    pub seeders: Option<u32>,
    pub cache: CacheState,
    pub tags: ReleaseTags,
}

/// Parse a Torrentio stream's `name` + `title`.
pub fn parse_torrentio(name: &str, title: &str) -> ParsedRelease {
    let combined = format!("{name}\n{title}");

    // Cache: a `[XX+]` debrid flag (`[RD+]` Real-Debrid, `[AD+]` AllDebrid,
    // `[PM+]` Premiumize, `[TB+]` Torbox) or `⚡` means instantly available.
    let cache = if name.contains("[RD+]")
        || name.contains("[AD+]")
        || name.contains("[PM+]")
        || name.contains("[TB+]")
        || name.contains('⚡')
    {
        CacheState::Cached
    } else if name.contains("[RD download]") || name.contains("[RD]") {
        CacheState::Uncached
    } else {
        CacheState::Unknown
    };

    // Provider is the trailing segment of `name` after the `[...]` tag. Without
    // a closing bracket there is no provider segment to extract, so yield empty
    // (→ falls back to "torrentio") rather than echoing the whole release name.
    let provider = name
        .rsplit_once(']')
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let quality = QUALITY_RE
        .find(&combined)
        .map(|m| Quality::from_label(m.as_str()))
        .unwrap_or(Quality::Unknown);

    let size_bytes = SIZE_RE
        .captures(title)
        .map(|c| parse_size_to_bytes(&format!("{} {}", &c[1], &c[2])))
        .unwrap_or(0);

    let seeders = SEEDERS_RE.captures(title).and_then(|c| c[1].parse().ok());

    let languages: Vec<String> = {
        let mut seen = Vec::new();
        for m in LANG_FLAGS_RE.find_iter(title) {
            let lang = flag_to_language(m.as_str()).to_string();
            if !seen.contains(&lang) {
                seen.push(lang);
            }
        }
        seen
    };

    let tags = ReleaseTags {
        video_codec: parse_video_codec(&combined),
        audio: parse_audio(&combined),
        hdr: parse_hdr(&combined),
        source_type: SOURCE_RE
            .find(&combined)
            .map(|m| normalize_source(m.as_str())),
        languages,
    };

    // Keyless streams have no `[...]` debrid tag, so `provider` is empty here.
    // Before defaulting to the literal "torrentio", try to recover the real
    // sub-tracker (1337x/RARBG/YTS/…) from the release title — and from any
    // non-bracket text in `name` — so the TUI Provider filter stays useful
    // without a debrid token. The bracket path above still wins when present.
    let provider = if provider.is_empty() {
        let name_no_brackets = name.replace(['[', ']'], " ");
        detect_tracker(title)
            .or_else(|| detect_tracker(&name_no_brackets))
            .map(|t| t.to_string())
            .unwrap_or_else(|| "torrentio".to_string())
    } else {
        provider
    };

    ParsedRelease {
        provider,
        quality,
        size_bytes,
        seeders,
        cache,
        tags,
    }
}

/// Parse a bare release name/filename (nyaa, no emoji): quality + codec tags.
pub fn parse_release_name(name: &str) -> (Quality, ReleaseTags) {
    let quality = QUALITY_RE
        .find(name)
        .map(|m| Quality::from_label(m.as_str()))
        .unwrap_or(Quality::Unknown);
    let tags = ReleaseTags {
        video_codec: parse_video_codec(name),
        audio: parse_audio(name),
        hdr: parse_hdr(name),
        source_type: SOURCE_RE.find(name).map(|m| normalize_source(m.as_str())),
        languages: Vec::new(),
    };
    (quality, tags)
}

/// Parse a human size like `"1.2 GB"` / `"800 MB"` / `"1.4 GiB"` into bytes.
pub fn parse_size_to_bytes(size_str: &str) -> u64 {
    let parts: Vec<&str> = size_str.split_whitespace().collect();
    if parts.len() != 2 {
        return 0;
    }
    let value: f64 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let mult: f64 = match parts[1].to_uppercase().as_str() {
        "TB" | "TIB" => 1024f64.powi(4),
        "GB" | "GIB" => 1024f64.powi(3),
        "MB" | "MIB" => 1024f64.powi(2),
        "KB" | "KIB" => 1024f64,
        _ => return 0,
    };
    (value * mult) as u64
}

fn parse_hdr(text: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in HDR_RE.find_iter(text) {
        let v = match cap.as_str().to_uppercase().replace(['.', ' '], "").as_str() {
            "DOVI" | "DV" | "DOLBYVISION" => "DV",
            "HDR10+" => "HDR10+",
            "HDR10" => "HDR10",
            "HDR" => "HDR",
            _ => continue,
        };
        if !out.iter().any(|x| x == v) {
            out.push(v.to_string());
        }
    }
    (!out.is_empty()).then(|| out.join(" / "))
}

fn parse_video_codec(text: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in VIDEO_CODEC_RE.find_iter(text) {
        let v = match cap.as_str().to_uppercase().replace('.', "").as_str() {
            "HEVC" | "H265" | "X265" => "HEVC",
            "AVC" | "H264" | "X264" => "AVC",
            "AV1" => "AV1",
            "VC-1" => "VC-1",
            _ => continue,
        };
        if !out.iter().any(|x| x == v) {
            out.push(v.to_string());
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

fn parse_audio(text: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    // Capture every distinct audio codec, not just the first (a release can
    // carry e.g. both `TrueHD` and `AC3` tracks).
    for m in AUDIO_RE.find_iter(text) {
        let c = m.as_str().to_uppercase();
        let norm = if c.contains("DTS-HD") || c.contains("DTS HD") {
            "DTS-HD MA"
        } else if c.contains("TRUEHD") {
            "TrueHD"
        } else if c.contains("ATMOS") {
            "Atmos"
        } else if c.contains("EAC3") || c.contains("E-AC-3") || c.contains("DD+") {
            "EAC3"
        } else if c.contains("AC3") || c.contains("DD") {
            "AC3"
        } else if c.contains("AAC") {
            "AAC"
        } else if c.contains("FLAC") {
            "FLAC"
        } else if c.contains("DTS") {
            "DTS"
        } else if c.contains("LPCM") {
            "LPCM"
        } else {
            ""
        };
        if !norm.is_empty() && !parts.iter().any(|p| p == norm) {
            parts.push(norm.to_string());
        }
    }
    if text.to_uppercase().contains("ATMOS") && !parts.iter().any(|p| p == "Atmos") {
        parts.push("Atmos".to_string());
    }
    if let Some(m) = AUDIO_CHANNELS_RE.find(text) {
        parts.push(m.as_str().to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn normalize_source(source: &str) -> String {
    match source.to_uppercase().replace(['-', ' ', '.'], "").as_str() {
        "UHDBLURAY" => "UHD BluRay".to_string(),
        "BLURAY" => "BluRay".to_string(),
        "BDRIP" | "BRRIP" => "BDRip".to_string(),
        "WEBDL" => "WEB-DL".to_string(),
        "WEBRIP" => "WEBRip".to_string(),
        "REMUX" => "REMUX".to_string(),
        "HDTV" => "HDTV".to_string(),
        "DVDRIP" => "DVDRip".to_string(),
        _ => source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cached_1080p_torrentio_stream() {
        let p = parse_torrentio(
            "[RD+] nyaasi",
            "Frieren S01E01 1080p WEB x264\n👤 150 💾 1.2 GB",
        );
        assert_eq!(p.provider, "nyaasi");
        assert_eq!(p.quality, Quality::P1080);
        assert_eq!(p.cache, CacheState::Cached);
        assert_eq!(p.seeders, Some(150));
        assert_eq!(p.size_bytes, parse_size_to_bytes("1.2 GB"));
        assert_eq!(p.tags.video_codec.as_deref(), Some("AVC"));
    }

    #[test]
    fn parses_remux_with_hdr_audio_languages() {
        let p = parse_torrentio(
            "Torrentio\n4k DV | HDR",
            "Movie.2024.2160p.UHD.BluRay.REMUX.HEVC.DTS-HD.MA.7.1-GRP\n👤 25 💾 45.5 GB\n🇬🇧 / 🇩🇪",
        );
        assert_eq!(p.quality, Quality::P2160);
        assert_eq!(p.tags.hdr.as_deref(), Some("DV / HDR"));
        assert_eq!(p.tags.video_codec.as_deref(), Some("HEVC"));
        assert_eq!(p.tags.audio.as_deref(), Some("DTS-HD MA 7.1"));
        assert_eq!(p.tags.source_type.as_deref(), Some("UHD BluRay"));
        assert!(p.tags.languages.contains(&"English".to_string()));
        assert!(p.tags.languages.contains(&"German".to_string()));
    }

    #[test]
    fn uncached_detection() {
        let p = parse_torrentio("[RD download] 1337x", "Show 720p\n👤 5 💾 600 MB");
        assert_eq!(p.cache, CacheState::Uncached);
        assert_eq!(p.quality, Quality::P720);
    }

    #[test]
    fn size_parsing_handles_units() {
        assert_eq!(parse_size_to_bytes("1 GB"), 1024 * 1024 * 1024);
        assert_eq!(parse_size_to_bytes("800 MB"), 800 * 1024 * 1024);
        assert_eq!(
            parse_size_to_bytes("1.4 GiB"),
            (1.4 * 1024f64.powi(3)) as u64
        );
        assert_eq!(parse_size_to_bytes("garbage"), 0);
    }

    #[test]
    fn bare_filename_parse_for_nyaa() {
        let (q, tags) = parse_release_name("[SubsPlease] Frieren - 01 (1080p) [HEVC].mkv");
        assert_eq!(q, Quality::P1080);
        assert_eq!(tags.video_codec.as_deref(), Some("HEVC"));
    }

    #[test]
    fn bit_depth_does_not_pollute_video_codec() {
        // `10bit` / `10-bit` must not be folded into the codec field.
        let (_, tags) = parse_release_name("Show 1080p x265 10bit HEVC");
        assert_eq!(tags.video_codec.as_deref(), Some("HEVC"));
        let (_, tags) = parse_release_name("Show 1080p AV1 10-bit");
        assert_eq!(tags.video_codec.as_deref(), Some("AV1"));
    }

    #[test]
    fn captures_multiple_distinct_audio_codecs() {
        let (_, tags) = parse_release_name("Movie.2024.1080p.BluRay.TrueHD.AC3-GRP");
        let audio = tags.audio.as_deref().unwrap();
        assert!(audio.contains("TrueHD"), "got {audio:?}");
        assert!(audio.contains("AC3"), "got {audio:?}");
    }

    #[test]
    fn provider_without_bracket_is_not_garbage() {
        // No `[...]` tag → no provider segment → falls back to "torrentio",
        // never echoes the release name back as the provider.
        let p = parse_torrentio("Frieren S01E01 1080p WEB x264", "👤 10 💾 1.2 GB");
        assert_eq!(p.provider, "torrentio");
    }

    #[test]
    fn keyless_provider_recovered_from_tracker_in_title() {
        // No debrid `[...]` tag, but the title names the real tracker → use it
        // as the provider instead of the literal "torrentio".
        let p = parse_torrentio(
            "Frieren S01E01 1080p WEB x264",
            "Frieren.S01E01.1080p.WEB.x264-RARBG\n👤 10 💾 1.2 GB",
        );
        assert_eq!(p.provider, "RARBG");

        // YIFY/YTS alias canonicalizes to "YTS".
        let p = parse_torrentio(
            "Movie 2024 1080p",
            "Movie.2024.1080p.BluRay.x264-YIFY\n👤 80 💾 2.0 GB",
        );
        assert_eq!(p.provider, "YTS");
    }

    #[test]
    fn debrid_bracket_provider_still_wins_over_title_tracker() {
        // The `[RD+] 1337x` bracket path must keep winning even if the title
        // mentions a different tracker.
        let p = parse_torrentio(
            "[RD+] 1337x",
            "Show.S01E01.1080p.WEB.x264-RARBG\n👤 5 💾 1 GB",
        );
        assert_eq!(p.provider, "1337x");
    }

    #[test]
    fn keyless_without_known_tracker_falls_back_to_torrentio() {
        let p = parse_torrentio(
            "Frieren S01E01 1080p WEB x264",
            "Frieren.S01E01.1080p.WEB.x264-NONAME\n👤 10 💾 1.2 GB",
        );
        assert_eq!(p.provider, "torrentio");
    }

    #[test]
    fn recognizes_alldebrid_premiumize_torbox_cache_flags() {
        for flag in ["[AD+]", "[PM+]", "[TB+]"] {
            let p = parse_torrentio(&format!("{flag} prov"), "Show 1080p\n👤 5 💾 1 GB");
            assert_eq!(p.cache, CacheState::Cached, "flag {flag} should be cached");
        }
    }

    #[test]
    fn torrentio_size_regex_matches_gib() {
        let p = parse_torrentio("[RD+] prov", "Show 2160p\n👤 5 💾 1.4 GiB");
        assert_eq!(p.size_bytes, parse_size_to_bytes("1.4 GiB"));
        let p = parse_torrentio("[RD+] prov", "Show 1080p\n👤 5 💾 700 MB");
        assert_eq!(p.size_bytes, parse_size_to_bytes("700 MB"));
    }
}
