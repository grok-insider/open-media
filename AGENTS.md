# AGENTS.md

Instructions for AI agents and contributors working on **open-media**.

## Project overview

open-media is a Rust terminal app to watch **movies, series, and anime** from one
interface. It discovers titles (TMDB / AniList), finds releasable files
(Torrentio / nyaa.si), turns the chosen one into a playable URL via a **debrid**
service (Real-Debrid: instant, cached, no P2P/seeding/VPN) or a **built-in P2P
streamer** (librqbit), and plays it in **mpv** (driven over IPC) or **vlc**. On
top of playback it layers resume, AniSkip intro/outro skipping, AniList/MAL
progress tracking, Discord presence, and optional external subtitles.

It is a from-scratch synthesis of the best ideas from `miru`, `toru`, `ani-cli`,
`curd`, `littlejohn`, and `rdbatch` — see `docs/RESEARCH.md` for the analysis and
`docs/ARCHITECTURE.md` for the design.

- **Native Rust**, async (tokio). No shelling out to other media CLIs.
- **Cargo workspace**, one crate per concern (see Module layout).
- **Ports & adapters (hexagonal) + SOLID.** Capabilities are trait *ports* in
  `open-media-core`; concrete *adapters* implement them; the app layer depends only on
  ports; the binary is the single composition root. Adding a backend = a new
  adapter, not an edit to the core.
- **License: MIT.**

## Module layout

One crate per concern. To add a top-level concern, add `crates/open-media-<name>` and a
member entry in the root `Cargo.toml`. Crate prefix is `open-media-`.

| Crate | Owns | Implements |
|-------|------|------------|
| `crates/open-media-core` | Domain model (`Media`, `Episode`, `SourceCandidate`, `Playback`, …), the **port traits**, error type, and pure scoring. **No I/O, no heavy deps.** | — (defines the ports) |
| `crates/open-media-net` | Shared HTTP client factory, user-agent, timeouts, and retry helper used by network adapters. | — |
| `crates/open-media-config` | Config schema, load/save, XDG paths, the secrets policy. | — |
| `crates/open-media-metadata` | TMDB + **Cinemeta** (keyless) for movies/series; AniList for anime. | `MetadataProvider` |
| `crates/open-media-sources` | Torrentio addon + direct nyaa.si (RSS); release-tag parsing. | `SourceProvider` |
| `crates/open-media-debrid` | Real-Debrid + TorBox REST clients (+ future AllDebrid/Premiumize). | `DebridProvider` |
| `crates/open-media-stream` | librqbit P2P engine + Range HTTP server, and the hybrid resolver that picks debrid-direct vs P2P. | `StreamResolver` |
| `crates/open-media-player` | mpv launch + JSON-IPC control plane; vlc launch-only. | `Player`, `PlaybackControl`, `PlaySession` |
| `crates/open-media-subs` | Adapter around the `open-subtitle` engine (OpenSubtitles/SubDL/Jimaku, decoded temp tracks). | `SubtitleProvider` |
| `crates/open-media-track` | AniList/MAL trackers (+ composite dual-write), AniSkip/Jikan enricher, Discord presence. | `Tracker`, `Enricher`, `PresenceReporter` |
| `crates/open-media-history` | SQLite watch-progress store for resume + recents. | `HistoryStore` |
| `crates/open-media-telemetry` | Anonymous, opt-out active-install ping (version/OS/arch/random id). **Never carries anything about what is watched.** | `UsageReporter` |
| `crates/open-media-app` | Application use-cases + the `Engine` that composes ports. **Depends only on `open-media-core`.** | — (consumes ports) |
| `crates/open-media-cli` | The `open-media` binary: arg parsing, the **composition root** (`compose.rs`), and the ratatui TUI. | — (wires adapters) |

### The dependency rule (do not break this)

```
open-media-cli ──▶ open-media-app ──▶ open-media-core ◀── every adapter crate
   │                                    ▲
   └──────────── wires ─────────────────┘   (only open-media-cli may name concrete adapters)
```

