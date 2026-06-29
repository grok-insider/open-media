//! Fribb anime-lists [`IdBridge`] — AniList/MAL → IMDB.
//!
//! Anime is discovered through AniList, which never carries an IMDB id. The
//! IMDB-keyed source path (Torrentio, and through it the debrid cache) therefore
//! short-circuits for anime, leaving them with only the anime-native (nyaa)
//! providers. This adapter closes the gap by mapping an anime's AniList/MAL id to
//! an IMDB id so the application layer can populate [`IdSet::imdb`] *before*
//! building the source query — at which point Torrentio/Real-Debrid light up for
//! anime with no change to the source providers.
//!
//! ## Data source
//! [Fribb's anime-lists] `anime-list-full.json`: a flat JSON array of objects
//! that cross-reference the major anime databases. We read three fields per
//! entry — `anilist_id`, `mal_id`, `imdb_id` — and build two lookup tables
//! (anilist→imdb, mal→imdb).
//!
//! **Partial coverage is expected and documented:** `imdb_id` is populated mainly
//! for standalone entries (anime *movies*) and a subset of series; many TV
//! entries have no `imdb_id` at all. The bridge enriches what it can and returns
//! `None` for the rest — that is correct behaviour, not a failure. Those titles
//! keep getting nyaa sources exactly as before.
//!
//! ## Delivery: fetch-and-cache (never bundled)
//! The dataset is several megabytes, so it is **not** compiled into the binary.
//! On first use it is downloaded to the XDG cache dir
//! (`<cache>/open-media/anime-id-map.json`) and reused; it is re-fetched when the
//! cached copy is older than [`MAX_AGE`]. A stale-but-present cache is preferred
//! over a failed refresh — availability beats freshness for a best-effort hint.
//!
//! ## Best-effort contract
//! Per [`IdBridge`], every failure path (no network, HTTP error, malformed JSON,
//! unwritable cache) degrades to "no enrichment" (`Ok(None)`), never an `Err`
//! that could abort the user's play/search action.
//!
//! [Fribb's anime-lists]: https://github.com/Fribb/anime-lists
//! [`IdBridge`]: open_media_core::ports::IdBridge
//! [`IdSet::imdb`]: open_media_core::model::IdSet::imdb

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use open_media_core::error::CoreResult;
use open_media_core::model::IdSet;
use open_media_core::ports::IdBridge;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::OnceCell;

/// Upstream dataset URL (raw GitHub).
const DEFAULT_URL: &str =
    "https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json";

/// Re-download the cached dataset once it is older than this (~7 days). The map
/// changes slowly (new seasons/movies), so a weekly refresh is ample.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// One raw entry from `anime-list-full.json`. We only deserialize the three id
/// fields we use; every other field (`themoviedb_id`, `type`, …) is ignored.
/// `imdb_id` is absent on most TV entries — `Option` captures that directly.
#[derive(Debug, Clone, Deserialize)]
struct FribbEntry {
    #[serde(default)]
    anilist_id: Option<i32>,
    #[serde(default)]
    mal_id: Option<i32>,
    #[serde(default)]
    imdb_id: Option<String>,
}

/// A parsed, in-memory id map: anilist→imdb and mal→imdb.
///
/// This is the **pure**, network-free core of the bridge — built from already
/// parsed entries and queried with a borrowed [`IdSet`] — so the lookup logic is
/// unit-testable offline without touching HTTP or the filesystem.
#[derive(Debug, Default)]
pub(crate) struct AnimeIdMap {
    anilist_to_imdb: HashMap<i32, String>,
    mal_to_imdb: HashMap<i32, String>,
}

impl AnimeIdMap {
    /// Build the lookup tables from raw entries.
    ///
    /// Entries without an `imdb_id` contribute nothing (they are the expected
    /// majority). A single IMDB id can legitimately be shared by several anilist
    /// ids (seasons/cours of one show mapped to one film/series id); we keep the
    /// first seen for a given key and ignore later collisions — the mapping is a
    /// hint, and any of the colliding ids resolves to the same `tt…`.
    fn from_entries(entries: impl IntoIterator<Item = FribbEntry>) -> Self {
        let mut map = AnimeIdMap::default();
        for entry in entries {
            let Some(imdb) = entry.imdb_id.as_deref() else {
                continue;
            };
            // Guard against empty/`""` imdb fields that occasionally appear.
            let imdb = imdb.trim();
            if !imdb.starts_with("tt") {
                continue;
            }
            if let Some(anilist) = entry.anilist_id {
                map.anilist_to_imdb
                    .entry(anilist)
                    .or_insert_with(|| imdb.to_string());
            }
            if let Some(mal) = entry.mal_id {
                map.mal_to_imdb
                    .entry(mal)
                    .or_insert_with(|| imdb.to_string());
            }
        }
        map
    }

