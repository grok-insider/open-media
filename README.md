# open-media

[![CI](https://github.com/grok-insider/open-media/actions/workflows/ci.yml/badge.svg)](https://github.com/grok-insider/open-media/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/grok-insider/open-media?sort=semver)](https://github.com/grok-insider/open-media/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Terminal media client** — search metadata (movies, series, anime), manage a
local library, drive **mpv**/**vlc** with resume and tracking. Optional adapters
can resolve playable streams via your own debrid account or local P2P.

> **Status: released — v0.6.3.** Discover → optional sources → resolve → play is
> implemented, tested, packaged (Nix + prebuilt binaries), and runs on
> **Linux, macOS, and Windows**. See [CHANGELOG.md](CHANGELOG.md).
>
> **Legal:** open-media is dual-use client software. It does not host media or
> grant rights to works. You are responsible for lawful use in your jurisdiction.
> See **[docs/LEGAL.md](docs/LEGAL.md)**.
> Optional torrent indexes and local P2P are **off by default**.

---

## Why

Terminal media tools often split discovery, playback control, tracking, and
architecture across separate projects. open-media brings them into **one** clean,
maintainable Rust application (engineering prior art is documented in
[docs/RESEARCH.md](docs/RESEARCH.md) — not a product promise of any particular
use):

- **One pipeline for metadata + playback control.** Movies and live-action series
  via TMDB or keyless Cinemeta, anime via AniList — same app for search, library,
  resume, and tracking.
- **Optional stream resolution.** With adapters enabled, you can use a debrid
  service you authenticate to (e.g. Real-Debrid/TorBox) or local P2P. Debrid
  resolution uses HTTPS from the provider's CDN (no local swarm when it
  succeeds; that is **not** a copyright license). Local P2P may download **and
  upload** pieces — enable only if you accept that (see
  [docs/LEGAL.md](docs/LEGAL.md)).
- **mpv done right.** Launches your existing mpv (and config) and drives it over
  IPC for resume, intro/outro auto-skip (AniSkip), progress tracking, and
  Discord presence.
- **Clean architecture.** Ports-and-adapters + SOLID, one crate per concern, so
  adding a debrid service, indexer, tracker, or player is a small isolated change.

## The pipeline

Metadata, library, and player control work out of the box. **Source providers and
local P2P are opt-in** (off by default).

```
              ┌─────────────┐   search    ┌──────────────────────┐
   you  ─────▶│ open-media  │────────────▶│  MetadataProvider     │  TMDB / Cinemeta
              └─────────────┘             └──────────┬───────────┘  / AniList
                     │  pick title                   │ Media + ids (imdb/mal/…)
                     ▼                                ▼
              ┌─────────────┐   find      ┌──────────────────────┐
              │   Engine    │────────────▶│  SourceProvider(s)    │  Torrentio + nyaa
              │  (open-media-app)   │             └──────────┬───────────┘  (opt-in)
              └─────────────┘   rank ◀───────────────┘ candidates (+cache flags)
                     │  best candidate (when sources enabled)
                     ▼
              ┌─────────────┐  resolve    ┌──────────────────────┐
              │StreamResolver│───────────▶│ DebridProvider (opt.)  │ ─▶ HTTPS URL
              │  (open-media-stream)│             │   or P2pEngine (opt-in)  │ ─▶ http://127.0.0.1
              └──────┬──────┘             └──────────────────────┘
                     │  Playback url
                     ▼
              ┌─────────────┐  IPC: seek / time-pos / chapters
              │   Player    │◀───────────  resume · auto-skip OP/ED · progress
              │  (mpv/vlc)  │───────────▶  Tracker (AniList/MAL) · Discord presence
              └─────────────┘              · History (resume next time)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design and
[docs/RESEARCH.md](docs/RESEARCH.md) for engineering prior art.

## Features

- **Metadata**: TMDB (richer, optional key) + **Cinemeta** (keyless default) for
  movies/series, AniList for anime — with IMDB/MAL id bridging and de-dup.
- **Sources (opt-in)**: Torrentio (all trackers, cache-aware) + direct nyaa.si
  (RSS). Off by default — see [Optional source adapters](#optional-source-adapters).
- **Debrid (optional)**: Real-Debrid or TorBox with **your** token — magnet → CDN
  URL when configured.
- **P2P (opt-in)**: librqbit local engine over a localhost Range-aware HTTP server.
  May upload to peers; off unless `streaming.allow_p2p = true`.
- **Player**: mpv (launch + JSON-IPC: resume seek, auto-skip OP/ED, progress) and
  vlc (launch-only).
- **Session**: SQLite resume, AniSkip/Jikan OP/ED + filler skip (with a
  fallback that reads the release's own chapter names when AniSkip has no
  data), AniList/MAL tracking, Discord presence. Episodic playback keeps a
  single mpv alive with the next episode pre-appended, so mpv's own Next
  button always works; `autoplay_next` additionally advances by itself.
- **Library**: a local watchlist (watching / completed / planning / …) that
  updates itself as you watch, browsable in the TUI and via
  `open-media library`, with best-effort AniList/MAL status dual-write.
- **Subtitles**: optional OpenSubtitles/SubDL/Jimaku fetch through the
  `open-subtitle` engine, passed to players as temp `--sub-file` tracks.
- **TUI**: `open-media` with no args → Home (Continue Watching + Library) →
  Search → Results → (optional) Seasons / Episodes / Sources → play when sources
  are enabled. Results stream in incrementally as providers respond; mouse
  (click/wheel) is supported; the Sources filter/sort panel persists to config.

## Install

**Prebuilt binaries (no compiling)** — each [GitHub Release](https://github.com/grok-insider/open-media/releases)
attaches `open-media-<version>-<target>.tar.gz` (+ `.sha256`) for static Linux
(x86_64/aarch64 musl), native macOS (x86_64/arm64), and native Windows (x86_64).
Download, verify, extract, and put `open-media` on your `PATH`.

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
- **All API tokens are optional.** Search works keyless via Cinemeta + AniList.
  Playback of remote releases needs **opt-in** source adapters and either a
  debrid token or `allow_p2p` (see below and [docs/LEGAL.md](docs/LEGAL.md)).

## Usage

```sh
open-media init                                  # create ~/.config/open-media/config.toml
open-media config set tmdb_api_key=...           # optional (Cinemeta already works keyless)
open-media                                # interactive TUI (Home, library, search)
open-media search "interstellar"                 # metadata search
open-media search "frieren" --kind anime
open-media library list                          # local watchlist (add --status watching)
open-media library plan "dune part two"          # save as plan-to-watch
open-media library watching "frieren"            # mark as currently watching
open-media library watched "interstellar"        # mark as completed
open-media login anilist                         # optional anime progress tracking token
open-media login mal                             # MyAnimeList OAuth (needs mal_client_id, see below)
open-media config show                           # print config summary (secrets masked)
open-media config path                           # print the config file path
```

`open-media play "…"` searches and plays via source adapters. **Those adapters
are off by default** — enable them first (see [Optional source adapters](#optional-source-adapters)
and [docs/LEGAL.md](docs/LEGAL.md)). Without them, play reports that no source
providers are enabled.

`open-media config set` supports the scalar keys most users need:
`tmdb_api_key`, `real_debrid_token`, `torbox_token`, `anilist_token`,
`mal_token`, `mal_client_id`, `mal_client_secret`,
`debrid_provider`, `player_command`, `quality`, `nyaa_category`, `theme`,
`show_uncached`, `torrentio`, `nyaa_direct`, `cinemeta`, `sources_acknowledged`,
`allow_p2p`, `skip_intro_outro`, `skip_filler`,
`autoplay_next`, `playlist_next`, `resume`, `discord_presence`, `telemetry`,
`cleanup_after_playback`, `complete_threshold`, `http_port`, and
`player.thumbnail_previews`. List/nested
values such as `torrentio_providers`, `player.args`, `[subtitles]`, and
`[ui.sources]` are still edited directly in `config.toml`.

## Optional source adapters

Torrent index adapters and local P2P are **disabled by default** so a fresh
install is metadata + library + player oriented. To enable the advanced path
(after reading [docs/LEGAL.md](docs/LEGAL.md)):

```sh
open-media config set torrentio=true
open-media config set nyaa_direct=true          # anime RSS
open-media config set sources_acknowledged=true
open-media config set real_debrid_token=...     # preferred over local P2P when available
# or, for local BitTorrent (may upload pieces to peers):
open-media config set allow_p2p=true
```

Debrid does **not** grant copyright rights to media. Local P2P may **upload**.
You remain responsible for lawful use under the laws of your jurisdiction.

## Configuration

A single TOML file at `~/.config/open-media/config.toml` (respects
`XDG_CONFIG_HOME`). **Secrets live only here** — never in the binary, the repo, or
the Nix store. `open-media init` creates it.

| Section / key | Default | Purpose |
|---------------|---------|---------|
| `[credentials]` `tmdb_api_key` | — | optional TMDB v3 key (Cinemeta is the keyless default) |
| `[credentials]` `real_debrid_token` | — | optional debrid CDN playback (your account) |
| `[credentials]` `torbox_token` | — | TorBox API key (used when `debrid_provider = "torbox"`) |
| `[credentials]` `anilist_token` / `mal_token` | — | anime progress tracking (`open-media login anilist` / `login mal`) |
| `[credentials]` `mal_client_id` | — | your MAL API client id (register at [myanimelist.net/apiconfig](https://myanimelist.net/apiconfig), App Type `other`, redirect URL `http://localhost:42069/callback`); MAL tokens then auto-refresh |
| `[credentials]` `debrid_provider` | `real-debrid` | active debrid backend: `real-debrid` or `torbox` |
| `[providers]` `cinemeta` | `true` | keyless movie/series metadata |
| `[providers]` `torrentio` / `nyaa_direct` | `false` / `false` | opt-in source adapters (see LEGAL.md) |
| `[providers]` `sources_acknowledged` | `false` | set when enabling sources after reading LEGAL |
| `[providers]` `quality` | `best` | `best` / `2160p` / `1080p` / `720p` / `480p` |
| `[providers]` `show_uncached` | `false` | include uncached sources (slower to start) |
| `[providers]` `torrentio_providers` | yts,eztv,… | Tracker list used **only when** `torrentio=true` |
| `[player]` `command` / `args` | `mpv` / `["--fullscreen"]` | player + extra args |
| `[player]` `thumbnail_previews` | `false` | seekbar thumbnails for streams; requires user-installed mpv scripts (thumbfast + uosc or a compatible OSC) |
| `[streaming]` `allow_p2p` | `false` | opt-in local BitTorrent streaming (may upload) |
| `[streaming]` `http_port` / `cleanup_after_playback` | `3131` / `true` | local P2P stream server |
| `[behavior]` `skip_intro_outro` / `resume` | `true` | auto-skip OP/ED (AniSkip, falling back to the file's own chapter names); resume from last position |
| `[behavior]` `playlist_next` | `true` | keep one mpv per episodic session with the next episode pre-appended, so mpv's own **Next** button works even without autoplay |
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

open-media is dual-use client software for services you authenticate to yourself
and optional public indexes. It does not host media or license copyrighted works.
You are responsible for complying with the laws of your jurisdiction and
third-party terms. See [docs/LEGAL.md](docs/LEGAL.md).