- `open-media-core` depends on nothing internal.
- `open-media-app` depends on **only** `open-media-core`. It must never `use` an adapter crate.
  If you find yourself wanting to, you need a new port in `open-media-core` instead.
- Adapter crates depend on `open-media-core` (+ their own I/O deps). They never depend on
  each other or on `open-media-app`.
- `open-media-cli` is the **only** crate allowed to depend on concrete adapters; it
  assembles them into the `Engine` in `crates/open-media-cli/src/compose.rs`.

## Architecture in one screen

1. **Ports** (`open-media-core::ports`) are small, focused, object-safe `async` traits:
   `MetadataProvider`, `SourceProvider`, `DebridProvider`, `StreamResolver`,
   `Player`/`PlaybackControl`, `Tracker`, `Enricher`, `HistoryStore`,
   `SubtitleProvider`, `IdBridge`, `PresenceReporter`, `UsageReporter`.

   **Privacy invariant (`UsageReporter`):** usage telemetry must only ever carry
   the fields in `UsageInfo` (app version, OS, arch, a random install id). It must
   **never** transmit anything about what a user watches — titles, queries, source
   names, tokens, or history. Do not extend the payload with content-derived data.
2. **Adapters** implement a port each. They map their concrete errors into
   `CoreError` at the boundary.
3. **`Engine`** (`open-media-app`) holds `Arc<dyn Port>` fields and implements the
   use-cases (`search`, `find_sources`, `play`). It is constructed by
   `EngineBuilder`; unset optional ports simply disable their features.
4. **Composition root** (`open-media-cli::compose::build_engine`) reads `Config` and
   chooses which adapters to instantiate.

The full playback orchestration (resolve → launch → resume/skip/track/presence →
advance) is specified in `docs/ARCHITECTURE.md#playback-orchestration`.

## Coding standards

- **SOLID, concretely:**
  - *SRP* — one reason to change per type/crate.
  - *OCP* — extend by adding an adapter/port impl, never by editing core/app.
  - *LSP* — every adapter must honor its port's documented contract (including
    error semantics: return the right `CoreError` variant).
  - *ISP* — keep ports narrow. A debrid backend must not depend on tracker types.
    Split a trait before it grows a method only one impl needs.
  - *DIP* — depend on `open-media-core` ports, never on a concrete adapter, outside
    `open-media-cli`.
- **Errors:** ports return `open_media_core::CoreResult<T>`. Adapters convert with
  explicit mapping (no `?` straight from `reqwest::Error` into `CoreError` unless
  there is a `From` that picks the right variant). User-facing messages are
  actionable.
- **Async:** tokio. Don't block the runtime; use `spawn_blocking` for sync I/O
  (e.g. rusqlite). Network fan-out (`search`, `find_sources`) parallelizes with
  `futures::join_all`.
- **No secrets in code or logs.** Tokens come only from `open-media-config`. Never log a
  token; mask in any display.
- **Formatting/lints:** `cargo fmt` + `cargo clippy --workspace --all-targets -D
  warnings` must be clean. Repo-wide clippy lints are set in the root
  `Cargo.toml`.
