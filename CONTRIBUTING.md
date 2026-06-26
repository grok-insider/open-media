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
components). On the NixOS dev host, the flake's devshell is canonical (lands in
v0.4).

## The golden rules

1. **Respect the dependency rule** (`AGENTS.md` → Module layout). `om-app` depends
   only on `om-core`. Only `om-cli` may name concrete adapters. If a change wants
   to break this, you need a new **port**, not a shortcut.
2. **Extend, don't edit, the core.** New backend = new adapter implementing an
   existing port + one line in `compose.rs` (OCP). New capability = new narrow
   port in `om-core::ports` (ISP).
3. **Map errors at the boundary.** Adapters convert concrete errors into the right
   `CoreError` variant. Callers branch on category, not on backend.
4. **No secrets in code, logs, or tests.** Tokens come only from `om-config` and
   are masked on display.

## Before you push

All four must be clean:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build
```

- **Tests:** unit-test pure logic (no network). Test adapters against recorded
  fixtures, not the live service. Test app logic with fake ports (see `om-app`).
- **Docs:** update `docs/PLAN.md` checkboxes when you complete phase work. Don't
  hand-edit `CHANGELOG.md` — release-plz regenerates it from your commit messages
  (see [Releases](#releases)), so a clear Conventional Commit *is* the changelog
  entry.
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
a scope when it helps (`feat(om-sources): …`). Small, focused commits.

A PR should leave `master` green (fmt + clippy + test) and the dependency rule
intact. Note any new config keys and update `om-config` + README.

## Releases

Releasing is automated with [release-plz](https://release-plz.dev)
(`release-plz.toml` + `.github/workflows/release.yml`). You don't bump versions or
write changelog entries by hand:

1. Merge Conventional-Commit PRs to `master` as usual.
2. release-plz keeps a **release PR** open (`chore: release v…`) that bumps the
   single `[workspace.package].version` (every crate inherits it via
   `version.workspace = true`), refreshes `Cargo.lock`, and regenerates
   `CHANGELOG.md` from the commits since the last tag. Polish that PR's notes if
   you like.
3. **Merge the release PR to ship.** It tags `vX.Y.Z`, creates the GitHub Release,
   and attaches a prebuilt `om` binary. The same push makes CI build and push
   `om-X.Y.Z` to the `grok-insider` cachix cache (`flake.nix` reads the version from
   `Cargo.toml`), and `om --version` reports it.

Nothing is published to crates.io.

**One-time setup.** Enable *Settings → Actions → General → "Allow GitHub Actions
to create and approve pull requests"* (so release-plz can open the release PR);
the `CACHIX_AUTH_TOKEN` secret is already configured. To anchor the first bump,
tag the current baseline once:

```bash
git tag -a v0.1.0 -m "open-media 0.1.0" && git push origin v0.1.0
```

## Adding an adapter — checklist

- [ ] New file in the owning crate; struct `impl`s the port; errors mapped.
- [ ] Exported from the crate's `lib.rs`.
- [ ] Selected in `crates/om-cli/src/compose.rs` (+ config keys in `om-config`).
- [ ] Fixture tests; `fmt`/`clippy`/`test` clean.
- [ ] No changes to `om-core`/`om-app` unless you intentionally added a port.

## Scope & legality

open-media is a client for services you authenticate to yourself and for public
indexes. Contributions must not bundle credentials, scrape behind auth walls, or
add DRM-circumvention. Keep it a clean, neutral tool.
