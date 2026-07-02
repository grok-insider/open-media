# continue-plan.md

Actionable engineering follow-ups after the 0.6.1 audit. The broader backlog
lives in `future-features.md`; this file is for concrete work that is close enough
to pick up.

Status legend: **P1** correctness bug still present · **P2** incomplete feature ·
**P3** smaller correctness nit / polish.

---

## Already shipped (context — do NOT redo)

Cinemeta keyless metadata + IMDB dedup; episode titles in mpv/Discord; RD
multi-file/season-pack fix; `HybridResolver` debrid→P2P fallback; Torrentio anime
no-op without IMDB; `complete_threshold`, `behavior.resume`, and `ui.theme` wired;
`OPEN_MEDIA_*` credential overrides; cached-source score tiebreak; MAL
`is_rewatching`; vlc resume; P2P metadata-wait lock fix; configurable nyaa
category; release-tag parser fixes; real runtime to AniSkip; TUI season navigation
and source filter/sort persistence; anime season matching, absolute-number fetch,
and singular `Season 1-5` range parsing; binge/auto-advance with filler skip;
TMDB/AniList pagination; AniList loopback login; real Discord application id;
anime per-episode titles from Jikan/streaming metadata; keyless Torrentio tracker
parsing; positional `open-media "query"` TUI prefill; Fribb AniList/MAL→IMDB
bridge; subtitles via `open-media-subs`; Windows mpv/Discord IPC and release
artifact; shared HTTP timeouts/retry in `open-media-net`; source-level playback
failover; MSRV CI; crates.io publishing.

---

## P1 — correctness bugs still present

### 1. Real-Debrid `check_cached` is a no-op

`crates/open-media-debrid/src/realdebrid.rs` currently returns an empty map because
the old `instantAvailability` behavior is not reliable for this pipeline. That is
safe for Torrentio-flagged candidates (the addon provides cache/direct-url state),
but non-Torrentio candidates stay `CacheState::Unknown`. Revisit when RD exposes a
working bulk endpoint. Note: the TorBox backend implements a real `check_cached`
(`/torrents/checkcached`), so this limitation is RD-specific now.

### 2. MAL OAuth acquisition and refresh

`open-media login anilist` exists, but `open-media login mal` still returns “not
yet supported.” MAL tokens are short-lived and need the OAuth2 PKCE flow plus
refresh-token persistence. The MAL tracker itself can already consume a bearer
token once configured.

---

## P2 — incomplete features

### 3. nyaa pagination / load more

TMDB and AniList fetch bounded additional pages. Direct nyaa remains one RSS
request per query (plus the absolute-episode second request when applicable). Add
pagination or an explicit “load more” path without slowing the common search.

### 4. Full `config set` coverage for list/nested keys

`open-media config set` covers scalar keys, but these still require direct TOML
editing: `providers.torrentio_providers`, `player.args`, `[subtitles].enabled`,
`[subtitles].languages`, and `[ui.sources]` defaults. Add typed setters or a clear
subcommand syntax for list/nested values. Also add `[subtitles]` to
`open-media config show`, which currently prints a summary but not every nested
section.

### 5. Telemetry collector activation

Telemetry plumbing and privacy tests exist, but the shipped endpoint is the
`PLACEHOLDER` sentinel, so default-on telemetry is inert. Either wire a real
collector endpoint or document the feature as disabled until the collector exists.

### 6. A second debrid backend — ✅ done (TorBox)

TorBox shipped as the second `DebridProvider` (`open-media-debrid/src/torbox.rs`):
new adapter + composition-root/config wiring only, no core/app changes — the OCP
claim held. AllDebrid/Premiumize remain in `future-features.md`.

---

## P3 — smaller correctness nits & polish

- **Local subtitle sidecars:** `open-media-subs` fetches remote subtitles and
  materializes temp tracks, but there is no local `.srt`/`.vtt` sidecar discovery.
- **Player discovery on Windows/macOS:** IPC parity and artifacts are done; player
  lookup is still PATH-based. Consider platform-aware default paths for mpv/vlc.
- **Recent-history home screen:** `HistoryStore::recent()` exists, but the TUI has
  no continue-watching home surface yet.
- **Rich stream progress:** expose P2P peers/speed/buffer health and debrid/cache
  status in the TUI.
- **Shell completions:** add `open-media completions {bash,zsh,fish}`.

---

## Release workflow note

Releases use release-plz plus the GitHub workflow:

- Feature/fix work lands in `dev`; only `dev` and `release-plz-*` PRs may target
  `master`.
- `feat:`/`fix:` commits on `master` make release-plz maintain a patch release PR;
  repo admins use the Manual Version Bump workflow for deliberate minor/major
  milestones.
- Merging the release PR tags `vX.Y.Z`, publishes every `open-media-*` crate to
  crates.io in dependency order, creates the GitHub Release, uploads prebuilt
  archives, and lets CI push the Nix closure to cachix.
- `open-media-subs` now depends on registry `open-subtitle-*` crates, so the old
  git-dependency/release-plz `continue-on-error` caveat is gone.
