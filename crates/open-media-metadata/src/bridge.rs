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
//! that cross-reference the major anime databases. Per entry we read
//! `anilist_id`, `mal_id`, `imdb_id`, `themoviedb_id`, `kitsu_id`, and the
//! `season`/`episode_offset` maps, and build two lookup tables (anilist→entry,
//! mal→entry).
//!
//! **The upstream schema drifts** — in 2025 `imdb_id` changed from a string to
//! an *array* of strings, and `themoviedb_id` became `{"tv": id}` /
//! `{"movie": [ids]}` objects. Parsing is therefore doubly tolerant: every id
//! field accepts both the legacy and the current shape (untagged enums), and
//! records are decoded **individually** so one malformed/unknown record can
//! never zero out the other ~42k entries.
//!
//! **Partial coverage is expected and documented:** `imdb_id` is populated
//! mainly for standalone entries (anime *movies*) and a subset of series; many
//! TV entries have no `imdb_id` at all. The bridge enriches what it can and
//! returns `None` for the rest — that is correct behaviour, not a failure.
//! Those titles keep getting nyaa sources exactly as before.
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
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use open_media_core::error::CoreResult;
use open_media_core::model::IdSet;
use open_media_core::ports::{BridgedIds, IdBridge};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

/// Upstream dataset URL (raw GitHub).
const DEFAULT_URL: &str =
    "https://raw.githubusercontent.com/Fribb/anime-lists/master/anime-list-full.json";

/// Re-download the cached dataset once it is older than this (~7 days). The map
/// changes slowly (new seasons/movies), so a weekly refresh is ample.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// `imdb_id` upstream: historically a plain string, since 2025 an **array** of
/// strings (a film can have several tt ids). Accept both; empty arrays count as
/// absent.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    /// The first non-empty `tt…` id, if any.
    fn first_imdb(&self) -> Option<&str> {
        let candidates: &[String] = match self {
            OneOrMany::One(s) => std::slice::from_ref(s),
            OneOrMany::Many(v) => v.as_slice(),
        };
        candidates
            .iter()
            .map(|s| s.trim())
            .find(|s| s.starts_with("tt"))
    }
}

/// `themoviedb_id` upstream: historically a bare number, currently an object —
/// `{"tv": 119495}` for series, `{"movie": [37585, …]}` for films.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TmdbIdField {
    Plain(u64),
    Split {
        #[serde(default)]
        tv: Option<u64>,
        #[serde(default)]
        movie: Option<Vec<u64>>,
    },
}

/// The `season` / `episode_offset` objects: per-database coordinates for where
/// this AniList/MAL entry lands inside the IMDB/TMDB series (`{"tvdb": 2,
/// "tmdb": 2}`). This is what maps "The Eminence in Shadow 2nd Season" (its own
/// AniList entry, episodes numbered from 1) onto season 2 of the one tt id.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct DbCoordinates {
    #[serde(default)]
    tvdb: Option<i32>,
    #[serde(default)]
    tmdb: Option<i32>,
}

/// One raw entry from `anime-list-full.json`. Only the fields we use are
/// deserialized; the rest (`anidb_id`, `type`, …) are ignored.
#[derive(Debug, Clone, Deserialize)]
struct FribbEntry {
    #[serde(default)]
    anilist_id: Option<i32>,
    #[serde(default)]
    mal_id: Option<i32>,
    #[serde(default)]
    imdb_id: Option<OneOrMany>,
    #[serde(default)]
    themoviedb_id: Option<TmdbIdField>,
    #[serde(default)]
    kitsu_id: Option<u64>,
    #[serde(default)]
    season: Option<DbCoordinates>,
    #[serde(default)]
    episode_offset: Option<DbCoordinates>,
}

