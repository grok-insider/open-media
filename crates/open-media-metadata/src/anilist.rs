//! AniList GraphQL metadata adapter (anime).
//!
//! Search/details are public (no token). The token-gated mutations (progress
//! tracking) live in `open-media-track`, not here.

use std::collections::HashMap;

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::model::{Episode, IdSet, Media, MediaKind, Season};
use open_media_core::ports::MetadataProvider;
use reqwest::Client;
use serde::Deserialize;

use crate::jikan::JikanTitles;
use crate::map_net;

const DEFAULT_BASE: &str = "https://graphql.anilist.co";

const SEARCH_QUERY: &str = r#"
query ($search: String, $page: Int, $perPage: Int) {
  Page(page: $page, perPage: $perPage) {
    pageInfo { hasNextPage }
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
      id idMal
      title { romaji english native }
      seasonYear episodes averageScore
      description(asHtml: false)
      coverImage { large }
      status format genres
      nextAiringEpisode { episode }
    }
  }
}"#;

/// Results requested per AniList search page. Raised from the original 15 to
/// give ranking a deeper candidate pool in one round-trip.
const SEARCH_PER_PAGE: u32 = 30;

/// Upper bound on AniList search pages fetched per query, so a broad term can't
/// fan out into many round-trips. Two pages (`SEARCH_PER_PAGE` each) is ample
/// depth for ranking.
const MAX_SEARCH_PAGES: u32 = 2;

const DETAIL_QUERY: &str = r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id idMal
    title { romaji english native }
    seasonYear episodes averageScore
    description(asHtml: false)
    coverImage { large }
    status format genres
    nextAiringEpisode { episode }
    streamingEpisodes { title thumbnail url }
    relations { edges { relationType node { id format episodes } } }
  }
}"#;

/// Trimmed detail query used while walking the prequel chain: we only need the
/// relation graph and episode counts, not titles/descriptions/streaming data.
const RELATIONS_QUERY: &str = r#"
query ($id: Int) {
  Media(id: $id, type: ANIME) {
    id episodes format
    relations { edges { relationType node { id format episodes } } }
  }
}"#;

/// Hop cap when following `PREQUEL` edges, so a malformed or cyclic relation
/// graph can never spin the walk forever. Five hops covers even long franchises
/// (a 5th-season sequel summing its four predecessors).
const PREQUEL_HOP_CAP: u8 = 5;

/// AniList-backed metadata provider (anime only).
pub struct AniListProvider {
    client: Client,
    base_url: String,
    /// Best-effort per-episode title source (MAL id keyed). Enriches the sparse
    /// `streamingEpisodes` titles in [`episodes`]; never a hard dependency.
    ///
    /// [`episodes`]: MetadataProvider::episodes
    jikan: JikanTitles,
}

impl AniListProvider {
    pub fn new() -> Self {
        Self {
            client: open_media_net::client(),
            base_url: DEFAULT_BASE.to_string(),
            jikan: JikanTitles::new(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            base_url: base_url.into(),
            jikan: JikanTitles::new(),
        }
    }

    /// Like [`with_base_url`], but also points the Jikan client at a base URL —
    /// so a test can serve both AniList and Jikan from one mock server.
    ///
    /// [`with_base_url`]: Self::with_base_url
    #[cfg(test)]
    fn with_bases(anilist_base: impl Into<String>, jikan_base: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            base_url: anilist_base.into(),
            jikan: JikanTitles::with_base_url(jikan_base),
        }
    }

