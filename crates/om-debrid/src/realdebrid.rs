//! Real-Debrid REST adapter (`https://api.real-debrid.com/rest/1.0`).
//!
//! Implements the canonical lifecycle that turns a magnet into an instant CDN
//! URL: `addMagnet` → poll `info` → `selectFiles` → poll → `unrestrict`. Auth is
//! a bearer token; POST bodies are form-encoded.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::ports::{AddedTorrent, DebridFile, DebridProvider};
use om_core::stream::{Playback, PlaybackOrigin, SourceCandidate};
use reqwest::{Client, RequestBuilder};
use serde::Deserialize;

const DEFAULT_BASE: &str = "https://api.real-debrid.com/rest/1.0";
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "m2ts",
];

/// Real-Debrid client.
pub struct RealDebrid {
    client: Client,
    token: String,
    base_url: String,
    poll_interval: Duration,
    poll_timeout: Duration,
}

impl RealDebrid {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
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
                CoreError::Timeout(format!("real-debrid: {e}"))
            } else {
                CoreError::Network(format!("real-debrid: {e}"))
            }
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<RdError>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| format!("HTTP {status}"));
        Err(match code {
            401 | 403 => CoreError::Auth(format!("real-debrid: {message}")),
            404 => CoreError::NotFound(format!("real-debrid: {message}")),
            _ => CoreError::Remote {
                service: "real-debrid".into(),
                message,
            },
        })
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> CoreResult<T> {
        let resp = self
            .send(self.client.get(format!("{}{path}", self.base_url)))
            .await?;
        parse_json(resp).await
    }

    async fn post_form_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> CoreResult<T> {
        let resp = self
            .send(
                self.client
                    .post(format!("{}{path}", self.base_url))
                    .form(form),
            )
            .await?;
        parse_json(resp).await
    }

    async fn post_form_unit(&self, path: &str, form: &[(&str, &str)]) -> CoreResult<()> {
        self.send(
            self.client
                .post(format!("{}{path}", self.base_url))
                .form(form),
        )
        .await?;
        Ok(())
    }

    async fn info(&self, id: &str) -> CoreResult<TorrentInfo> {
        self.get_json(&format!("/torrents/info/{id}")).await
    }

    async fn delete(&self, id: &str) -> CoreResult<()> {
        self.send(
            self.client
                .delete(format!("{}/torrents/delete/{id}", self.base_url)),
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl DebridProvider for RealDebrid {
    fn name(&self) -> &str {
        "real-debrid"
    }

    async fn account_summary(&self) -> CoreResult<String> {
        let user: UserInfo = self.get_json("/user").await?;
        Ok(format!(
            "{} ({}, expires {})",
            user.username,
            user.account_type.unwrap_or_else(|| "unknown".into()),
            user.expiration.unwrap_or_else(|| "n/a".into())
        ))
    }

    async fn check_cached(&self, _info_hashes: &[String]) -> CoreResult<HashMap<String, bool>> {
        // Real-Debrid deprecated /torrents/instantAvailability in 2024; cache
        // state comes from the source addon's flags instead. Best-effort empty.
        tracing::debug!("real-debrid check_cached is a no-op (instantAvailability deprecated)");
        Ok(HashMap::new())
    }

    async fn add_magnet(&self, magnet: &str) -> CoreResult<AddedTorrent> {
        let resp: AddMagnetResp = self
            .post_form_json("/torrents/addMagnet", &[("magnet", magnet)])
            .await?;
        Ok(AddedTorrent {
            id: resp.id,
            name: String::new(),
            status: "added".into(),
        })
    }

    async fn list_files(&self, torrent_id: &str) -> CoreResult<Vec<DebridFile>> {
        let info = self.info(torrent_id).await?;
        Ok(info
            .files
            .into_iter()
            .map(|f| DebridFile {
                id: f.id.to_string(),
                path: f.path,
                bytes: f.bytes,
            })
            .collect())
    }

    async fn select_files(&self, torrent_id: &str, file_ids: &[String]) -> CoreResult<()> {
        let files = if file_ids.is_empty() {
            "all".to_string()
        } else {
            file_ids.join(",")
        };
        self.post_form_unit(
            &format!("/torrents/selectFiles/{torrent_id}"),
            &[("files", files.as_str())],
        )
        .await
    }

    async fn unrestrict(&self, link: &str) -> CoreResult<String> {
        let resp: UnrestrictResp = self
            .post_form_json("/unrestrict/link", &[("link", link)])
            .await?;
        Ok(resp.download)
    }

    async fn resolve_playback(&self, candidate: &SourceCandidate) -> CoreResult<Playback> {
        let magnet = candidate
            .magnet_or_from_hash()
            .ok_or_else(|| CoreError::NoSource("candidate has no magnet or infohash".into()))?;

        let added = self.add_magnet(&magnet).await?;
        let id = added.id;
        let mut selected = false;
        let start = Instant::now();

        loop {
            let info = self.info(&id).await?;
            match info.status.as_str() {
                "magnet_error" | "error" | "virus" | "dead" => {
                    let _ = self.delete(&id).await;
                    return Err(CoreError::Remote {
                        service: "real-debrid".into(),
                        message: format!("torrent status: {}", info.status),
                    });
                }
                "waiting_files_selection" if !selected => {
                    let ids = video_file_ids(&info.files);
                    self.select_files(&id, &ids).await?;
                    selected = true;
                }
                "downloaded" if !info.links.is_empty() => {
                    // RD's `links` carries one URL per *selected* file, in order —
                    // not per full-torrent file. Map the candidate's file_index
                    // (a full-list position) onto the right selected link so a
                    // season pack plays the requested episode, not the first file.
                    let (idx, file_name) = match choose_selected_link(
                        &info.files,
                        info.links.len(),
                        candidate.file_index,
                    ) {
                        Some((idx, file)) => (idx, basename(&file.path)),
                        // No per-file signal (e.g. single-file torrent): fall
                        // back to the first link + the torrent name.
                        None => (0, info.filename.clone().unwrap_or_else(|| "stream".into())),
                    };
                    let url = self.unrestrict(&info.links[idx]).await?;
                    return Ok(Playback {
                        url,
                        origin: PlaybackOrigin::Debrid,
                        file_name,
                    });
                }
                _ => {}
            }
            if start.elapsed() > self.poll_timeout {
                let _ = self.delete(&id).await;
                return Err(CoreError::Timeout(
                    "real-debrid: torrent did not become ready".into(),
                ));
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

/// Pick the video file ids to select; empty (→ "all") if none look like video.
fn video_file_ids(files: &[FileEntry]) -> Vec<String> {
    files
        .iter()
        .filter(|f| is_video(&f.path))
        .map(|f| f.id.to_string())
        .collect()
}

/// Map a torrent `file_index` (Torrentio's position in the *full* file list)
/// onto the index into RD's `links` array, which holds one URL per **selected**
/// file in list order. Returns the link index and the chosen file.
///
/// Falls back to the largest selected file when the target can't be pinpointed
/// (e.g. the index is missing, out of range, or points at a non-selected file) —
/// that's the feature movie/episode in the common single-video case.
fn choose_selected_link(
    files: &[FileEntry],
    links_len: usize,
    file_index: Option<usize>,
) -> Option<(usize, &FileEntry)> {
    if links_len == 0 {
        return None;
    }
    // Files RD actually selected, in order — these line up 1:1 with `links`.
    // Older API responses may omit `selected`; fall back to video files then all.
    let selected: Vec<&FileEntry> = {
        let by_flag: Vec<&FileEntry> = files.iter().filter(|f| f.selected == 1).collect();
        if !by_flag.is_empty() {
            by_flag
        } else {
            let videos: Vec<&FileEntry> = files.iter().filter(|f| is_video(&f.path)).collect();
            if videos.is_empty() {
                files.iter().collect()
            } else {
                videos
            }
        }
    };
    if selected.is_empty() {
        return None;
    }

    // Target by Torrentio fileIdx: the file at that position in the full list,
    // located among the selected files to get its link index.
    if let Some(idx) = file_index {
        if let Some(target) = files.get(idx) {
            if let Some(pos) = selected.iter().position(|f| f.id == target.id) {
                if pos < links_len {
                    return Some((pos, selected[pos]));
                }
            }
        }
    }

    // Fallback: the largest selected file, clamped into the links range.
    let (pos, file) = selected.iter().enumerate().max_by_key(|(_, f)| f.bytes)?;
    Some((pos.min(links_len - 1), *file))
}

/// Last path segment of a torrent file path (RD uses `/`-rooted paths).
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

async fn parse_json<T: for<'de> Deserialize<'de>>(resp: reqwest::Response) -> CoreResult<T> {
    let text = resp
        .text()
        .await
        .map_err(|e| CoreError::Network(format!("real-debrid: {e}")))?;
    // selectFiles etc. can return 204 with an empty body; treat as `{}`.
    let text = if text.trim().is_empty() {
        "{}".to_string()
    } else {
        text
    };
    serde_json::from_str(&text).map_err(|e| CoreError::Parse {
        what: "real-debrid response".into(),
        message: e.to_string(),
    })
}

// --- RD response shapes ---

#[derive(Debug, Deserialize)]
struct RdError {
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    #[serde(default)]
    username: String,
    #[serde(default)]
    expiration: Option<String>,
    #[serde(default, rename = "type")]
    account_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddMagnetResp {
    id: String,
}

#[derive(Debug, Deserialize)]
struct TorrentInfo {
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    files: Vec<FileEntry>,
    #[serde(default)]
    links: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    id: u32,
    #[serde(default)]
    path: String,
    #[serde(default)]
    bytes: u64,
    /// RD marks files chosen via `selectFiles` with `selected: 1`. `links` holds
    /// one URL per selected file, in this list's order.
    #[serde(default)]
    selected: u8,
}

#[derive(Debug, Deserialize)]
struct UnrestrictResp {
    #[serde(default)]
    download: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_video_files() {
        assert!(is_video("/Movie.2024.1080p.mkv"));
        assert!(is_video("episode.MP4"));
        assert!(!is_video("readme.txt"));
        assert!(!is_video("sample.nfo"));
    }

    #[test]
    fn selects_only_video_files() {
        let files = vec![
            file(1, "/sample.txt", 1, 0),
            file(2, "/Movie.mkv", 100, 0),
            file(3, "/poster.jpg", 2, 0),
        ];
        assert_eq!(video_file_ids(&files), vec!["2".to_string()]);
    }

    fn file(id: u32, path: &str, bytes: u64, selected: u8) -> FileEntry {
        FileEntry {
            id,
            path: path.into(),
            bytes,
            selected,
        }
    }

    #[test]
    fn season_pack_maps_file_index_to_correct_selected_link() {
        // Full torrent: txt + 3 episodes; only the videos were selected, so RD's
        // `links` has 3 entries for files at full-list positions 1,2,3.
        let files = vec![
            file(1, "/Show/readme.txt", 10, 0),
            file(2, "/Show/S01E01.mkv", 100, 1),
            file(3, "/Show/S01E02.mkv", 100, 1),
            file(4, "/Show/S01E03.mkv", 100, 1),
        ];
        // Torrentio fileIdx for E02 is full-list position 2 → 2nd selected link.
        let (idx, chosen) = choose_selected_link(&files, 3, Some(2)).unwrap();
        assert_eq!(idx, 1, "E02 is the 2nd selected file → links[1]");
        assert_eq!(chosen.path, "/Show/S01E02.mkv");
    }

    #[test]
    fn missing_file_index_falls_back_to_largest_selected() {
        let files = vec![
            file(1, "/Show/S01E01.mkv", 100, 1),
            file(2, "/Show/Extras.mkv", 800, 1),
        ];
        let (idx, chosen) = choose_selected_link(&files, 2, None).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(chosen.bytes, 800);
    }

    #[test]
    fn basename_takes_last_segment() {
        assert_eq!(basename("/Show/S01E01.mkv"), "S01E01.mkv");
        assert_eq!(basename("Movie.mkv"), "Movie.mkv");
    }
}
