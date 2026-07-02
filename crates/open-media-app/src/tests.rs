use super::*;
use crate::search::dedup_by_imdb;
use async_trait::async_trait;
use open_media_core::model::{Episode, IdSet, Season};
use open_media_core::ports::{
    Chapter, PlayOptions, PlaySession, PlaybackControl, PlaylistControl, PlaylistItem, SourceQuery,
};
use open_media_core::stream::{CacheState, Playback, PlaybackOrigin, Quality, SourceCandidate};
use open_media_core::tracking::{Activity, LibraryItem, ListStatus, SkipTimes, WatchProgress};
use std::sync::Mutex;
use std::time::Duration;

// A fake metadata provider proves the app layer works against the *port*,
// with zero network and no concrete adapter crate in scope (DIP).
struct FakeMeta;

#[async_trait]
impl MetadataProvider for FakeMeta {
    fn name(&self) -> &str {
        "fake"
    }
    async fn search(&self, query: &str, kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Ok(vec![Media {
            kind: kind.unwrap_or(MediaKind::Movie),
            ids: IdSet::default().with_imdb("tt0000000"),
            title: format!("Result for {query}"),
            original_title: None,
            year: Some(2026),
            score: None,
            overview: None,
            poster: None,
            genres: vec![],
            status: None,
            episode_count: None,
            season_count: None,
        }])
    }
    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("fake.details"))
    }
    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Ok(vec![])
    }
    async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn search_merges_provider_results() {
    let engine = Engine::builder().add_metadata(Arc::new(FakeMeta)).build();
    let results = engine
        .search("frieren", Some(MediaKind::Anime))
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].kind, MediaKind::Anime);
}

#[tokio::test]
async fn search_without_providers_errors() {
    let engine = Engine::builder().build();
    assert!(engine.search("x", None).await.is_err());
}

// A metadata provider whose `details` answers with only the ids it knows
// (here: anilist), dropping any others the caller already discovered.
struct PartialDetailsMeta;

#[async_trait]
impl MetadataProvider for PartialDetailsMeta {
    fn name(&self) -> &str {
        "partial"
    }
    async fn search(&self, _query: &str, _kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Ok(vec![])
    }
    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        // Note: no mal id — the provider only speaks anilist.
        Ok(media_with_ids(
            "Frieren",
            IdSet::default().with_anilist(154587),
        ))
    }
    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Ok(vec![])
    }
    async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn details_merges_input_ids_into_result() {
    let engine = Engine::builder()
        .add_metadata(Arc::new(PartialDetailsMeta))
        .build();
    // Caller carries a mal id discovered during search; the provider's
    // result lacks it and must not drop it.
    let media = engine
        .details(&IdSet::default().with_anilist(154587).with_mal(52991))
        .await
        .unwrap();
    assert_eq!(media.ids.mal, Some(52991));
    assert_eq!(media.ids.anilist, Some(154587));
}

fn media_with_ids(title: &str, ids: IdSet) -> Media {
    Media {
        kind: MediaKind::Movie,
        ids,
        title: title.into(),
        original_title: None,
        year: None,
        score: None,
        overview: None,
        poster: None,
        genres: vec![],
        status: None,
        episode_count: None,
        season_count: None,
    }
}

#[test]
fn dedup_collapses_shared_imdb_and_folds_ids() {
    // Same film from Cinemeta (imdb only) and TMDB (imdb + tmdb).
    let items = vec![
        media_with_ids("Interstellar", IdSet::default().with_imdb("tt0816692")),
        media_with_ids(
            "Interstellar",
            IdSet::default().with_imdb("tt0816692").with_tmdb(157336),
        ),
    ];
    let out = dedup_by_imdb(items);
    assert_eq!(out.len(), 1);
    // First occurrence kept; the later duplicate donated its tmdb id.
    assert_eq!(out[0].ids.tmdb, Some(157336));
}

#[test]
fn dedup_keeps_items_without_imdb() {
    // Two AniList anime (no imdb) must not collapse into one.
    let items = vec![
        media_with_ids("Frieren", IdSet::default().with_anilist(154587)),
        media_with_ids("Bocchi", IdSet::default().with_anilist(140960)),
    ];
    let out = dedup_by_imdb(items);
    assert_eq!(out.len(), 2);
}

struct BatchMeta {
    name: &'static str,
    delay_ms: u64,
    items: Vec<Media>,
    fail: bool,
}

#[async_trait]
impl MetadataProvider for BatchMeta {
    fn name(&self) -> &str {
        self.name
    }
    async fn search(&self, _query: &str, _kind: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        if self.fail {
            Err(CoreError::Network(format!("{} failed", self.name)))
        } else {
            Ok(self.items.clone())
        }
    }
    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("batch.details"))
    }
    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Ok(vec![])
    }
    async fn episodes(&self, _ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn incremental_search_emits_each_completed_provider() {
    let engine = Engine::builder()
        .add_metadata(Arc::new(BatchMeta {
            name: "slow",
            delay_ms: 40,
            items: vec![media_with_ids("Slow", IdSet::default().with_imdb("tt2"))],
            fail: false,
        }))
        .add_metadata(Arc::new(BatchMeta {
            name: "fast",
            delay_ms: 1,
            items: vec![media_with_ids("Fast", IdSet::default().with_imdb("tt1"))],
            fail: false,
        }))
        .build();

    let mut snapshots = Vec::new();
    let final_results = engine
        .search_incremental("x", None, |progress| {
            snapshots.push((progress.results, progress.finished));
        })
        .await
        .unwrap();

    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].0[0].title, "Fast");
    assert!(!snapshots[0].1);
    assert_eq!(snapshots[1].0.len(), 2);
    assert!(snapshots[1].1);
    assert_eq!(final_results.len(), 2);
}

