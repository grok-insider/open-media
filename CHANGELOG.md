# Changelog

All notable changes to open-media are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-06-25

### Added — Phase 10: packaging (Nix flake + cachix CI)
- `flake.nix`: `packages.x86_64-linux.{om,default}` via `rustPlatform.buildRustPackage`
  (cmake + bindgenHook for aws-lc-sys + bundled rusqlite; no OpenSSL/system sqlite;
  webpki-roots ⇒ no ca-certificates), `homeManagerModules.default`
  (`programs.open-media`), `devShells.default`, and `nixConfig` for the grok-insider
  cachix cache. Secrets stay out of the store (config via `om init`); mpv is a
  documented runtime dep.
- `.github/workflows/ci.yml`: `rust` job (fmt + clippy `-D warnings` + tests) on
  every push/PR; `build` job (master/tags/dispatch) builds `om` with Nix and
  pushes the closure to `grok-insider.cachix.org`.
- Published to `github:grok-insider/open-media`; wired into the NixOS host so `rebuild`
  installs `om` from cache. `nix build .#om` + `nix flake check` verified.

### Added — keyless metadata + UX
- **Cinemeta metadata (`om-metadata`)**: new keyless, IMDB-native `CinemetaProvider`
  (Stremio `v3-cinemeta.strem.io`) for movies and live-action series. It is wired
  on by default (`providers.cinemeta`), so **search works with no TMDB key** —
  `om search "breaking bad"` now returns results out of the box. TMDB stays as an
  optional richer source when a key is set; results are de-duplicated by IMDB id.
- **Season navigation (`om-cli` TUI)**: a Seasons screen plus real per-season
  episode lists fetched from the metadata provider; multi-season series are now
  fully browsable (previously only season 1, with a fabricated 12-episode list).
- **Sources filter/sort panel (`om-cli` TUI)**: a focusable right-hand panel
  (`Tab` to focus, `j/k` + `←/→` to adjust) to sort by relevance/seeders/quality/
  size and filter by quality, language, provider, and cached-only, with a live
  Details box for the highlighted release. Rows are now one scannable line each.
  Selections persist to `[ui.sources]` in `config.toml`.
- **Accurate player titles (`om-core::title`)**: mpv's `--force-media-title` (and
  Discord presence) now read `Series - S01E01 - Episode Title` (degrading to
  `Series - S01E01`, or `Movie (Year)`), via a shared pure title helper. The
  selected episode's title is threaded through `PlayRequest`.

### Fixed
- **Real-Debrid multi-file playback (`om-debrid`)**: a season pack now unrestricts
  the *requested* episode. RD's `links` array is indexed per *selected* file, not
  per full-torrent file; the resolver maps the candidate's file index correctly
  and labels the stream with the real file name.
- **Resolver cache-gating (`om-stream`)**: `HybridResolver` no longer forces a slow
  add→download→unrestrict warm-up for *uncached* candidates when P2P can stream
  them, and now falls back to P2P if a debrid resolve fails.
- **Torrentio for anime (`om-sources`)**: returns empty (instead of erroring) for
  titles with no IMDB id, so AniList anime cleanly fall through to nyaa.
- **Dead config key (`om-cli`)**: `behavior.complete_threshold` is now passed to the
  engine (previously always the hard-coded 0.85).
- **Debrid gating consistency (`om-cli`)**: the `realdebrid=` Torrentio parameter is
  only injected when Real-Debrid is the active backend, matching the provider
  wiring (`Config::has_real_debrid`).
- Removed the outdated "metadata adapters are stubbed until Phase 1" message and
  stale `[Phase 8]`/"Phase 4" doc comments.

### Added — Phases 4–9: streaming, players, session, history, tracking, TUI
- **P2P streaming (`om-stream`)**: `P2pEngine` over librqbit (rust-tls + http-api,
  no system OpenSSL) — magnet → metadata → largest video file → librqbit's
  Range-aware stream URL; `HybridResolver` made functional. Live-streamed Sintel
  in ~2.5s via real peers.
- **Players (`om-player`)**: `MpvPlayer` (launch + JSON-IPC `PlaybackControl`:
  seek/position/duration/pause/chapters/quit) and `VlcPlayer` (launch-only).
  Verified against real mpv 0.41.
- **Orchestration (`om-app` `Engine::play`)**: resolve → resume → enrich → launch →
  IPC monitor (auto-skip OP/ED on the 2s trigger, ~1 Hz progress persistence,
  Discord presence, chapters) cancelled by player exit via `select!` → teardown →
  completion → tracker update. `Engine::details`/`resolve` use-cases; `om play`
  wired to the full flow.
