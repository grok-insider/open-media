# open-media

[![CI](https://github.com/grok-insider/open-media/actions/workflows/ci.yml/badge.svg)](https://github.com/grok-insider/open-media/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/grok-insider/open-media?sort=semver)](https://github.com/grok-insider/open-media/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Watch movies, series, and anime from your terminal** — instantly via
[Real-Debrid](https://real-debrid.com/) (cached, no seeding, no VPN) or directly
over P2P, streamed into **mpv** or **vlc**. One fast TUI for everything.

> **Status: released — v0.2.0.** The full pipeline — discover → source → resolve
> (Real-Debrid or P2P) → play in mpv/vlc — is implemented, tested, packaged
> (Nix + prebuilt binaries), and runs on **Linux, macOS, and Windows**. See
> [CHANGELOG.md](CHANGELOG.md).

---

## Why

There are great single-purpose terminal tools — [`ani-cli`] for anime,
[`miru`]/[`toru`] for streaming, [`curd`] for AniList-tracked anime — but each
covers only part of the picture, and the best ideas are scattered across five
codebases in three languages. open-media unifies them into **one** clean,
maintainable Rust application:

- **One pipeline for all content.** Movies and live-action series via TMDB or
  keyless Cinemeta, anime via AniList — all resolved through the same source →
  debrid/P2P → player path.
- **Real-Debrid first.** Cached releases play instantly from RD's CDN over HTTPS.
  No torrent traffic on your machine, no seeding, no VPN needed. Falls back to
  built-in P2P streaming when you have no debrid account or a release is uncached.
- **nyaa included.** Anime sources come from nyaa.si (directly and via Torrentio),
  so you get the SubsPlease/Erai-raws releases you actually want.
- **mpv done right.** Launches your existing mpv (and config) and drives it over
  IPC for resume, intro/outro auto-skip (AniSkip), progress tracking, and
  Discord presence.
- **Clean architecture.** Ports-and-adapters + SOLID, one crate per concern, so
  adding a debrid service, indexer, tracker, or player is a small isolated change.

[`ani-cli`]: https://github.com/pystardust/ani-cli
[`miru`]: https://github.com/YannickHerrero/miru
[`toru`]: https://github.com/sweetbbak/toru
[`curd`]: https://github.com/Wraient/curd

## The pipeline

```
              ┌─────────────┐   search    ┌──────────────────────┐
   you  ─────▶│  om (TUI)   │────────────▶│  MetadataProvider     │  TMDB / Cinemeta
              └─────────────┘             └──────────┬───────────┘  / AniList
                     │  pick title                   │ Media + ids (imdb/mal/…)
                     ▼                                ▼
              ┌─────────────┐   find      ┌──────────────────────┐
              │   Engine    │────────────▶│  SourceProvider(s)    │  Torrentio + nyaa
              │  (om-app)   │             └──────────┬───────────┘
              └─────────────┘   rank ◀───────────────┘ candidates (+cache flags)
                     │  best candidate
                     ▼
              ┌─────────────┐  resolve    ┌──────────────────────┐
              │StreamResolver│───────────▶│ DebridProvider (cached)│ ─▶ direct HTTPS URL
              │  (om-stream)│             │   or P2pEngine (librqbit)│ ─▶ http://127.0.0.1
              └──────┬──────┘             └──────────────────────┘
                     │  Playback url
                     ▼
              ┌─────────────┐  IPC: seek / time-pos / chapters
              │   Player    │◀───────────  resume · auto-skip OP/ED · progress
              │  (mpv/vlc)  │───────────▶  Tracker (AniList/MAL) · Discord presence
              └─────────────┘              · History (resume next time)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design and
[docs/RESEARCH.md](docs/RESEARCH.md) for the prior-art analysis this is built on.

## Features

- **Metadata**: TMDB (richer, optional key) + **Cinemeta** (keyless default) for
  movies/series, AniList for anime — with IMDB/MAL id bridging and de-dup.
- **Sources**: Torrentio (all trackers, cache-aware) + direct nyaa.si (RSS).
- **Debrid**: Real-Debrid magnet → instant CDN URL (add → select → unrestrict).
- **P2P**: librqbit engine streaming uncached / no-debrid torrents over a local
  Range-aware HTTP server.
- **Player**: mpv (launch + JSON-IPC: resume seek, auto-skip OP/ED, progress) and
  vlc (launch-only).
- **Session**: SQLite resume, AniSkip/Jikan, AniList/MAL tracking, Discord presence.
- **TUI**: `om` with no args → Search → Results → Seasons → Episodes → Sources →
  play, with a focusable filter/sort panel that persists to your config.

## Install

**Prebuilt binaries (no compiling)** — each [GitHub Release](https://github.com/grok-insider/open-media/releases)
attaches a static `om` for Linux (x86_64/aarch64, musl), macOS (x86_64/arm64), and
Windows (x86_64) as `om-<version>-<target>.tar.gz` (+ `.sha256`). Download, verify,
extract, and put `om` on your `PATH`.

**Nix / NixOS** (x86_64-linux; prebuilt on the `grok-insider` cachix cache):

```nix
# flake.nix inputs
inputs.open-media.url = "github:grok-insider/open-media";

# Home Manager — installs `om` (configure at runtime with `om init`; secrets
# never enter the Nix store)
imports = [ inputs.open-media.homeManagerModules.default ];
programs.open-media.enable = true;
```

Or ad hoc: `nix run github:grok-insider/open-media -- search "frieren"`.

**From source** (Rust ≥ 1.82):

```sh
git clone https://github.com/grok-insider/open-media
cd open-media
cargo build --release        # binary at target/release/om
```

> Building from source needs a C toolchain plus **cmake** and **clang/libclang**
> (for `aws-lc-sys`, the rustls crypto backend); SQLite is vendored. `nix develop`
> provides all of this. Prebuilt binaries and the Nix package have no such
> requirement.

## Prerequisites

- An external player on your `PATH`: **mpv** (recommended — unlocks IPC resume,
  OP/ED skip, and progress tracking) or **vlc** (launch-only).
- **All API tokens are optional.** Search works keyless via Cinemeta + AniList;
  without a Real-Debrid token, playback falls back to built-in P2P streaming.

## Usage

```sh
om init                                  # create ~/.config/open-media/config.toml
om config set real_debrid_token=...      # optional, recommended (instant cached playback)
om config set tmdb_api_key=...           # optional (Cinemeta already works keyless)
om                                       # interactive TUI
om search "interstellar"                 # list matches
om search "frieren" --kind anime
om play "interstellar"                   # one-shot: search → best source → play
om play "frieren" --season 1 --episode 1
om config show                           # print resolved config (secrets masked)
om config path                           # print the config file path
```

`om config set` edits these keys: `tmdb_api_key`, `real_debrid_token`,
`anilist_token`, `mal_token`, `debrid_provider`, `player_command`. Everything else
is set by editing `config.toml` directly.

## Configuration

A single TOML file at `~/.config/open-media/config.toml` (respects
`XDG_CONFIG_HOME`). **Secrets live only here** — never in the binary, the repo, or
the Nix store. `om init` creates it.

| Section / key | Default | Purpose |
|---------------|---------|---------|
| `[credentials]` `tmdb_api_key` | — | optional TMDB v3 key (Cinemeta is the keyless default) |
| `[credentials]` `real_debrid_token` | — | instant cached playback (else P2P) |
| `[credentials]` `anilist_token` / `mal_token` | — | anime progress tracking |
| `[credentials]` `debrid_provider` | `real-debrid` | active debrid backend |
| `[providers]` `cinemeta` / `nyaa_direct` | `true` | keyless movie/series source; direct nyaa.si |
| `[providers]` `quality` | `best` | `best` / `2160p` / `1080p` / `720p` / `480p` |
| `[providers]` `show_uncached` | `false` | include uncached sources (slower to start) |
| `[providers]` `torrentio_providers` | yts,eztv,…,nyaasi | Torrentio trackers, priority order |
| `[player]` `command` / `args` | `mpv` / `["--fullscreen"]` | player + extra args |
| `[streaming]` `http_port` / `cleanup_after_playback` | `3131` / `true` | local P2P stream server |
| `[behavior]` `skip_intro_outro` / `resume` | `true` | AniSkip OP/ED; resume from last position |
| `[behavior]` `skip_filler` / `complete_threshold` / `discord_presence` | `false` / `0.85` / `false` | binge filler-skip; mark-complete fraction; Discord RPC |
| `[ui]` `theme` / `[ui.sources]` | `auto` | UI theme; persisted Sources filter/sort panel |

> Discord rich presence is wired but ships with a placeholder application id, so
> it won't connect until a registered Discord app id is set.

## Project layout

A Cargo workspace of 11 crates, one per concern (full table in
[AGENTS.md](AGENTS.md#module-layout)):

```
crates/
  om-core      domain model + ports (traits) + scoring   — no I/O
  om-config    config schema + load/save + secrets policy
  om-metadata  TMDB + Cinemeta (keyless) + AniList        (MetadataProvider)
  om-sources   Torrentio, nyaa.si                         (SourceProvider)
  om-debrid    Real-Debrid (+ future AllDebrid/Torbox)    (DebridProvider)
  om-stream    librqbit P2P engine + hybrid resolver      (StreamResolver)
  om-player    mpv (IPC) + vlc                            (Player)
  om-track     AniList/MAL trackers + AniSkip + Discord   (Tracker/Enricher/…)
  om-history   SQLite watch history + resume              (HistoryStore)
  om-app       use-cases + Engine (composition)           — depends only on om-core
  om-cli       the `om` binary (composition root + TUI)
```

## Development

```sh
cargo test --workspace      # hermetic suite (unit + e2e vs. in-process mock servers)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Every network adapter has unit tests plus end-to-end integration tests against
in-process mock servers (`wiremock`); a composition-root e2e
(`crates/om-cli/tests/pipeline_e2e.rs`) drives search → details → sources →
resolve through the real `Engine`. Live integration tests (gated `#[ignore]` +
env) cover real Real-Debrid, real mpv, and live P2P.

## License

[MIT](LICENSE) © 2026 Grok Insider.

This is a client for services you bring your own account to (TMDB, Real-Debrid,
AniList) and for public indexes. You are responsible for complying with the laws
of your jurisdiction and the terms of those services.