    async fn query<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> CoreResult<T> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        // AniList search/details are read-only GraphQL queries (idempotent), so a
        // transient transport failure is safe to retry.
        let resp = open_media_net::retry(|| async {
            self.client
                .post(&self.base_url)
                .json(&body)
                .send()
                .await
                .map_err(|e| map_net("anilist", e))
        })
        .await?;
        let status = resp.status();
        if !status.is_success() {
            let message = anilist_http_error_message(status, resp).await;
            return Err(CoreError::Remote {
                service: "anilist".into(),
                message,
            });
        }
        let parsed: GqlResponse<T> = resp.json().await.map_err(|e| CoreError::Parse {
            what: "anilist response".into(),
            message: e.to_string(),
        })?;
        if let Some(errors) = parsed.errors {
            if !errors.is_empty() {
                return Err(CoreError::Remote {
                    service: "anilist".into(),
                    message: errors
                        .into_iter()
                        .map(|e| e.message)
                        .collect::<Vec<_>>()
                        .join("; "),
                });
            }
        }
        parsed.data.ok_or_else(|| CoreError::Parse {
            what: "anilist response".into(),
            message: "missing data field".into(),
        })
    }

    /// Fetch the raw `Media` object (keeping `streamingEpisodes`, which
    /// `into_media` drops) so callers can enrich per-episode data.
    async fn fetch_media(&self, ids: &IdSet) -> CoreResult<AniListMedia> {
        let id = ids
            .anilist
            .ok_or_else(|| CoreError::NotFound("anilist id required for details".into()))?;
        let data: GqlData = self
            .query(DETAIL_QUERY, serde_json::json!({ "id": id }))
            .await?;
        data.media
            .ok_or_else(|| CoreError::NotFound("anilist media".into()))
    }

    /// Fetch just the relation graph + episode count for an id, used while
    /// hopping back through the prequel chain (no titles/streaming data needed).
    async fn fetch_relations(&self, id: i32) -> CoreResult<RelationMedia> {
        let data: RelationData = self
            .query(RELATIONS_QUERY, serde_json::json!({ "id": id }))
            .await?;
        data.media
            .ok_or_else(|| CoreError::NotFound("anilist media".into()))
    }

    /// Sum the episode counts of every prior `TV` `PREQUEL` in this title's
    /// franchise — the absolute-numbering offset for the entry whose immediate TV
    /// prequel is `first`.
    ///
    /// Walks `PREQUEL` edges backwards from `first`, adding each prequel's episode
    /// count and continuing from that prequel, bounded by [`PREQUEL_HOP_CAP`]. A
    /// prequel whose own episode count is unknown contributes 0 but the walk still
    /// continues through it. The caller passes `None` (→ result `None`) when there
    /// is no TV prequel at all (a true season 1), so absolute matching stays off
    /// rather than treating an offset of 0 specially.
    async fn prequel_offset(&self, first: Option<RelationNode>) -> CoreResult<Option<u32>> {
        let Some(first) = first else {
            return Ok(None);
        };
        let mut total: u32 = 0;
        let mut next = Some(first);
        let mut hops = 0u8;
        while let Some(node) = next {
            total = total.saturating_add(node.episodes.unwrap_or(0));
            hops += 1;
            // Stop before exceeding the hop cap or when the node has no id to
            // recurse into (we already counted its episodes above).
            let Some(id) = node.id.filter(|_| hops < PREQUEL_HOP_CAP) else {
                break;
            };
            // A network hiccup mid-chain shouldn't poison the whole offset; take
            // what we have so far rather than failing the lookup.
            match self.fetch_relations(id).await {
                Ok(media) => next = media.tv_prequel().cloned(),
                Err(e) => {
                    tracing::debug!(error = %e, id, "anilist prequel hop failed; using partial offset");
                    break;
                }
            }
        }
        Ok(Some(total))
    }
}

async fn anilist_http_error_message(
    status: reqwest::StatusCode,
    resp: reqwest::Response,
) -> String {
    let fallback = format!("HTTP {status}");
    let Ok(body) = resp.text().await else {
        return fallback;
    };
    let Some(message) = anilist_error_body_message(&body) else {
        return fallback;
    };
    format!("HTTP {status}: {message}")
}