    /// Parse the dataset JSON (a flat array) into a map. Tolerant by design:
    /// unknown fields are ignored and malformed individual records would fail the
    /// whole parse, so the caller treats a parse error as "no map".
    fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let entries: Vec<FribbEntry> = serde_json::from_slice(bytes)?;
        Ok(Self::from_entries(entries))
    }

    /// The IMDB id for the given ids, preferring the AniList dialect over MAL
    /// (AniList is open-media's primary anime id). `None` when neither id is
    /// present or neither has a mapping.
    pub(crate) fn imdb_for(&self, ids: &IdSet) -> Option<String> {
        if let Some(anilist) = ids.anilist {
            if let Some(imdb) = self.anilist_to_imdb.get(&anilist) {
                return Some(imdb.clone());
            }
        }
        if let Some(mal) = ids.mal {
            if let Some(imdb) = self.mal_to_imdb.get(&mal) {
                return Some(imdb.clone());
            }
        }
        None
    }
}

/// Fribb-backed [`IdBridge`]. Keyless and always constructible; the dataset is
/// fetched + cached lazily on the first lookup.
pub struct FribbIdBridge {
    client: Client,
    url: String,
    cache_path: PathBuf,
    /// Lazily-loaded map. `None` inside the cell means "we tried and have no
    /// usable map" — cached so a failed/empty load is not retried on every call.
    map: OnceCell<Option<AnimeIdMap>>,
}

impl Default for FribbIdBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl FribbIdBridge {
    /// Construct with the production dataset URL and the default cache path
    /// (`<cache>/open-media/anime-id-map.json`).
    pub fn new() -> Self {
        Self {
            client: open_media_net::client(),
            url: DEFAULT_URL.to_string(),
            cache_path: default_cache_path(),
            map: OnceCell::new(),
        }
    }

    /// Construct against a custom URL + cache path (used by tests to point at a
    /// mock server and a temp dir).
    pub fn with_url_and_cache(url: impl Into<String>, cache_path: PathBuf) -> Self {
        Self {
            client: open_media_net::client(),
            url: url.into(),
            cache_path,
            map: OnceCell::new(),
        }
    }

    /// Load the map once (memoized): read a fresh-enough cache, else download and
    /// rewrite the cache. Every failure degrades to `None` (no enrichment).
    async fn load(&self) -> &Option<AnimeIdMap> {
        self.map
            .get_or_init(|| async { self.load_uncached().await })
            .await
    }

    /// The uncached load path: cache-first with a network refresh fallback.
    async fn load_uncached(&self) -> Option<AnimeIdMap> {
        // 1. A fresh-enough cache wins — no network at all.
        if let Some(bytes) = self.read_fresh_cache() {
            match AnimeIdMap::from_json(&bytes) {
                Ok(map) => return Some(map),
                Err(e) => {
                    tracing::debug!(error = %e, "anime id-map cache parse failed; refetching")
                }
            }
        }

        // 2. Otherwise fetch, parse, and (best-effort) persist for next time.
        match self.fetch().await {
            Ok(bytes) => match AnimeIdMap::from_json(&bytes) {
                Ok(map) => {
                    self.write_cache(&bytes);
                    Some(map)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "anime id-map parse failed; bridge disabled");
                    self.stale_cache_fallback()
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "anime id-map fetch failed; bridge disabled");
                // 3. Network down but a stale cache exists → use it anyway.
                self.stale_cache_fallback()
            }
        }
    }

    /// Read the cache file only if it exists and is younger than [`MAX_AGE`].
    fn read_fresh_cache(&self) -> Option<Vec<u8>> {
        let meta = std::fs::metadata(&self.cache_path).ok()?;
        let age = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .unwrap_or(Duration::MAX);
        if age > MAX_AGE {
            return None;
        }
        std::fs::read(&self.cache_path).ok()
    }

    /// Last resort when a refresh failed: parse whatever cache exists regardless
    /// of age. A stale hint beats no hint.
    fn stale_cache_fallback(&self) -> Option<AnimeIdMap> {
        let bytes = std::fs::read(&self.cache_path).ok()?;
        AnimeIdMap::from_json(&bytes).ok()
    }

    /// Download the dataset, retrying transient transport failures.
    async fn fetch(&self) -> CoreResult<Vec<u8>> {
        use open_media_core::error::CoreError;

        let resp = open_media_net::retry(|| async {
            self.client.get(&self.url).send().await.map_err(|e| {
                if e.is_timeout() {
                    CoreError::Timeout(format!("fribb anime-lists: {e}"))
                } else {
                    CoreError::Network(format!("fribb anime-lists: {e}"))
                }
            })
        })
        .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(CoreError::Remote {
                service: "fribb anime-lists".into(),
                message: format!("HTTP {status}"),
            });
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| CoreError::Network(format!("fribb anime-lists body: {e}")))
    }

    /// Persist the raw JSON to the cache path. Best-effort: a write failure
    /// (unwritable dir, full disk) is logged and ignored — the in-memory map is
    /// already built, only next-run caching is lost.
    fn write_cache(&self, bytes: &[u8]) {
        if let Some(parent) = self.cache_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::debug!(error = %e, dir = %parent.display(), "anime id-map cache dir create failed");
                return;
            }
        }
        if let Err(e) = std::fs::write(&self.cache_path, bytes) {
            tracing::debug!(error = %e, path = %self.cache_path.display(), "anime id-map cache write failed");
        }
    }
}

