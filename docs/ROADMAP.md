# Roadmap

High-level version milestones and the feature matrix. The phased engineering plan
lives in `docs/PLAN.md`; deferred follow-ups live in `continue-plan.md` and
nice-to-haves in `future-features.md`.

## Status (2026-06)

The full engineering plan (PLAN Phases 0–10) is **implemented and shipping as
`0.1.0`** — discover → source → resolve → play, the session features, the TUI, and
Nix/CI packaging are all done. Notable post-MVP work already landed: **Cinemeta**
keyless movie/series metadata (no TMDB key needed), episode titles in the player
window/Discord, a focusable **filter/sort side panel** on the Sources screen, real
**season navigation** with per-season episode lists, and anime **season-aware nyaa
matching**. The remaining v0.2 item not yet built is unattended **auto-advance /
binge**; see `continue-plan.md` for the tracked follow-ups.

## Versions

> These are **milestone names**, not the literal crate version. Actual releases are
> cut automatically by [release-plz](https://release-plz.dev) from Conventional
> Commits (see `CONTRIBUTING.md` → Releases): the workspace is single-versioned
> (started at `0.1.0`), each `feat:`/`fix:` bumps it, and merging the release PR
> tags `vX.Y.Z` → GitHub Release (+ prebuilt `open-media`) → cachix.

### v0.1 — "it plays" (MVP)
The vertical slice: discover → source → resolve (Real-Debrid + P2P) → play in mpv.
- TMDB + **Cinemeta** (keyless) + AniList metadata — search works with no API key.
- Torrentio + direct nyaa sources (season-aware for anime).
- Real-Debrid resolve, librqbit P2P fallback.
- mpv/vlc launch; basic `open-media play`.
- Config + secrets; CLI (`init`/`config`/`search`/`play`).
- Corresponds to PLAN Phases 1–5 (**M2**).

### v0.2 — "it remembers and skips"
The session features that make it pleasant.
- mpv IPC: resume from last position, auto-skip OP/ED (AniSkip), progress polling.
- SQLite history + "continue watching".
- AniList/MAL progress sync (composite); Discord presence.
- Full playback orchestration + auto-advance / skip-filler binge.
- PLAN Phases 6–8 (**M3**).

### v0.3 — "it's a joy in the terminal"
- ratatui TUI: search → results → seasons → episodes → sources → playing, with a
  focusable filter/sort side panel on Sources (persisted to `[ui.sources]`).
- Still to come: themes, init wizard, masked secrets, poster thumbnails via
  terminal image protocols (kitty/sixel) where available.
- PLAN Phase 9 (**M4**).

### v0.4 — "it ships"
- Nix flake + Home Manager module + cachix (matches sibling repos).
- CI (fmt/clippy/test) + automated releases: release-plz opens a version-bump PR on
  each `feat:`/`fix:`; merging it tags `vX.Y.Z`, publishes a GitHub Release with a
  prebuilt `open-media` binary, and pushes `open-media-X.Y.Z` to cachix.
- PLAN Phase 10 (**M5**).

### v1.0 — "stable & broad"
- A second debrid backend (Torbox or AllDebrid) proving the abstraction.
- Subtitles (OpenSubtitles) + language-aware track selection.
- Robustness: retries/backoff everywhere, graceful source failover, good error UX.
- Documented, tested, MSRV-pinned.

## Feature matrix (target = v1.0)

| Area | Feature | Target |
|------|---------|--------|
| Discovery | TMDB movies/series (with key) | v0.1 |
| Discovery | Cinemeta movies/series (keyless default) | v0.1 |
| Discovery | AniList anime (+MAL bridge) | v0.1 |
| Sources | Torrentio (all trackers incl. nyaa) | v0.1 |
| Sources | Direct nyaa.si (RSS) | v0.1 |
| Sources | Jackett/Prowlarr indexers | future |
| Debrid | Real-Debrid | v0.1 |
| Debrid | Torbox / AllDebrid / Premiumize | v1.0 / future |
| Streaming | librqbit P2P + Range server | v0.1 |
| Player | mpv (launch) | v0.1 |
| Player | mpv IPC (resume/skip/track) | v0.2 |
| Player | vlc | v0.1 |
| Anime | AniSkip OP/ED auto-skip | v0.2 |
| Anime | Jikan filler/recap skip | v0.2 |
| Tracking | AniList + MAL (dual) | v0.2 |
| Tracking | Trakt (movies/series) | future |
| Presence | Discord RPC | v0.2 |
| History | SQLite resume + recents | v0.2 |
| UI | clap CLI | v0.1 |
| UI | ratatui TUI + themes | v0.3 |
| UI | poster thumbnails | v0.3 |
| Subtitles | OpenSubtitles + track selection | v1.0 |
| Packaging | Nix flake + HM module | v0.4 |
| Packaging | Automated releases (release-plz → GitHub Releases + cachix) | v0.4 |
| Watch-together | syncplay | future |
| Library mode | Zurg/Jellyfin-style RD library | future |

## Non-goals (for now)
- A media *server* (Jellyfin/Plex replacement). open-media is a player/client. A
  library mode is a "future" item, not a core direction.
- DRM/official-streaming integrations.
- A GUI. The terminal is the product.
