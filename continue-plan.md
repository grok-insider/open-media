# continue-plan.md

Outstanding engineering follow-ups, captured so we can pick them up later. These
are concrete next steps (with file references) from the 2026-06 audit and the
recent metadata/TUI/source work. The broader, aspirational backlog lives in
`future-features.md`; this file is the actionable "still to do" list.

Status legend: **P1** correctness bug still present · **P2** incomplete feature ·
**P3** smaller correctness nit / polish.

---

## Already shipped (context — do NOT redo)

Cinemeta keyless metadata + IMDB dedup; episode titles in mpv/Discord
(`om-core::title`); RD multi-file/season-pack fix; `HybridResolver` debrid→P2P
fallback; Torrentio anime no-op without IMDB; `complete_threshold` wired;
debrid/Torrentio token-gating consistency (`Config::has_real_debrid`); TUI season
navigation + real episode lists; Sources filter/sort side panel (persisted to
`[ui.sources]`); Cinemeta episode ordering; **anime season matching Phase 1**
(`om-sources/src/season.rs` — explicit markers, multi-season ranges, roman
numerals, bare-ordinal shorthand).

---

## P1 — correctness bugs still present

### 1. Anime absolute episode numbering (season fix, Phase 2)
- **Where:** `crates/om-sources/src/season.rs`, `nyaa.rs`; `crates/om-metadata/src/anilist.rs`; `crates/om-core/{ports.rs,stream.rs}`; `crates/om-app/src/lib.rs`.
- **Problem:** some groups number a sequel continuously (S2E01 released as `… - 21`
  when S1 had 20 eps). The season classifier keys off title markers, so an
  absolute-numbered S2 release with no marker is treated as season 1 and won't
  match an S2 pick (and the `… 01` query never fetches `… 21` anyway).
- **Plan:**
  1. AniList: add `relations { edges { relationType node { id format episodes } } }`
     to the detail query; compute an episode **offset** = Σ episodes of prior
     `TV` `PREQUEL`s (recurse with a small hop cap; verify shape vs live API).
  2. `om-core`: add a defaulted `MetadataProvider::episode_offset(ids) -> Option<u32>`
     (default `Ok(None)`; only AniList implements). Add `absolute_episode: Option<u32>`
     to `SourceQuery`.
  3. `om-app::find_sources`: for anime with an episode, set
     `absolute_episode = offset + episode` from the first provider returning `Some`.
  4. `nyaa`: extend the parser to also extract episode coverage (single / `01~20`
     range / batch); accept a no-marker candidate whose episode == the absolute
     number as the requested season; issue a **second RSS fetch** with the
     absolute number and merge/dedup by infohash.
- **Tests:** AniList relations→offset (fixture); nyaa e2e where S2E01 appears only
  as `- 21` and is matched; an S1 search must NOT pick up `- 21`.
- Also note (lower priority): singular `Season 1-5` batches are read as season 1
  only (plural `Seasons 1-5` and `S01-S05` work); cross-arc/OVA chains unmodeled.

### 2. AniList mis-models anime movies & multi-cour seasons
- **Where:** `crates/om-metadata/src/anilist.rs` (`AniListMedia` ~ln 206-225, `into_media` ~244, `seasons`/`episodes` 148-172).
- **Problem:** the GraphQL query fetches `format` (TV/MOVIE/OVA/ONA/SPECIAL) but the
  struct has **no `format` field**, so it's discarded — every entry maps to
  `MediaKind::Anime` with `season_count: Some(1)`. Anime *movies* are mis-modeled
  as episodic.
- **Plan:** deserialize `format`; map MOVIE→`Movie` (or a movie flag) so the play
  path skips season/episode coordinates for anime films.

### 3. AniList airing shows yield zero episodes
- **Where:** `anilist.rs::episodes` (160-171) — fabricates `1..=episode_count`.
- **Problem:** currently-airing anime report `episodes: null` → `episode_count`
  `None` → **empty** episode list. The TUI then falls back to 1 fabricated episode.
- **Plan:** fall back to `nextAiringEpisode.episode - 1` (add to query) or a sane
  minimum; consider a Jikan/AniList per-episode title fetch (also fills the
  "anime episodes have no titles → `Series - S01E01`" gap).

