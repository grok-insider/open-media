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

**2026-06 roadmap batch (landed via `dev` → `master`):** AniList `MOVIE` format →
`Movie` kind (#6); AniList airing-anime episode count from `nextAiringEpisode`
(#18, re-land of #16); `Engine::details` merges cross-provider ids (#7);
`OPEN_MEDIA_*` env overrides applied on load (#8); cached-source unconditional
score tiebreak (#9); MAL `is_rewatching` for repeating status (#10); vlc resume
via `--start-time` (#11); P2P state lock not held across the metadata wait (#12);
release-tag parser nits — bit-depth/multi-audio/provider-guard/AD+PM+TB+/GiB
(#13); configurable nyaa category (#14); real episode runtime → AniSkip (#15);
plus a `dev`-only-into-`master` CI guard (#19).

---

## P1 — correctness bugs still present

> **#1 Anime absolute episode numbering (season fix, Phase 2) — done.** AniList
> `DETAIL_QUERY` now fetches `relations { edges { relationType node { id format
> episodes } } }`; `AniListProvider::episode_offset` sums prior `TV` `PREQUEL`
> episode counts, walking the chain (hop cap 5). `MetadataProvider::episode_offset`
> is a defaulted port method (`Ok(None)`; only AniList overrides). `SourceQuery`
> carries `absolute_episode`; `om-app::find_sources` sets it to `offset + episode`
> from the first provider returning `Some`. `om-sources::season::release_episode`
> parses the episode coordinate, and `nyaa` issues a second RSS fetch on the
> absolute number, accepts a marker-less release whose episode == the absolute
> number as the requested season, and dedups by infohash. Tested: AniList
> relations→offset (fixture), nyaa S2E01-as-`- 21` matched, S1 does not pick up
> `- 21`.
- Still open (lower priority): singular `Season 1-5` batches are read as season 1
  only (plural `Seasons 1-5` and `S01-S05` work); cross-arc/OVA chains unmodeled.

> **#2 AniList anime-movie modeling — done** (PR #6): `format: MOVIE` → `Movie`.
> **#3 AniList airing episodes — done** (PR #18): falls back to
> `nextAiringEpisode.episode - 1`. Still open: per-episode titles for anime
> (the "anime episodes have no titles → `Series - S01E01`" gap) — moved to P3 below.
> **#4 `Engine::details` id-merge — done** (PR #7).

### 5. Dead/misleading config keys — **all done**
- ~~**`OPEN_MEDIA_*` env overrides**~~ — done (PR #8).
- ~~**`behavior.resume`**~~ — done (PR #28): `resume(bool)` on the Engine/builder,
  wired from config; when false the start-seek is skipped (progress still saved).
- ~~**`ui.theme`**~~ — done (PR #25): a dark/light Theme threaded through the TUI
  (`auto`→dark for now; true auto-detection is a follow-up).

---

## P2 — incomplete features

> **#6 Binge / auto-advance — done** (PR #26): after an episode completes,
> auto-advance to the next when `behavior.autoplay_next` is set; filler-aware via
> `Enricher::filler_episodes`.
> **#7 Pagination — done** (PR #27): TMDB reads `total_pages` and AniList fetches
> more results, inside each provider (no port change). nyaa stays single-page.
> **#8 `om config set` coverage — done** (PR #24): setters for the full key set
> with typed parsing; `config show` prints all loaded keys (secrets masked).

### 9. Tracker OAuth acquisition + MAL refresh
- AniList/MAL consume a pre-obtained token; no loopback OAuth flow, and MAL tokens
  (~1h) have no refresh. Add an `om login {anilist,mal}` flow.

### 10. Discord application id
- `crates/om-cli/src/compose.rs:106` `DISCORD_CLIENT_ID = "0000…"` placeholder →
  presence can't work. Register a real app id and ship it.

---

## P3 — smaller correctness nits & polish

Done in the 2026-06 batch (struck): ~~Scoring cached tiebreak~~ (#9);
~~MAL `Repeating`→`is_rewatching`~~ (#10); ~~AniSkip `episodeLength` real runtime~~
(#15); ~~vlc resume `--start-time`~~ (#11); ~~P2P mutex held across metadata wait~~
(#12); ~~nyaa category configurable~~ (#14); ~~release-tag parser nits~~ (#13).

Still open:
- **RD `check_cached` is a no-op** (`om-debrid/src/realdebrid.rs` ~150) — cache
  state for non-Torrentio-flagged candidates is always `Unknown`. Revisit when RD
  exposes a working bulk endpoint.
- **Anime per-episode titles** — AniList episodes have no `title` (`anilist.rs`),
  so the player/Discord shows `Series - S01E01`. Consider a Jikan/AniList
  per-episode title fetch (carried over from #3).

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
