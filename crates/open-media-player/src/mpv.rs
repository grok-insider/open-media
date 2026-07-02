//! mpv adapter with JSON-IPC control.
//!
//! Launches mpv with `--input-ipc-server=<socket>` and exposes a
//! [`PlaybackControl`] that speaks mpv's line-delimited JSON IPC over that unix
//! socket (`{"command":[...]}\n` → `{error,data}`). One channel powers resume
//! (seek), progress (time-pos), presence (pause), and AniSkip (seek + chapters).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use open_media_core::error::{CoreError, CoreResult};
use open_media_core::ports::{
    Chapter, PlayOptions, PlaySession, PlaybackControl, Player, PlaylistControl, PlaylistItem,
};
use open_media_core::stream::Playback;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ipc;

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

/// mpv player with IPC control.
pub struct MpvPlayer {
    command: String,
    args: Vec<String>,
    thumbnail_previews: bool,
}

impl MpvPlayer {
    pub fn new(command: impl Into<String>, args: Vec<String>, thumbnail_previews: bool) -> Self {
        Self {
            command: command.into(),
            args,
            thumbnail_previews,
        }
    }
}

#[async_trait]
impl Player for MpvPlayer {
    fn name(&self) -> &str {
        "mpv"
    }

    fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }

    async fn play(
        &self,
        playback: &Playback,
        opts: &PlayOptions,
    ) -> CoreResult<Box<dyn PlaySession>> {
        if !self.is_available() {
            return Err(CoreError::Player(format!(
                "{} not found in PATH",
                self.command
            )));
        }

        let socket = unique_socket_path();
        let mut cmd = tokio::process::Command::new(&self.command);
        for a in self.launch_args(playback, opts, &socket) {
            cmd.arg(a);
        }

        tracing::info!(url = %playback.url, socket = %socket.display(), "launching mpv");
        let child = cmd
            .spawn()
            .map_err(|e| CoreError::Player(format!("failed to launch mpv: {e}")))?;

        // Wait for the IPC socket to come up before handing back a controller.
        wait_for_socket(&socket, Duration::from_secs(10)).await;

        let control = Arc::new(MpvControl::new(socket.clone()));
        Ok(Box::new(MpvSession {
            child,
            socket,
            control,
        }))
    }
}

impl MpvPlayer {
    fn launch_args(&self, playback: &Playback, opts: &PlayOptions, socket: &Path) -> Vec<String> {
        let mut args = vec![
            "--no-terminal".to_string(),
            "--really-quiet".to_string(),
            "--force-window=yes".to_string(),
            format!("--input-ipc-server={}", socket.display()),
        ];
        if let Some(title) = &opts.title {
            args.push(format!("--force-media-title={title}"));
        }
        if let Some(secs) = opts.start_at_secs {
            args.push(format!("--start=+{secs}"));
        }
        if self.thumbnail_previews {
            // thumbfast disables remote/network thumbnails by default. This is
            // harmless when the user has not installed thumbfast; mpv just stores
            // the script option for scripts that choose to read it.
            args.push("--script-opts=thumbfast-network=yes".to_string());
        }
        if opts.hold_at_end {
            // Pause on the last frame instead of advancing to the next playlist
            // entry (or exiting): manual-Next mode. `always` (not `yes`) so it
            // also applies when a next playlist entry exists.
            args.push("--keep-open=always".to_string());
        }
        args.extend(self.args.iter().cloned());
        args.extend(opts.extra_args.iter().cloned());
        args.push(playback.url.clone());
        args
    }
}

/// A running mpv process + its control handle.
pub struct MpvSession {
    child: tokio::process::Child,
    socket: PathBuf,
    control: Arc<MpvControl>,
}

#[async_trait]
impl PlaySession for MpvSession {
    async fn wait(&mut self) -> CoreResult<()> {
        let status = self
            .child
            .wait()
            .await
            .map_err(|e| CoreError::Player(format!("mpv wait failed: {e}")))?;
        cleanup_socket(&self.socket);
        tracing::info!(?status, "mpv exited");
        Ok(())
    }

    fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
        Some(self.control.clone())
    }

    fn playlist_control(&self) -> Option<Arc<dyn PlaylistControl>> {
        Some(self.control.clone())
    }
}

impl Drop for MpvSession {
    fn drop(&mut self) {
        cleanup_socket(&self.socket);
    }
}

/// IPC client for a running mpv. Connects per-request with a short retry —
/// simple and robust against transient socket hiccups (mirrors curd).
pub struct MpvControl {
    socket: PathBuf,
}