### 4. `Engine::details` drops cross-provider ids (no `IdSet::merge`)
- **Where:** `crates/om-app/src/lib.rs::details` (~91-101).
- **Problem:** `details` returns the answering provider's `Media` wholesale; ids
  discovered during search (e.g. AniList's `mal`) are lost if a different provider
  resolves details. `IdSet::merge` exists and is used in search dedup (ln 417) but
  not here.
- **Plan:** merge the input `ids` into the returned `media.ids` before returning.

### 5. Dead/misleading config keys
- **`behavior.resume`** (`compose.rs` never passes it; `om-app/lib.rs:210` always
  resumes when history has a position). Wire it (skip resume when false) or remove.
- **`ui.theme`** loaded but unused (TUI hardcodes colors). Wire or remove.
- **`OPEN_MEDIA_*` env overrides** — `om-config` doc claims they exist; `load()`
  reads only the file. Implement env overrides or drop the doc line.

---

## P2 — incomplete features

### 6. Binge / auto-advance to next episode
- **Where:** `crates/om-app/src/lib.rs::play` (doc mentions it ~ln 185; not done).
- **Plan:** after playback ends and progress ≥ threshold, advance to the next
  episode in the season and recurse; call `Enricher::filler_episodes` to skip
  filler (the enricher is wired when `skip_filler`, but never consulted).

### 7. Pagination / "load more"
- TMDB `search` fetches page 1 only (`tmdb.rs` — `total_pages` not read); AniList
  `Page(perPage: 15)`; nyaa single RSS page. Add paging or a "load more" affordance.

### 8. `om config set` covers only 6 keys
- **Where:** `crates/om-cli/src/main.rs::cmd_config` (~168-176).
- Add setters for `quality`, `show_uncached`, `nyaa_direct`, `cinemeta`,
  `skip_intro_outro`, `http_port`, `complete_threshold`, … (currently file-edit
  only). Also `config show` omits many loaded keys — make it complete.

### 9. Tracker OAuth acquisition + MAL refresh
- AniList/MAL consume a pre-obtained token; no loopback OAuth flow, and MAL tokens
  (~1h) have no refresh. Add an `om login {anilist,mal}` flow.

### 10. Discord application id
- `crates/om-cli/src/compose.rs:106` `DISCORD_CLIENT_ID = "0000…"` placeholder →
  presence can't work. Register a real app id and ship it.

---

## P3 — smaller correctness nits & polish

- **Scoring:** cached results get **no bonus** when `prefer_cached == false`
  (`om-core/src/scoring.rs` ~51), so a slow uncached source can outrank an instant
  cached one of equal quality. Give cached a small unconditional tiebreak.
- **MAL `Repeating` → `"watching"`** collapse (`om-track/src/mal.rs` ~91); use
  `is_rewatching`.
- **AniSkip `episodeLength=0`** hardcoded (`om-track/src/aniskip.rs` ~59) disables
  interval validation; pass the real runtime when known.
- **vlc ignores resume** (`om-player/src/vlc.rs` ~48) — pass a `--start=` equivalent.
- **P2P holds the state mutex across the ~90s metadata wait**
  (`om-stream/src/p2p.rs`) — serializes concurrent calls/cleanup; don't hold the
  lock while sleeping.
- **RD `check_cached` is a no-op** (`om-debrid/src/realdebrid.rs` ~150) — cache
  state for non-Torrentio-flagged candidates is always `Unknown`. Revisit when RD
  exposes a working bulk endpoint.
- **nyaa category hardcoded** `c=1_2` (English-translated) (`nyaa.rs:92`) — no
  raw/all-anime option; make configurable.
- **Release-tag parser nits** (`om-sources/src/tags.rs`): bit-depth (`10bit`) folded
  into the video codec field; only the first audio codec captured; provider
  extraction can yield garbage when `name` has no `[...]`; `[AD+]`/`[PM+]`/`[TB+]`
  (AllDebrid/Premiumize/Torbox) cache flags unrecognized; `GiB` not matched in
  Torrentio's emoji size regex.

---

## UX / smaller observations (from wisp drives)

- **Provider column shows "Torrentio" for all keyless results** — the sub-tracker
  (RARBG/1337x/…) is only split out when a debrid tag like `[RD+] 1337x` is present.
  With a Real-Debrid token the Provider filter is far more useful; consider parsing
  the tracker from the title as a fallback.
- **TUI `initial_query` is unused** — `om "<query>"` doesn't pre-fill the TUI search
  (`tui::run` takes `initial_query` but `main.rs` always passes `None`, and there's
  no positional arg for the no-subcommand path). Wire a positional query.

---

## Note: release workflow (in place)

Releases are automated with **release-plz** (`release-plz.toml` +
`.github/workflows/release.yml`); `v0.1.0` is tagged. **`CHANGELOG.md` is generated
from Conventional Commits — do not hand-edit it.** Land each item above with a
`fix:`/`feat:` (or `docs:`/`refactor:`/etc.) commit so the version bump and
changelog entry are produced automatically. See `CONTRIBUTING.md` → Releases and
`AGENTS.md` → Releasing & versioning.