- **History (`om-history`)**: `SqliteHistory` (rusqlite bundled) — upserting
  progress, resume, recents; persists across reopen.
- **Tracking/enrich/presence (`om-track`)**: AniSkip (OP/ED by MAL) + Jikan filler;
  AniList (GraphQL) + MAL (REST) trackers + composite fan-out; Discord rich
  presence over real IPC framing (best-effort).
- **TUI (`om-cli`)**: ratatui interactive app (Search → Results → Episodes →
  Sources → play) with an `mpsc` render loop; logs routed off the alternate
  screen. Verified end-to-end with Wisp: live AniList search → 28 episodes → 43
  ranked nyaa sources.
- **Live validation**: Real-Debrid (premium account + 88 cached Torrentio direct
  URLs), real-mpv IPC, live P2P (Sintel), and the full TUI flow — all confirmed.
  Live/real-binary tests are gated `#[ignore]` + env so CI stays hermetic.
- **Tests**: 63 hermetic (unit + wiremock e2e + composition-root e2e) green;
  `cargo clippy -D warnings` clean.

### Added — Phases 1–3: discovery → resolve pipeline (implemented + tested)
- **Metadata (`om-metadata`)**: real `TmdbProvider` (movie/tv/multi search,
  details with IMDB hydration, seasons, episodes) and `AniListProvider` (GraphQL
  anime search/details with AniList+MAL ids). Errors mapped to `CoreError`.
- **Sources (`om-sources`)**: `TorrentioSource` (cache-aware streams, direct URLs
  + infohashes) and `NyaaSource` (namespace-robust RSS via quick-xml), plus a
  ported, unit-tested release-tag parser (quality/HDR/codec/audio/language/size).
- **Debrid (`om-debrid`)**: `RealDebrid` with the full
  add→poll→select→poll→unrestrict flow, account summary, and Auth/Remote/Timeout
  error mapping.
- **Resolver (`om-stream`)**: `HybridResolver` now functional — cached/direct →
  addon URL, otherwise → debrid `resolve_playback`, else P2P (Phase 4 stub).
- **App (`om-app`)**: `Engine::search`/`find_sources` parallelized with
  `futures::join_all`; added `Engine::details` and `Engine::resolve` use-cases.
- **Testing**: every network adapter ships unit tests + wiremock-based e2e
  integration tests, plus a composition-root e2e (`om-cli/tests/pipeline_e2e.rs`)
  that drives the whole search→details→sources→resolve pipeline (both the
  cached-direct and uncached-via-Real-Debrid branches) through the real `Engine`.
  **41 tests** total (16 unit + 25 integration/e2e); `cargo clippy -D warnings`
  clean.

### Added — Phase 0: scaffold & docs
- Cargo workspace with 11 `om-*` crates and an enforced ports-and-adapters
  dependency rule (hexagonal architecture).
- `om-core`: domain model (`Media`, `Episode`, `SourceCandidate`, `Playback`,
  `IdSet`, …), the full set of port traits (`MetadataProvider`, `SourceProvider`,
  `DebridProvider`, `StreamResolver`, `Player`/`PlaybackControl`, `Tracker`,
  `Enricher`, `HistoryStore`, `PresenceReporter`), a unified `CoreError`, and
  pure, unit-tested candidate scoring.
- `om-config`: TOML config schema, load/save, XDG paths, and a secrets policy
  (tokens live only in the user config file, never in code/repo/logs).
- Adapter crates (`om-metadata`, `om-sources`, `om-debrid`, `om-stream`,
  `om-player`, `om-track`, `om-history`) as contract-verified stubs — each
  implements its port returning `NotImplemented`, so the whole interface compiles.
- A real `CompositeTracker` (dual-write fan-out) in `om-track`.
- `om-app`: the `Engine` + `EngineBuilder` with working `search`/`find_sources`
  fan-out and ranking, depending only on `om-core` (tested with fake ports).
- `om-cli`: the `om` binary with `init`, `config {show,path,set}`, `search`, and
  `play` (scaffold), plus the composition root (`compose.rs`).
- Documentation: `README`, `AGENTS.md`, `docs/ARCHITECTURE.md`,
  `docs/RESEARCH.md`, `docs/PLAN.md`, `docs/ROADMAP.md`, `future-features.md`,
  `CONTRIBUTING.md`, MIT `LICENSE`.

### Notes
- The full discover → source → resolve → play pipeline is implemented and tested
  (Phases 1–9); network adapters are real, not stubs. Remaining work is packaging
  and polish — see `docs/PLAN.md`.

[Unreleased]: https://github.com/grok-insider/open-media/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/grok-insider/open-media/releases/tag/v0.1.0