#[tokio::test]
async fn incremental_search_dedups_without_reordering_visible_rows() {
    let engine = Engine::builder()
        .add_metadata(Arc::new(BatchMeta {
            name: "slow-primary",
            delay_ms: 40,
            items: vec![media_with_ids(
                "Primary",
                IdSet::default().with_imdb("tt1").with_tmdb(1),
            )],
            fail: false,
        }))
        .add_metadata(Arc::new(BatchMeta {
            name: "fast-duplicate",
            delay_ms: 1,
            items: vec![media_with_ids(
                "First Seen",
                IdSet::default().with_imdb("tt1"),
            )],
            fail: false,
        }))
        .build();

    let mut titles = Vec::new();
    let final_results = engine
        .search_incremental("x", None, |progress| {
            titles.push(
                progress
                    .results
                    .iter()
                    .map(|m| m.title.clone())
                    .collect::<Vec<_>>(),
            );
        })
        .await
        .unwrap();

    assert_eq!(titles, vec![vec!["First Seen"], vec!["First Seen"]]);
    assert_eq!(final_results.len(), 1);
    assert_eq!(final_results[0].title, "First Seen");
    assert_eq!(final_results[0].ids.tmdb, Some(1));
}

#[tokio::test]
async fn incremental_search_reports_provider_failures_without_failing() {
    let engine = Engine::builder()
        .add_metadata(Arc::new(BatchMeta {
            name: "bad",
            delay_ms: 1,
            items: vec![],
            fail: true,
        }))
        .add_metadata(Arc::new(BatchMeta {
            name: "good",
            delay_ms: 5,
            items: vec![media_with_ids("Good", IdSet::default().with_imdb("tt1"))],
            fail: false,
        }))
        .build();

    let mut failed_counts = Vec::new();
    let final_results = engine
        .search_incremental("x", None, |progress| {
            failed_counts.push((progress.failed_providers, progress.finished));
        })
        .await
        .unwrap();

    assert_eq!(failed_counts, vec![(1, false), (1, true)]);
    assert_eq!(final_results.len(), 1);
}

// ---- Binge / auto-advance ------------------------------------------------
//
// These fakes exercise the play→advance loop with zero I/O. The episode
// number flows end-to-end through the chain so the player can record exactly
// which episodes were binged: the source provider stamps the requested
// episode into the candidate title, the resolver copies it into the
// `Playback.file_name`, and the fake player parses + records it.

/// Episodic metadata with a fixed-size single season (numbers `1..=count`).
struct SeasonMeta {
    count: u32,
}

#[async_trait]
impl MetadataProvider for SeasonMeta {
    fn name(&self) -> &str {
        "season-meta"
    }
    async fn search(&self, _q: &str, _k: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Ok(vec![])
    }
    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("season-meta.details"))
    }
    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Ok(vec![Season {
            number: 1,
            episode_count: self.count,
            name: None,
        }])
    }
    async fn episodes(&self, _ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        Ok((1..=self.count)
            .map(|n| Episode {
                season,
                number: n,
                title: Some(format!("Episode {n}")),
                air_date: None,
                overview: None,
                runtime_minutes: Some(24),
                rating: None,
                still: None,
            })
            .collect())
    }
}

/// One always-resolvable candidate per request, tagged with the episode so the
/// rest of the chain can carry it through to the player.
struct StampSource;

#[async_trait]
impl SourceProvider for StampSource {
    fn name(&self) -> &str {
        "stamp"
    }
    async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        let ep = query.episode.unwrap_or(0);
        Ok(vec![SourceCandidate {
            provider: "stamp".into(),
            title: format!("ep={ep}"),
            quality: Quality::P1080,
            size_bytes: 1,
            seeders: Some(1),
            info_hash: Some("0".repeat(40)),
            magnet: None,
            direct_url: None,
            file_index: None,
            cache: CacheState::Cached,
            tags: Default::default(),
        }])
    }
}

/// Copies the candidate title (the `ep=N` stamp) into the playback file name.
struct EchoResolver;

#[async_trait]
impl StreamResolver for EchoResolver {
    async fn resolve(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
        Ok(Playback {
            url: "http://localhost/stream".into(),
            origin: PlaybackOrigin::LocalP2p,
            file_name: candidate.title.clone(),
        })
    }
}

/// A player that "completes" instantly: it records the episode number parsed
/// from the playback file name, then its session reports a watched position
/// (>= threshold) and exits. The control reports position synchronously so the
/// monitor stores it on its first tick, before `wait()` resolves.
struct RecordingPlayer {
    played: Arc<Mutex<Vec<u32>>>,
    /// fraction watched to report (controls "completed").
    fraction: f32,
}

#[async_trait]
impl Player for RecordingPlayer {
    fn name(&self) -> &str {
        "recording"
    }
    fn is_available(&self) -> bool {
        true
    }
    async fn play(
        &self,
        playback: &Playback,
        _opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        let ep = playback
            .file_name
            .strip_prefix("ep=")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        self.played.lock().unwrap().push(ep);
        Ok(Box::new(RecordingSession {
            control: Arc::new(RecordingControl {
                pos: (100.0 * self.fraction) as u32,
                dur: 100,
            }),
        }))
    }
}

struct RecordingSession {
    control: Arc<RecordingControl>,
}

#[async_trait]
impl PlaySession for RecordingSession {
    async fn wait(&mut self) -> CoreResult<()> {
        // Outlive one monitor tick (1s) so progress is stored before exit.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        Ok(())
    }
    fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
        Some(self.control.clone())
    }
}

struct RecordingControl {
    pos: u32,
    dur: u32,
}

