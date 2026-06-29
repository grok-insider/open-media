# Architecture

open-media is built as a **hexagon** (ports & adapters) with a strict dependency
rule and SOLID boundaries. This document is the canonical description of the
design: the layers, the domain model, every port, the playback orchestration,
and the cross-cutting concerns.

Read `docs/RESEARCH.md` first if you want the prior-art rationale for *why* the
pieces are shaped the way they are.

---

## 1. Goals that shape the design

1. **One pipeline for movies, series, and anime.** The differences (TMDB vs
   AniList, IMDB vs MAL ids, nyaa vs general trackers) are isolated behind ports;
   the orchestration is identical.
2. **Debrid-first, P2P-capable.** Real-Debrid gives instant cached HTTPS streams;
   when absent or uncached, the same chosen release is streamed P2P locally. The
   player only ever sees an HTTP URL.
3. **Replaceable everything.** Debrid service, indexer, metadata source, tracker,
   player — each is an adapter behind a trait. New ones drop in without touching
   the core or the app.
4. **Testable without the network.** All policy (scoring, id bridging, config,
   the orchestration state machine) is testable with fakes.

## 2. Layers and the dependency rule

```
┌───────────────────────────────────────────────────────────────────────┐
│  Interface         open-media-cli  (clap CLI, ratatui TUI, composition root)    │
├───────────────────────────────────────────────────────────────────────┤
│  Application       open-media-app  (Engine + use-cases)   depends ONLY on core  │
├───────────────────────────────────────────────────────────────────────┤
│  Domain / Ports    open-media-core (models, traits, scoring)   depends on nothing│
├───────────────────────────────────────────────────────────────────────┤
│  Adapters          open-media-metadata open-media-sources open-media-debrid open-media-stream           │
│  (infrastructure)  open-media-player open-media-track open-media-history   each impl a port      │
└───────────────────────────────────────────────────────────────────────┘
```

- Dependencies point **inward**: adapters → core, app → core, cli → {app, core,
  adapters}. Nothing points outward from core.
- `open-media-app` must never name a concrete adapter. The compiler enforces it: `open-media-app`
  has no dependency on any adapter crate.
- `open-media-cli` is the **only** composition root. `crates/open-media-cli/src/compose.rs` is the
  single file that knows which concrete types exist.

This is Dependency Inversion at the crate level: the high-level policy
(`open-media-app`) and the low-level details (adapters) both depend on the abstraction
(`open-media-core` ports), not on each other.

## 3. Domain model (`open-media-core::model`, `::stream`, `::tracking`)

Pure data, `serde`-serializable, no behavior beyond small helpers.

- **Identity.** `MediaId` (one of `Tmdb`/`Imdb`/`AniList`/`Mal`) and `IdSet` (all
  known ids for an item). Adapters pick the dialect they need; `IdSet::merge`
  accumulates ids as different providers contribute them. This is the crux of
  cross-service interop: TMDB gives `imdb`, AniList gives `mal`, AniSkip needs
  `mal`, Torrentio needs `imdb`.
- **Content.** `MediaKind` (`Movie`/`Series`/`Anime` — anime is first-class so
  routing is explicit), `Media`, `Season`, `Episode`.
- **Sources & playback.** `SourceCandidate` (a found file: provider, title,
  `Quality`, size, seeders, infohash/magnet/direct-url, `CacheState`,
  `ReleaseTags`) and `Playback` (a resolved, player-openable URL + `PlaybackOrigin`
  so cleanup knows whether a torrent must be torn down).
- **Tracking.** `Interval`/`SkipTimes`, `WatchProgress` (resume + completion),
  `ListStatus`, `Activity` (presence).

## 4. Ports (`open-media-core::ports`)

Every port is a small, object-safe, `#[async_trait]` trait so the `Engine` can
hold `Arc<dyn Port>` chosen at runtime. ISP keeps them separate — a debrid
backend knows nothing about trackers.

