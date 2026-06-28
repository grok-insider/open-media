# AGENTS.md

Instructions for AI agents and contributors working on **open-media**.

## Project overview

open-media is a Rust terminal app to watch **movies, series, and anime** from one
interface. It discovers titles (TMDB / AniList), finds releasable files
(Torrentio / nyaa.si), turns the chosen one into a playable URL via a **debrid**
service (Real-Debrid: instant, cached, no P2P/seeding/VPN) or a **built-in P2P
streamer** (librqbit), and plays it in **mpv** (driven over IPC) or **vlc**. On
top of playback it layers resume, AniSkip intro/outro skipping, AniList/MAL
progress tracking, and Discord presence.

It is a from-scratch synthesis of the best ideas from `miru`, `toru`, `ani-cli`,
`curd`, `littlejohn`, and `rdbatch` — see `docs/RESEARCH.md` for the analysis and
`docs/ARCHITECTURE.md` for the design.

- **Native Rust**, async (tokio). No shelling out to other media CLIs.
- **Cargo workspace**, one crate per concern (see Module layout).
- **Ports & adapters (hexagonal) + SOLID.** Capabilities are trait *ports* in
  `om-core`; concrete *adapters* implement them; the app layer depends only on
  ports; the binary is the single composition root. Adding a backend = a new
  adapter, not an edit to the core.
- **License: MIT.**

## Module layout

One crate per concern. To add a top-level concern, add `crates/om-<name>` and a
member entry in the root `Cargo.toml`. Crate prefix is `om-`.

| Crate | Owns | Implements |
|-------|------|------------|
| `crates/om-core` | Domain model (`Media`, `Episode`, `SourceCandidate`, `Playback`, …), the **port traits**, error type, and pure scoring. **No I/O, no heavy deps.** | — (defines the ports) |
| `crates/om-config` | Config schema, load/save, XDG paths, the secrets policy. | — |
| `crates/om-metadata` | TMDB + **Cinemeta** (keyless) for movies/series; AniList for anime. | `MetadataProvider` |
| `crates/om-sources` | Torrentio addon + direct nyaa.si (RSS); release-tag parsing. | `SourceProvider` |
| `crates/om-debrid` | Real-Debrid REST client (+ future AllDebrid/Torbox/Premiumize). | `DebridProvider` |
| `crates/om-stream` | librqbit P2P engine + Range HTTP server, and the hybrid resolver that picks debrid-direct vs P2P. | `StreamResolver` |
| `crates/om-player` | mpv launch + JSON-IPC control plane; vlc launch-only. | `Player`, `PlaybackControl`, `PlaySession` |
| `crates/om-track` | AniList/MAL trackers (+ composite dual-write), AniSkip/Jikan enricher, Discord presence. | `Tracker`, `Enricher`, `PresenceReporter` |
| `crates/om-history` | SQLite watch-progress store for resume + recents. | `HistoryStore` |
| `crates/om-telemetry` | Anonymous, opt-out active-install ping (version/OS/arch/random id). **Never carries anything about what is watched.** | `UsageReporter` |
| `crates/om-app` | Application use-cases + the `Engine` that composes ports. **Depends only on `om-core`.** | — (consumes ports) |
| `crates/om-cli` | The `om` binary: arg parsing, the **composition root** (`compose.rs`), and the ratatui TUI. | — (wires adapters) |

### The dependency rule (do not break this)

```
om-cli ──▶ om-app ──▶ om-core ◀── every adapter crate
   │                                    ▲
   └──────────── wires ─────────────────┘   (only om-cli may name concrete adapters)
```

- `om-core` depends on nothing internal.
- `om-app` depends on **only** `om-core`. It must never `use` an adapter crate.
  If you find yourself wanting to, you need a new port in `om-core` instead.
- Adapter crates depend on `om-core` (+ their own I/O deps). They never depend on
  each other or on `om-app`.
- `om-cli` is the **only** crate allowed to depend on concrete adapters; it
  assembles them into the `Engine` in `crates/om-cli/src/compose.rs`.

## Architecture in one screen

1. **Ports** (`om-core::ports`) are small, focused, object-safe `async` traits:
   `MetadataProvider`, `SourceProvider`, `DebridProvider`, `StreamResolver`,
   `Player`/`PlaybackControl`, `Tracker`, `Enricher`, `HistoryStore`,
   `PresenceReporter`, `UsageReporter`.

   **Privacy invariant (`UsageReporter`):** usage telemetry must only ever carry
   the fields in `UsageInfo` (app version, OS, arch, a random install id). It must
   **never** transmit anything about what a user watches — titles, queries, source
   names, tokens, or history. Do not extend the payload with content-derived data.
2. **Adapters** implement a port each. They map their concrete errors into
   `CoreError` at the boundary.
3. **`Engine`** (`om-app`) holds `Arc<dyn Port>` fields and implements the
   use-cases (`search`, `find_sources`, `play`). It is constructed by
   `EngineBuilder`; unset optional ports simply disable their features.