fn anilist_error_body_message(body: &str) -> Option<String> {
    let parsed: GqlResponse<serde_json::Value> = serde_json::from_str(body).ok()?;
    let errors = parsed.errors?;
    let message = errors
        .into_iter()
        .map(|e| e.message.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    (!message.is_empty()).then_some(message)
}

/// Pick the TV `PREQUEL` node (most episodes wins) from a connection — shared by
/// [`AniListMedia`] details and the trimmed [`RelationMedia`].
fn pick_tv_prequel(conn: &MediaConnection) -> Option<&RelationNode> {
    conn.edges
        .iter()
        .filter(|e| e.relation_type.as_deref() == Some("PREQUEL"))
        .filter_map(|e| e.node.as_ref())
        .filter(|n| n.format.as_deref() == Some("TV"))
        .max_by_key(|n| n.episodes.unwrap_or(0))
}

impl Default for AniListProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetadataProvider for AniListProvider {
    fn name(&self) -> &str {
        "anilist"
    }

    async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        // AniList is an anime-only source: stay out of the way for explicit
        // movie/series searches; contribute for None or Anime.
        if matches!(kind, Some(MediaKind::Movie) | Some(MediaKind::Series)) {
            return Ok(Vec::new());
        }
        // Fetch a bounded number of pages and merge, so ranking has more than a
        // single page of candidates. We stop early once AniList reports no
        // further page (`hasNextPage`), and never exceed `MAX_SEARCH_PAGES`.
        let mut media = Vec::new();
        for page_no in 1..=MAX_SEARCH_PAGES {
            let data: GqlData = self
                .query(
                    SEARCH_QUERY,
                    serde_json::json!({
                        "search": query,
                        "page": page_no,
                        "perPage": SEARCH_PER_PAGE,
                    }),
                )
                .await?;
            let page = data.page.ok_or_else(|| CoreError::Parse {
                what: "anilist search".into(),
                message: "missing Page".into(),
            })?;
            media.extend(page.media);
            if !page.page_info.map(|p| p.has_next_page).unwrap_or(false) {
                break;
            }
        }
        Ok(media.into_iter().map(AniListMedia::into_media).collect())
    }

    async fn details(&self, ids: &IdSet) -> CoreResult<Media> {
        Ok(self.fetch_media(ids).await?.into_media())
    }

    async fn seasons(&self, ids: &IdSet) -> CoreResult<Vec<Season>> {
        // AniList anime are flat-numbered; model as a single season.
        let media = self.details(ids).await?;
        Ok(vec![Season {
            number: 1,
            episode_count: media.episode_count.unwrap_or(0),
            name: None,
        }])
    }

    async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        let media = self.fetch_media(ids).await?;
        let count = media.effective_episode_count();

        // AniList exposes per-episode title + thumbnail only via
        // `streamingEpisodes`, whose entries are NOT guaranteed to be 1:1 with
        // episode numbers (free-form titles, gaps, duplicates, no number
        // field). So we build the canonical `1..=count` range and best-effort
        // enrich each from the streaming entry whose leading number matches —
        // never by blind index — leaving unmatched episodes bare.
        let mut enrich: HashMap<u32, &StreamingEpisode> = HashMap::new();
        for se in &media.streaming_episodes {
            if let Some(n) = se.title.as_deref().and_then(parse_episode_number) {
                enrich.entry(n).or_insert(se);
            }
        }

        // AniList's per-episode titles are sparse, so when we have a MAL id,
        // pull richer titles from Jikan (MAL's episode list, keyed by episode
        // number). This is best-effort: a failure yields an empty map and we
        // simply fall back to the `streamingEpisodes`-derived title below.
        let jikan_titles = match ids.mal {
            Some(mal) => self.jikan.episode_titles(mal).await,
            None => HashMap::new(),
        };

        Ok((1..=count)
            .map(|n| {
                let se = enrich.get(&n);
                // Prefer Jikan's title; fall back to the streaming-episode one.
                let title = jikan_titles
                    .get(&n)
                    .cloned()
                    .or_else(|| se.and_then(|s| s.clean_title()));
                Episode {
                    season,
                    number: n,
                    title,
                    air_date: None,
                    overview: None,
                    runtime_minutes: None,
                    rating: None,
                    still: se.and_then(|s| non_empty(s.thumbnail.clone())),
                }
            })
            .collect())
    }

    /// Σ episodes of all prior `TV` `PREQUEL`s — the absolute-numbering offset for
    /// this AniList entry. `None` (default) for a title with no TV prequel.
    async fn episode_offset(&self, ids: &IdSet) -> CoreResult<Option<u32>> {
        let media = self.fetch_media(ids).await?;
        self.prequel_offset(media.tv_prequel().cloned()).await
    }
}

/// Parse the leading episode number out of an AniList streaming-episode title
/// such as `"Episode 12 - The Journey's End"`, `"12. Title"`, or `"Ep. 3"`.
/// Returns `None` when no plausible leading number is present (e.g. specials,
/// OVAs, or recap titles), so those entries are simply not matched.
fn parse_episode_number(title: &str) -> Option<u32> {
    let t = title.trim();
    // Strip a leading "Episode"/"Ep"/"E" word if present, then take the first
    // run of digits.
    let rest = t
        .strip_prefix("Episode")
        .or_else(|| t.strip_prefix("episode"))
        .or_else(|| t.strip_prefix("Ep."))
        .or_else(|| t.strip_prefix("Ep"))
        .or_else(|| t.strip_prefix("EP"))
        .unwrap_or(t)
        .trim_start_matches(['.', ' ', '-', '#', ':']);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Convert AniList's inline-HTML description into plain text.
///
/// AniList returns markup even when queried with `description(asHtml: false)` —
/// a long-standing API quirk: `<i>`, `<b>`, `<br>` and a handful of character
/// entities appear inline. The domain `overview` contract is **plain text**
/// (see `open_media_core::model::Media::overview`), so the quirk is stripped at
/// the adapter boundary: `<br>` variants become newlines, every other tag is
/// dropped, the common entities are decoded, and 3+ consecutive newlines
/// collapse to one blank line.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match parse_tag(after) {
            Some((name, consumed)) => {
                if name.eq_ignore_ascii_case("br") {
                    out.push('\n');
                }
                rest = &after[consumed..];
            }
            None => {
                // A '<' that doesn't start a tag (`3 < 5`) is content.
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);

    // Decode the entities AniList actually emits. `&amp;` must go LAST so a
    // literal "&amp;lt;" decodes to "&lt;", not "<".
    let mut out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");

    // `<br><br>` runs plus surrounding whitespace produce noisy gaps; keep at
    // most one blank line and trim the edges.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim().to_string()
}