/// The cross-database ids one AniList/MAL entry bridges to.
///
/// `imdb`/`tmdb_tv` come with the **series-level** id plus the season this
/// entry occupies (`tvdb_season` follows IMDB/TVDB numbering, `tmdb_season`
/// TMDB's), because AniList numbers every season as its own entry from 1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BridgedEntry {
    pub(crate) imdb: Option<String>,
    pub(crate) tmdb_tv: Option<u64>,
    pub(crate) tmdb_movie: Option<u64>,
    pub(crate) kitsu: Option<u64>,
    pub(crate) tvdb_season: Option<i32>,
    pub(crate) tmdb_season: Option<i32>,
    pub(crate) tvdb_episode_offset: Option<i32>,
    pub(crate) tmdb_episode_offset: Option<i32>,
}

impl BridgedEntry {
    fn from_raw(entry: &FribbEntry) -> Self {
        let (tmdb_tv, tmdb_movie) = match &entry.themoviedb_id {
            // A legacy bare number can't tell tv from movie; historically the
            // dataset used it for both, so surface it under both keys and let
            // the caller pick by media kind.
            Some(TmdbIdField::Plain(id)) => (Some(*id), Some(*id)),
            Some(TmdbIdField::Split { tv, movie }) => {
                (*tv, movie.as_ref().and_then(|m| m.first().copied()))
            }
            None => (None, None),
        };
        Self {
            imdb: entry
                .imdb_id
                .as_ref()
                .and_then(|i| i.first_imdb())
                .map(str::to_string),
            tmdb_tv,
            tmdb_movie,
            kitsu: entry.kitsu_id,
            tvdb_season: entry.season.and_then(|s| s.tvdb),
            tmdb_season: entry.season.and_then(|s| s.tmdb),
            tvdb_episode_offset: entry.episode_offset.and_then(|s| s.tvdb),
            tmdb_episode_offset: entry.episode_offset.and_then(|s| s.tmdb),
        }
    }

    /// Whether the entry carries anything worth indexing.
    fn is_useful(&self) -> bool {
        self.imdb.is_some()
            || self.tmdb_tv.is_some()
            || self.tmdb_movie.is_some()
            || self.kitsu.is_some()
    }

    /// Map into the port-level DTO. Episode offsets are non-negative by
    /// construction upstream; a (theoretical) negative one is dropped.
    fn to_bridged_ids(entry: &BridgedEntry) -> BridgedIds {
        BridgedIds {
            imdb: entry.imdb.clone(),
            tmdb_tv: entry.tmdb_tv,
            tmdb_movie: entry.tmdb_movie,
            kitsu: entry.kitsu,
            imdb_season: entry.tvdb_season,
            tmdb_season: entry.tmdb_season,
            imdb_episode_offset: entry.tvdb_episode_offset.and_then(|o| o.try_into().ok()),
            tmdb_episode_offset: entry.tmdb_episode_offset.and_then(|o| o.try_into().ok()),
        }
    }
}

/// A parsed, in-memory id map: anilist→entry and mal→entry.
///
/// This is the **pure**, network-free core of the bridge — built from already
/// parsed entries and queried with a borrowed [`IdSet`] — so the lookup logic is
/// unit-testable offline without touching HTTP or the filesystem.
#[derive(Debug, Default)]
pub(crate) struct AnimeIdMap {
    by_anilist: HashMap<i32, BridgedEntry>,
    by_mal: HashMap<i32, BridgedEntry>,
}

impl AnimeIdMap {
    /// Build the lookup tables from raw entries.
    ///
    /// Entries with no bridgeable ids contribute nothing (they are the expected
    /// majority). A single IMDB id can legitimately be shared by several anilist
    /// ids (seasons/cours of one show mapped to one series id — each with its
    /// own `season` coordinate); we keep the first seen for a given key and
    /// ignore later collisions.
    fn from_entries(entries: impl IntoIterator<Item = FribbEntry>) -> Self {
        let mut map = AnimeIdMap::default();
        for entry in entries {
            let bridged = BridgedEntry::from_raw(&entry);
            if !bridged.is_useful() {
                continue;
            }
            if let Some(anilist) = entry.anilist_id {
                map.by_anilist
                    .entry(anilist)
                    .or_insert_with(|| bridged.clone());
            }
            if let Some(mal) = entry.mal_id {
                map.by_mal.entry(mal).or_insert_with(|| bridged.clone());
            }
        }
        map
    }