- **Tests:** pure logic (scoring, parsers, id bridging, config) is unit-tested
  with no network. Adapters get tests against recorded fixtures, not the live
  service. The app layer is tested with fake ports (see `open-media-app`'s tests).
- **Comments** explain *why* (a protocol quirk, a rate limit, a scoring choice),
  not *what*. Keep them factual.

## How to add an adapter (the common task)

Example: add a Torbox debrid backend.

1. In `crates/open-media-debrid/src/`, add `torbox.rs` with a `Torbox` struct that
   `impl DebridProvider`. Map Torbox's `{success,data}` envelope + errors into
   `CoreError`.
2. Export it from `open-media-debrid`'s `lib.rs`.
3. In `crates/open-media-cli/src/compose.rs`, select it when
   `cfg.credentials.debrid_provider == "torbox"`.
4. Add any config keys to `open-media-config`.
5. Tests + `cargo clippy`/`fmt`. **No other crate changes** — that's OCP working.

Adding a new *capability* (not just a backend) means a new port trait in
`open-media-core::ports`, consumed by `open-media-app`, wired in `open-media-cli`.

## Commands

```bash
cargo build                                   # debug build; binary: target/debug/open-media
cargo run -p open-media-cli -- search "frieren"       # run the CLI
cargo test --workspace                        # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Smoke-test the binary without touching your real config by setting a throwaway
`XDG_CONFIG_HOME`:

```bash
XDG_CONFIG_HOME=/tmp/om ./target/debug/open-media init && XDG_CONFIG_HOME=/tmp/om ./target/debug/open-media config show
```

## Roadmap & planning

- `docs/PLAN.md` — the phased build plan + acceptance criteria (Phases 0–10 done;
  post-0.1 hardening tracked at the bottom).
- `docs/ROADMAP.md` — version milestones and the feature matrix.
- `continue-plan.md` — the actionable "still to do" list (audit follow-ups, the
  bugs and incomplete features we deferred). Check here before starting new work.
- `future-features.md` — the broader, aspirational backlog / nice-to-haves.
- `CHANGELOG.md` — keep-a-changelog, generated by release-plz from Conventional
  Commits and then enriched in the release PR by the release workflow. Don't
  hand-edit it outside a release PR; a clear `feat:`/`fix:` commit *is* the input.

When you finish a phase, tick its boxes in `docs/PLAN.md`. When you finish a
deferred item, strike it from `continue-plan.md`. Newly discovered nice-to-haves
go in `future-features.md`.

## Releasing & versioning

Releases are automated with **release-plz** (`release-plz.toml` +
`.github/workflows/release.yml`); see `CONTRIBUTING.md` → Releases.

- **Conventional Commits are required** — the history drives patch release PRs and
  the changelog. `feat:` and `fix:` both trigger automatic **patch** releases;
  repo-admin-only manual version-bump PRs own deliberate minor/major milestones.
  Avoid `feat!:` / `BREAKING CHANGE:` in the normal automatic stream because
  release-plz treats breaking commits as minor/major signals.
  `docs/refactor/perf/test/chore/ci` don't trigger a release on their own (only
  `feat`/`fix` open a release PR — `release_commits`).
- **One version, single-sourced:** root `Cargo.toml` `[workspace.package].version`;
  every crate inherits it (`version.workspace = true`). Never bump it by hand —
  release-plz does, in lockstep across all crates (`version_group`). Internal path
  deps carry a `version` req (needed by `cargo package`) that release-plz also keeps
  in lockstep; don't edit those by hand.
- **Flow:** land feature/fix PRs in `dev`, then open the single sanctioned
  `dev`→`master` integration PR (release-plz PRs are the only other branch allowed
  into `master`). A `feat:`/`fix:` on `master` makes release-plz maintain a patch
  release PR (version bump + `CHANGELOG` + `Cargo.lock`); merging it tags
  `vX.Y.Z`, publishes crates to crates.io in dependency order, creates the GitHub
  Release + prebuilt `open-media` archives, and CI pushes `open-media-X.Y.Z` to
  cachix (`flake.nix` reads the version). Repo admins can run the Manual Version
  Bump workflow when a minor/major milestone should ship.

## Conventions

- Repo: `github.com/grok-insider/open-media`. Binary name: `open-media`.
- Phases 0–10 are complete: production adapters implement their ports for real
  (only test fakes use `NotImplemented`). New work is a new adapter (another backend) or a new port
  in `open-media-core` (a new capability) — not edits to the core/app; see "How to add an
  adapter".
- Prefer fixing the contract in `open-media-core` over working around it in an adapter.
