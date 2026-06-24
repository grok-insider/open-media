# Research & prior art

open-media is a from-scratch synthesis, but it stands on five existing projects.
This document records what each does, what we take, what we deliberately avoid,
and the concrete protocol details we reverse-engineered — so the knowledge lives
in the repo instead of in scattered codebases.

Analyzed: **miru** (Rust), **toru** (Go), **littlejohn** (Rust), **rdbatch**
(Go), **curd** (Go). Honorable mention: **ani-cli** (shell) for UX conventions.

---

## 1. What each project contributes

### miru — the architectural base (Rust)
TMDB → Torrentio → Real-Debrid → mpv, with a librqbit P2P fallback and a ratatui
TUI. Clean module split (`api`/`config`/`streaming`/`player`/`ui`/`history`).
- **Take:** the metadata-driven pipeline; Torrentio config-string trick incl.
  `[RD+]`/`⚡` cache detection; rich release-tag parsing (quality/HDR/codec/audio/
  language); librqbit streaming via its built-in `/torrents/{id}/stream/{idx}`
  endpoint; SQLite history; config ergonomics (`#[serde(default)]`).
- **Improve:** miru hardcodes Real-Debrid *inside* the Torrentio client and uses
  concrete types throughout (no traits). We invert this into ports so debrid,
  sources, and metadata are independently swappable, and we add mpv-IPC control
  (miru has none → no resume, no auto-skip).

### toru — the torrent streaming engine (Go)
Direct nyaa.si search + real-time torrent streaming over a localhost HTTP server
(anacrolix/torrent + `http.ServeContent` Range handling).
- **Take:** the direct-nyaa source (independent of Torrentio) and the streaming
  server contract (Range-aware HTTP serving a torrent file, pieces prioritized at
  the read head).
- **Improve:** prefer nyaa's **RSS feed** (`?page=rss`, carries `<nyaa:infoHash>`,
  seeders, magnet) over toru's brittle "exactly 4 links / 8 tds" HTML scrape; set
  a real User-Agent + timeout; in Rust, librqbit's `FileStream` + its shipped
  axum stream endpoint replace the hand-rolled `ServeContent`. Avoid toru's dead
  code and bugs (CWD `.torrent` write, hardcoded `video/mp4`, unencoded
  `filepath`, global ServeMux, `:port` binding all interfaces).

### littlejohn — the Real-Debrid flow + TUI loop (Rust)
ratatui torrent-search client with a clean RD client.
- **Take:** the canonical RD lifecycle (below); the `AppMode`-enum +
  `mpsc::unbounded` + `try_recv`-drain render loop (our TUI scaffold); the
  concurrent multi-scraper fan-out pattern.
- **Improve:** littlejohn uses free functions, not a trait; we lift sources behind
  `SourceProvider`. Add cache-awareness (it always does the slow add→download).

### rdbatch — the debrid provider abstraction (Go)
The artifact we most directly generalize: one `Provider` interface over
Real-Debrid **and** Torbox, with provider-agnostic `Torrent`/`File` structs, plus
cache-aware Comet/Cinemeta search.
- **Take:** the `DebridProvider` trait shape; the `⚡`-emoji cache classification;
  the `imdbID:season:episode` id convention; the rule **cached → stream the
  proxy/unrestricted URL straight to mpv; uncached → add-magnet to warm cache**.

### curd — the anime power-features (Go)
AniList/MAL tracking, Discord RPC, AniSkip intro/outro, Jikan filler, mpv-IPC
control, resume.
- **Take:** the **mpv JSON-IPC control plane** (the universal seek/position/pause/
  chapters channel); the AniSkip auto-skip rule; the `Tracker` composite/dual-write
  pattern; resume + 85%-complete threshold.
- **Improve:** SQLite history instead of full-file-rewrite CSV.

## 2. What open-media takes — matrix

| Capability | Primary source | Port |
|------------|----------------|------|
| Metadata-driven discovery (movies/series) | miru | `MetadataProvider` (TMDB) |
| Anime discovery + MAL bridge | curd/ani-cli | `MetadataProvider` (AniList) |
| Torrentio sourcing + cache flags | miru/rdbatch | `SourceProvider` (Torrentio) |
| Direct nyaa.si sourcing | toru | `SourceProvider` (nyaa) |
| Debrid abstraction (RD/Torbox/…) | rdbatch | `DebridProvider` |
| RD add→select→unrestrict flow | littlejohn | `DebridProvider` (RealDebrid) |
| P2P streaming server | toru | `StreamResolver`/`P2pEngine` |
| Release-tag parsing + scoring | miru | `om-core::scoring` + adapter parsers |
| mpv IPC: resume/skip/track | curd | `PlaybackControl` |
| AniSkip OP/ED, Jikan filler | curd | `Enricher` |
| AniList/MAL tracking (dual) | curd | `Tracker` + `CompositeTracker` |
| Discord presence | curd | `PresenceReporter` |
| Watch history / resume | miru/curd | `HistoryStore` |
| ratatui state-machine TUI | littlejohn/miru | `om-cli` (Phase 9) |

## 3. Concrete protocol notes (the reusable specifics)