#[async_trait]
impl PlaybackControl for RecordingControl {
    async fn position(&self) -> CoreResult<Option<u32>> {
        Ok(Some(self.pos))
    }
    async fn duration(&self) -> CoreResult<Option<u32>> {
        Ok(Some(self.dur))
    }
    async fn is_paused(&self) -> CoreResult<Option<bool>> {
        Ok(Some(false))
    }
    async fn seek_absolute(&self, _secs: u32) -> CoreResult<()> {
        Ok(())
    }
    async fn set_chapters(&self, _chapters: &[Chapter]) -> CoreResult<()> {
        Ok(())
    }
    async fn quit(&self) -> CoreResult<()> {
        Ok(())
    }
}

struct PlaylistRecordingPlayer {
    played: Arc<Mutex<Vec<u32>>>,
    appended_titles: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl Player for PlaylistRecordingPlayer {
    fn name(&self) -> &str {
        "playlist-recording"
    }
    fn is_available(&self) -> bool {
        true
    }
    async fn play(
        &self,
        playback: &Playback,
        _opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        let ep = playback
            .file_name
            .strip_prefix("ep=")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        self.played.lock().unwrap().push(ep);
        Ok(Box::new(PlaylistRecordingSession {
            control: Arc::new(PlaylistRecordingControl {
                appended_titles: self.appended_titles.clone(),
            }),
        }))
    }
}

struct PlaylistRecordingSession {
    control: Arc<PlaylistRecordingControl>,
}

#[async_trait]
impl PlaySession for PlaylistRecordingSession {
    async fn wait(&mut self) -> CoreResult<()> {
        Ok(())
    }
    fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
        Some(self.control.clone())
    }
    fn playlist_control(&self) -> Option<Arc<dyn PlaylistControl>> {
        Some(self.control.clone())
    }
}

struct PlaylistRecordingControl {
    appended_titles: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl PlaybackControl for PlaylistRecordingControl {
    async fn position(&self) -> CoreResult<Option<u32>> {
        Ok(Some(95))
    }
    async fn duration(&self) -> CoreResult<Option<u32>> {
        Ok(Some(100))
    }
    async fn is_paused(&self) -> CoreResult<Option<bool>> {
        Ok(Some(false))
    }
    async fn seek_absolute(&self, _secs: u32) -> CoreResult<()> {
        Ok(())
    }
    async fn set_chapters(&self, _chapters: &[Chapter]) -> CoreResult<()> {
        Ok(())
    }
    async fn quit(&self) -> CoreResult<()> {
        Ok(())
    }
}

#[async_trait]
impl PlaylistControl for PlaylistRecordingControl {
    async fn append(&self, item: &PlaylistItem) -> CoreResult<()> {
        self.appended_titles
            .lock()
            .unwrap()
            .push(item.title.clone());
        Ok(())
    }
    async fn active_index(&self) -> CoreResult<Option<usize>> {
        Ok(Some(0))
    }
}

/// An enricher that reports a fixed filler set (and no skip windows).
struct FillerEnricher {
    filler: Vec<u32>,
}

#[async_trait]
impl Enricher for FillerEnricher {
    async fn skip_times(
        &self,
        _ids: &IdSet,
        _episode: u32,
        _len: Option<u32>,
    ) -> CoreResult<SkipTimes> {
        Ok(SkipTimes::default())
    }
    async fn filler_episodes(&self, _ids: &IdSet) -> CoreResult<Vec<u32>> {
        Ok(self.filler.clone())
    }
}

fn episodic_media() -> Media {
    Media {
        kind: MediaKind::Anime,
        ids: IdSet::default().with_anilist(1),
        title: "Test Show".into(),
        original_title: None,
        year: None,
        score: None,
        overview: None,
        poster: None,
        genres: vec![],
        status: None,
        episode_count: None,
        season_count: None,
    }
}

fn play_request(media: Media) -> PlayRequest {
    PlayRequest {
        media,
        season: Some(1),
        episode: Some(1),
        episode_title: None,
        episode_still: None,
        episode_runtime_minutes: None,
        include_uncached: false,
    }
}

fn resolvable_candidate() -> SourceCandidate {
    SourceCandidate {
        provider: "stamp".into(),
        title: "ep=1".into(),
        quality: Quality::P1080,
        size_bytes: 1,
        seeders: Some(1),
        info_hash: Some("0".repeat(40)),
        magnet: None,
        direct_url: None,
        file_index: None,
        cache: CacheState::Cached,
        tags: Default::default(),
    }
}

#[tokio::test]
async fn binge_advances_to_season_end_then_stops() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_metadata(Arc::new(SeasonMeta { count: 3 }))
        .add_source(Arc::new(StampSource))
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played: played.clone(),
            fraction: 0.95, // every episode completes → keep advancing.
        }))
        .autoplay_next(true)
        .build();

    engine
        .play(&play_request(episodic_media()), &resolvable_candidate())
        .await
        .unwrap();

    // Played E1 (initial) then auto-advanced E2, E3, and stopped at the
    // season's last episode (no E4).
    assert_eq!(*played.lock().unwrap(), vec![1, 2, 3]);
}

#[tokio::test]
async fn binge_disabled_does_not_advance() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_metadata(Arc::new(SeasonMeta { count: 3 }))
        .add_source(Arc::new(StampSource))
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played: played.clone(),
            fraction: 0.95,
        }))
        .autoplay_next(false)
        .build();

    engine
        .play(&play_request(episodic_media()), &resolvable_candidate())
        .await
        .unwrap();

    // Only the initial episode played; no auto-advance.
    assert_eq!(*played.lock().unwrap(), vec![1]);
}

