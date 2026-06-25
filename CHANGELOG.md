# Changelog

All notable changes to open-media are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- This release is a foundation: it builds, passes `cargo clippy -D warnings`, and
  has green tests, but network adapters are stubbed. See `docs/PLAN.md` for the
  phased path to the first watchable build (Milestone M2).

[Unreleased]: https://github.com/0xfell/open-media/commits/main