    /// Parse the dataset JSON (a flat array) into a map.
    ///
    /// Doubly tolerant: the array is decoded as raw values first and each record
    /// is then decoded **individually**, so a single record with a shape we
    /// don't understand is skipped (counted, debug-logged) instead of failing
    /// the whole 42k-entry parse — that failure mode silently disabled the
    /// bridge for every anime when the upstream `imdb_id` schema changed.
    /// Only a top-level failure (not a JSON array at all) is an error.
    fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let raw: Vec<serde_json::Value> = serde_json::from_slice(bytes)?;
        let total = raw.len();
        let mut skipped = 0usize;
        let entries =
            raw.into_iter()
                .filter_map(|v| match serde_json::from_value::<FribbEntry>(v) {
                    Ok(e) => Some(e),
                    Err(_) => {
                        skipped += 1;
                        None
                    }
                });
        let map = Self::from_entries(entries);
        if skipped > 0 {
            tracing::debug!(
                skipped,
                total,
                "anime id-map records skipped (unknown shape)"
            );
        }
        Ok(map)
    }

    /// The full bridged entry for the given ids, preferring the AniList dialect
    /// over MAL (AniList is open-media's primary anime id). `None` when neither
    /// id is present or neither has a mapping.
    pub(crate) fn entry_for(&self, ids: &IdSet) -> Option<&BridgedEntry> {
        if let Some(anilist) = ids.anilist {
            if let Some(entry) = self.by_anilist.get(&anilist) {
                return Some(entry);
            }
        }
        if let Some(mal) = ids.mal {
            if let Some(entry) = self.by_mal.get(&mal) {
                return Some(entry);
            }
        }
        None
    }

    /// The IMDB id for the given ids (see [`Self::entry_for`]).
    #[cfg(test)]
    pub(crate) fn imdb_for(&self, ids: &IdSet) -> Option<String> {
        self.entry_for(ids).and_then(|e| e.imdb.clone())
    }
}

/// Don't hammer the network after a failed load: subsequent lookups short-
/// circuit to "no map" until this much time has passed, then one lookup retries.
/// (A *successful* load is kept for the process lifetime.)
const RETRY_COOLDOWN: Duration = Duration::from_secs(120);

/// Lazily-loaded map state. Unlike a `OnceCell`, a **failed** load is not
/// memoized forever — it only suppresses retries for [`RETRY_COOLDOWN`], so a
/// transient network/parse hiccup at startup can't disable the bridge for the
/// whole session.
#[derive(Default)]
struct LoadState {
    map: Option<Arc<AnimeIdMap>>,
    last_failure: Option<Instant>,
}

