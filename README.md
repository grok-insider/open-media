# open-media

**Watch movies, series, and anime from your terminal** — instantly via
[Real-Debrid](https://real-debrid.com/) (cached, no seeding, no VPN) or directly
over P2P, streamed into **mpv** or **vlc**. One fast TUI for everything.

> Status: **functional (pre-release).** The full pipeline — discover → source →
> resolve (Real-Debrid or P2P) → play in mpv — is implemented and tested, with a
> working TUI. Remaining: packaging/CI (Phase 10) and polish. See
> [What works today](#what-works-today) and the [plan](docs/PLAN.md).

---

## Why

There are great single-purpose terminal tools — [`ani-cli`] for anime,
[`miru`]/[`toru`] for streaming, [`curd`] for AniList-tracked anime — but each
covers only part of the picture, and the best ideas are scattered across five
codebases in three languages. open-media unifies them into **one** clean,
maintainable Rust application:

- **One pipeline for all content.** Movies and live-action series via TMDB, anime
  via AniList — both resolved through the same source → debrid/P2P → player path.
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
   you  ─────▶│  om (TUI)   │────────────▶│  MetadataProvider     │  TMDB / AniList
              └─────────────┘             └──────────┬───────────┘
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

## What works today

The whole pipeline is **implemented and tested** (Phases 1–9):

- **Metadata**: TMDB (movies/series) + AniList (anime), with IMDB/MAL id bridging.
- **Sources**: Torrentio (all trackers, cache-aware) + direct nyaa.si (RSS).
- **Debrid**: Real-Debrid magnet → instant CDN URL (add→select→unrestrict).
- **P2P**: librqbit engine streaming uncached/no-debrid torrents over a local
  Range-aware HTTP server.
- **Player**: mpv (launch + JSON-IPC: resume seek, auto-skip OP/ED, progress) and
  vlc (launch-only).
- **Session**: SQLite resume, AniSkip/Jikan, AniList/MAL tracking, Discord presence.
- **TUI**: `om` with no args → Search → Results → Episodes → Sources → play.

```console
$ om init                                # create ~/.config/open-media/config.toml
$ om config set real_debrid_token=...    # optional, recommended
$ om config set tmdb_api_key=...         # for movies/series (anime works without)
$ om                                     # interactive TUI
$ om play "interstellar"                 # one-shot: search → best source → mpv
```

Still to come: packaging (Nix flake + Home Manager), CI, and the OAuth-acquisition
flow for tracker tokens. See [docs/PLAN.md](docs/PLAN.md).

### Testing

Every network adapter has unit tests plus **end-to-end integration tests against
in-process mock servers** (`wiremock`); a composition-root e2e
(`crates/om-cli/tests/pipeline_e2e.rs`) drives search → details → sources →
resolve through the real `Engine` (both cached-direct and uncached-via-RD). Live
integration tests (gated `#[ignore]` + env) cover real Real-Debrid, real mpv, and
live P2P; the TUI is verified end-to-end with the `wisp` driver.

```console
$ cargo test --workspace      # 63 hermetic tests (unit + e2e)
$ cargo clippy --workspace --all-targets -- -D warnings
```

## Install (dev)

Requires a recent stable Rust toolchain.

```console
$ git clone https://github.com/grok-insider/open-media
$ cd open-media
$ cargo build --release        # binary at target/release/om
$ ./target/release/om --help
```

A Nix flake (for the NixOS host this was built on) lands alongside the first
playable release.

## Configuration

A single TOML file at `~/.config/open-media/config.toml` (respects
`XDG_CONFIG_HOME`). **Secrets live only here** — never in the binary, the repo,
or the Nix store. `om init` creates it; `om config set key=value` edits it.

| Key | Required | Purpose |
|-----|----------|---------|
| `tmdb_api_key` | ✅ | Movie/series search (TMDB v3 key) |
| `real_debrid_token` | ➖ | Instant cached playback (else P2P) |
| `anilist_token` / `mal_token` | ➖ | Anime progress tracking |
| `player.command` | ➖ | `mpv` (default, IPC features) or `vlc` |
| `providers.quality` | ➖ | `best` / `1080p` / `720p` / … |

## Project layout

A Cargo workspace, one crate per concern (full table in
[AGENTS.md](AGENTS.md#module-layout)):

```
crates/
  om-core      domain model + ports (traits) + scoring   — no I/O
  om-config    config schema + load/save + secrets policy
  om-metadata  TMDB, AniList                              (MetadataProvider)
  om-sources   Torrentio, nyaa.si                         (SourceProvider)
  om-debrid    Real-Debrid (+ future AllDebrid/Torbox)    (DebridProvider)
  om-stream    librqbit P2P engine + hybrid resolver      (StreamResolver)
  om-player    mpv (IPC) + vlc                            (Player)
  om-track     AniList/MAL trackers + AniSkip + Discord   (Tracker/Enricher/…)
  om-history   SQLite watch history + resume              (HistoryStore)
  om-app       use-cases + Engine (composition)           — depends only on om-core
  om-cli       the `om` binary (composition root + TUI)
```

## License

[MIT](LICENSE) © 2026 Grok Insider.

This is a client for services you bring your own account to (TMDB, Real-Debrid,
AniList) and for public indexes. You are responsible for complying with the laws
of your jurisdiction and the terms of those services.