| Port | Responsibility | Adapters |
|------|----------------|----------|
| `MetadataProvider` | search / details / seasons / episodes | TMDB, AniList |
| `SourceProvider` | find candidate files for a media+episode | Torrentio, nyaa |
| `DebridProvider` | magnet → instant CDN link; cache check | Real-Debrid (+future) |
| `StreamResolver` | chosen candidate → `Playback` (debrid or P2P) | HybridResolver |
| `Player` | launch a player for a `Playback` | mpv, vlc |
| `PlaybackControl` | live IPC: seek / position / pause / chapters / quit | mpv |
| `Tracker` | sync progress/status/score to a list service | AniList, MAL, Composite |
| `Enricher` | skip-times + filler/recap flags for an episode | AniSkip, Jikan |
| `HistoryStore` | persist/resume local watch progress | SQLite |
| `PresenceReporter` | "now watching" rich presence | Discord |

**Why `Player` and `PlaybackControl` are separate.** Launching and controlling
are different capabilities (ISP). vlc can only be launched; mpv additionally
exposes IPC. A `PlaySession::control()` returns `Some` only for players that
support it, so resume/auto-skip degrade gracefully instead of being impossible to
express.

**Why `DebridProvider` is shaped like rdbatch's `Provider`.** rdbatch proved one
interface cleanly abstracts Real-Debrid *and* Torbox despite very different APIs.
We keep the provider-agnostic `AddedTorrent`/`DebridFile` shapes so the resolver
and UI never see backend JSON, and `resolve_playback` hides the multi-step
add→poll→select→poll→unrestrict flow inside the adapter.

## 5. Application layer (`open-media-app`)

`Engine` holds the selected ports and exposes use-cases:

- `search(query, kind)` — fan out across `MetadataProvider`s, merge. Provider
  failures are logged, not fatal.
- `find_sources(req)` — fan out across applicable `SourceProvider`s (skipping
  ones whose `supports(kind)` is false), merge, and rank with
  `om_core::scoring`. Ranking lives in core so it is identical regardless of
  which providers ran.
- `play(req, candidate)` — the orchestrator (next section).

`EngineBuilder` assembles an `Engine`; optional ports left unset disable their
features (no tracker token ⇒ no tracking; vlc ⇒ no resume).

## 6. Playback orchestration

This is the heart of the app — the synthesis of toru's streaming, miru's
resolve, and curd's mpv-IPC control loop. Implemented in `Engine::play` (Phase
8).

```
play(req, candidate):
  1. resolve        StreamResolver::resolve(candidate) -> Playback
                      ├─ candidate.direct_url present (cached debrid)  -> use it
                      ├─ cached + debrid configured -> DebridProvider::resolve_playback
                      └─ else -> P2pEngine.stream_magnet(magnet)  (local HTTP, Range)
  2. enrich (anime) Enricher::skip_times(ids, ep)  +  (binge) filler_episodes
  3. resume         HistoryStore::resume(key, s, e) -> start position
  4. launch         Player::play(playback, {title, start_at}) -> PlaySession
  5. if control = session.control():            // mpv only
       spawn concurrent tasks over the IPC channel:
         • resume-seek: once playback starts, seek_absolute(resume_pos)
         • skip-loop:   poll time-pos; when inside OP/ED window -> seek_absolute(end)
         • chapters:    set_chapters([OP, main, ED]) once
         • progress:    poll time-pos ~1s -> HistoryStore::save; on >= threshold
                        -> Tracker::update_progress(ids, ep)
         • presence:    on pause/episode change -> PresenceReporter::update
  6. await           session.wait()              // blocks until player exits
  7. teardown        persist final position; StreamResolver::cleanup()
  8. advance         (binge) next non-filler episode -> recurse
```

Everything in step 5 flows through the *one* `PlaybackControl` channel — seek
(resume + skip), time-pos (progress/tracking), pause (presence), chapters
(AniSkip). That single control plane is built first; every higher feature is a
task that polls or seeks it.

## 7. Key data journeys

**Movie via Real-Debrid (cached).** TMDB search → `Media{imdb}` → Torrentio with
`realdebrid=KEY|debridoptions=nodownloadlinks` returns cached streams with
direct URLs and `[RD+]` flags → score → chosen candidate already has
`direct_url` → resolver returns it verbatim → mpv. *No torrent, no wait.*

**Anime via Real-Debrid.** AniList search → `Media{anilist, mal}` (+ TMDB/imdb
bridge) → Torrentio (`nyaasi` provider) and direct nyaa → cached `[RD+]` picks →
resolver returns RD URL → mpv, with AniSkip (keyed by `mal`) driving auto-skip
and AniList progress updates.

