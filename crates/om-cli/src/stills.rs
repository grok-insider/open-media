//! Episode/poster still rendering for terminals with an image protocol.
//!
//! Strategy: detect the terminal's graphics capability once at startup with
//! [`ratatui_image::picker::Picker`] (kitty / sixel / iTerm2, falling back to
//! unicode half-blocks). Stills are fetched + decoded + encoded **off the UI
//! thread** in a tokio task and the ready, cheap-to-render
//! [`ratatui_image::protocol::Protocol`] is posted back to the render loop, so
//! the UI never blocks on network or image work.
//!
//! Rendering itself is then just an [`ratatui_image::Image`] widget over the
//! cached protocol. When no protocol is available (detection failed, not a TTY)
//! or a still fails to load, the panel falls back to text — handled by the
//! caller.

use std::collections::HashMap;

use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use reqwest::Client;
use tokio::sync::mpsc::UnboundedSender;

/// Load state for one still URL, keyed in [`Stills::cache`].
pub enum StillState {
    /// Fetch/decode in flight; render the text fallback meanwhile.
    Loading,
    /// Ready, cheap-to-render protocol-encoded image.
    Ready(Box<Protocol>),
    /// Permanent failure for this URL (bad bytes, network, unsupported format).
    /// Cached so we don't retry every cursor move.
    Failed,
}

/// Holds the detected picker + a per-URL cache of decoded stills. Lives in the
/// `App`. When `picker` is `None`, image rendering is disabled entirely.
pub struct Stills {
    picker: Option<Picker>,
    client: Client,
    cache: HashMap<String, StillState>,
}

impl Stills {
    /// Detect terminal graphics capabilities. MUST be called after entering the
    /// alternate screen but before reading terminal events (it briefly queries
    /// stdio). Detection failure (e.g. piped/non-TTY) disables images cleanly.
    pub fn detect() -> Self {
        let picker = Picker::from_query_stdio().ok();
        Self {
            picker,
            client: Client::new(),
            cache: HashMap::new(),
        }
    }

    /// Whether any image protocol (or the half-block fallback) is available.
    pub fn enabled(&self) -> bool {
        self.picker.is_some()
    }

    /// The ready protocol for `url`, if it has finished loading.
    pub fn ready(&self, url: &str) -> Option<&Protocol> {
        match self.cache.get(url) {
            Some(StillState::Ready(p)) => Some(p),
            _ => None,
        }
    }

    /// Kick off a background fetch+decode for `url` if we haven't already seen
    /// it. Idempotent: a URL that is loading, ready, or failed is left alone.
    /// `target` is the `(cols, rows)` cell box the image should fit within. The
    /// decoded protocol is delivered via [`StillMsg`] on `tx`.
    pub fn request(&mut self, url: &str, target: (u16, u16), tx: UnboundedSender<StillMsg>) {
        let Some(picker) = self.picker.clone() else {
            return;
        };
        if self.cache.contains_key(url) {
            return;
        }
        self.cache.insert(url.to_string(), StillState::Loading);

        let client = self.client.clone();
        let url = url.to_string();
        tokio::spawn(async move {
            let result = load_still(&client, &url, &picker, target).await;
            // The receiver may be gone if the app exited; ignore send errors.
            let _ = tx.send(StillMsg { url, result });
        });
    }

    /// Apply a completed fetch result to the cache.
    pub fn apply(&mut self, msg: StillMsg) {
        let state = match msg.result {
            Some(protocol) => StillState::Ready(Box::new(protocol)),
            None => StillState::Failed,
        };
        self.cache.insert(msg.url, state);
    }
}

/// A completed still load, posted back to the UI loop.
pub struct StillMsg {
    pub url: String,
    /// `Some` protocol on success, `None` on any failure (cached as `Failed`).
    pub result: Option<Protocol>,
}

/// Fetch the image bytes, decode them, and encode them into a terminal
/// protocol sized to `target`. Returns `None` on any failure — stills are
/// best-effort decoration, never fatal.
async fn load_still(
    client: &Client,
    url: &str,
    picker: &Picker,
    target: (u16, u16),
) -> Option<Protocol> {
    let bytes = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;

    // Decode + encode are CPU-bound and blocking; keep them off the async
    // worker so the runtime stays responsive.
    let picker = picker.clone();
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).ok()?;
        picker
            .new_protocol(img, target.into(), Resize::Fit(None))
            .ok()
    })
    .await
    .ok()
    .flatten()
}
