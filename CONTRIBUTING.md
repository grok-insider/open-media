# Contributing to open-media

Thanks for hacking on open-media. This project optimizes for **clean,
maintainable, extensible** code — read `AGENTS.md` and `docs/ARCHITECTURE.md`
before a first change.

## Dev setup

```bash
git clone https://github.com/0xfell/open-media && cd open-media
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
- **Docs:** update `docs/PLAN.md` checkboxes when you complete phase work, and add
  a `CHANGELOG.md` `Unreleased` entry for anything user-visible.
- **Comments:** explain *why* (a quirk, a rate limit, a scoring trade-off), not
  *what*.

## Commit & PR style

- Small, focused commits; imperative subject (`add Real-Debrid resolve flow`).
- Reference the phase where relevant (`Phase 3: …`).
- A PR should leave `main` green (fmt + clippy + test) and the dependency rule
  intact. Note any new config keys and update `om-config` + README.

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