#[async_trait]
impl IdBridge for FribbIdBridge {
    fn name(&self) -> &str {
        "fribb-anime-lists"
    }

    async fn imdb_for(&self, ids: &IdSet) -> CoreResult<Option<String>> {
        // Nothing to bridge from if the caller has neither anilist nor mal id.
        if ids.anilist.is_none() && ids.mal.is_none() {
            return Ok(None);
        }
        Ok(self.load().await.as_ref().and_then(|map| map.imdb_for(ids)))
    }
}

/// `<cache>/open-media/anime-id-map.json`, honouring `XDG_CACHE_HOME` then
/// `~/.cache` (mirrors the hand-rolled XDG resolution in om-history).
fn default_cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("open-media").join("anime-id-map.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small fixture mirroring the upstream shape: an anime movie with an imdb
    /// id, a series with an imdb id, and a TV entry with none (the common case).
    const FIXTURE: &str = r#"[
        { "anilist_id": 199,   "mal_id": 164,  "imdb_id": "tt0245429", "type": "MOVIE" },
        { "anilist_id": 21,    "mal_id": 21,   "imdb_id": "tt0388629", "type": "TV" },
        { "anilist_id": 154587,"mal_id": 52991,"type": "TV" },
        { "anilist_id": 1,     "mal_id": 1,    "imdb_id": "", "type": "TV" }
    ]"#;

    fn map() -> AnimeIdMap {
        AnimeIdMap::from_json(FIXTURE.as_bytes()).expect("fixture parses")
    }

    #[test]
    fn looks_up_imdb_by_anilist_id() {
        let imdb = map().imdb_for(&IdSet::default().with_anilist(199));
        assert_eq!(imdb.as_deref(), Some("tt0245429"));
    }

    #[test]
    fn looks_up_imdb_by_mal_id() {
        let imdb = map().imdb_for(&IdSet::default().with_mal(21));
        assert_eq!(imdb.as_deref(), Some("tt0388629"));
    }

    #[test]
    fn prefers_anilist_over_mal_when_both_present() {
        // anilist 199 → tt0245429; mal 21 → tt0388629. AniList wins.
        let ids = IdSet::default().with_anilist(199).with_mal(21);
        assert_eq!(map().imdb_for(&ids).as_deref(), Some("tt0245429"));
    }

    #[test]
    fn entry_without_imdb_yields_none() {
        // The TV entry (anilist 154587 / mal 52991) carries no imdb_id — the
        // expected partial-coverage case.
        let ids = IdSet::default().with_anilist(154587).with_mal(52991);
        assert_eq!(map().imdb_for(&ids), None);
    }

    #[test]
    fn empty_imdb_string_is_ignored() {
        // anilist 1 has imdb_id "" — must not be indexed as a real id.
        assert_eq!(map().imdb_for(&IdSet::default().with_anilist(1)), None);
    }

    #[test]
    fn unknown_id_yields_none() {
        assert_eq!(map().imdb_for(&IdSet::default().with_anilist(999999)), None);
    }

    #[test]
    fn no_anime_ids_yields_none() {
        // Only an imdb/tmdb id present — nothing to bridge from.
        let ids = IdSet::default().with_tmdb(157336);
        assert_eq!(map().imdb_for(&ids), None);
    }
}