/// Fribb-backed [`IdBridge`]. Keyless and always constructible; the dataset is
/// fetched + cached lazily on the first lookup.
pub struct FribbIdBridge {
    client: Client,
    url: String,
    cache_path: PathBuf,
    state: Mutex<LoadState>,
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
            state: Mutex::new(LoadState::default()),
        }
    }

    /// Construct against a custom URL + cache path (used by tests to point at a
    /// mock server and a temp dir).
    pub fn with_url_and_cache(url: impl Into<String>, cache_path: PathBuf) -> Self {
        Self {
            client: open_media_net::client(),
            url: url.into(),
            cache_path,
            state: Mutex::new(LoadState::default()),
        }
    }

    /// Pretend the last failure happened long ago so the next lookup retries
    /// immediately (tests exercise the retry path without waiting out
    /// [`RETRY_COOLDOWN`]).
    #[doc(hidden)]
    pub async fn expire_retry_cooldown(&self) {
        let mut state = self.state.lock().await;
        if state.last_failure.is_some() {
            state.last_failure = Instant::now().checked_sub(RETRY_COOLDOWN);
        }
    }

    /// Load the map (memoizing success): read a fresh-enough cache, else
    /// download and rewrite the cache. A failed load degrades to `None` (no
    /// enrichment) and is retried after [`RETRY_COOLDOWN`].
    async fn load(&self) -> Option<Arc<AnimeIdMap>> {
        let mut state = self.state.lock().await;
        if let Some(map) = &state.map {
            return Some(map.clone());
        }
        if let Some(failed_at) = state.last_failure {
            if failed_at.elapsed() < RETRY_COOLDOWN {
                return None;
            }
        }
        match self.load_uncached().await {
            Some(map) => {
                let map = Arc::new(map);
                state.map = Some(map.clone());
                state.last_failure = None;
                Some(map)
            }
            None => {
                state.last_failure = Some(Instant::now());
                None
            }
        }
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

    async fn resolve(&self, ids: &IdSet) -> CoreResult<Option<BridgedIds>> {
        // Nothing to bridge from if the caller has neither anilist nor mal id.
        if ids.anilist.is_none() && ids.mal.is_none() {
            return Ok(None);
        }
        Ok(self
            .load()
            .await
            .and_then(|map| map.entry_for(ids).map(BridgedEntry::to_bridged_ids)))
    }
}