impl MpvControl {
    /// Build a controller bound to an existing mpv IPC socket path.
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// Send one `command` array and return its `data` (or `Null`).
    pub async fn request(&self, command: Value) -> CoreResult<Value> {
        let mut last_err = None;
        for _ in 0..3 {
            match self.request_once(&command).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| CoreError::Player("mpv ipc failed".into())))
    }

    async fn request_once(&self, command: &Value) -> CoreResult<Value> {
        // One connection per request: write the command, then read replies on the
        // same stream (request/response, so a sequential write-then-read is enough
        // and keeps the transport identical across the Unix-socket/named-pipe seam).
        let mut stream = ipc::connect(&self.socket)
            .await
            .map_err(|e| CoreError::Player(format!("mpv ipc connect: {e}")))?;

        let payload = serde_json::to_string(&json!({ "command": command })).unwrap();
        stream
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .map_err(|e| CoreError::Player(format!("mpv ipc write: {e}")))?;

        let mut reader = BufReader::new(&mut stream);
        // Skip async event lines; take the first reply carrying an `error`.
        for _ in 0..50 {
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| CoreError::Player(format!("mpv ipc read: {e}")))?;
            if n == 0 {
                break;
            }
            let v: Value = match serde_json::from_str(line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("event").is_some() {
                continue;
            }
            match v.get("error").and_then(Value::as_str) {
                Some("success") | None => return Ok(v.get("data").cloned().unwrap_or(Value::Null)),
                Some(err) => return Err(CoreError::Player(format!("mpv: {err}"))),
            }
        }
        Err(CoreError::Player("mpv ipc: no reply".into()))
    }

    async fn get_f64(&self, property: &str) -> CoreResult<Option<f64>> {
        let data = self.request(json!(["get_property", property])).await?;
        Ok(data.as_f64())
    }
}

#[async_trait]
impl PlaybackControl for MpvControl {
    async fn position(&self) -> CoreResult<Option<u32>> {
        Ok(self.get_f64("time-pos").await?.map(|v| v as u32))
    }

    async fn duration(&self) -> CoreResult<Option<u32>> {
        Ok(self.get_f64("duration").await?.map(|v| v as u32))
    }

    async fn is_paused(&self) -> CoreResult<Option<bool>> {
        let data = self.request(json!(["get_property", "pause"])).await?;
        Ok(data.as_bool())
    }

    async fn seek_absolute(&self, secs: u32) -> CoreResult<()> {
        self.request(json!(["seek", secs, "absolute"])).await?;
        Ok(())
    }

    async fn set_chapters(&self, chapters: &[Chapter]) -> CoreResult<()> {
        let list: Vec<Value> = chapters
            .iter()
            .map(|c| json!({ "title": c.title, "time": c.time_secs }))
            .collect();
        self.request(json!(["set_property", "chapter-list", list]))
            .await?;
        Ok(())
    }

    async fn chapters(&self) -> CoreResult<Vec<Chapter>> {
        let data = self
            .request(json!(["get_property", "chapter-list"]))
            .await?;
        Ok(parse_chapter_list(&data))
    }

    async fn eof_reached(&self) -> CoreResult<Option<bool>> {
        let data = self.request(json!(["get_property", "eof-reached"])).await?;
        Ok(data.as_bool())
    }

    async fn quit(&self) -> CoreResult<()> {
        self.request(json!(["quit"])).await?;
        Ok(())
    }
}

#[async_trait]
impl PlaylistControl for MpvControl {
    async fn append(&self, item: &PlaylistItem) -> CoreResult<()> {
        self.request(append_command(item)).await?;
        Ok(())
    }

    async fn active_index(&self) -> CoreResult<Option<usize>> {
        let data = self
            .request(json!(["get_property", "playlist-pos"]))
            .await?;
        Ok(data.as_u64().map(|v| v as usize))
    }
}

fn append_command(item: &PlaylistItem) -> Value {
    // Per-file options travel in `loadfile`'s options dict, so they apply only
    // to this playlist entry.
    let mut options = serde_json::Map::new();
    if let Some(title) = &item.title {
        options.insert("force-media-title".into(), json!(title));
    }
    if let Some(sub) = &item.sub_file {
        options.insert("sub-file".into(), json!(sub));
    }
    if options.is_empty() {
        json!(["loadfile", item.url, "append-play"])
    } else {
        json!(["loadfile", item.url, "append-play", options])
    }
}

