//! TorBox REST adapter (`https://api.torbox.app/v1/api`).
//!
//! TorBox differs from Real-Debrid in three ways that shape this adapter:
//! - Every response is wrapped in a `{ success, error, detail, data }` envelope.
//! - There is no file-selection step: a created torrent downloads all files, so
//!   [`DebridProvider::select_files`] is a documented no-op.
//! - Download links are minted per file via `requestdl` (they expire, so we
//!   request one fresh per playback) instead of unrestricting hoster links.
//!
//! Unlike Real-Debrid, TorBox still exposes a working bulk cache check
//! (`/torrents/checkcached`), so [`DebridProvider::check_cached`] is real here.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::ports::{AddedTorrent, DebridFile, DebridProvider};
use open_media_core::stream::{Playback, PlaybackOrigin, SourceCandidate};
use reqwest::{Client, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::Deserialize;

const DEFAULT_BASE: &str = "https://api.torbox.app/v1/api";
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "m2ts",
];

/// TorBox client.
pub struct Torbox {
    client: Client,
    token: String,
    base_url: String,
    poll_interval: Duration,
    poll_timeout: Duration,
}

impl Torbox {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            client: open_media_net::client(),
            token: token.into(),
            base_url: DEFAULT_BASE.to_string(),
            poll_interval: Duration::from_secs(2),
            poll_timeout: Duration::from_secs(300),
        }
    }

    pub fn with_base_url(token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::new(token)
        }
    }

    /// Override the poll cadence (tests use a tiny interval).
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    async fn send(&self, req: RequestBuilder) -> CoreResult<reqwest::Response> {
        let resp = req.bearer_auth(&self.token).send().await.map_err(|e| {
            if e.is_timeout() {
                CoreError::Timeout(format!("torbox: {e}"))
            } else {
                CoreError::Network(format!("torbox: {e}"))
            }
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Envelope<serde_json::Value>>(&body)
            .ok()
            .and_then(|e| e.message())
            .unwrap_or_else(|| format!("HTTP {status}"));
        Err(match code {
            401 | 403 => CoreError::Auth(format!("torbox: {message}")),
            404 => CoreError::NotFound(format!("torbox: {message}")),
            _ => CoreError::Remote {
                service: "torbox".into(),
                message,
            },
        })
    }

    /// GET `path` and unwrap the TorBox envelope into its `data` payload.
    async fn get_data<T: DeserializeOwned>(&self, path_and_query: &str) -> CoreResult<T> {
        let resp = self
            .send(
                self.client
                    .get(format!("{}{path_and_query}", self.base_url)),
            )
            .await?;
        unwrap_envelope(resp).await
    }

    async fn torrent(&self, id: &str) -> CoreResult<TorrentData> {
        // `bypass_cache` skips TorBox's 600s list cache so polling sees fresh state.
        self.get_data(&format!("/torrents/mylist?id={id}&bypass_cache=true"))
            .await
    }

    async fn delete(&self, id: &str) -> CoreResult<()> {
        let _: serde_json::Value = {
            let resp = self
                .send(
                    self.client
                        .post(format!("{}/torrents/controltorrent", self.base_url))
                        .json(&serde_json::json!({
                            "torrent_id": id,
                            "operation": "delete",
                        })),
                )
                .await?;
            unwrap_envelope(resp).await?
        };
        Ok(())
    }

    /// Mint a fresh CDN link for one file of a torrent. Links expire after a few
    /// hours, so one is requested per playback rather than stored.
    async fn request_download_link(&self, torrent_id: &str, file_id: u64) -> CoreResult<String> {
        // `requestdl` authenticates via the `token` query parameter (not the
        // bearer header). Never log this URL.
        self.get_data(&format!(
            "/torrents/requestdl?token={}&torrent_id={torrent_id}&file_id={file_id}",
            self.token
        ))
        .await
    }
}

#[async_trait]
impl DebridProvider for Torbox {
    fn name(&self) -> &str {
        "torbox"
    }

    async fn account_summary(&self) -> CoreResult<String> {
        let user: UserData = self.get_data("/user/me?settings=false").await?;
        Ok(format!(
            "{} ({}, expires {})",
            user.email.unwrap_or_else(|| "torbox user".into()),
            plan_name(user.plan),
            user.premium_expires_at.unwrap_or_else(|| "n/a".into())
        ))
    }

    async fn check_cached(&self, info_hashes: &[String]) -> CoreResult<HashMap<String, bool>> {
        if info_hashes.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out = HashMap::new();
        // The endpoint caps out around 100 hashes per request.
        for chunk in info_hashes.chunks(100) {
            let joined = chunk.join(",");
            let cached: HashMap<String, serde_json::Value> = self
                .get_data(&format!(
                    "/torrents/checkcached?hash={joined}&format=object&list_files=false"
                ))
                .await?;
            // `data` maps only *cached* hashes (lowercase) to their metadata;
            // absent means uncached.
            for hash in chunk {
                let hit = cached.contains_key(&hash.to_ascii_lowercase());
                out.insert(hash.clone(), hit);
            }
        }
        Ok(out)
    }

    async fn add_magnet(&self, magnet: &str) -> CoreResult<AddedTorrent> {
        let form = reqwest::multipart::Form::new()
            .text("magnet", magnet.to_string())
            // 1 = the account's default seeding preference.
            .text("seed", "1")
            .text("allow_zip", "false");
        let resp = self
            .send(
                self.client
                    .post(format!("{}/torrents/createtorrent", self.base_url))
                    .multipart(form),
            )
            .await?;
        let data: CreateTorrentData = unwrap_envelope(resp).await?;
        Ok(AddedTorrent {
            id: data.torrent_id.to_string(),
            name: data.name.unwrap_or_default(),
            status: "added".into(),
        })
    }

    async fn list_files(&self, torrent_id: &str) -> CoreResult<Vec<DebridFile>> {
        let torrent = self.torrent(torrent_id).await?;
        Ok(torrent
            .files
            .into_iter()
            .map(|f| DebridFile {
                id: f.id.to_string(),
                path: f.name,
                bytes: f.size,
            })
            .collect())
    }

    async fn select_files(&self, _torrent_id: &str, _file_ids: &[String]) -> CoreResult<()> {
        // TorBox has no file-selection step: a created torrent downloads all
        // files. Accepting and ignoring the call keeps the port contract.
        Ok(())
    }

    async fn unrestrict(&self, link: &str) -> CoreResult<String> {
        // TorBox has no hoster-link unrestricting in the torrent flow; the
        // nearest equivalent is resolving a `requestdl` permalink into the
        // current CDN URL, which is exactly a GET returning the usual envelope.
        let url = if link.starts_with("http") {
            link.to_string()
        } else {
            format!("{}{link}", self.base_url)
        };
        let resp = self.send(self.client.get(url)).await?;
        unwrap_envelope(resp).await
    }

    async fn resolve_playback(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
        let magnet = candidate
            .magnet_or_from_hash()
            .ok_or_else(|| CoreError::NoSource("candidate has no magnet or infohash".into()))?;

        let added = self.add_magnet(&magnet).await?;
        let id = added.id;
        let start = Instant::now();

        loop {
            let torrent = self.torrent(&id).await?;
            if is_failed_state(&torrent.download_state) {
                let _ = self.delete(&id).await;
                return Err(CoreError::Remote {
                    service: "torbox".into(),
                    message: format!("torrent status: {}", torrent.download_state),
                });
            }
            // `download_present` is TorBox's "the data is on our servers" flag —
            // the authoritative readiness signal (cached hits set it immediately).
            if torrent.download_finished && torrent.download_present {
                let file = choose_file(&torrent.files, candidate.file_index).ok_or_else(|| {
                    CoreError::Remote {
                        service: "torbox".into(),
                        message: "torrent is ready but reports no files".into(),
                    }
                })?;
                let url = self.request_download_link(&id, file.id).await?;
                return Ok(Playback {
                    url,
                    origin: PlaybackOrigin::Debrid,
                    file_name: file
                        .short_name
                        .clone()
                        .unwrap_or_else(|| basename(&file.name)),
                });
            }
            if start.elapsed() > self.poll_timeout {
                let _ = self.delete(&id).await;
                return Err(CoreError::Timeout(
                    "torbox: torrent did not become ready".into(),
                ));
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

/// Pick the file to play: the candidate's `file_index` (Torrentio's position in
/// the full file list) when valid, else the largest video file, else the largest
/// file overall.
fn choose_file(files: &[TorrentFile], file_index: Option<usize>) -> Option<&TorrentFile> {
    if let Some(idx) = file_index {
        if let Some(f) = files.get(idx) {
            return Some(f);
        }
    }
    files
        .iter()
        .filter(|f| is_video(&f.name))
        .max_by_key(|f| f.size)
        .or_else(|| files.iter().max_by_key(|f| f.size))
}

/// TorBox surfaces qBittorrent-style states; anything error-ish is terminal.
fn is_failed_state(state: &str) -> bool {
    let s = state.to_ascii_lowercase();
    s.contains("error") || s.contains("failed") || s.contains("missing")
}

fn plan_name(plan: Option<u8>) -> &'static str {
    match plan {
        Some(0) => "free",
        Some(1) => "essential",
        Some(2) => "pro",
        Some(3) => "standard",
        _ => "unknown plan",
    }
}

/// Last path segment of a file path (TorBox names include the torrent folder).
fn basename(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn is_video(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .map(|ext| VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Parse a TorBox `{ success, error, detail, data }` envelope, mapping
/// `success: false` onto [`CoreError::Remote`].
async fn unwrap_envelope<T: DeserializeOwned>(resp: reqwest::Response) -> CoreResult<T> {
    let text = resp
        .text()
        .await
        .map_err(|e| CoreError::Network(format!("torbox: {e}")))?;
    let envelope: Envelope<T> = serde_json::from_str(&text).map_err(|e| CoreError::Parse {
        what: "torbox response".into(),
        message: e.to_string(),
    })?;
    if !envelope.success {
        return Err(CoreError::Remote {
            service: "torbox".into(),
            message: envelope
                .message()
                .unwrap_or_else(|| "unspecified torbox error".into()),
        });
    }
    envelope.data.ok_or_else(|| CoreError::Parse {
        what: "torbox response".into(),
        message: "missing data field".into(),
    })
}

// --- TorBox response shapes ---

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    /// Machine error code (e.g. `"AUTH_ERROR"`); null on success.
    #[serde(default)]
    error: Option<String>,
    /// Human-readable message.
    #[serde(default)]
    detail: Option<String>,
    #[serde(default = "Option::default")]
    data: Option<T>,
}

impl<T> Envelope<T> {
    fn message(&self) -> Option<String> {
        match (&self.detail, &self.error) {
            (Some(d), Some(e)) => Some(format!("{d} ({e})")),
            (Some(d), None) => Some(d.clone()),
            (None, Some(e)) => Some(e.clone()),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UserData {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    plan: Option<u8>,
    #[serde(default)]
    premium_expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTorrentData {
    torrent_id: u64,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TorrentData {
    #[serde(default)]
    download_state: String,
    #[serde(default)]
    download_finished: bool,
    #[serde(default)]
    download_present: bool,
    #[serde(default)]
    files: Vec<TorrentFile>,
}

#[derive(Debug, Deserialize)]
struct TorrentFile {
    id: u64,
    /// Full path inside the torrent (TorBox includes the folder).
    #[serde(default)]
    name: String,
    /// Just the file name, when provided.
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: u64, name: &str, size: u64) -> TorrentFile {
        TorrentFile {
            id,
            name: name.into(),
            short_name: None,
            size,
        }
    }

    #[test]
    fn file_index_targets_full_list_position() {
        let files = vec![
            file(10, "Show/readme.txt", 10),
            file(11, "Show/S01E01.mkv", 100),
            file(12, "Show/S01E02.mkv", 100),
        ];
        let chosen = choose_file(&files, Some(2)).unwrap();
        assert_eq!(chosen.name, "Show/S01E02.mkv");
        assert_eq!(chosen.id, 12);
    }

    #[test]
    fn missing_index_falls_back_to_largest_video() {
        let files = vec![
            file(1, "Show/Extras.iso", 900),
            file(2, "Show/S01E01.mkv", 100),
            file(3, "Show/S01E02.mkv", 200),
        ];
        let chosen = choose_file(&files, None).unwrap();
        assert_eq!(chosen.name, "Show/S01E02.mkv");
    }

    #[test]
    fn no_videos_falls_back_to_largest_file() {
        let files = vec![file(1, "a.txt", 5), file(2, "b.zip", 50)];
        assert_eq!(choose_file(&files, None).unwrap().name, "b.zip");
    }

    #[test]
    fn failed_states_detected() {
        assert!(is_failed_state("error"));
        assert!(is_failed_state("failed (tracker)"));
        assert!(is_failed_state("missingFiles"));
        assert!(!is_failed_state("downloading"));
        assert!(!is_failed_state("cached"));
        assert!(!is_failed_state("stalled (no seeds)"));
    }

    #[test]
    fn basename_takes_last_segment() {
        assert_eq!(basename("Show/S01E01.mkv"), "S01E01.mkv");
        assert_eq!(basename("Movie.mkv"), "Movie.mkv");
    }
}