#[tokio::test]
async fn binge_stops_when_episode_not_completed() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_metadata(Arc::new(SeasonMeta { count: 3 }))
        .add_source(Arc::new(StampSource))
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played: played.clone(),
            fraction: 0.10, // quit at low progress → no advance.
        }))
        .autoplay_next(true)
        .build();

    engine
        .play(&play_request(episodic_media()), &resolvable_candidate())
        .await
        .unwrap();

    assert_eq!(*played.lock().unwrap(), vec![1]);
}

#[tokio::test]
async fn binge_skips_filler_episodes() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_metadata(Arc::new(SeasonMeta { count: 4 }))
        .add_source(Arc::new(StampSource))
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played: played.clone(),
            fraction: 0.95,
        }))
        .enricher(Arc::new(FillerEnricher { filler: vec![2, 3] }))
        .skip_filler(true)
        .autoplay_next(true)
        .build();

    engine
        .play(&play_request(episodic_media()), &resolvable_candidate())
        .await
        .unwrap();

    // E2 and E3 are filler → bridged over: E1 then E4.
    assert_eq!(*played.lock().unwrap(), vec![1, 4]);
}

#[tokio::test]
async fn playlist_player_appends_next_episode_without_new_launch() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let appended_titles = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_metadata(Arc::new(SeasonMeta { count: 3 }))
        .add_source(Arc::new(StampSource))
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(PlaylistRecordingPlayer {
            played: played.clone(),
            appended_titles: appended_titles.clone(),
        }))
        .autoplay_next(true)
        .build();

    engine
        .play(&play_request(episodic_media()), &resolvable_candidate())
        .await
        .unwrap();

    assert_eq!(*played.lock().unwrap(), vec![1]);
    let titles = appended_titles.lock().unwrap();
    assert_eq!(titles.len(), 1);
    assert!(titles[0].as_deref().unwrap_or_default().contains("S01E02"));
}

struct RecordingPresence {
    images: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl PresenceReporter for RecordingPresence {
    async fn update(&self, activity: &Activity) -> CoreResult<()> {
        self.images.lock().unwrap().push(activity.image_url.clone());
        Ok(())
    }
    async fn clear(&self) -> CoreResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn presence_prefers_episode_still_over_poster() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let images = Arc::new(Mutex::new(Vec::new()));
    let mut req = play_request(episodic_media());
    req.media.poster = Some("https://img.example/poster.jpg".into());
    req.episode_still = Some("https://img.example/still.jpg".into());
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played,
            fraction: 0.95,
        }))
        .presence(Arc::new(RecordingPresence {
            images: images.clone(),
        }))
        .build();

    engine.play(&req, &resolvable_candidate()).await.unwrap();

    assert!(images
        .lock()
        .unwrap()
        .iter()
        .any(|url| url.as_deref() == Some("https://img.example/still.jpg")));
}

// ---- behavior.resume gating ---------------------------------------------
//
// A history store that always has a saved position, plus a player that
// captures the `start_at_secs` it was handed, isolate exactly what the
// `resume` flag controls: whether the saved position becomes a start seek.

/// History with a fixed saved position; records every `save` it receives so
/// the test can assert progress is still recorded when resume is off.
struct SavedHistory {
    position: u32,
    saved: Arc<Mutex<Vec<u32>>>,
}

#[derive(Default)]
struct MemoryLibrary {
    items: Arc<Mutex<Vec<LibraryItem>>>,
}

impl LibraryStore for MemoryLibrary {
    fn upsert(&self, item: &LibraryItem) -> CoreResult<()> {
        let mut items = self.items.lock().unwrap();
        if let Some(existing) = items.iter_mut().find(|i| i.media_key == item.media_key) {
            *existing = item.clone();
        } else {
            items.push(item.clone());
        }
        Ok(())
    }

    fn list(&self, status: Option<ListStatus>) -> CoreResult<Vec<LibraryItem>> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|item| status.is_none_or(|s| item.status == s))
            .cloned()
            .collect())
    }
}

struct FailingStatusTracker;

#[async_trait]
impl Tracker for FailingStatusTracker {
    fn name(&self) -> &str {
        "failing-status"
    }
    async fn update_progress(&self, _ids: &IdSet, _episode: u32) -> CoreResult<()> {
        Ok(())
    }
    async fn set_status(&self, _ids: &IdSet, _status: ListStatus) -> CoreResult<()> {
        Err(CoreError::Network("tracker down".into()))
    }
    async fn rate(&self, _ids: &IdSet, _score: f32) -> CoreResult<()> {
        Ok(())
    }
}

impl HistoryStore for SavedHistory {
    fn save(&self, progress: &WatchProgress) -> CoreResult<()> {
        self.saved.lock().unwrap().push(progress.position_secs);
        Ok(())
    }
    fn resume(
        &self,
        media_key: &str,
        season: u32,
        episode: u32,
    ) -> CoreResult<Option<WatchProgress>> {
        Ok(Some(WatchProgress {
            media_key: media_key.into(),
            season,
            episode,
            position_secs: self.position,
            duration_secs: 1000,
            updated_at: 0,
        }))
    }
    fn recent(&self, _limit: usize) -> CoreResult<Vec<WatchProgress>> {
        Ok(vec![])
    }
}

/// A player that records the `start_at_secs` it was asked to start at, then
/// exits immediately. Used to observe how the engine gated the resume seek.
struct StartCapturePlayer {
    start: Arc<Mutex<Option<Option<u32>>>>,
}

#[async_trait]
impl Player for StartCapturePlayer {
    fn name(&self) -> &str {
        "start-capture"
    }
    fn is_available(&self) -> bool {
        true
    }
    async fn play(
        &self,
        _playback: &Playback,
        opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        *self.start.lock().unwrap() = Some(opts.start_at_secs);
        Ok(Box::new(InstantSession))
    }
}

