# Future features (backlog)

Ideas deferred past the committed roadmap. Pull these into `docs/PLAN.md` or
`continue-plan.md` only when they become scheduled work. Not promises.

## Sources & Discovery

- Jackett / Prowlarr indexer adapter (`SourceProvider`) for users who self-host.
- Comet/MediaFusion addon adapter as a Torrentio alternative, including cache-aware
  emoji/tag parsing.
- Trakt/MDBList/Simkl watchlist import as a discovery surface.
- “Latest” / trending catalogs (TMDB trending, nyaa latest) as browsable lists.
- Better AniList→IMDB coverage beyond the current Fribb bridge where public data
  allows it.

## Debrid

- Torbox, AllDebrid, or Premiumize backend (v1.0 should ship at least one).
- Debrid library mode: list/manage what is already in the user's debrid account;
  replay without a fresh source search.
- Background cache warming for the next episode while watching the current one.

## Playback

- Local subtitle sidecar discovery for existing `.srt`/`.vtt` files. Remote
  subtitle fetch via `open-media-subs` is already implemented.
- Anime4K / custom mpv profile auto-apply per `MediaKind` (anime vs live-action).
- `catt`/Chromecast and “open in mpv-android (intent)” player targets.
- syncplay integration (watch-together).
- Smarter completion detection, such as credits-aware thresholds via chapters.

## Tracking & Presence

- Trakt scrobbling for movies/series (the non-anime analogue of AniList/MAL).
- MAL OAuth2 PKCE acquisition + refresh-token persistence.
- Per-title rating prompt on completion.

## UI / UX

- Init wizard for first-run setup.
- Recent-history / continue-watching home screen.
- fzf-style fuzzy in-TUI filter; alias map (`jjk` → `Jujutsu Kaisen`).
- Rich progress for P2P (peers, speed, buffer health) and debrid (cache/cache-warm
  state).
- Platform-aware player discovery beyond PATH defaults, especially Windows/macOS.

## Library / Server-Adjacent

- A Zurg/rclone/Jellyfin-style “infinite library” mode that exposes the debrid
  account as a browsable library. This is explicitly future/non-core.

## Platform & Packaging

- Shell completions (`open-media completions {bash,zsh,fish}`).
- Broader Nix platform support beyond the current `x86_64-linux` flake output.

## Engineering

- Replace `async-trait` with native async-fn-in-trait once MSRV allows and
  object-safety holds for the trait-object design.
- Pluggable adapters via a small registry to reduce `compose.rs` churn.
- Property tests for scoring; fuzz release-tag and season parsers.
- Metrics/structured tracing spans across the resolve→play path.
