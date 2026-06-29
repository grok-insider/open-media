//! Direct nyaa.si source adapter (anime) via the RSS feed.
//!
//! The RSS feed (`?page=rss`) is far more robust than scraping the HTML table:
//! each `<item>` carries `<nyaa:infoHash>`, `<nyaa:seeders>`, and `<nyaa:size>`,
//! from which we build a magnet. Independent of Torrentio, so anime keeps working
//! if the addon is down and nyaa-only releases are reachable.

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::MediaKind;
use open_media_core::ports::{SourceProvider, SourceQuery};
use open_media_core::stream::{CacheState, SourceCandidate};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;

use crate::tags::{parse_release_name, parse_size_to_bytes};

const DEFAULT_BASE: &str = "https://nyaa.si";
/// nyaa.si category for English-translated anime.
const DEFAULT_CATEGORY: &str = "1_2";

/// Direct nyaa.si RSS source (anime only).
pub struct NyaaSource {
    client: Client,
    base_url: String,
    category: String,
}

impl NyaaSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE.to_string(),
            category: DEFAULT_CATEGORY.to_string(),
        }
    }

    /// Build a source for a specific nyaa category (the `c=` RSS parameter, e.g.
    /// `"1_2"` for English-translated anime, `"1_3"` for raw/untranslated).
    pub fn with_category(category: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE.to_string(),
            category: category.into(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            category: DEFAULT_CATEGORY.to_string(),
        }
    }

    /// Build the search context used to find + filter results.
    ///
    /// Returns `(base, ordinal)`:
    /// - `base` is the franchise name with any season suffix stripped, so the
    ///   query targets the whole franchise and the season filter (not the query)
    ///   does the precision work — which also fixes recall for sequels whose
    ///   release naming differs from AniList's ("2nd Season" vs "S2").
    /// - `ordinal` is which season the selected entry is (1 when unmarked).
    fn plan_query(query: &SourceQuery) -> (String, u32) {
        // Release groups (SubsPlease/Erai-raws) name files with the romaji title,
        // so prefer `original_title`; and drop any English subtitle after a colon
        // ("Frieren: Beyond Journey's End" → "Frieren") so the search matches nyaa
        // filenames.
        let raw = query
            .media
            .original_title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| query.media.display_title());
        let no_sub = raw.split(':').next().unwrap_or(raw).trim();
        let (base, ordinal) = crate::season::parse_title_season(no_sub);
        (base, ordinal)
    }

    /// One RSS round-trip for `{base} {episode:02}` (or just `{base}` for a movie/
    /// season-pack search), returning the parsed candidates.
    async fn fetch(&self, base: &str, episode: Option<u32>) -> CoreResult<Vec<SourceCandidate>> {
        let qtext = match episode {
            Some(ep) => format!("{base} {ep:02}"),
            None => base.to_string(),
        };
        let q = urlencoding::encode(&qtext).into_owned();
        // c defaults to 1_2 (English-translated anime), sorted by seeders desc.
        let url = format!(
            "{}/?page=rss&q={q}&c={}&f=0&s=seeders&o=desc",
            self.base_url, self.category
        );
        tracing::debug!(%url, "nyaa rss request");

        let resp = self.client.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                CoreError::Timeout(format!("nyaa: {e}"))
            } else {
                CoreError::Network(format!("nyaa: {e}"))
            }
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "nyaa".into(),
                message: format!("HTTP {status}"),
            });
        }
        let xml = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("nyaa: {e}")))?;
        parse_rss(&xml)
    }
}

impl Default for NyaaSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SourceProvider for NyaaSource {
    fn name(&self) -> &str {
        "nyaa"
    }

    fn supports(&self, kind: MediaKind) -> bool {
        matches!(kind, MediaKind::Anime)
    }

    async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        let (base, ordinal) = Self::plan_query(query);

        // The relative-episode search (`{base} {episode}`) is always issued. When
        // the engine computed an absolute (franchise-continuous) number that
        // differs — a sequel numbered `… - 21` instead of S2 `… - 01` — issue a
        // second search for that number too, since the `… 01` query never returns
        // the `… 21` files. Results are merged and deduped by infohash.
        let absolute = query
            .absolute_episode
            .filter(|abs| Some(*abs) != query.episode);

