# open-media

**Watch movies, series, and anime from your terminal** — instantly via
[Real-Debrid](https://real-debrid.com/) (cached, no seeding, no VPN) or directly
over P2P, streamed into **mpv** or **vlc**. One fast TUI for everything.

> Status: **early scaffold.** The architecture, ports, and docs are in place and
> the workspace builds; the network adapters are being filled in per the
> [roadmap](docs/ROADMAP.md). See [What works today](#what-works-today).

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

The discovery → resolve pipeline is **implemented and tested** (Phases 1–3):

- **Metadata**: TMDB (movies/series) + AniList (anime) search/details, with IMDB
  and MAL id bridging.
- **Sources**: Torrentio (all trackers, cache-aware) + direct nyaa.si (RSS).
- **Debrid**: Real-Debrid magnet → instant CDN URL (full
  add→select→unrestrict flow), with a P2P fallback path (Phase 4) stubbed.
- **Engine**: parallel `search` / `find_sources` + ranking, `details`, `resolve`.

```console
$ om --help
$ om init                                # create ~/.config/open-media/config.toml
$ om config set tmdb_api_key=...         # required
$ om config set real_debrid_token=...    # optional, recommended
$ om search "frieren" --kind anime       # real TMDB + AniList results
```

Still to come: the local P2P engine (Phase 4), player launch + mpv IPC (Phase 5),
resume/skip/tracking (Phases 6–8), and the TUI (Phase 9). See
[docs/PLAN.md](docs/PLAN.md).

### Testing

Every network adapter has unit tests plus **end-to-end integration tests against
in-process mock servers** (`wiremock`), and a composition-root e2e
(`crates/om-cli/tests/pipeline_e2e.rs`) drives the entire
search → details → sources → resolve flow through the real `Engine` — including
both the cached-direct and uncached-via-Real-Debrid branches.

```console
$ cargo test --workspace      # 41 tests (16 unit + 25 integration/e2e)
$ cargo clippy --workspace --all-targets -- -D warnings
```

## Install (dev)

Requires a recent stable Rust toolchain.

```console
$ git clone https://github.com/0xfell/open-media
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

[MIT](LICENSE) © 2026 0xfell.

This is a client for services you bring your own account to (TMDB, Real-Debrid,
AniList) and for public indexes. You are responsible for complying with the laws
of your jurisdiction and the terms of those services.