/// A session with no control channel that exits at once: the play path runs
/// through to teardown without waiting on a monitor tick.
struct InstantSession;

#[async_trait]
impl PlaySession for InstantSession {
    async fn wait(&mut self) -> CoreResult<()> {
        Ok(())
    }
    fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
        None
    }
}

fn movie_request() -> PlayRequest {
    PlayRequest {
        media: media_with_ids("Interstellar", IdSet::default().with_imdb("tt0816692")),
        season: None,
        episode: None,
        episode_title: None,
        episode_still: None,
        episode_runtime_minutes: None,
        include_uncached: false,
    }
}

#[tokio::test]
async fn manual_library_status_persists_when_tracker_sync_fails() {
    let library = Arc::new(MemoryLibrary::default());
    let engine = Engine::builder()
        .library(library.clone())
        .tracker(Arc::new(FailingStatusTracker))
        .build();

    let item = engine
        .set_library_status(&movie_request().media, ListStatus::Planning)
        .await
        .unwrap();

    assert_eq!(item.status, ListStatus::Planning);
    let items = library.list(Some(ListStatus::Planning)).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Interstellar");
}

#[tokio::test]
async fn playback_updates_movie_library_to_completed() {
    let library = Arc::new(MemoryLibrary::default());
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played: Arc::new(Mutex::new(Vec::new())),
            fraction: 0.95,
        }))
        .library(library.clone())
        .build();

    engine
        .play(&movie_request(), &resolvable_candidate())
        .await
        .unwrap();

    let items = library.list(None).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, ListStatus::Completed);
    assert_eq!(items[0].position_secs, 95);
    assert_eq!(items[0].duration_secs, 100);
}

#[tokio::test]
async fn playback_updates_episodic_library_progress_as_watching() {
    let library = Arc::new(MemoryLibrary::default());
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(RecordingPlayer {
            played: Arc::new(Mutex::new(Vec::new())),
            fraction: 0.95,
        }))
        .library(library.clone())
        .build();

    engine
        .play(&play_request(episodic_media()), &resolvable_candidate())
        .await
        .unwrap();

    let items = library.list(None).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, ListStatus::Watching);
    assert_eq!(items[0].last_season, Some(1));
    assert_eq!(items[0].last_episode, Some(1));
}

#[tokio::test]
async fn resume_disabled_starts_from_zero_despite_saved_position() {
    let start = Arc::new(Mutex::new(None));
    let saved = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(StartCapturePlayer {
            start: start.clone(),
        }))
        .history(Arc::new(SavedHistory {
            position: 120,
            saved: saved.clone(),
        }))
        .resume(false)
        .build();

    engine
        .play(&movie_request(), &resolvable_candidate())
        .await
        .unwrap();

    // History has a 120s position, but resume is off → no start seek.
    assert_eq!(*start.lock().unwrap(), Some(None));
}

#[tokio::test]
async fn resume_enabled_starts_from_saved_position() {
    let start = Arc::new(Mutex::new(None));
    let saved = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(StartCapturePlayer {
            start: start.clone(),
        }))
        .history(Arc::new(SavedHistory {
            position: 120,
            saved: saved.clone(),
        }))
        .resume(true)
        .build();

    engine
        .play(&movie_request(), &resolvable_candidate())
        .await
        .unwrap();

    // resume on (the default) → the saved 120s becomes the start seek.
    assert_eq!(*start.lock().unwrap(), Some(Some(120)));
}

// ---- subtitle auto-fetch -------------------------------------------------
//
// A fake provider returns canned tracks; a player captures the `extra_args`
// it was handed so the test can assert each track became a `--sub-file=`
// pointing at a real temp file. After `play` returns, the temp files must be
// gone (best-effort cleanup ran).

use open_media_core::ports::SubtitleProvider;
use open_media_core::subtitle::{SubtitleQuery, SubtitleTrack};

/// A subtitle provider returning a fixed set of tracks, recording the query it
/// was asked (so the test can assert the languages were threaded through).
struct FakeSubs {
    tracks: Vec<SubtitleTrack>,
    seen_langs: Arc<Mutex<Option<Vec<String>>>>,
}

#[async_trait]
impl SubtitleProvider for FakeSubs {
    fn name(&self) -> &str {
        "fake-subs"
    }
    async fn fetch(&self, query: &SubtitleQuery) -> CoreResult<Vec<SubtitleTrack>> {
        *self.seen_langs.lock().unwrap() = Some(query.languages.clone());
        Ok(self.tracks.clone())
    }
}

/// A subtitle provider that always errors — proves playback continues with no
/// subtitles when the fetch fails.
struct FailingSubs;

#[async_trait]
impl SubtitleProvider for FailingSubs {
    fn name(&self) -> &str {
        "failing-subs"
    }
    async fn fetch(&self, _query: &SubtitleQuery) -> CoreResult<Vec<SubtitleTrack>> {
        Err(CoreError::Network("boom".into()))
    }
}