### Torrentio (Stremio addon)
- Base: `https://torrentio.strem.fun`.
- Config segment example (with RD):
  `providers=yts,…,nyaasi|sort=qualitysize|qualityfilter=scr,cam|debridoptions=nodownloadlinks|realdebrid=KEY`
  Omit `debridoptions=nodownloadlinks` to also see uncached results.
- Endpoints: `/{config}/stream/movie/{imdb}.json`,
  `/{config}/stream/series/{imdb}:{season}:{episode}.json`.
- Per stream: `name` (carries `[RD+]`/`⚡` cache flag + provider), `title`
  (release name + `👤 seeders` `💾 size`), `url` (direct, when cached via debrid)
  or `infoHash` + `fileIdx` (for P2P). `nyaasi` is a provider → anime via nyaa.

### nyaa.si (direct)
- HTML: `https://nyaa.si/?f={filter}&c={cat}&q={query}&s={sort}&o={order}&p={page}`
  where `c` is a code like `1_2` (English-translated anime), `s=id` means date,
  `f` is `0|1|2` (none/no-remakes/trusted).
- **Prefer RSS** (`&page=rss`): items carry `<nyaa:infoHash>`, `<nyaa:seeders>`,
  `<nyaa:size>`, magnet — far less brittle than scraping the table.
- Infohash comes from the magnet (`xt=urn:btih:…`); search HTML omits it.

### Real-Debrid REST (`https://api.real-debrid.com/rest/1.0`)
Auth: `Authorization: Bearer <token>`. POST bodies are **form-encoded**.
Canonical flow:
1. `POST /torrents/addMagnet` (`magnet`) → `{id, uri}`.
2. poll `GET /torrents/info/{id}` until `status == "waiting_files_selection"`.
3. `POST /torrents/selectFiles/{id}` (`files=1,2,3` or `all`).
4. poll `GET /torrents/info/{id}` until `status == "downloaded"`.
5. `POST /unrestrict/link` (`link`) for each entry in `info.links` → direct URL.
Notes: `/torrents/instantAvailability` was deprecated in 2024 — cache state comes
from the addon (Torrentio/Comet flags), not RD. Respect ~250 req/min; add backoff.

### AniSkip (intro/outro)
- `GET https://api.aniskip.com/v1/skip-times/{malId}/{episode}?types=op&types=ed`
  → `{found, results:[{interval:{start_time,end_time}, skip_type}]}`. Keyed by
  **MAL id** (bridge AniList→MAL via the media's `idMal`).
- Auto-skip rule (curd): poll `time-pos`; when `op.start < t < op.start+2` and
  `op.start != op.end`, `seek` to `op.end`. Same for ED. Also expose as mpv
  chapters for manual nav.

### Jikan (filler/recap)
- `GET https://api.jikan.moe/v4/anime/{malId}/episodes` (paginated; self-limit
  ~3 req/s, back off on 429). Episodes with `filler==true` / `recap==true`.

### mpv JSON IPC
- Launch: `mpv --no-terminal --really-quiet --idle=yes --force-window=yes
  --input-ipc-server=<socket> [--force-media-title=<title>] <url>`.
  Socket: `/tmp/om-mpv-<rand>` (unix) / `\\.\pipe\om-mpv-<rand>` (windows).
- Codec: connect the socket; write `{"command":[...]}\n` (line-delimited JSON);
  read a line; parse `{error, data}` (`error == "success"` is OK). 1s read
  deadline, ~3× retry on transient errors.
- Helpers: `["seek", t, "absolute"]`, `get_property time-pos|duration|pause`,
  `["set_property","chapter-list",[…]]`, `["quit"]`. `observe_property` gives an
  event stream for `time-pos`/`pause`.

### AniList / MAL OAuth
- AniList: GraphQL `POST https://graphql.anilist.co`, `Authorization: Bearer`.
  Auth = code-grant via a loopback `http://localhost:PORT/oauth/callback` server,
  exchange at `https://anilist.co/api/v2/oauth/token`. `SaveMediaListEntry`
  mutation updates progress/status/score.
- MAL: OAuth2 + PKCE, API base `https://api.myanimelist.net/v2`.

### librqbit mapping (the Rust port of toru's engine)
`anacrolix/torrent` → `librqbit`. `add_torrent(AddTorrent::from_url(magnet))` →
`wait_until_initialized()` (≈ `<-GotInfo()`). Largest video file by extension.
Serve via librqbit's built-in `/torrents/{id}/stream/{file_idx}` (Range-aware,
piece-prioritized) — bind `127.0.0.1`. `session.delete(id, delete_files)` on
cleanup. Vendored OpenSSL feature avoids a system `libssl` dependency.

## 4. Credits

Thanks to the authors of
[miru](https://github.com/YannickHerrero/miru),
[toru](https://github.com/sweetbbak/toru),
[littlejohn](https://github.com/mat-lo/littlejohn),
[rdbatch](https://github.com/Snazzyham/rdbatch),
[curd](https://github.com/Wraient/curd), and
[ani-cli](https://github.com/pystardust/ani-cli). open-media reimplements ideas
from these projects in a unified architecture; it does not copy their code.
