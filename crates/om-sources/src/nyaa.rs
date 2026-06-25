//! Direct nyaa.si source adapter (anime) via the RSS feed.
//!
//! The RSS feed (`?page=rss`) is far more robust than scraping the HTML table:
//! each `<item>` carries `<nyaa:infoHash>`, `<nyaa:seeders>`, and `<nyaa:size>`,
//! from which we build a magnet. Independent of Torrentio, so anime keeps working
//! if the addon is down and nyaa-only releases are reachable.

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::model::MediaKind;
use om_core::ports::{SourceProvider, SourceQuery};
use om_core::stream::{CacheState, SourceCandidate};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;

use crate::tags::{parse_release_name, parse_size_to_bytes};

const DEFAULT_BASE: &str = "https://nyaa.si";

/// Direct nyaa.si RSS source (anime only).
pub struct NyaaSource {
    client: Client,
    base_url: String,
}

impl NyaaSource {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE.to_string(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Build the nyaa search text plus the season context used to filter results.
    ///
    /// Returns `(query_text, base, ordinal)`:
    /// - `base` is the franchise name with any season suffix stripped, so the
    ///   query targets the whole franchise and the season filter (not the query)
    ///   does the precision work — which also fixes recall for sequels whose
    ///   release naming differs from AniList's ("2nd Season" vs "S2").
    /// - `ordinal` is which season the selected entry is (1 when unmarked).
    fn plan_query(query: &SourceQuery) -> (String, String, u32) {
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
        let text = match query.episode {
            Some(ep) => format!("{base} {ep:02}"),
            None => base.clone(),
        };
        (text, base, ordinal)
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
        let (qtext, base, ordinal) = Self::plan_query(query);
        let q = urlencoding::encode(&qtext).into_owned();
        // c=1_2 = English-translated anime, sorted by seeders desc.
        let url = format!(
            "{}/?page=rss&q={q}&c=1_2&f=0&s=seeders&o=desc",
            self.base_url
        );
        tracing::debug!(%url, ordinal, "nyaa rss request");

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
        let all = parse_rss(&xml)?;

        // Keep only releases for the requested season (AniList numbers each season
        // from 1, so episode "01" otherwise matches every season's premiere).
        let filtered: Vec<SourceCandidate> = all
            .iter()
            .filter(|c| crate::season::release_season(&c.title, &base).covers(ordinal))
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

    let err = |e: quick_xml::Error| CoreError::Parse {
        what: "nyaa rss".into(),
        message: e.to_string(),
    };

    loop {
        match reader.read_event().map_err(err)? {
            Event::Eof => break,
            Event::Start(e) => match local_name(e.name().as_ref()).as_str() {
                "item" => cur = Some(RawItem::default()),
                "title" if cur.is_some() => field = Some(Field::Title),
                "infoHash" if cur.is_some() => field = Some(Field::Hash),
                "seeders" if cur.is_some() => field = Some(Field::Seeders),
                "size" if cur.is_some() => field = Some(Field::Size),
                _ => {}
            },
            Event::Text(t) => assign(&mut cur, field, &t.unescape().map_err(err)?),
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
        assert_eq!(first.quality, om_core::stream::Quality::P1080);
        assert_eq!(first.seeders, Some(1234));
        assert_eq!(first.size_bytes, parse_size_to_bytes("1.4 GiB"));
        assert!(first
            .magnet
            .as_deref()
            .unwrap()
            .contains("abcdef0123456789"));
        assert!(first.is_resolvable());

        assert_eq!(candidates[1].quality, om_core::stream::Quality::P720);
        assert_eq!(candidates[1].seeders, Some(50));
    }

    #[test]
    fn empty_channel_is_ok() {
        let xml = r#"<rss><channel><title>Nyaa</title></channel></rss>"#;
        assert!(parse_rss(xml).unwrap().is_empty());
    }
}
