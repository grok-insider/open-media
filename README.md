# open-media

[![CI](https://github.com/grok-insider/open-media/actions/workflows/ci.yml/badge.svg)](https://github.com/grok-insider/open-media/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/grok-insider/open-media?sort=semver)](https://github.com/grok-insider/open-media/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Watch movies, series, and anime from your terminal** — instantly via
[Real-Debrid](https://real-debrid.com/) (cached, no seeding, no VPN) or directly
over P2P, streamed into **mpv** or **vlc**. One fast TUI for everything.

> **Status: released — v0.6.3.** The full pipeline — discover → source → resolve
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
   you  ─────▶│ open-media  │────────────▶│  MetadataProvider     │  TMDB / Cinemeta
              └─────────────┘             └──────────┬───────────┘  / AniList
                     │  pick title                   │ Media + ids (imdb/mal/…)
                     ▼                                ▼
              ┌─────────────┐   find      ┌──────────────────────┐
              │   Engine    │────────────▶│  SourceProvider(s)    │  Torrentio + nyaa
              │  (open-media-app)   │             └──────────┬───────────┘
              └─────────────┘   rank ◀───────────────┘ candidates (+cache flags)
                     │  best candidate
                     ▼
              ┌─────────────┐  resolve    ┌──────────────────────┐
              │StreamResolver│───────────▶│ DebridProvider (cached)│ ─▶ direct HTTPS URL
              │  (open-media-stream)│             │   or P2pEngine (librqbit)│ ─▶ http://127.0.0.1
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
- **Debrid**: Real-Debrid (add → select → unrestrict) or TorBox (create →
  request link, with a real bulk cache check) — magnet → instant CDN URL.
- **P2P**: librqbit engine streaming uncached / no-debrid torrents over a local
  Range-aware HTTP server.
- **Player**: mpv (launch + JSON-IPC: resume seek, auto-skip OP/ED, progress) and
  vlc (launch-only).
- **Session**: SQLite resume, AniSkip/Jikan, AniList/MAL tracking, Discord
  presence. Autoplay-next keeps a single mpv alive and appends the next episode
  to its playlist, so mpv's own Next button works too.
- **Library**: a local watchlist (watching / completed / planning / …) that
  updates itself as you watch, browsable in the TUI and via
  `open-media library`, with best-effort AniList/MAL status dual-write.
- **Subtitles**: optional OpenSubtitles/SubDL/Jimaku fetch through the
  `open-subtitle` engine, passed to players as temp `--sub-file` tracks.
- **TUI**: `open-media` with no args → Home (Continue Watching + Library) →
  Search → Results → Seasons → Episodes → Sources → play. Results stream in
  incrementally as providers respond, mouse (click/wheel) is supported, and the
  Sources filter/sort panel persists to your config.

## Install

**Prebuilt binaries (no compiling)** — each [GitHub Release](https://github.com/grok-insider/open-media/releases)
attaches `open-media-<version>-<target>.tar.gz` (+ `.sha256`) for static Linux
(x86_64/aarch64 musl), native macOS (x86_64/arm64), and native Windows (x86_64).
Download, verify, extract, and put `open-media` on your `PATH`.

**Cargo / crates.io**:

```sh
cargo install open-media-cli
```

**Nix / NixOS** (x86_64-linux; prebuilt on the `grok-insider` cachix cache):

```nix
# flake.nix inputs
inputs.open-media.url = "github:grok-insider/open-media";

# Home Manager — installs `open-media` (configure at runtime with `open-media init`; secrets
# never enter the Nix store)
imports = [ inputs.open-media.homeManagerModules.default ];
programs.open-media.enable = true;
```

Or ad hoc: `nix run github:grok-insider/open-media -- search "frieren"`.

**From source** (Rust ≥ 1.82):

```sh
git clone https://github.com/grok-insider/open-media
cd open-media
cargo build --release        # binary at target/release/open-media
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
open-media init                                  # create ~/.config/open-media/config.toml
open-media config set real_debrid_token=...      # optional, recommended (instant cached playback)
open-media config set tmdb_api_key=...           # optional (Cinemeta already works keyless)
open-media                                # interactive TUI
open-media search "interstellar"                 # list matches
open-media search "frieren" --kind anime
open-media play "interstellar"                   # one-shot: search → best source → play
open-media play "frieren" --season 1 --episode 1
open-media login anilist                         # optional anime progress tracking token
open-media library list                          # local watchlist (add --status watching)
open-media library plan "dune part two"          # save as plan-to-watch
open-media library watching "frieren"            # mark as currently watching
open-media library watched "interstellar"        # mark as completed
open-media config show                           # print config summary (secrets masked)
open-media config path                           # print the config file path
```

`open-media config set` supports the scalar keys most users need:
`tmdb_api_key`, `real_debrid_token`, `anilist_token`, `mal_token`,
`debrid_provider`, `player_command`, `quality`, `nyaa_category`, `theme`,
`show_uncached`, `nyaa_direct`, `cinemeta`, `skip_intro_outro`, `skip_filler`,
`autoplay_next`, `resume`, `discord_presence`, `telemetry`,
`cleanup_after_playback`, `complete_threshold`, `http_port`, and
`player.thumbnail_previews`. List/nested
values such as `torrentio_providers`, `player.args`, `[subtitles]`, and
`[ui.sources]` are still edited directly in `config.toml`.

## Configuration

A single TOML file at `~/.config/open-media/config.toml` (respects
`XDG_CONFIG_HOME`). **Secrets live only here** — never in the binary, the repo, or
the Nix store. `open-media init` creates it.

| Section / key | Default | Purpose |
|---------------|---------|---------|
| `[credentials]` `tmdb_api_key` | — | optional TMDB v3 key (Cinemeta is the keyless default) |
| `[credentials]` `real_debrid_token` | — | instant cached playback (else P2P) |
| `[credentials]` `torbox_token` | — | TorBox API key (used when `debrid_provider = "torbox"`) |
| `[credentials]` `anilist_token` / `mal_token` | — | anime progress tracking |
| `[credentials]` `debrid_provider` | `real-debrid` | active debrid backend: `real-debrid` or `torbox` |
| `[providers]` `cinemeta` / `nyaa_direct` | `true` | keyless movie/series source; direct nyaa.si |
| `[providers]` `quality` | `best` | `best` / `2160p` / `1080p` / `720p` / `480p` |
| `[providers]` `show_uncached` | `false` | include uncached sources (slower to start) |
| `[providers]` `torrentio_providers` | yts,eztv,…,nyaasi | Torrentio trackers, priority order |
| `[player]` `command` / `args` | `mpv` / `["--fullscreen"]` | player + extra args |
| `[player]` `thumbnail_previews` | `false` | seekbar thumbnails for streams; requires user-installed mpv scripts (thumbfast + uosc or a compatible OSC) |
| `[streaming]` `http_port` / `cleanup_after_playback` | `3131` / `true` | local P2P stream server |
| `[behavior]` `skip_intro_outro` / `resume` | `true` | AniSkip OP/ED; resume from last position |
| `[behavior]` `skip_filler` / `complete_threshold` / `discord_presence` | `false` / `0.85` / `false` | binge filler-skip; mark-complete fraction; Discord RPC |
| `[behavior]` `autoplay_next` | `false` | keep playing the next episode after completion |
| `[subtitles]` `enabled` / `languages` | `false` / `["en"]` | optional external subtitle fetch, preferred languages first |
| `[ui]` `theme` / `[ui.sources]` | `auto` | UI theme; persisted Sources filter/sort panel |
| `[telemetry]` `enabled` | `true` | anonymous active-install ping (opt-out; see Telemetry below) |

> Discord rich presence publishes a "Watching …" status when the Discord desktop
> client is running and `discord_presence = true`. It is best-effort: with no
> Discord client running it is a silent no-op and never blocks playback.

## Telemetry

open-media has a single **anonymous** usage ping once per launch so the project
can estimate how many active installs exist. It is **on by default (opt-out)**,
but the shipped binaries (as of `0.6.3`) still point at a placeholder collector endpoint,
so the reporter is currently inert and sends nothing until a real endpoint is
configured in a future release.

When a collector endpoint is configured, this is exactly what is sent — and
nothing else, ever:

```json
{ "v": "<app version>", "os": "<linux|macos|windows>", "arch": "<x86_64|aarch64>", "id": "<random uuid>" }
```

- `id` is a random UUID generated once on this machine (`[telemetry] install_id`
  in your config). It lets us count *unique* installs without any personal data;
  it is not tied to you and reveals nothing about you.
- **Never transmitted:** anything about what you watch — titles, search queries,
  source names, file hashes, watch history — or any API token. That is a hard
  invariant of the telemetry code, not a setting.

It is best-effort and fire-and-forget: it runs detached with a short timeout,
never blocks or breaks playback, and silently does nothing if no collector is
configured or the collector cannot be reached.

**Opt out at any time:**

```sh
open-media config set telemetry=false
```

Download counts are derived separately from GitHub Releases' own statistics — the
app does not phone home to count downloads.

## Project layout

A Cargo workspace of 14 crates, one per concern (full table in
[AGENTS.md](AGENTS.md#module-layout)):

```
crates/
  open-media-core      domain model + ports (traits) + scoring   — no I/O
  open-media-net       shared HTTP client, timeouts, retry helper
  open-media-config    config schema + load/save + secrets policy
  open-media-metadata  TMDB + Cinemeta (keyless) + AniList        (MetadataProvider)
  open-media-sources   Torrentio, nyaa.si                         (SourceProvider)
  open-media-debrid    Real-Debrid (+ future AllDebrid/Torbox)    (DebridProvider)
  open-media-stream    librqbit P2P engine + hybrid resolver      (StreamResolver)
  open-media-player    mpv (IPC) + vlc                            (Player)
  open-media-subs      open-subtitle adapter                       (SubtitleProvider)
  open-media-track     AniList/MAL trackers + AniSkip + Discord   (Tracker/Enricher/…)
  open-media-history   SQLite watch history + resume              (HistoryStore)
  open-media-telemetry anonymous usage ping adapter                (UsageReporter)
  open-media-app       use-cases + Engine (composition)           — depends only on open-media-core
  open-media-cli       the `open-media` binary (composition root + TUI)
```

## Development

```sh
cargo test --workspace      # hermetic suite (unit + e2e vs. in-process mock servers)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Every network adapter has unit tests plus end-to-end integration tests against
in-process mock servers (`wiremock`); a composition-root e2e
(`crates/open-media-cli/tests/pipeline_e2e.rs`) drives search → details → sources →
resolve through the real `Engine`. Live integration tests (gated `#[ignore]` +
env) cover real Real-Debrid, real mpv, and live P2P.

## License

[MIT](LICENSE) © 2026 Grok Insider.

This is a client for services you bring your own account to (TMDB, Real-Debrid,
AniList) and for public indexes. You are responsible for complying with the laws
of your jurisdiction and the terms of those services.