4. **Composition root** (`om-cli::compose::build_engine`) reads `Config` and
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
  - *DIP* — depend on `om-core` ports, never on a concrete adapter, outside
    `om-cli`.
- **Errors:** ports return `om_core::CoreResult<T>`. Adapters convert with
  explicit mapping (no `?` straight from `reqwest::Error` into `CoreError` unless
  there is a `From` that picks the right variant). User-facing messages are
  actionable.
- **Async:** tokio. Don't block the runtime; use `spawn_blocking` for sync I/O
  (e.g. rusqlite). Network fan-out (`search`, `find_sources`) parallelizes with
  `futures::join_all`.
- **No secrets in code or logs.** Tokens come only from `om-config`. Never log a
  token; mask in any display.
- **Formatting/lints:** `cargo fmt` + `cargo clippy --workspace --all-targets -D
  warnings` must be clean. Repo-wide clippy lints are set in the root
  `Cargo.toml`.
- **Tests:** pure logic (scoring, parsers, id bridging, config) is unit-tested
  with no network. Adapters get tests against recorded fixtures, not the live
  service. The app layer is tested with fake ports (see `om-app`'s tests).
- **Comments** explain *why* (a protocol quirk, a rate limit, a scoring choice),
  not *what*. Keep them factual.

## How to add an adapter (the common task)

Example: add a Torbox debrid backend.

1. In `crates/om-debrid/src/`, add `torbox.rs` with a `Torbox` struct that
   `impl DebridProvider`. Map Torbox's `{success,data}` envelope + errors into
   `CoreError`.
2. Export it from `om-debrid`'s `lib.rs`.
3. In `crates/om-cli/src/compose.rs`, select it when
   `cfg.credentials.debrid_provider == "torbox"`.
4. Add any config keys to `om-config`.
5. Tests + `cargo clippy`/`fmt`. **No other crate changes** — that's OCP working.

Adding a new *capability* (not just a backend) means a new port trait in
`om-core::ports`, consumed by `om-app`, wired in `om-cli`.

## Commands

```bash
cargo build                                   # debug build; binary: target/debug/om
cargo run -p om-cli -- search "frieren"       # run the CLI
cargo test --workspace                        # all tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Smoke-test the binary without touching your real config by setting a throwaway
`XDG_CONFIG_HOME`:

```bash
XDG_CONFIG_HOME=/tmp/om ./target/debug/om init && XDG_CONFIG_HOME=/tmp/om ./target/debug/om config show
```

## Roadmap & planning

- `docs/PLAN.md` — the phased build plan + acceptance criteria (Phases 0–10 done;
  post-0.1 hardening tracked at the bottom).
- `docs/ROADMAP.md` — version milestones and the feature matrix.
- `continue-plan.md` — the actionable "still to do" list (audit follow-ups, the
  bugs and incomplete features we deferred). Check here before starting new work.
- `future-features.md` — the broader, aspirational backlog / nice-to-haves.
- `CHANGELOG.md` — keep-a-changelog, **generated by release-plz from Conventional
  Commits**. Don't hand-edit it; a clear `feat:`/`fix:` commit *is* the entry.

When you finish a phase, tick its boxes in `docs/PLAN.md`. When you finish a
deferred item, strike it from `continue-plan.md`. Newly discovered nice-to-haves
go in `future-features.md`.

## Releasing & versioning

Releases are automated with **release-plz** (`release-plz.toml` +
`.github/workflows/release.yml`); see `CONTRIBUTING.md` → Releases.

- **Conventional Commits are required** — the history drives the version bump and
  the changelog. `feat:` → minor, `fix:` → patch (this is an app, so
  `features_always_increment_minor` makes `feat` bump minor even on 0.x); `feat!:` /
  `BREAKING CHANGE:` → breaking; `docs/refactor/perf/test/chore/ci` don't trigger a
  release on their own (only `feat`/`fix` open a release PR — `release_commits`).
- **One version, single-sourced:** root `Cargo.toml` `[workspace.package].version`;
  every crate inherits it (`version.workspace = true`). Never bump it by hand —
  release-plz does, in lockstep across all crates (`version_group`). Internal path
  deps carry a `version` req (needed by `cargo package`) that release-plz also keeps
  in lockstep; don't edit those by hand.
- **Flow:** push `feat:`/`fix:` to `master` → release-plz maintains a release PR
  (bump + `CHANGELOG` + `Cargo.lock`) → merge it → tags `vX.Y.Z`, publishes the
  GitHub Release + a prebuilt `om`, and CI pushes `om-X.Y.Z` to cachix (`flake.nix`
  reads the version). Nothing goes to crates.io (`git_only`).

## Conventions

- Repo: `github.com/grok-insider/open-media`. Binary name: `om`.
- Phases 0–10 are complete: every adapter implements its port for real (no more
  `NotImplemented` stubs). New work is a new adapter (another backend) or a new port
  in `om-core` (a new capability) — not edits to the core/app; see "How to add an
  adapter".
- Prefer fixing the contract in `om-core` over working around it in an adapter.
