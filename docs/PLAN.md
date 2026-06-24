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

## Phase 1 — Metadata adapters (`om-metadata`)
- [ ] `TmdbProvider`: search (movie+tv), details (+ external `imdb_id`), seasons,
      episodes. Real reqwest+serde client; map errors to `CoreError`.
- [ ] `AniListProvider`: GraphQL search/details; populate `anilist` + `mal` ids.
- [ ] Parallelize `Engine::search` with `futures::join_all`.
- [ ] Fixture-based tests (recorded JSON).
- **Acceptance:** `om search "frieren"` and `om search "dune" --kind movie` print
  real, deduplicated results with ids.

## Phase 2 — Source adapters (`om-sources`)
- [ ] `TorrentioSource`: build config string, fetch movie/series JSON, parse
      `name`/`title` → `SourceCandidate` (+ `ReleaseTags`, cache flags, direct
      url / infohash).
- [ ] `NyaaSource`: RSS feed search → candidates (magnet/infohash/seeders/size).
- [ ] Release-tag parsers (quality/HDR/codec/audio/language) ported from miru,
      unit-tested.
- **Acceptance:** `om sources "<title>"` (debug cmd) lists ranked candidates with
  cache/quality/seeders; nyaa contributes anime rows.

## Phase 3 — Debrid (`om-debrid`)
- [ ] `RealDebrid`: `account_summary`, `add_magnet`, `list_files`, `select_files`,
      `unrestrict`, and `resolve_playback` (full add→poll→select→poll→unrestrict),
      with rate-limit backoff.
- [ ] `check_cached` best-effort (addon flags are primary).
- **M1 — Acceptance:** given a cached candidate, `om` resolves a direct RD URL
  (printed). Account summary shows premium status.

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