        let mut all = self.fetch(&base, query.episode).await?;
        if let Some(abs) = absolute {
            match self.fetch(&base, Some(abs)).await {
                Ok(extra) => all.extend(extra),
                // The primary search already succeeded; a failed secondary fetch
                // is a missed-recall, not a reason to fail the whole lookup.
                Err(e) => tracing::debug!(error = %e, abs, "nyaa absolute-episode fetch failed"),
            }
            dedup_by_infohash(&mut all);
        }

        // Keep only releases for the requested season. AniList numbers each season
        // from 1, so episode "01" otherwise matches every season's premiere — a
        // release is in-season when its title's season marker covers the ordinal,
        // OR (absolute numbering) it carries no season marker yet its episode
        // coordinate is exactly the absolute number (the `… - 21` sequel case).
        let filtered: Vec<SourceCandidate> = all
            .iter()
            .filter(|c| in_requested_season(&c.title, &base, ordinal, absolute))
            .cloned()
            .collect();

        // Safety net: if the season heuristic removed everything (e.g. an unusual
        // naming scheme), show the unfiltered set rather than a dead-end.
        if filtered.is_empty() && !all.is_empty() {
            tracing::debug!(%base, ordinal, "season filter matched nothing; returning unfiltered");
            return Ok(all);
        }
        Ok(filtered)
    }
}

/// Whether a release belongs to the requested season.
///
/// Primary signal is the title's season marker ([`release_season`]). The
/// absolute-numbering escape hatch: a release with *no* season marker
/// ([`SeasonMatch::None`]) whose parsed episode coordinate equals the absolute
/// number is the requested sequel episode published continuously (`… - 21`).
fn in_requested_season(title: &str, base: &str, ordinal: u32, absolute: Option<u32>) -> bool {
    use crate::season::SeasonMatch;
    let season = crate::season::release_season(title, base);
    if season.covers(ordinal) {
        return true;
    }
    // Only a marker-less release can be an absolute-numbered sequel; one that
    // explicitly says S1/S3/etc. is not the requested season just because a
    // number coincides.
    match (absolute, season) {
        (Some(abs), SeasonMatch::None) => crate::season::release_episode(title, base).covers(abs),
        _ => false,
    }
}

/// Drop candidates sharing an infohash, keeping the first occurrence (RSS is
/// seeder-sorted, so the first is the best-seeded). Candidates without an
/// infohash are always kept — there's nothing to dedup them by.
fn dedup_by_infohash(items: &mut Vec<SourceCandidate>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|c| match &c.info_hash {
        Some(h) => seen.insert(h.to_ascii_lowercase()),
        None => true,
    });
}

/// One raw RSS `<item>` accumulated during the event walk.
#[derive(Debug, Default)]
struct RawItem {
    title: String,
    info_hash: Option<String>,
    seeders: Option<String>,
    size: Option<String>,
}

impl RawItem {
    fn into_candidate(self) -> SourceCandidate {
        let (quality, tags) = parse_release_name(&self.title);
        let size_bytes = self.size.as_deref().map(parse_size_to_bytes).unwrap_or(0);
        let seeders = self.seeders.as_deref().and_then(|s| s.trim().parse().ok());
        let magnet = self
            .info_hash
            .as_ref()
            .map(|h| format!("magnet:?xt=urn:btih:{}", h.trim()));
        SourceCandidate {
            provider: "nyaa".to_string(),
            title: self.title,
            quality,
            size_bytes,
            seeders,
            info_hash: self.info_hash.map(|h| h.trim().to_string()),
            magnet,
            direct_url: None,
            file_index: None,
            cache: CacheState::Unknown,
            tags,
        }
    }
}

/// Which `<item>` child we're currently capturing text into.
#[derive(Clone, Copy, PartialEq)]
enum Field {
    Title,
    Hash,
    Seeders,
    Size,
}

