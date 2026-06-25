# Future features (backlog)

Ideas deferred past the roadmap's committed milestones. Pulled into `docs/PLAN.md`
when scheduled. Not promises.

## Sources & discovery
- Jackett / Prowlarr indexer adapter (`SourceProvider`) for users who self-host.
- Comet/MediaFusion addon adapter (alt to Torrentio; cache-aware via emoji flags).
- Trakt/MDBList/Simkl watchlist import as a discovery surface.
- "Latest" / trending catalogs (TMDB trending, nyaa latest) as browseable lists.

## Debrid
- Torbox, AllDebrid, Premiumize backends (prove the abstraction; v1.0 ships one).
- Debrid library mode: list/manage what's already in your RD account; replay.
- Background "warm the cache" for the next episode while watching the current.

## Playback
- Subtitles: OpenSubtitles fetch + language-priority track auto-selection;
  external `.srt` sidecar handling.
- Anime4K / custom mpv profile auto-apply per `MediaKind` (anime vs live-action).
- `catt`/Chromecast and "open in mpv-android (intent)" player targets.
- syncplay integration (watch-together).

## Tracking & presence
- Trakt scrobbling for movies/series (the non-anime analogue of AniList).
- Smarter completion detection (credits-aware via chapters).
- Per-title rating prompt on completion.

## UI / UX
- Poster + still thumbnails via kitty/sixel image protocols.
- fzf-style fuzzy in-TUI filter; alias map ("jjk" → Jujutsu Kaisen).
- Rich progress for P2P (peers, speed, buffer health) and debrid (cache warming).
- Recent-history / continue-watching home screen.

## Library / server-adjacent
- A Zurg/rclone/Jellyfin-style "infinite library" mode that exposes the RD
  account as a browseable library (explicitly a *future*, non-core direction).

## Platform & packaging
- Nix flake + Home Manager module (committed in v0.4) + cachix cache.
- Windows/macOS player paths and IPC (named pipe) parity.
- Shell completions (`om completions {bash,zsh,fish}`).

## Engineering
- Replace `async-trait` with native async-fn-in-trait once MSRV allows and
  object-safety holds.
- Pluggable adapters via a small registry to reduce `compose.rs` churn.
- Property tests for scoring; fuzz the release-tag parsers.
- Metrics/structured tracing spans across the resolve→play path.

## Known gaps / deferred fixes (from the 2026-06 audit)
- **AniList → IMDB bridge**: AniList anime carry no IMDB id, so Torrentio can't
  serve them (nyaa does). Bridge via Cinemeta/anime-lists so anime also get
  Torrentio/RD cached sources. Cinemeta episode titles for anime would also fill
  the `Series - S01E01` (no title) gap.
- **Binge auto-advance**: `Engine::play` plays one episode; add the
  advance-to-next-(non-filler)-episode loop and call `Enricher::filler_episodes`.
- **Pagination**: TMDB (page 1 only), AniList (`perPage: 15`), and nyaa (one RSS
  page) silently truncate. Add paging / "load more".
- **OAuth acquisition**: AniList/MAL trackers consume a pre-obtained token; add a
  loopback OAuth flow (and MAL refresh, since its tokens are short-lived).
- **Discord client id**: presence uses a placeholder app id (`compose.rs`); register
  a real application and ship it.
- **`config set` coverage**: only 6 keys are settable via CLI; add the rest
  (quality, show_uncached, nyaa_direct, cinemeta, skip_intro_outro, http_port, …).
- **Wire or drop** `behavior.resume`, `ui.theme`, and the documented `OPEN_MEDIA_*`
  env overrides (loaded but unused today).
- **Smaller correctness**: MAL `Repeating` collapses to `watching`; AniSkip sends
  `episodeLength=0`; vlc ignores the resume position; scoring gives cached results
  no bonus when `prefer_cached=false`; P2P holds the state mutex across the ~90s
  metadata wait.