/// Parse mpv's `chapter-list` property: `[{ "title": "...", "time": 12.3 }]`.
/// Entries without a time are dropped; a missing title becomes empty (some
/// muxers emit unnamed chapters).
fn parse_chapter_list(data: &Value) -> Vec<Chapter> {
    data.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|c| {
                    let time = c.get("time")?.as_f64()?;
                    Some(Chapter {
                        title: c
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        time_secs: time.max(0.0).round() as u32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A process/instance-unique IPC endpoint name shared by both platforms.
fn unique_token() -> String {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("om-mpv-{pid}-{nanos}-{id}")
}

/// Unix: a `.sock` file under the temp dir.
#[cfg(unix)]
fn unique_socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("{}.sock", unique_token()))
}

/// Windows: a named pipe under the `\\.\pipe\` namespace (no filesystem entry).
#[cfg(windows)]
fn unique_socket_path() -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\{}", unique_token()))
}

/// Remove the IPC endpoint after mpv exits.
///
/// Unix sockets are real files that must be unlinked; Windows named pipes are
/// owned by mpv's process and disappear when it closes them, so this is a no-op
/// there.
fn cleanup_socket(path: &Path) {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(path);
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

async fn wait_for_socket(path: &Path, timeout: Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if ipc::connect(path).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_media_core::stream::PlaybackOrigin;

    #[test]
    fn parses_mpv_chapter_list_property() {
        let data = json!([
            { "title": "OP", "time": 59.9 },
            { "title": "Part A", "time": 150.2 },
            { "time": 700.0 },              // unnamed chapter
            { "title": "no time given" }    // dropped: unusable without a time
        ]);
        let chapters = parse_chapter_list(&data);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].title, "OP");
        assert_eq!(chapters[0].time_secs, 60);
        assert_eq!(chapters[1].time_secs, 150);
        assert_eq!(chapters[2].title, "");
    }

    #[test]
    fn non_array_chapter_list_is_empty() {
        assert!(parse_chapter_list(&json!(null)).is_empty());
        assert!(parse_chapter_list(&json!("nope")).is_empty());
    }

    fn playback() -> Playback {
        Playback {
            url: "https://example.invalid/video.mkv".into(),
            origin: PlaybackOrigin::Debrid,
            file_name: "video.mkv".into(),
        }
    }

    #[test]
    fn thumbnail_previews_add_thumbfast_network_before_user_args() {
        let player = MpvPlayer::new("mpv", vec!["--fullscreen".into()], true);
        let opts = PlayOptions {
            extra_args: vec!["--sid=no".into()],
            ..PlayOptions::default()
        };

        let args = player.launch_args(&playback(), &opts, Path::new("/tmp/open-media-mpv.sock"));

        let thumbfast = args
            .iter()
            .position(|arg| arg == "--script-opts=thumbfast-network=yes")
            .expect("thumbnail opt is present");
        let player_arg = args
            .iter()
            .position(|arg| arg == "--fullscreen")
            .expect("player args are preserved");
        let extra_arg = args
            .iter()
            .position(|arg| arg == "--sid=no")
            .expect("extra args are preserved");

        assert!(thumbfast < player_arg);
        assert!(player_arg < extra_arg);
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.invalid/video.mkv")
        );
    }

    #[test]
    fn thumbnail_previews_disabled_keeps_launch_args_unchanged() {
        let player = MpvPlayer::new("mpv", vec!["--fullscreen".into()], false);
        let args = player.launch_args(
            &playback(),
            &PlayOptions::default(),
            Path::new("/tmp/open-media-mpv.sock"),
        );

        assert!(!args
            .iter()
            .any(|arg| arg == "--script-opts=thumbfast-network=yes"));
        assert!(args.iter().any(|arg| arg == "--fullscreen"));
    }

    #[test]
    fn append_command_includes_title_metadata_when_present() {
        let item = PlaylistItem {
            url: "https://example.invalid/e2.mkv".into(),
            title: Some("Show S01E02 - Two".into()),
            sub_file: None,
        };

        assert_eq!(
            append_command(&item),
            json!(["loadfile", "https://example.invalid/e2.mkv", "append-play", { "force-media-title": "Show S01E02 - Two" }])
        );
    }

    #[test]
    fn append_command_attaches_per_file_subtitles() {
        let item = PlaylistItem {
            url: "https://example.invalid/e2.mkv".into(),
            title: Some("Show S01E02 - Two".into()),
            sub_file: Some("/tmp/om-subs/e2.en.srt".into()),
        };

        assert_eq!(
            append_command(&item),
            json!(["loadfile", "https://example.invalid/e2.mkv", "append-play", {
                "force-media-title": "Show S01E02 - Two",
                "sub-file": "/tmp/om-subs/e2.en.srt"
            }])
        );
    }

    #[test]
    fn append_command_without_metadata_has_no_options_dict() {
        let item = PlaylistItem {
            url: "https://example.invalid/e2.mkv".into(),
            title: None,
            sub_file: None,
        };
        assert_eq!(
            append_command(&item),
            json!(["loadfile", "https://example.invalid/e2.mkv", "append-play"])
        );
    }

    #[test]
    fn hold_at_end_adds_keep_open_always() {
        let player = MpvPlayer::new("mpv", vec![], false);
        let opts = PlayOptions {
            hold_at_end: true,
            ..PlayOptions::default()
        };
        let args = player.launch_args(&playback(), &opts, Path::new("/tmp/om-mpv.sock"));
        assert!(args.iter().any(|a| a == "--keep-open=always"));

        let plain = player.launch_args(
            &playback(),
            &PlayOptions::default(),
            Path::new("/tmp/om-mpv.sock"),
        );
        assert!(!plain.iter().any(|a| a.starts_with("--keep-open")));
    }
}
