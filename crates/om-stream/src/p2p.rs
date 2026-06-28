//! Local P2P streaming engine over librqbit.
//!
//! Starts a librqbit [`Session`] plus its built-in HTTP API server (which exposes
//! the Range-aware `/torrents/{id}/stream/{file_idx}` endpoint), adds a magnet,
//! waits for metadata, picks the largest video file, and returns the local stream
//! URL. This is the Rust equivalent of toru's localhost stream server — but
//! librqbit ships the Range/seek handling, so we mount it rather than hand-roll.
//!
//! The session + HTTP server are started lazily on the first `stream_magnet` so
//! constructing the engine is cheap and binds no port until P2P is actually used.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Api, Session, SessionOptions};
use om_core::error::{CoreError, CoreResult};
use om_core::stream::{human_bytes, Playback, PlaybackOrigin};
use tokio::sync::Mutex;

const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "m2ts",
];
const METADATA_TIMEOUT: Duration = Duration::from_secs(90);

/// librqbit-backed P2P streaming engine.
pub struct P2pEngine {
    http_port: u16,
    cleanup_after_playback: bool,
    state: Mutex<Option<EngineState>>,
}

struct EngineState {
    session: std::sync::Arc<Session>,
    api: Api,
    active: Option<usize>,
}

impl P2pEngine {
    pub fn new(http_port: u16, cleanup_after_playback: bool) -> Self {
        Self {
            http_port,
            cleanup_after_playback,
            state: Mutex::new(None),
        }
    }

    /// Eagerly start the session + HTTP stream server (otherwise lazy on first
    /// `stream_magnet`). Useful for tests and warm-up.
    pub async fn start(&self) -> CoreResult<()> {
        self.ensure_started().await
    }

    /// Lazily start the session + HTTP stream server.
    async fn ensure_started(&self) -> CoreResult<()> {
        let mut guard = self.state.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let download_dir = std::env::temp_dir().join("open-media-p2p");
        std::fs::create_dir_all(&download_dir)
            .map_err(|e| CoreError::Other(format!("p2p: create dir: {e}")))?;

        let opts = SessionOptions {
            disable_dht_persistence: true,
            ..Default::default()
        };
        let session = Session::new_with_opts(download_dir, opts)
            .await
            .map_err(|e| CoreError::Other(format!("p2p: session init: {e}")))?;

        // 3rd arg is the log line-broadcast (tracing-subscriber-utils feature).
        let api = Api::new(session.clone(), None, None);

        // Start librqbit's HTTP API (serves the Range-aware stream endpoint).
        let http_api = librqbit::http_api::HttpApi::new(api.clone(), None);
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.http_port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CoreError::Other(format!("p2p: bind {addr}: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = http_api.make_http_api_and_run(listener, None).await {
                tracing::warn!(error = %e, "p2p http server stopped");
            }
        });

        tracing::info!(port = self.http_port, "p2p stream server started");
        *guard = Some(EngineState {
            session,
            api,
            active: None,
        });
        Ok(())
    }

    /// Add a magnet and return its local stream URL.
    pub async fn stream_magnet(&self, magnet: &str) -> CoreResult<Playback> {
        self.ensure_started().await?;

        // Acquire the lock only long enough to clone the handles we need and to
        // take ownership of any previous torrent id for teardown. The lengthy
        // add+metadata wait below must NOT hold the state lock, otherwise
        // concurrent `stream_magnet`/`cleanup` calls serialize behind the ~90s
        // metadata timeout.
        let (session, api, prev) = {
            let mut guard = self.state.lock().await;
            let state = guard.as_mut().expect("started");
            let prev = state.active.take();
            (state.session.clone(), state.api.clone(), prev)
        };

        // Tear down any previous stream first (lock released).
        if let Some(prev) = prev {
            let _ = session
                .delete(prev.into(), self.cleanup_after_playback)
                .await;
        }

        let response = session
            .add_torrent(
                AddTorrent::from_url(magnet),
                Some(AddTorrentOptions {
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| CoreError::Other(format!("p2p: add magnet: {e}")))?;

        let (id, handle) = match response {
            AddTorrentResponse::Added(id, h) => (id, h),
            AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
            AddTorrentResponse::ListOnly(_) => {
                return Err(CoreError::NoSource("p2p: torrent added list-only".into()))
            }
        };

        // Wait for metadata (file list + sizes) to arrive. The lock is NOT held
        // here, so other calls can proceed/clean up concurrently.
        let start = Instant::now();
        loop {
            if handle.stats().total_bytes > 0 {
                break;
            }
            if start.elapsed() > METADATA_TIMEOUT {
                let _ = session.delete(id.into(), true).await;
                return Err(CoreError::Timeout("p2p: metadata not received".into()));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let (file_index, file_name) = pick_video_file(&api, id)?;

        // Re-acquire briefly to record the now-active torrent. If a concurrent
        // call set a different active torrent while we were waiting, tear that
        // one down so we don't leak it. The guard is dropped before the (short)
        // delete await so we never hold the lock across an await.
        let stale = {
            let mut guard = self.state.lock().await;
            let state = guard.as_mut().expect("started");
            state.active.replace(id).filter(|&prev| prev != id)
        };
        if let Some(stale) = stale {
            let _ = session
                .delete(stale.into(), self.cleanup_after_playback)
                .await;
        }

        let url = format!(
            "http://127.0.0.1:{}/torrents/{}/stream/{}",
            self.http_port, id, file_index
        );
        tracing::info!(%url, file = %file_name, "p2p streaming");
        Ok(Playback {
            url,
            origin: PlaybackOrigin::LocalP2p,
            file_name,
        })
    }

    /// Tear down the active torrent (and files when configured).
    pub async fn cleanup(&self) {
        let mut guard = self.state.lock().await;
        if let Some(state) = guard.as_mut() {
            if let Some(id) = state.active.take() {
                let _ = state
                    .session
                    .delete(id.into(), self.cleanup_after_playback)
                    .await;
            }
        }
    }
}

/// Pick the largest video file in the torrent.
fn pick_video_file(api: &Api, torrent_id: usize) -> CoreResult<(usize, String)> {
    let details = api
        .api_torrent_details(torrent_id.into())
        .map_err(|e| CoreError::Other(format!("p2p: torrent details: {e}")))?;
    let files = details
        .files
        .ok_or_else(|| CoreError::NoSource("p2p: no files in torrent".into()))?;

    let mut best: Option<(usize, String, u64)> = None;
    for (idx, file) in files.iter().enumerate() {
        let name = file.name.clone();
        let is_video = name
            .rsplit('.')
            .next()
            .map(|ext| VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if is_video && best.as_ref().is_none_or(|(_, _, s)| file.length > *s) {
            best = Some((idx, name, file.length));
        }
    }

    best.map(|(idx, name, len)| {
        tracing::debug!(idx, file = %name, size = %human_bytes(len), "selected video file");
        (idx, name)
    })
    .ok_or_else(|| CoreError::NoSource("p2p: no video file in torrent".into()))
}