/// Try to read an HTML tag immediately after a `<`. Returns the tag name and
/// the bytes consumed (through the closing `>`), or `None` when the text is
/// not tag-shaped — an optional `/`, then a letter run, then `>`/space/`/` —
/// so prose like `3 < 5 but 7 > 2` survives untouched. Real inline tags are
/// short; anything longer than 60 bytes is treated as content too.
fn parse_tag(s: &str) -> Option<(&str, usize)> {
    let inner = s.strip_prefix('/').unwrap_or(s);
    let name_len = inner
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    if name_len == 0 {
        return None;
    }
    let end = s.find('>')?;
    if end > 60 {
        return None;
    }
    match inner[name_len..].chars().next() {
        Some('>') | Some(' ') | Some('/') | Some('\t') => Some((&inner[..name_len], end + 1)),
        _ => None,
    }
}

// --- GraphQL response shapes ---

#[derive(Debug, Deserialize)]
struct GqlResponse<T> {
    #[serde(default = "none")]
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<GqlError>>,
}

/// `#[serde(default)]` on a generic `Option<T>` field needs `T: Default`, which
/// we don't want to require; this gives the field a `T`-free default instead.
fn none<T>() -> Option<T> {
    None
}

#[derive(Debug, Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct GqlData {
    #[serde(rename = "Page", default)]
    page: Option<Page>,
    #[serde(rename = "Media", default)]
    media: Option<AniListMedia>,
}

/// The `data` shape for [`RELATIONS_QUERY`] — a trimmed `Media` with no required
/// title, so a relations-only response deserializes cleanly.
#[derive(Debug, Deserialize)]
struct RelationData {
    #[serde(rename = "Media", default)]
    media: Option<RelationMedia>,
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    media: Vec<AniListMedia>,
    #[serde(rename = "pageInfo", default)]
    page_info: Option<PageInfo>,
}

/// AniList `Page.pageInfo` — we only need `hasNextPage` to decide whether to
/// fetch another page.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    #[serde(default)]
    has_next_page: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AniListMedia {
    id: i32,
    #[serde(default)]
    id_mal: Option<i32>,
    title: AniListTitle,
    #[serde(default)]
    season_year: Option<i32>,
    #[serde(default)]
    episodes: Option<u32>,
    #[serde(default)]
    average_score: Option<i32>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    cover_image: Option<CoverImage>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    next_airing_episode: Option<NextAiringEpisode>,
    #[serde(default)]
    streaming_episodes: Vec<StreamingEpisode>,
    #[serde(default)]
    relations: Option<MediaConnection>,
}

/// AniList's `relations` connection: related franchise entries (prequels,
/// sequels, side stories, …) reached via directional [`MediaEdge`]s.
#[derive(Debug, Deserialize)]
struct MediaConnection {
    #[serde(default)]
    edges: Vec<MediaEdge>,
}

/// One edge in the relation graph. `relation_type` is the relation of `node`
/// *to the queried media* — so for an S2 entry, its S1 prequel is the edge whose
/// `relation_type` is `PREQUEL`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaEdge {
    #[serde(default)]
    relation_type: Option<String>,
    #[serde(default)]
    node: Option<RelationNode>,
}

/// The lightweight `node` carried by a relation edge — just enough to identify a
/// prior TV season and count its episodes, and to keep walking the chain.
#[derive(Debug, Clone, Deserialize)]
struct RelationNode {
    #[serde(default)]
    id: Option<i32>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    episodes: Option<u32>,
}

