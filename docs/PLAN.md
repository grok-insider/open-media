# Build plan

Phased, vertical-slice plan. Each phase replaces a set of `NotImplemented` stubs
with working adapters and ends with concrete acceptance criteria. Phase numbers
match the `// Phase N` markers in the crate stubs.

Legend: `[x]` done · `[ ]` todo · **Mn** = user-visible milestone.

---

## Phase 0 — Scaffold & docs ✅
- [x] Cargo workspace, 11 `om-*` crates, dependency rule enforced.
- [x] `om-core`: domain model, all ports, scoring (+ unit tests).
- [x] `om-config`: schema, load/save, secrets policy (+ tests).
- [x] Every adapter stub implements its port (contract compile-checked).
- [x] `om-app` Engine/builder with real `search`/`find_sources` fan-out + ranking.
- [x] `om-cli` composition root + `om {init,config,search,play}`.
- [x] `cargo fmt` clean, `cargo clippy -D warnings` clean, tests green.
- [x] README, AGENTS, ARCHITECTURE, RESEARCH, PLAN, ROADMAP, CONTRIBUTING.

## Phase 1 — Metadata adapters (`om-metadata`) ✅
- [x] `TmdbProvider`: search (movie/tv/multi), details (+ external `imdb_id`),
      seasons, episodes. Real reqwest+serde client; errors mapped to `CoreError`.
- [x] `AniListProvider`: GraphQL search/details; populates `anilist` + `mal` ids;
      stays out of explicit movie/series searches.
- [x] Parallelized `Engine::search` (and `find_sources`) with `futures::join_all`.
- [x] Unit tests + wiremock e2e (`tests/metadata_e2e.rs`): 5 unit + 6 e2e.
- **Acceptance:** met under mock servers (search → results with ids; details →
  imdb). Real-network smoke test pending a live key.

## Phase 2 — Source adapters (`om-sources`) ✅
- [x] `TorrentioSource`: builds config string, fetches movie/series JSON, parses
      `name`/`title` → `SourceCandidate` (+ `ReleaseTags`, cache flags, direct
      url / infohash).
- [x] `NyaaSource`: RSS-feed search → candidates (magnet/infohash/seeders/size),
      via quick-xml event reader (namespace-robust).
- [x] Release-tag parsers (quality/HDR/codec/audio/language/size) ported from
      miru, unit-tested.
- [x] Unit tests + wiremock e2e (`tests/sources_e2e.rs`): 7 unit + 5 e2e.
- **Acceptance:** met — ranked, cache-aware candidates; nyaa contributes anime.

## Phase 3 — Debrid (`om-debrid`) ✅
- [x] `RealDebrid`: `account_summary`, `add_magnet`, `list_files`, `select_files`,
      `unrestrict`, and `resolve_playback` (full add→poll→select→poll→unrestrict).
- [x] `check_cached` best-effort no-op (instantAvailability deprecated; addon
      flags are primary). Auth/Remote/Timeout error mapping.
- [x] Unit tests + wiremock e2e (`tests/realdebrid_e2e.rs`): 2 unit + 4 e2e,
      incl. the full state-machine resolve and an auth-failure path.
- **M1 — met:** the composition-root e2e (`om-cli/tests/pipeline_e2e.rs`) resolves
  both a cached candidate (addon direct URL) and an uncached one (full RD flow).
  Remaining for a live M1: a real-token account-summary smoke test.

> **Cross-phase:** a wiremock-based e2e harness now covers the whole
> discovery→resolve pipeline through the real `Engine`. Phases 4–9 below build on
> it. Test totals so far: **41** (16 unit + 25 integration/e2e).

## Phase 4 — P2P streaming (`om-stream`) ✅
- [x] `P2pEngine` over librqbit (rust-tls + http-api, no system OpenSSL): add
      magnet, wait for metadata, pick largest video file, expose librqbit's
      `/torrents/{id}/stream/{idx}`, cleanup.
- [x] `HybridResolver`: direct (cached) → addon URL; else debrid `resolve_playback`;
      else P2P.
- [x] Hermetic HTTP-server test + ignored live test.
- **Acceptance:** met — live test streamed **Sintel** via real peers in ~2.5s
  (metadata → largest video file → ranged HTTP 206).

## Phase 5 — Players (`om-player`) ✅
- [x] `MpvPlayer::play`: spawn mpv with `--input-ipc-server`, `--force-media-title`,
      `--start`; `PlaySession::wait()` resolves on exit; `control()` → IPC.
- [x] `VlcPlayer::play`: launch-only, `--play-and-exit` (no control).
- [x] Fake-mpv IPC e2e + ignored real-mpv test (verified on mpv 0.41).
- **M2 — met:** real mpv launches, IPC round-trips (duration/seek/position), exits.

## Phase 6 — Tracking, enrich, presence (`om-track`) ✅
- [x] `MpvPlayer` IPC `PlaybackControl` (seek/position/duration/pause/chapters/quit).
- [x] `AniSkipEnricher`: AniSkip OP/ED (by MAL) + Jikan filler (paginated).
- [x] `AniListTracker` (GraphQL) + `MalTracker` (REST v2); `CompositeTracker` fan-out.
- [x] `DiscordPresence` (real IPC framing, best-effort).
- [x] 8 unit + 8 wiremock e2e. **Note:** trackers consume an existing token; the
      OAuth loopback *acquisition* flow is deferred to a follow-up.

## Phase 7 — History & resume (`om-history`) ✅
- [x] `SqliteHistory` (rusqlite bundled): `save`/`resume`/`recent`, upsert, reopen-persist.
- [x] Wired into `Engine::play` (resume via `--start`, progress saved ~1/s).
- [x] 5 tests. **Acceptance:** met (persist-across-reopen verified).

## Phase 8 — Playback orchestration (`om-app`) ✅
- [x] Full `Engine::play`: resolve → enrich (skip) → resume → launch → monitor over
      IPC (auto-skip OP/ED, progress/history, presence, chapters) via `select!` →
      teardown → complete→tracker. Optional ports degrade gracefully.
- **M3 — substantially met:** the session loop is implemented and wired; full
  unattended auto-advance/binge is a thin follow-up on top.

## Phase 9 — TUI (`om-cli`) ✅
- [x] ratatui app: `Screen` state machine + `mpsc` render loop (littlejohn pattern);
      Search → Results → Episodes → Sources → play; vim/arrow nav.
- [x] Logs routed off the alternate screen in TUI mode.
- **M4 — met:** verified end-to-end with Wisp — live AniList search → 28 episodes →
  **43 ranked nyaa sources** → clean quit. (Themes/wizard polish: follow-up.)

## Phase 10 — Packaging & release
- [ ] Nix flake (binary + Home Manager module), cachix push, like sibling repos.
- [ ] CI: fmt + clippy + test on stable; release artifacts.
- [ ] `CHANGELOG` 0.1.0; tag.
- **M5 — Acceptance:** `nix run github:0xfell/open-media -- play "…"` works on the
  NixOS host.

---

## Sequencing notes
- Phases 1→5 are the critical path to the first watchable build (**M2**).
- Phase 6's IPC work is the foundation for resume/skip/track; do it before 7/8.
- Anime-only features (AniSkip, AniList) never block movie/series playback —
  optional ports degrade gracefully.
