# next-task.md — open-media

> Working session brief. Read it first, then follow the workflow at the bottom.
> Do **not** merge anything.

## Current status

- Current workspace version: **0.0.1** (the public version line was reset; earlier 0.x milestones referenced in docs are history).
- PLAN Phases 0–10 are complete; post-0.1 hardening through crates.io publishing,
  subtitles, Windows IPC/artifacts, auto-advance, pagination, episode titles,
  source failover, and shared HTTP retry has landed.
- Repo: `github.com/grok-insider/open-media`. Binary: `open-media`.
- `master` is release-protected: feature/fix work lands in `dev`; only `dev` and
  `release-plz-*` PRs may target `master`.

## Current priorities

### P1 — correctness / release-adjacent

1. Real-Debrid `check_cached` is still a no-op; non-Torrentio cache state remains
   `Unknown` until RD or another backend exposes a reliable bulk cache API.
   (TorBox implements a real `check_cached`, so this is RD-specific.)

### P2 — incomplete features

- Direct nyaa pagination / load-more. TMDB and AniList paginate; nyaa remains one
  RSS page plus the absolute-episode second query when applicable.
- `open-media config set` lacks list/nested setters for `torrentio_providers`,
  `player.args`, `[subtitles]`, and `[ui.sources]`.
- Telemetry plumbing is present but inert because the collector endpoint is the
  `PLACEHOLDER` sentinel.

### P3 — polish

- Local subtitle sidecar discovery.
- Rich P2P/debrid progress in the TUI.
- Shell completions.
- Platform-aware player lookup beyond PATH defaults.

## Build / test / verify

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workflow

1. Create a focused branch off `dev`.
2. Make focused Conventional Commits (`feat:`/`fix:` for release-triggering work).
3. Get `fmt + clippy + test` green locally.
4. Push the branch and open a PR into `dev`.
5. **STOP. Do not merge.** Summarize what you did + the PR URL.