/// Strip a namespace prefix (`nyaa:seeders` → `seeders`).
fn local_name(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => s.into_owned(),
    }
}

/// Parse a nyaa RSS document into candidates.
///
/// Uses quick-xml's event reader (not serde) because the meaningful fields live
/// in the `nyaa:` namespace, and prefix handling through serde is unreliable
/// across versions. Matching by local name is deterministic.
pub fn parse_rss(xml: &str) -> CoreResult<Vec<SourceCandidate>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut out = Vec::new();
    let mut cur: Option<RawItem> = None;
    let mut field: Option<Field> = None;

    let err = |e: &dyn std::fmt::Display| CoreError::Parse {
        what: "nyaa rss".into(),
        message: e.to_string(),
    };

    loop {
        match reader.read_event().map_err(|e| err(&e))? {
            Event::Eof => break,
            Event::Start(e) => match local_name(e.name().as_ref()).as_str() {
                "item" => cur = Some(RawItem::default()),
                "title" if cur.is_some() => field = Some(Field::Title),
                "infoHash" if cur.is_some() => field = Some(Field::Hash),
                "seeders" if cur.is_some() => field = Some(Field::Seeders),
                "size" if cur.is_some() => field = Some(Field::Size),
                _ => {}
            },
            Event::Text(t) => assign(&mut cur, field, &t.xml_content().map_err(|e| err(&e))?),
            Event::CData(c) => assign(&mut cur, field, &String::from_utf8_lossy(c.as_ref())),
            Event::End(e) => {
                if local_name(e.name().as_ref()) == "item" {
                    if let Some(item) = cur.take() {
                        out.push(item.into_candidate());
                    }
                }
                field = None;
            }
            _ => {}
        }
    }
    Ok(out)
}

fn assign(cur: &mut Option<RawItem>, field: Option<Field>, text: &str) {
    if let (Some(item), Some(f)) = (cur.as_mut(), field) {
        match f {
            Field::Title => item.title.push_str(text),
            Field::Hash => item.info_hash = Some(text.to_string()),
            Field::Seeders => item.seeders = Some(text.to_string()),
            Field::Size => item.size = Some(text.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:nyaa="https://nyaa.si/xmlns/nyaa">
  <channel>
    <title>Nyaa</title>
    <item>
      <title>[SubsPlease] Frieren - 01 (1080p) [F00BAR].mkv</title>
      <link>https://nyaa.si/download/1.torrent</link>
      <nyaa:seeders>1234</nyaa:seeders>
      <nyaa:leechers>5</nyaa:leechers>
      <nyaa:size>1.4 GiB</nyaa:size>
      <nyaa:infoHash>abcdef0123456789abcdef0123456789abcdef01</nyaa:infoHash>
    </item>
    <item>
      <title>[Erai-raws] Frieren - 01 (720p)</title>
      <nyaa:seeders>50</nyaa:seeders>
      <nyaa:size>600.0 MiB</nyaa:size>
      <nyaa:infoHash>0011223344556677889900112233445566778899</nyaa:infoHash>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_nyaa_rss_items() {
        let candidates = parse_rss(SAMPLE).unwrap();
        assert_eq!(candidates.len(), 2);

        let first = &candidates[0];
        assert_eq!(first.provider, "nyaa");
        assert_eq!(first.quality, open_media_core::stream::Quality::P1080);
        assert_eq!(first.seeders, Some(1234));
        assert_eq!(first.size_bytes, parse_size_to_bytes("1.4 GiB"));
        assert!(first
            .magnet
            .as_deref()
            .unwrap()
            .contains("abcdef0123456789"));
        assert!(first.is_resolvable());

        assert_eq!(
            candidates[1].quality,
            open_media_core::stream::Quality::P720
        );
        assert_eq!(candidates[1].seeders, Some(50));
    }

    #[test]
    fn empty_channel_is_ok() {
        let xml = r#"<rss><channel><title>Nyaa</title></channel></rss>"#;
        assert!(parse_rss(xml).unwrap().is_empty());
    }
}
