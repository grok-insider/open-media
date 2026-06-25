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

## Phase 4 — P2P streaming (`om-stream`)
- [ ] `P2pEngine` over librqbit: add magnet, wait for metadata, pick largest video
      file, expose `/torrents/{id}/stream/{idx}`, cleanup.
- [ ] `HybridResolver`: cached/direct → debrid URL; else warm-cache or P2P.
- **Acceptance:** an uncached/no-debrid candidate yields a working
  `http://127.0.0.1:PORT/...` URL that streams + seeks in a browser/mpv.

## Phase 5 — Players (`om-player`)
- [ ] `MpvPlayer::play`: spawn mpv, `--force-media-title`, return a `PlaySession`
      whose `wait()` resolves on exit.
- [ ] `VlcPlayer::play`: launch-only, `--play-and-exit`.
- [ ] Minimal `Engine::play` straight-through (resolve → launch → wait), no IPC.
- **M2 — Acceptance:** `om play "dune"` plays a cached RD stream in mpv,
  end-to-end. **First watchable build.**

## Phase 6 — Tracking, enrich, presence (`om-track`)
- [ ] `MpvPlayer` IPC: `PlaybackControl` (seek/position/duration/pause/chapters/
      quit) over the unix socket; `PlaySession::control()` returns it.
- [ ] `AniSkipEnricher`: AniSkip (by mal) + Jikan filler.
- [ ] `AniListTracker` + `MalTracker` (OAuth loopback); `CompositeTracker` already
      done.
- [ ] `DiscordPresence` (throttled).
- **Acceptance:** during mpv playback, intros auto-skip; AniList progress updates
  at the completion threshold; presence shows "watching".

## Phase 7 — History & resume (`om-history`)
- [ ] `SqliteHistory`: schema + migrations; `save`/`resume`/`recent` via
      `spawn_blocking`.
- [ ] Wire resume (seek on start) + recents list.
- **Acceptance:** quitting mid-episode and replaying resumes at the saved second;
  `om` shows a "continue watching" list.

## Phase 8 — Playback orchestration (`om-app`)
- [ ] Full `Engine::play`: resolve → enrich → resume → launch → spawn IPC tasks
      (resume-seek, skip-loop, progress/tracking, presence) → wait → teardown →
      advance (binge, skip filler).
- **M3 — Acceptance:** a full session works unattended: resume, auto-skip,
  progress sync, presence, auto-advance to the next non-filler episode.

## Phase 9 — TUI (`om-cli` → maybe split `om-tui`)
- [ ] ratatui app: `AppMode` state machine + `mpsc` render loop (littlejohn
      pattern); search → results → seasons/episodes → sources → playing.
- [ ] Themes (auto/dark/light), key nav, masked-secret init wizard.
- **M4 — Acceptance:** `om` with no args launches the TUI; full flow is mouse-free.

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