/// A player that records the `extra_args` it was handed, then exits at once.
struct ArgsCapturePlayer {
    args: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Player for ArgsCapturePlayer {
    fn name(&self) -> &str {
        "args-capture"
    }
    fn is_available(&self) -> bool {
        true
    }
    async fn play(
        &self,
        _playback: &Playback,
        opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        *self.args.lock().unwrap() = opts.extra_args.clone();
        Ok(Box::new(InstantSession))
    }
}

fn sub_track(format: &str, text: &str) -> SubtitleTrack {
    SubtitleTrack {
        language: "en".into(),
        format: format.into(),
        text: text.into(),
        title: None,
    }
}

#[tokio::test]
async fn subtitles_become_sub_file_args_and_are_cleaned_up() {
    let args = Arc::new(Mutex::new(Vec::new()));
    let seen_langs = Arc::new(Mutex::new(None));
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(ArgsCapturePlayer { args: args.clone() }))
        .subtitles(Arc::new(FakeSubs {
            tracks: vec![sub_track("srt", "1\n00:00:01,000 --> 00:00:02,000\nHi\n")],
            seen_langs: seen_langs.clone(),
        }))
        .subtitle_languages(vec!["en".into(), "ja".into()])
        .build();

    engine
        .play(&movie_request(), &resolvable_candidate())
        .await
        .unwrap();

    // One track → one --sub-file arg, pointing at a real .srt path.
    let captured = args.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let path = captured[0]
        .strip_prefix("--sub-file=")
        .expect("arg is a --sub-file=");
    assert!(path.ends_with(".srt"), "extension follows the track format");
    // The configured languages were threaded into the query.
    assert_eq!(
        seen_langs.lock().unwrap().clone(),
        Some(vec!["en".to_string(), "ja".to_string()])
    );
    // Cleanup ran after the player exited.
    assert!(
        !std::path::Path::new(path).exists(),
        "temp subtitle file removed after playback"
    );
}

#[tokio::test]
async fn no_subtitle_provider_means_no_extra_args() {
    let args = Arc::new(Mutex::new(vec!["sentinel".to_string()]));
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(ArgsCapturePlayer { args: args.clone() }))
        .build();

    engine
        .play(&movie_request(), &resolvable_candidate())
        .await
        .unwrap();

    // The player was handed an empty extra_args (overwrote the sentinel).
    assert!(args.lock().unwrap().is_empty());
}

#[tokio::test]
async fn subtitle_fetch_error_does_not_block_playback() {
    let args = Arc::new(Mutex::new(vec!["sentinel".to_string()]));
    let engine = Engine::builder()
        .resolver(Arc::new(EchoResolver))
        .player(Arc::new(ArgsCapturePlayer { args: args.clone() }))
        .subtitles(Arc::new(FailingSubs))
        .subtitle_languages(vec!["en".into()])
        .build();

    // Playback still succeeds; no subtitle args were added.
    engine
        .play(&movie_request(), &resolvable_candidate())
        .await
        .unwrap();
    assert!(args.lock().unwrap().is_empty());
}

// ---- across-candidate source failover ------------------------------------
//
// The resolver rejects a named candidate (simulating a dead torrent / debrid
// that won't cache the hash) and resolves anything else. With two ranked
// candidates where the first is the rejected one, `play_with_fallback` must
// fall through to the second and play it — not abort.

/// A resolver that errors for one specific candidate title and echoes any
/// other into a playable URL. Records every title it was asked to resolve so
/// the test can assert the order candidates were tried.
struct FailingResolver {
    fail_title: String,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl StreamResolver for FailingResolver {
    async fn resolve(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
        self.seen.lock().unwrap().push(candidate.title.clone());
        if candidate.title == self.fail_title {
            return Err(CoreError::NoSource(format!(
                "cannot resolve {}",
                candidate.title
            )));
        }
        Ok(Playback {
            url: "http://localhost/stream".into(),
            origin: PlaybackOrigin::LocalP2p,
            file_name: candidate.title.clone(),
        })
    }
}

/// A player that records the playback file name it was handed, then exits at
/// once (no monitor). Lets the test assert *which* candidate actually played.
struct PlayedCapturePlayer {
    played: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Player for PlayedCapturePlayer {
    fn name(&self) -> &str {
        "played-capture"
    }
    fn is_available(&self) -> bool {
        true
    }
    async fn play(
        &self,
        playback: &Playback,
        _opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        self.played.lock().unwrap().push(playback.file_name.clone());
        Ok(Box::new(InstantSession))
    }
}

fn named_candidate(title: &str) -> SourceCandidate {
    SourceCandidate {
        provider: title.into(),
        title: title.into(),
        quality: Quality::P1080,
        size_bytes: 1,
        seeders: Some(1),
        info_hash: Some("0".repeat(40)),
        magnet: None,
        direct_url: None,
        file_index: None,
        cache: CacheState::Cached,
        tags: Default::default(),
    }
}

#[tokio::test]
async fn play_falls_through_to_next_candidate_on_resolve_failure() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let played = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .resolver(Arc::new(FailingResolver {
            fail_title: "first".into(),
            seen: seen.clone(),
        }))
        .player(Arc::new(PlayedCapturePlayer {
            played: played.clone(),
        }))
        .build();

    let candidates = vec![named_candidate("first"), named_candidate("second")];
    engine
        .play_with_fallback(&movie_request(), &candidates)
        .await
        .unwrap();

    // The resolver was asked for "first" (failed) then "second" (succeeded).
    assert_eq!(*seen.lock().unwrap(), vec!["first", "second"]);
    // Only the second candidate actually reached the player.
    assert_eq!(*played.lock().unwrap(), vec!["second".to_string()]);
}

#[tokio::test]
async fn play_errors_when_all_candidates_fail_to_resolve() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let played = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        // Resolver fails for "first"; the only other candidate is
        // non-resolvable, so nothing can play.
        .resolver(Arc::new(FailingResolver {
            fail_title: "first".into(),
            seen: seen.clone(),
        }))
        .player(Arc::new(PlayedCapturePlayer {
            played: played.clone(),
        }))
        .build();

    let candidates = vec![named_candidate("first")];
    let err = engine
        .play_with_fallback(&movie_request(), &candidates)
        .await
        .unwrap_err();