/// `<cache>/open-media/anime-id-map.json`, honouring `XDG_CACHE_HOME` then
/// `~/.cache` (mirrors the hand-rolled XDG resolution in open-media-history).
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

    /// A fixture mirroring the **current** upstream shape (recorded 2026-07):
    /// `imdb_id` is an array, `themoviedb_id` is a `{tv}`/`{movie: []}` object,
    /// and later seasons are separate entries sharing the series ids with their
    /// own `season` coordinates. Entry 5 keeps the **legacy** string/plain
    /// shapes (pre-2025 schema / stale caches); entry 6 is kitsu-only; the last
    /// two exercise empty-value and unknown-shape tolerance.
    const FIXTURE: &str = r#"[
        { "type": "TV", "anilist_id": 130298, "mal_id": 48316, "imdb_id": ["tt14115938"],
          "kitsu_id": 44107, "themoviedb_id": {"tv": 119495}, "season": {"tvdb": 1, "tmdb": 1} },
        { "type": "TV", "anilist_id": 161964, "mal_id": 54595, "imdb_id": ["tt14115938"],
          "kitsu_id": 47099, "themoviedb_id": {"tv": 119495}, "season": {"tvdb": 2, "tmdb": 2} },
        { "type": "MOVIE", "anilist_id": 1441, "mal_id": 1441, "imdb_id": ["tt1920940", "tt0089206"],
          "kitsu_id": 1294, "themoviedb_id": {"movie": [37585]} },
        { "type": "OVA", "anilist_id": 190, "mal_id": 190, "imdb_id": ["tt0279570"],
          "kitsu_id": 167, "themoviedb_id": {"tv": 30992},
          "season": {"tvdb": 0, "tmdb": 0}, "episode_offset": {"tvdb": 2, "tmdb": 2} },
        { "type": "MOVIE", "anilist_id": 199, "mal_id": 164, "imdb_id": "tt0245429",
          "themoviedb_id": 4935 },
        { "type": "TV", "anilist_id": 171952, "mal_id": 57584, "kitsu_id": 48346 },
        { "type": "TV", "anilist_id": 154587, "mal_id": 52991 },
        { "type": "TV", "anilist_id": 1, "mal_id": 1, "imdb_id": [""] },
        { "type": "TV", "anilist_id": 2, "mal_id": 2, "imdb_id": { "unexpected": "shape" } }
    ]"#;

    fn map() -> AnimeIdMap {
        AnimeIdMap::from_json(FIXTURE.as_bytes()).expect("fixture parses")
    }

    #[test]
    fn looks_up_imdb_from_array_schema_by_anilist_id() {
        let imdb = map().imdb_for(&IdSet::default().with_anilist(130298));
        assert_eq!(imdb.as_deref(), Some("tt14115938"));
    }

    #[test]
    fn looks_up_imdb_by_mal_id() {
        let imdb = map().imdb_for(&IdSet::default().with_mal(48316));
        assert_eq!(imdb.as_deref(), Some("tt14115938"));
    }

    #[test]
    fn multi_imdb_array_takes_the_first_id() {
        let imdb = map().imdb_for(&IdSet::default().with_anilist(1441));
        assert_eq!(imdb.as_deref(), Some("tt1920940"));
    }

    #[test]
    fn legacy_string_schema_still_parses() {
        // Entry 5 uses the pre-2025 shapes: imdb_id string, themoviedb_id bare
        // number (as a stale on-disk cache would).
        let entry_map = map();
        let ids = IdSet::default().with_anilist(199);
        assert_eq!(entry_map.imdb_for(&ids).as_deref(), Some("tt0245429"));
        let entry = entry_map.entry_for(&ids).unwrap();
        assert_eq!(entry.tmdb_movie, Some(4935));
        assert_eq!(entry.tmdb_tv, Some(4935));
    }

    #[test]
    fn prefers_anilist_over_mal_when_both_present() {
        // anilist 199 → tt0245429; mal 48316 → tt14115938. AniList wins.
        let ids = IdSet::default().with_anilist(199).with_mal(48316);
        assert_eq!(map().imdb_for(&ids).as_deref(), Some("tt0245429"));
    }

    #[test]
    fn later_season_entry_carries_series_ids_and_season_coordinates() {
        // "2nd Season" is its own AniList entry sharing the series tt/tmdb ids;
        // the season object says where it lands in IMDB/TMDB numbering.
        let entry_map = map();
        let s2 = entry_map
            .entry_for(&IdSet::default().with_anilist(161964))
            .unwrap();
        assert_eq!(s2.imdb.as_deref(), Some("tt14115938"));
        assert_eq!(s2.tmdb_tv, Some(119495));
        assert_eq!(s2.kitsu, Some(47099));
        assert_eq!(s2.tvdb_season, Some(2));
        assert_eq!(s2.tmdb_season, Some(2));
        assert_eq!(s2.tvdb_episode_offset, None);
    }

    #[test]
    fn episode_offset_is_captured() {
        let entry_map = map();
        let ova = entry_map
            .entry_for(&IdSet::default().with_anilist(190))
            .unwrap();
        assert_eq!(ova.tvdb_season, Some(0));
        assert_eq!(ova.tmdb_episode_offset, Some(2));
    }

    #[test]
    fn kitsu_only_entry_is_indexed_without_imdb() {
        let entry_map = map();
        let ids = IdSet::default().with_anilist(171952);
        assert_eq!(entry_map.imdb_for(&ids), None);
        assert_eq!(entry_map.entry_for(&ids).unwrap().kitsu, Some(48346));
    }

    #[test]
    fn entry_without_bridgeable_ids_yields_none() {
        // The TV entry (anilist 154587 / mal 52991) carries nothing — the
        // expected partial-coverage case.
        let ids = IdSet::default().with_anilist(154587).with_mal(52991);
        assert_eq!(map().imdb_for(&ids), None);
        assert!(map().entry_for(&ids).is_none());
    }

    #[test]
    fn empty_imdb_string_is_ignored() {
        // anilist 1 has imdb_id [""] — must not be indexed as a real id.
        assert_eq!(map().imdb_for(&IdSet::default().with_anilist(1)), None);
    }

    #[test]
    fn unknown_record_shape_is_skipped_not_fatal() {
        // anilist 2 has an imdb_id object we don't understand: that one record
        // is dropped while the rest of the map parses fine — a schema change
        // must never zero out the whole bridge again.
        let entry_map = map();
        assert_eq!(entry_map.imdb_for(&IdSet::default().with_anilist(2)), None);
        assert!(entry_map
            .imdb_for(&IdSet::default().with_anilist(130298))
            .is_some());
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