impl AniListMedia {
    /// The single TV `PREQUEL` of this entry, if any.
    ///
    /// Only `TV` prequels count toward absolute numbering: movies, OVAs, ONAs and
    /// specials are not part of the franchise's continuous episode count, and the
    /// edge must be `PREQUEL` (directional — a `SEQUEL` edge points the other
    /// way). When AniList lists more than one TV prequel (rare; e.g. a recap
    /// re-edit), the one with the most episodes is chosen as the true predecessor.
    fn tv_prequel(&self) -> Option<&RelationNode> {
        self.relations.as_ref().and_then(pick_tv_prequel)
    }
}

/// The trimmed media shape returned by [`RELATIONS_QUERY`] — no required title,
/// so it deserializes from a response that omits everything but ids/episodes/
/// relations.
#[derive(Debug, Deserialize)]
struct RelationMedia {
    #[serde(default)]
    relations: Option<MediaConnection>,
}

impl RelationMedia {
    /// The single TV `PREQUEL` of this entry, if any (see [`pick_tv_prequel`]).
    fn tv_prequel(&self) -> Option<&RelationNode> {
        self.relations.as_ref().and_then(pick_tv_prequel)
    }
}

/// AniList's `nextAiringEpisode.episode` is the number of the *next* episode to
/// air (1-based), so for a currently-airing show with `episodes: null`, the
/// count of already-aired episodes is `episode - 1`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NextAiringEpisode {
    #[serde(default)]
    episode: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct StreamingEpisode {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    url: Option<String>,
}

