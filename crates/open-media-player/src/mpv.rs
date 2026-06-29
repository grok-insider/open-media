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
use open_media_core::ports::{Chapter, PlayOptions, PlaySession, PlaybackControl, Player};
use open_media_core::stream::Playback;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ipc;

static SOCKET_COUNTER: AtomicU64 = AtomicU64::new(0);

/// mpv player with IPC control.
pub struct MpvPlayer {
    command: String,
    args: Vec<String>,
}

impl MpvPlayer {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
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
        cmd.arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--force-window=yes")
            .arg(format!("--input-ipc-server={}", socket.display()));
        if let Some(title) = &opts.title {
            cmd.arg(format!("--force-media-title={title}"));
        }
        if let Some(secs) = opts.start_at_secs {
            cmd.arg(format!("--start=+{secs}"));
        }
        for a in &self.args {
            cmd.arg(a);
        }
        for a in &opts.extra_args {
            cmd.arg(a);
        }
        cmd.arg(&playback.url);

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

    async fn quit(&self) -> CoreResult<()> {
        self.request(json!(["quit"])).await?;
        Ok(())
    }
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
