//! vlc adapter — launch-only (no IPC control).
//!
//! vlc plays the URL and exits with `--play-and-exit`. There is no control
//! channel, so [`PlaySession::control`] returns `None`, which disables
//! resume/auto-skip for vlc (the engine degrades gracefully).

use std::sync::Arc;

use async_trait::async_trait;
use om_core::error::{CoreError, CoreResult};
use om_core::ports::{PlayOptions, PlaySession, PlaybackControl, Player};
use om_core::stream::Playback;

/// vlc player.
pub struct VlcPlayer {
    command: String,
    args: Vec<String>,
}

impl VlcPlayer {
    pub fn new(args: Vec<String>) -> Self {
        Self {
            command: "vlc".to_string(),
            args,
        }
    }
}

#[async_trait]
impl Player for VlcPlayer {
    fn name(&self) -> &str {
        "vlc"
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
            return Err(CoreError::Player("vlc not found in PATH".into()));
        }
        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.arg("--play-and-exit");
        if let Some(title) = &opts.title {
            cmd.arg(format!("--meta-title={title}"));
        }
        for a in &self.args {
            cmd.arg(a);
        }
        for a in &opts.extra_args {
            cmd.arg(a);
        }
        cmd.arg(&playback.url);

        tracing::info!(url = %playback.url, "launching vlc");
        let child = cmd
            .spawn()
            .map_err(|e| CoreError::Player(format!("failed to launch vlc: {e}")))?;
        Ok(Box::new(VlcSession { child }))
    }
}

struct VlcSession {
    child: tokio::process::Child,
}

#[async_trait]
impl PlaySession for VlcSession {
    async fn wait(&mut self) -> CoreResult<()> {
        self.child
            .wait()
            .await
            .map_err(|e| CoreError::Player(format!("vlc wait failed: {e}")))?;
        Ok(())
    }

    fn control(&self) -> Option<Arc<dyn PlaybackControl>> {
        None
    }
}