impl StreamingEpisode {
    /// The episode title with a leading `"Episode N - "` / `"N. "` prefix
    /// stripped, since the episode number is shown separately in the UI.
    fn clean_title(&self) -> Option<String> {
        let raw = self.title.as_deref()?.trim();
        let stripped = match raw.find([':', '-', '.', '–']) {
            // Only strip when the part before the separator looks like an
            // episode marker (contains a digit), so real titles like
            // "Re:Zero" or "Bake-mono" aren't mangled.
            Some(i) if raw[..i].chars().any(|c| c.is_ascii_digit()) => raw[i + 1..].trim(),
            _ => raw,
        };
        non_empty(Some(stripped.to_string()))
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

#[derive(Debug, Deserialize)]
struct AniListTitle {
    #[serde(default)]
    romaji: Option<String>,
    #[serde(default)]
    english: Option<String>,
    #[serde(default)]
    native: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoverImage {
    #[serde(default)]
    large: Option<String>,
}

impl AniListMedia {
    /// The number of watchable episodes.
    ///
    /// Finished shows report `episodes` directly. Currently-airing shows leave
    /// `episodes: null`, so fall back to `nextAiringEpisode.episode - 1` (the
    /// count of episodes that have *already* aired). When neither is known we
    /// assume a single episode rather than zero, so the episode list is never
    /// empty for a real title.
    fn effective_episode_count(&self) -> u32 {
        if let Some(n) = self.episodes {
            return n;
        }
        if let Some(next) = self
            .next_airing_episode
            .as_ref()
            .and_then(|n| n.episode)
            .filter(|&e| e > 0)
        {
            return next - 1;
        }
        1
    }

    fn into_media(self) -> Media {
        let mut ids = IdSet::default().with_anilist(self.id);
        if let Some(mal) = self.id_mal {
            ids = ids.with_mal(mal);
        }
        let title = self
            .title
            .english
            .clone()
            .or_else(|| self.title.romaji.clone())
            .or_else(|| self.title.native.clone())
            .unwrap_or_default();
        let original = self.title.romaji.or(self.title.native);
        // AniList tags anime films as `format: "MOVIE"`. Model them as Movies
        // with no season/episode coordinates so the play path streams the film
        // directly instead of asking for a season+episode it doesn't have.
        let is_movie = self.format.as_deref() == Some("MOVIE");
        let (kind, episode_count, season_count) = if is_movie {
            (MediaKind::Movie, None, None)
        } else {
            (MediaKind::Anime, self.episodes, Some(1))
        };
        Media {
            kind,
            ids,
            title,
            original_title: original,
            year: self.season_year,
            score: self.average_score.map(|s| s as f32 / 10.0),
            overview: self
                .description
                .map(|s| strip_html(&s))
                .filter(|s| !s.is_empty()),
            poster: self.cover_image.and_then(|c| c.large),
            genres: self.genres,
            status: self.status,
            episode_count,
            season_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn strip_html_converts_anilist_markup_to_plain_text() {
        // The real shape AniList returns (asHtml: false still carries markup).
        let raw = "The second season of <i>Kage no Jitsuryokusha ni Naritakute!</i>\
                   <br><br>Everything has been going according to plan.\
                   <br><br>(Source: HIDIVE, edited)<br><br><i>Note: A world premiere \
                   screening of Episode 1 was shown on July 1, 2023.</i>";
        let text = strip_html(raw);
        assert!(!text.contains('<'), "no tags may survive: {text}");
        assert!(text.starts_with("The second season of Kage no Jitsuryokusha ni Naritakute!"));
        assert!(text.contains("\n\nEverything has been going according to plan."));
        assert!(text.contains("\n\n(Source: HIDIVE, edited)"));
        assert!(text.ends_with("July 1, 2023."));
    }

    #[test]
    fn strip_html_handles_br_variants_and_bold() {
        assert_eq!(strip_html("a<br>b"), "a\nb");
        assert_eq!(strip_html("a<br/>b"), "a\nb");
        assert_eq!(strip_html("a<br />b"), "a\nb");
        assert_eq!(strip_html("a<BR>b"), "a\nb");
        assert_eq!(strip_html("<b>bold</b> and <em>em</em>"), "bold and em");
    }

    #[test]
    fn strip_html_decodes_entities_without_double_decoding() {
        assert_eq!(
            strip_html("Q&amp;A &quot;quoted&quot; it&#039;s"),
            "Q&A \"quoted\" it's"
        );
        // A literal "&amp;lt;" is the *text* "&lt;", not a tag.
        assert_eq!(strip_html("&amp;lt;"), "&lt;");
    }

    #[test]
    fn strip_html_collapses_break_runs_and_trims() {
        assert_eq!(strip_html("<br><br>a<br><br><br>b<br>"), "a\n\nb");
    }

    #[test]
    fn strip_html_keeps_lone_angle_bracket_text() {
        assert_eq!(strip_html("score < 9000 is fine"), "score < 9000 is fine");
        assert_eq!(strip_html("3 < 5 but 7 > 2"), "3 < 5 but 7 > 2");
    }

    #[test]
    fn strip_html_plain_text_passes_through() {
        assert_eq!(
            strip_html("Just a plain synopsis."),
            "Just a plain synopsis."
        );
    }

    /// One AniList `Page` response with `n` media (sequential ids offset by
    /// `start`) and a `hasNextPage` flag.
    fn anilist_page(start: i32, n: i32, has_next: bool) -> serde_json::Value {
        let media: Vec<_> = (0..n)
            .map(|i| {
                let id = start + i;
                serde_json::json!({
                    "id": id,
                    "title": { "romaji": format!("Show {id}") },
                    "episodes": 12,
                    "format": "TV"
                })
            })
            .collect();
        serde_json::json!({
            "data": { "Page": { "pageInfo": { "hasNextPage": has_next }, "media": media } }
        })
    }

    #[tokio::test]
    async fn search_merges_two_pages_when_more_exist() {
        let server = MockServer::start().await;

        // Page 1 reports hasNextPage → adapter fetches page 2 and merges.
        Mock::given(method("POST"))
            .and(body_string_contains(r#""page":1"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(anilist_page(1, 30, true)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_string_contains(r#""page":2"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(anilist_page(100, 5, false)))
            .mount(&server)
            .await;

        let provider = AniListProvider::with_base_url(server.uri());
        let results = provider.search("x", None).await.unwrap();

        assert_eq!(results.len(), 35, "30 from page 1 + 5 from page 2");
        assert_eq!(results[0].ids.anilist, Some(1));
        // First and last of the merged page-2 block (ids 100..=104).
        assert_eq!(results[30].ids.anilist, Some(100));
        assert_eq!(results[34].ids.anilist, Some(104));
    }

    #[tokio::test]
    async fn search_stops_when_no_next_page() {
        let server = MockServer::start().await;
        // hasNextPage = false on page 1 → only one round-trip; a page-2 request
        // would 404 (no mock) and fail the test if it happened.
        Mock::given(method("POST"))
            .and(body_string_contains(r#""page":1"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(anilist_page(1, 4, false)))
            .mount(&server)
            .await;

        let provider = AniListProvider::with_base_url(server.uri());
        let results = provider.search("x", None).await.unwrap();
        assert_eq!(results.len(), 4);
    }

    #[test]
    fn maps_media_with_mal_bridge_and_score() {
        let json = serde_json::json!({
            "id": 154587,
            "idMal": 52991,
            "title": { "romaji": "Sousou no Frieren", "english": "Frieren", "native": "葬送のフリーレン" },
            "seasonYear": 2023,
            "episodes": 28,
            "averageScore": 89,
            "description": "After the party of heroes...",
            "coverImage": { "large": "https://img/frieren.jpg" },
            "status": "FINISHED",
            "format": "TV",
            "genres": ["Adventure", "Drama", "Fantasy"]
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        let m = media.into_media();
        assert_eq!(m.kind, MediaKind::Anime);
        assert_eq!(m.ids.anilist, Some(154587));
        assert_eq!(m.ids.mal, Some(52991));
        assert_eq!(m.title, "Frieren");
        assert_eq!(m.original_title.as_deref(), Some("Sousou no Frieren"));
        assert_eq!(m.year, Some(2023));
        assert_eq!(m.score, Some(8.9));
        assert_eq!(m.episode_count, Some(28));
    }

    #[test]
    fn movie_format_maps_to_movie_kind_without_coordinates() {
        let json = serde_json::json!({
            "id": 199,
            "title": { "romaji": "Sen to Chihiro no Kamikakushi", "english": "Spirited Away" },
            "seasonYear": 2001,
            "episodes": 1,
            "format": "MOVIE",
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        let m = media.into_media();
        assert_eq!(m.kind, MediaKind::Movie);
        // Films carry no season/episode coordinates so the play path streams
        // the file directly.
        assert_eq!(m.season_count, None);
        assert_eq!(m.episode_count, None);
    }

    #[test]
    fn tv_format_still_maps_to_anime_kind() {
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Some Show" },
            "episodes": 12,
            "format": "TV",
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        let m = media.into_media();
        assert_eq!(m.kind, MediaKind::Anime);
        assert_eq!(m.season_count, Some(1));
        assert_eq!(m.episode_count, Some(12));
    }

    #[test]
    fn airing_anime_derives_episode_count_from_next_airing_episode() {
        // Currently-airing show: `episodes` is null, but episode 5 is up next,
        // so 4 episodes have already aired.
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Airing Show" },
            "episodes": null,
            "status": "RELEASING",
            "format": "TV",
            "nextAiringEpisode": { "episode": 5 },
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        assert_eq!(media.episodes, None);
        assert_eq!(media.effective_episode_count(), 4);
    }

    #[test]
    fn missing_episode_data_falls_back_to_single_episode() {
        // Neither a known count nor a next-airing hint → assume one episode so
        // the list is never empty.
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Unknown" },
            "episodes": null,
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        assert_eq!(media.effective_episode_count(), 1);
    }

    #[test]
    fn title_falls_back_to_romaji_when_no_english() {
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Romaji Only", "english": null, "native": null }
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        assert_eq!(media.into_media().title, "Romaji Only");
    }

    #[test]
    fn parses_leading_episode_number_from_varied_titles() {
        assert_eq!(parse_episode_number("Episode 12 - The End"), Some(12));
        assert_eq!(parse_episode_number("episode 3"), Some(3));
        assert_eq!(parse_episode_number("Ep. 5: Title"), Some(5));
        assert_eq!(parse_episode_number("Ep 7"), Some(7));
        assert_eq!(parse_episode_number("1. The Beginning"), Some(1));
        assert_eq!(parse_episode_number("  14  "), Some(14));
        // No plausible leading number → not matched (specials, recaps).
        assert_eq!(parse_episode_number("OVA - Special"), None);
        assert_eq!(parse_episode_number("Recap"), None);
        assert_eq!(parse_episode_number(""), None);
    }

    #[test]
    fn clean_title_strips_episode_prefix_but_not_real_titles() {
        let se = |t: &str| StreamingEpisode {
            title: Some(t.to_string()),
            thumbnail: None,
            url: None,
        };
        assert_eq!(
            se("Episode 12 - The Journey's End")
                .clean_title()
                .as_deref(),
            Some("The Journey's End")
        );
        assert_eq!(
            se("3. A New Dawn").clean_title().as_deref(),
            Some("A New Dawn")
        );
        // No digit before the separator → leave the title intact.
        assert_eq!(se("Re:Zero").clean_title().as_deref(), Some("Re:Zero"));
        assert_eq!(
            se("Bake-mono Tale").clean_title().as_deref(),
            Some("Bake-mono Tale")
        );
    }

    #[test]
    fn episodes_enrich_by_matched_number_not_index() {
        // Streaming entries out of order, with a gap (no ep 2) and a special.
        let json = serde_json::json!({
            "id": 1,
            "title": { "romaji": "Show" },
            "episodes": 3,
            "streamingEpisodes": [
                { "title": "Episode 3 - Third", "thumbnail": "https://t/3.jpg" },
                { "title": "Episode 1 - First", "thumbnail": "https://t/1.jpg" },
                { "title": "Special - Recap", "thumbnail": "https://t/x.jpg" }
            ]
        });
        let media: AniListMedia = serde_json::from_value(json).unwrap();
        let count = media.effective_episode_count();
        let mut enrich: HashMap<u32, &StreamingEpisode> = HashMap::new();
        for se in &media.streaming_episodes {
            if let Some(n) = se.title.as_deref().and_then(parse_episode_number) {
                enrich.entry(n).or_insert(se);
            }
        }
        let eps: Vec<Episode> = (1..=count)
            .map(|n| {
                let se = enrich.get(&n);
                Episode {
                    season: 1,
                    number: n,
                    title: se.and_then(|s| s.clean_title()),
                    air_date: None,
                    overview: None,
                    runtime_minutes: None,
                    rating: None,
                    still: se.and_then(|s| non_empty(s.thumbnail.clone())),
                }
            })
            .collect();

        assert_eq!(eps.len(), 3);
        // Ep 1 matched by number despite being second in the list.
        assert_eq!(eps[0].title.as_deref(), Some("First"));
        assert_eq!(eps[0].still.as_deref(), Some("https://t/1.jpg"));
        // Ep 2 has no streaming entry → bare.
        assert_eq!(eps[1].title, None);
        assert_eq!(eps[1].still, None);
        // Ep 3 matched.
        assert_eq!(eps[2].title.as_deref(), Some("Third"));
    }

    /// An AniList `Media` detail response with two streaming episodes (eps 1+2)
    /// and a MAL id, used by the Jikan-enrichment tests.
    fn detail_with_mal(mal: i32) -> serde_json::Value {
        serde_json::json!({
            "data": { "Media": {
                "id": 1,
                "idMal": mal,
                "title": { "romaji": "Show" },
                "episodes": 3,
                "format": "TV",
                "streamingEpisodes": [
                    { "title": "Episode 1 - AniList One", "thumbnail": "https://t/1.jpg" },
                    { "title": "Episode 2 - AniList Two", "thumbnail": "https://t/2.jpg" }
                ]
            } }
        })
    }

    /// A Jikan `/episodes` page (titles for eps 1 and 2 only; ep 3 absent).
    fn jikan_episodes() -> serde_json::Value {
        serde_json::json!({
            "pagination": { "has_next_page": false },
            "data": [
                { "mal_id": 1, "title": "Jikan One" },
                { "mal_id": 2, "title": "Jikan Two" }
            ]
        })
    }

    #[tokio::test]
    async fn episodes_prefer_jikan_titles_then_fall_back() {
        // One mock server serves both: AniList details over POST, Jikan
        // episodes over GET /v4/anime/{mal}/episodes.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(detail_with_mal(52991)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/v4/anime/\d+/episodes$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jikan_episodes()))
            .mount(&server)
            .await;

        let provider = AniListProvider::with_bases(server.uri(), server.uri());
        let ids = IdSet::default().with_anilist(1).with_mal(52991);
        let eps = provider.episodes(&ids, 1).await.unwrap();

        assert_eq!(eps.len(), 3);
        // Eps 1+2: Jikan's title wins over the AniList streaming one.
        assert_eq!(eps[0].title.as_deref(), Some("Jikan One"));
        assert_eq!(eps[1].title.as_deref(), Some("Jikan Two"));
        // Ep 3: Jikan has no title → fall back (no streaming entry either → bare).
        assert_eq!(eps[2].title, None);
        // Stills still come from AniList's streamingEpisodes.
        assert_eq!(eps[0].still.as_deref(), Some("https://t/1.jpg"));
    }

    #[tokio::test]
    async fn episodes_without_mal_use_streaming_titles_only() {
        // No idMal → Jikan is never consulted; a GET would 404 (no GET mock)
        // but the adapter must not issue one, so behavior is unchanged.
        let server = MockServer::start().await;
        let detail = serde_json::json!({
            "data": { "Media": {
                "id": 1,
                "title": { "romaji": "Show" },
                "episodes": 2,
                "format": "TV",
                "streamingEpisodes": [
                    { "title": "Episode 1 - AniList One", "thumbnail": "https://t/1.jpg" },
                    { "title": "Episode 2 - AniList Two", "thumbnail": "https://t/2.jpg" }
                ]
            } }
        });
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(detail))
            .mount(&server)
            .await;

        let provider = AniListProvider::with_base_url(server.uri());
        let ids = IdSet::default().with_anilist(1);
        let eps = provider.episodes(&ids, 1).await.unwrap();

        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].title.as_deref(), Some("AniList One"));
        assert_eq!(eps[1].title.as_deref(), Some("AniList Two"));
    }
}