**Anime via P2P (no debrid / uncached).** Same discovery → chosen candidate has
only a magnet/infohash → `P2pEngine` adds it to librqbit, waits for metadata,
selects the largest video file, and serves
`http://127.0.0.1:3131/torrents/{id}/stream/{idx}` with Range support → mpv seeks
freely while pieces download around the read head.

## 8. The local P2P streaming engine (`open-media-stream`)

The Rust port of toru's crown jewel, but leaner: `librqbit` already ships a
Range-aware streaming endpoint (`/torrents/{id}/stream/{file_idx}`) backed by a
`FileStream` that prioritizes pieces around the read/seek head. So we **mount
librqbit's server** instead of hand-rolling `http.ServeContent`. Responsibilities:

- add magnet, `wait_until_initialized` (≈ toru's `<-GotInfo()`),
- pick the largest video file by extension,
- expose the stream URL,
- on cleanup, delete the torrent + (optionally) its files.

Binds `127.0.0.1` only. No firewall changes; debrid playback needs no inbound at
all.

## 9. Concurrency model

- One tokio multi-threaded runtime owned by `open-media-cli`.
- Network fan-out (`search`, `find_sources`) → `futures::join_all` (Phase 1).
- The TUI uses the littlejohn pattern: render-from-state loop + an
  `mpsc::unbounded` channel; all I/O is `tokio::spawn`ed and posts result
  messages back, so the UI never blocks (Phase 9).
- Playback spawns the step-5 tasks; they share the `Arc<dyn PlaybackControl>` and
  are aborted on `session.wait()` completion.
- Sync stores (rusqlite) run via `spawn_blocking`.

## 10. Errors

`CoreError` is the single error vocabulary across ports (variants:
`MissingCredential`, `Auth`, `Remote`, `Network`, `Parse`, `NotFound`,
`NoSource`, `Timeout`, `Storage`, `Player`, `Config`, `NotImplemented`, `Other`).
Adapters map their concrete errors to the right variant at the boundary so the
app can branch on failure *category* (e.g. retry on `Network`, prompt re-auth on
`Auth`) without knowing the backend. The CLI renders them as actionable messages.

## 11. Configuration & secrets

- One TOML document (`open-media-config::Config`) under `XDG_CONFIG_HOME`. `#[serde(default)]`
  throughout so a minimal file works; hand-written `Default` impls are kept in
  sync with the serde defaults (see the `Credentials` note in code).
- **Secrets (tokens) live only in that file.** Never compiled in, never in the
  repo, never logged (masked on display). Optional `OPEN_MEDIA_*` env overrides
  for CI/ephemeral use.

## 12. Scoring (`open-media-core::scoring`)

Pure, deterministic, unit-tested ranking applied after merge. Dominance order:
cached (when preferred) ≫ quality (rank + exact-target bonus) ≫ seeders/language
≫ codec/size tie-breakers. Centralized so ranking is identical across providers
and trivially testable.

## 13. Testing strategy

- **Core**: scoring, id bridging, size/quality parsing, config round-trips — pure
  unit tests (already present).
- **Adapters**: parse real recorded fixtures (a Torrentio JSON, a nyaa RSS page,
  an RD `torrents/info` body); no live calls in CI.
- **App**: fake ports (see `open-media-app`'s `FakeMeta` test) to assert orchestration
  branching, merge, and ranking without a network.
- **Gate**: `cargo fmt`, `cargo clippy -D warnings`, `cargo test --workspace`.

## 14. Mapping to SOLID

| Principle | Where it shows up |
|-----------|-------------------|
| **S**RP | one crate per concern; one adapter per backend; scoring isolated from I/O |
| **O**CP | new debrid/indexer/tracker/player = new adapter + one line in `compose.rs` |
| **L**SP | every adapter honors its port's contract incl. `CoreError` semantics |
| **I**SP | many narrow ports; `Player` vs `PlaybackControl` split; debrid ⊥ tracker |
| **D**IP | `open-media-app` depends on `open-media-core` ports; only `open-media-cli` names concrete adapters |

## 15. Known deferrals

See `docs/ROADMAP.md` for phase ordering and `future-features.md` for the
backlog (subtitles/OpenSubtitles, poster art via terminal image protocols,
syncplay/watch-together, a Jellyfin/Zurg-style library mode, additional debrid
backends, Trakt tracking).