    assert!(matches!(err, CoreError::NoSource(_)));
    assert!(played.lock().unwrap().is_empty());
}

// ---- IdBridge: anime → IMDB enrichment before source lookup -------------
//
// Proves the app-layer contract against the *port*: an anime Media with no
// imdb id gets `ids.imdb` populated from the bridge before the SourceQuery is
// built, so an IMDB-keyed source provider sees the bridged id.

/// A bridge that returns fixed bridged ids for any ids carrying an anime id.
struct FakeBridge {
    bridged: Option<open_media_core::ports::BridgedIds>,
}

impl FakeBridge {
    /// The common case: bridge to just an IMDB id.
    fn imdb(id: &str) -> Self {
        Self {
            bridged: Some(open_media_core::ports::BridgedIds {
                imdb: Some(id.to_string()),
                ..Default::default()
            }),
        }
    }
}

#[async_trait]
impl IdBridge for FakeBridge {
    fn name(&self) -> &str {
        "fake-bridge"
    }
    async fn resolve(&self, ids: &IdSet) -> CoreResult<Option<open_media_core::ports::BridgedIds>> {
        // Mirror the real bridge: only answer when there's an anime id to
        // bridge from.
        if ids.anilist.is_some() || ids.mal.is_some() {
            Ok(self.bridged.clone())
        } else {
            Ok(None)
        }
    }
}

/// A source provider that records the imdb id present on each query's media
/// (so the test can assert what the bridge contributed) and returns nothing.
struct ImdbCaptureSource {
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl SourceProvider for ImdbCaptureSource {
    fn name(&self) -> &str {
        "imdb-capture"
    }
    async fn find(&self, query: &SourceQuery) -> CoreResult<Vec<SourceCandidate>> {
        self.seen.lock().unwrap().push(query.media.ids.imdb.clone());
        Ok(vec![])
    }
}

fn anime_no_imdb() -> Media {
    Media {
        kind: MediaKind::Anime,
        ids: IdSet::default().with_anilist(199),
        title: "Spirited Away".into(),
        original_title: None,
        year: None,
        score: None,
        overview: None,
        poster: None,
        genres: vec![],
        status: None,
        episode_count: None,
        season_count: None,
    }
}

#[tokio::test]
async fn bridge_fills_imdb_for_anime_before_source_query() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_source(Arc::new(ImdbCaptureSource { seen: seen.clone() }))
        .id_bridge(Arc::new(FakeBridge::imdb("tt0245429")))
        .build();

    let req = play_request(anime_no_imdb());
    engine.find_sources(&req).await.unwrap();

    // The source provider saw the bridged IMDB id, not None.
    assert_eq!(
        *seen.lock().unwrap(),
        vec![Some("tt0245429".to_string())],
        "anime with no imdb should be enriched before the source query"
    );
    // The caller's request media is untouched (enrichment is on a clone).
    assert_eq!(req.media.ids.imdb, None);
}

#[tokio::test]
async fn bridge_miss_leaves_anime_without_imdb() {
    // Bridge returns None (the common partial-coverage case) → no enrichment.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_source(Arc::new(ImdbCaptureSource { seen: seen.clone() }))
        .id_bridge(Arc::new(FakeBridge { bridged: None }))
        .build();

    engine
        .find_sources(&play_request(anime_no_imdb()))
        .await
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![None]);
}

#[tokio::test]
async fn bridge_not_invoked_without_bridge_wired() {
    // No bridge → anime keeps its missing imdb (nyaa-only path), no panic.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_source(Arc::new(ImdbCaptureSource { seen: seen.clone() }))
        .build();

    engine
        .find_sources(&play_request(anime_no_imdb()))
        .await
        .unwrap();

    assert_eq!(*seen.lock().unwrap(), vec![None]);
}

// ---- Bridged episode-detail overlay (anime stills/synopsis) ----------------
//
// AniList serves anime episode lists with titles but no stills/synopses; the
// TMDB/IMDB-keyed providers have them but can't be queried without the bridged
// series id + the season this entry occupies. These tests prove the overlay in
// `Engine::episodes` against fake ports.

fn bare_episode(number: u32, title: &str) -> Episode {
    Episode {
        season: 1,
        number,
        title: Some(title.to_string()),
        air_date: None,
        overview: None,
        runtime_minutes: None,
        rating: None,
        still: None,
    }
}

/// An "AniList-like" provider: bare titled episodes, only for anilist-keyed ids.
struct AnimeEpisodesMeta;

#[async_trait]
impl MetadataProvider for AnimeEpisodesMeta {
    fn name(&self) -> &str {
        "fake-anilist"
    }
    async fn search(&self, _q: &str, _k: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Ok(vec![])
    }
    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("fake.details"))
    }
    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Ok(vec![])
    }
    async fn episodes(&self, ids: &IdSet, _season: u32) -> CoreResult<Vec<Episode>> {
        if ids.anilist.is_none() {
            return Err(CoreError::NotFound("needs an anilist id".into()));
        }
        Ok(vec![
            bare_episode(1, "The Hated Classmate"),
            bare_episode(2, "Shadow Garden Is Born"),
        ])
    }
}

/// A "TMDB-like" provider: rich episodes, only for tmdb-keyed ids, recording
/// the season it was asked for.
struct RichEpisodesMeta {
    /// Which id dialect unlocks this provider: `"tmdb"` or `"imdb"`.
    keyed_by: &'static str,
    asked_season: Arc<Mutex<Vec<u32>>>,
}

