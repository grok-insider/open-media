//! Discord rich-presence reporter over the Discord IPC socket.
//!
//! Speaks Discord's local IPC framing — `[u32 LE opcode][u32 LE length][json]` —
//! with an opcode-0 handshake then opcode-1 `SET_ACTIVITY` frames. It is
//! **best-effort**: if Discord is not running (no socket), `update`/`clear`
//! become no-ops rather than errors, so presence never blocks playback.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use om_core::error::CoreResult;
use om_core::ports::PresenceReporter;
use om_core::tracking::Activity;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

static NONCE: AtomicU64 = AtomicU64::new(1);

/// Discord rich-presence reporter.
pub struct DiscordPresence {
    client_id: String,
    conn: Mutex<Option<UnixStream>>,
}

impl DiscordPresence {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            conn: Mutex::new(None),
        }
    }

    /// Ensure a handshaken connection exists. Returns false if Discord is absent.
    async fn ensure(&self) -> bool {
        let mut guard = self.conn.lock().await;
        if guard.is_some() {
            return true;
        }
        for path in candidate_paths() {
            if let Ok(mut stream) = UnixStream::connect(&path).await {
                let handshake = encode_frame(0, &json!({ "v": 1, "client_id": self.client_id }));
                if stream.write_all(&handshake).await.is_ok() {
                    let _ = read_frame(&mut stream).await; // consume handshake reply
                    *guard = Some(stream);
                    return true;
                }
            }
        }
        false
    }

    async fn send(&self, payload: Value) {
        if !self.ensure().await {
            return; // best-effort: Discord not running
        }
        let frame = encode_frame(1, &payload);
        let mut guard = self.conn.lock().await;
        if let Some(stream) = guard.as_mut() {
            if stream.write_all(&frame).await.is_err() {
                *guard = None; // drop a dead connection; reconnect next time
            } else {
                let _ = read_frame(stream).await;
            }
        }
    }
}

#[async_trait]
impl PresenceReporter for DiscordPresence {
    async fn update(&self, activity: &Activity) -> CoreResult<()> {
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        self.send(json!({
            "cmd": "SET_ACTIVITY",
            "nonce": nonce.to_string(),
            "args": {
                "pid": std::process::id(),
                "activity": build_activity(activity),
            }
        }))
        .await;
        Ok(())
    }

    async fn clear(&self) -> CoreResult<()> {
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        self.send(json!({
            "cmd": "SET_ACTIVITY",
            "nonce": nonce.to_string(),
            "args": { "pid": std::process::id(), "activity": Value::Null }
        }))
        .await;
        Ok(())
    }
}

/// Build the Discord activity object (type 3 = Watching).
fn build_activity(a: &Activity) -> Value {
    let mut activity = json!({
        "type": 3,
        "details": a.title,
        "state": a.detail,
    });
    // Elapsed/remaining timestamps only while actually playing.
    if !a.paused && a.duration_secs > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let start = now - a.position_secs as i64;
        let end = start + a.duration_secs as i64;
        activity["timestamps"] = json!({ "start": start, "end": end });
    }
    if let Some(img) = &a.image_url {
        activity["assets"] = json!({ "large_image": img, "large_text": a.title });
    }
    activity
}

fn encode_frame(opcode: u32, payload: &Value) -> Vec<u8> {
    let data = serde_json::to_vec(payload).unwrap_or_default();
    let mut buf = Vec::with_capacity(8 + data.len());
    buf.extend_from_slice(&opcode.to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&data);
    buf
}

async fn read_frame(stream: &mut UnixStream) -> std::io::Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).await?;
    let opcode = u32::from_le_bytes(header[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await?;
    Ok((opcode, data))
}

fn candidate_paths() -> Vec<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut paths = Vec::new();
    for i in 0..10 {
        paths.push(base.join(format!("discord-ipc-{i}")));
        // Flatpak/snap sandboxed Discord socket locations.
        paths.push(base.join(format!("app/com.discordapp.Discord/discord-ipc-{i}")));
        paths.push(base.join(format!("snap.discord/discord-ipc-{i}")));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_encodes_opcode_len_and_body() {
        let payload = json!({ "v": 1 });
        let frame = encode_frame(0, &payload);
        let body = serde_json::to_vec(&payload).unwrap();
        assert_eq!(u32::from_le_bytes(frame[0..4].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_le_bytes(frame[4..8].try_into().unwrap()) as usize,
            body.len()
        );
        assert_eq!(&frame[8..], body.as_slice());
    }

    #[test]
    fn playing_activity_has_timestamps() {
        let a = Activity {
            title: "Frieren".into(),
            detail: "S01E01 - The Journey's End".into(),
            paused: false,
            position_secs: 60,
            duration_secs: 1440,
            image_url: Some("https://img".into()),
        };
        let v = build_activity(&a);
        assert_eq!(v["type"], 3);
        assert_eq!(v["details"], "Frieren");
        // The episode coordinate/title (from om_core::title) flows to `state`.
        assert_eq!(v["state"], "S01E01 - The Journey's End");
        assert!(v.get("timestamps").is_some());
        assert!(v.get("assets").is_some());
    }

    #[test]
    fn paused_activity_omits_timestamps() {
        let a = Activity {
            title: "Frieren".into(),
            detail: "S01E01".into(),
            paused: true,
            position_secs: 60,
            duration_secs: 1440,
            image_url: None,
        };
        let v = build_activity(&a);
        assert!(v.get("timestamps").is_none());
    }
}
