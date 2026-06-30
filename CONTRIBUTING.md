# Contributing to open-media

Thanks for hacking on open-media. This project optimizes for **clean,
maintainable, extensible** code — read `AGENTS.md` and `docs/ARCHITECTURE.md`
before a first change.

## Dev setup

```bash
git clone https://github.com/grok-insider/open-media && cd open-media
cargo build
cargo test --workspace
```

A recent stable Rust toolchain is required (`rust-toolchain.toml` pins channel +
components). On the NixOS dev host, the flake's devshell is canonical and
provides the Rust toolchain plus native build glue.

## The golden rules

1. **Respect the dependency rule** (`AGENTS.md` → Module layout). `open-media-app` depends
   only on `open-media-core`. Only `open-media-cli` may name concrete adapters. If a change wants
   to break this, you need a new **port**, not a shortcut.
2. **Extend, don't edit, the core.** New backend = new adapter implementing an
   existing port + one line in `compose.rs` (OCP). New capability = new narrow
   port in `open-media-core::ports` (ISP).
3. **Map errors at the boundary.** Adapters convert concrete errors into the right
   `CoreError` variant. Callers branch on category, not on backend.
4. **No secrets in code, logs, or tests.** Tokens come only from `open-media-config` and
   are masked on display. Public *identifiers* are not secrets: the Discord
   application id (`compose.rs`) is sent in the presence handshake by design and is
   correctly hardcoded, not a config secret.
5. **Telemetry privacy invariant.** The `UsageReporter` payload (`open-media-telemetry`)
   must only ever carry `UsageInfo` — app version, OS, arch, a random install id.
   Never add anything about what a user watches (titles, queries, source names,
   tokens, history). Telemetry is opt-out (`telemetry=false`) and best-effort.

## Before you push

All four must be clean:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build
```

- **Tests:** unit-test pure logic (no network). Test adapters against recorded
  fixtures, not the live service. Test app logic with fake ports (see `open-media-app`).
- **Docs:** update `docs/PLAN.md` / `continue-plan.md` when you complete roadmap
  work. Don't hand-edit `CHANGELOG.md` outside a release PR — release-plz creates
  it from commits and the release workflow enriches that PR's notes, so a clear
  Conventional Commit *is* the changelog input.
- **Comments:** explain *why* (a quirk, a rate limit, a scoring trade-off), not
  *what*.

## Commit & PR style

This repo uses [Conventional Commits](https://www.conventionalcommits.org). The
commit history drives automated versioning and the changelog (see
[Releases](#releases)), so prefix every commit subject with a type:

- `feat: …` — a user-visible feature → **minor** bump (`0.x.0`).
- `fix: …` — a bug fix → **patch** bump (`0.0.x`).
- `feat!: …` (or a `BREAKING CHANGE:` footer) — a breaking change. While the
  project is `0.x` this bumps the **minor**, per Cargo's SemVer rules.
- `docs:`, `refactor:`, `perf:`, `test:`, `chore:`, `ci:` — don't trigger a
  release on their own; grouped into the changelog where relevant.

Keep subjects short and imperative (`fix: unrestrict the requested RD file`); add
a scope when it helps (`feat(open-media-sources): …`). Small, focused commits.

A PR should leave its target branch green (fmt + clippy + test) and the
dependency rule intact. Note any new config keys and update `open-media-config` +
README.

## Releases

Releasing is automated with [release-plz](https://release-plz.dev)
(`release-plz.toml` + `.github/workflows/release.yml`). You usually don't bump
versions or write changelog entries by hand:

1. Land feature/fix PRs into `dev`.
2. Open the single sanctioned `dev` → `master` integration PR. The `guard master`
   workflow rejects other PR branches into `master`, except `release-plz-*`.
3. When `feat:`/`fix:` commits reach `master`, release-plz keeps a **release PR**
   open (`chore: release v…`) that bumps the
   single `[workspace.package].version` (every crate inherits it via
   `version.workspace = true`), refreshes `Cargo.lock`, and regenerates
   `CHANGELOG.md` from the commits since the last tag. The workflow then enriches
   that changelog section with AI-written user-facing notes.
4. **Merge the release PR to ship.** It tags `vX.Y.Z`, publishes every crate to
   crates.io in dependency order, creates the GitHub Release, and attaches
   prebuilt `open-media` archives for Linux/macOS/Windows. The same push makes CI build and push
   `open-media-X.Y.Z` to the `grok-insider` cachix cache (`flake.nix` reads the version from
   `Cargo.toml`), and `open-media --version` reports it.

**One-time setup.** Enable *Settings → Actions → General → "Allow GitHub Actions
to create and approve pull requests"* (so release-plz can open the release PR);
configure `RELEASE_PLZ_TOKEN`, `CARGO_REGISTRY_TOKEN`, `OPENROUTER_API_KEY`, and
`CACHIX_AUTH_TOKEN` secrets. To anchor a fresh release history, tag the current
baseline once:

```bash
git tag -a v0.1.0 -m "open-media 0.1.0" && git push origin v0.1.0
```

## Adding an adapter — checklist

- [ ] New file in the owning crate; struct `impl`s the port; errors mapped.
- [ ] Exported from the crate's `lib.rs`.
- [ ] Selected in `crates/open-media-cli/src/compose.rs` (+ config keys in `open-media-config`).
- [ ] Fixture tests; `fmt`/`clippy`/`test` clean.
- [ ] No changes to `open-media-core`/`open-media-app` unless you intentionally added a port.

## Scope & legality

open-media is a client for services you authenticate to yourself and for public
indexes. Contributions must not bundle credentials, scrape behind auth walls, or
add DRM-circumvention. Keep it a clean, neutral tool.