#[async_trait]
impl MetadataProvider for RichEpisodesMeta {
    fn name(&self) -> &str {
        "fake-rich"
    }
    async fn search(&self, _q: &str, _k: Option<MediaKind>) -> CoreResult<Vec<Media>> {
        Ok(vec![])
    }
    async fn details(&self, _ids: &IdSet) -> CoreResult<Media> {
        Err(CoreError::NotImplemented("fake.details"))
    }
    async fn seasons(&self, _ids: &IdSet) -> CoreResult<Vec<Season>> {
        Ok(vec![])
    }
    async fn episodes(&self, ids: &IdSet, season: u32) -> CoreResult<Vec<Episode>> {
        let unlocked = match self.keyed_by {
            "tmdb" => ids.tmdb.is_some(),
            _ => ids.imdb.is_some(),
        };
        if !unlocked {
            return Err(CoreError::NotFound("wrong id dialect".into()));
        }
        self.asked_season.lock().unwrap().push(season);
        Ok((1..=4)
            .map(|n| Episode {
                season,
                number: n,
                title: Some(format!("Rich title {n}")),
                air_date: Some("2023-10-04".into()),
                overview: Some(format!("Synopsis {n}")),
                runtime_minutes: Some(24),
                rating: Some(8.0),
                still: Some(format!("https://img.example/e{n}.jpg")),
            })
            .collect())
    }
}

fn bridged_tmdb(season: i32, offset: Option<u32>) -> open_media_core::ports::BridgedIds {
    open_media_core::ports::BridgedIds {
        tmdb_tv: Some(119495),
        tmdb_season: Some(season),
        tmdb_episode_offset: offset,
        ..Default::default()
    }
}

#[tokio::test]
async fn anime_episodes_gain_stills_and_synopsis_from_bridged_tmdb() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::builder()
        .add_metadata(Arc::new(AnimeEpisodesMeta))
        .add_metadata(Arc::new(RichEpisodesMeta {
            keyed_by: "tmdb",
            asked_season: asked.clone(),
        }))
        .id_bridge(Arc::new(FakeBridge {
            bridged: Some(bridged_tmdb(2, None)),
        }))
        .build();

    let ids = IdSet::default().with_anilist(161964);
    let eps = engine.episodes(&ids, 1).await.unwrap();

    // The overlay queried the bridged season (2), not the AniList season (1).
    assert_eq!(*asked.lock().unwrap(), vec![2]);
    assert_eq!(eps.len(), 2);
    // AniList titles are kept; missing details are filled from the overlay.
    assert_eq!(eps[0].title.as_deref(), Some("The Hated Classmate"));
    assert_eq!(eps[0].still.as_deref(), Some("https://img.example/e1.jpg"));
    assert_eq!(eps[0].overview.as_deref(), Some("Synopsis 1"));
    assert_eq!(eps[0].runtime_minutes, Some(24));
    assert_eq!(eps[1].still.as_deref(), Some("https://img.example/e2.jpg"));
}

#[tokio::test]
async fn overlay_applies_episode_offset() {
    // The entry starts partway into the bridged season: entry episode 1 is
    // season episode 3 (offset 2).
    let engine = Engine::builder()
        .add_metadata(Arc::new(AnimeEpisodesMeta))
        .add_metadata(Arc::new(RichEpisodesMeta {
            keyed_by: "tmdb",
            asked_season: Arc::new(Mutex::new(Vec::new())),
        }))
        .id_bridge(Arc::new(FakeBridge {
            bridged: Some(bridged_tmdb(1, Some(2))),
        }))
        .build();

    let eps = engine
        .episodes(&IdSet::default().with_anilist(190), 1)
        .await
        .unwrap();
    assert_eq!(eps[0].still.as_deref(), Some("https://img.example/e3.jpg"));
    assert_eq!(eps[0].overview.as_deref(), Some("Synopsis 3"));
    assert_eq!(eps[1].still.as_deref(), Some("https://img.example/e4.jpg"));
}

#[tokio::test]
async fn overlay_falls_back_to_imdb_keyed_provider() {
    // No TMDB mapping — the keyless (Cinemeta-like) IMDB-keyed provider serves.
    let engine = Engine::builder()
        .add_metadata(Arc::new(AnimeEpisodesMeta))
        .add_metadata(Arc::new(RichEpisodesMeta {
            keyed_by: "imdb",
            asked_season: Arc::new(Mutex::new(Vec::new())),
        }))
        .id_bridge(Arc::new(FakeBridge {
            bridged: Some(open_media_core::ports::BridgedIds {
                imdb: Some("tt14115938".into()),
                imdb_season: Some(1),
                ..Default::default()
            }),
        }))
        .build();

    let eps = engine
        .episodes(&IdSet::default().with_anilist(130298), 1)
        .await
        .unwrap();
    assert_eq!(eps[0].still.as_deref(), Some("https://img.example/e1.jpg"));
}

#[tokio::test]
async fn overlay_misses_leave_episode_list_untouched() {
    // Bridge has no mapping → the bare AniList list survives unchanged.
    let engine = Engine::builder()
        .add_metadata(Arc::new(AnimeEpisodesMeta))
        .id_bridge(Arc::new(FakeBridge { bridged: None }))
        .build();

    let eps = engine
        .episodes(&IdSet::default().with_anilist(1), 1)
        .await
        .unwrap();
    assert_eq!(eps.len(), 2);
    assert!(eps[0].still.is_none());
    assert!(eps[0].overview.is_none());

    // And with no bridge wired at all.
    let engine = Engine::builder()
        .add_metadata(Arc::new(AnimeEpisodesMeta))
        .build();
    let eps = engine
        .episodes(&IdSet::default().with_anilist(1), 1)
        .await
        .unwrap();
    assert_eq!(eps.len(), 2);
    assert!(eps[0].still.is_none());
}
