# Contributing to Kryptos

Thanks for your interest. Kryptos is a vim-first Signal desktop client
written in Rust on GTK4 + libadwaita, talking to `signal-cli` over D-Bus.
This doc covers how to get a working dev loop and what we expect from
patches.

## Dev environment

The supported path is Nix:

```sh
nix develop          # gtk4, libadwaita, signal-cli, rust toolchain
cargo run
cargo nextest run
```

Without Nix you need rustc ≥ 1.80, gtk4 ≥ 4.14, libadwaita ≥ 1.5,
pkg-config, glib, sqlite, openssl, dbus, and `signal-cli` on `$PATH`.
See the README for the full list.

A scratch config lives at `config.example.toml`. Copy it to
`~/.config/kryptos/config.toml` — Kryptos hot-reloads on save, so you
can iterate on bindings and styling without restarting.

## Module layout

Keep changes scoped to one module when you can. The boundaries are
load-bearing — the vim engine and UI must stay protocol-agnostic so we
can add Telegram (and others) later as a sibling to `dbus/`.

```
src/
├── config/   toml schema, hot-reload via notify
├── core/     error type, logging, app-wide primitives
├── dbus/     zbus proxy for signal-cli
├── vim/      modal state machine, keymaps
└── ui/       gtk4 / libadwaita view layer (relm4)
```

Cross-module changes are fine when needed, but call them out in the PR
description.

## Coding standards

- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must be
  clean. CI will reject otherwise.
- Tests live next to the code they cover as `#[cfg(test)] mod tests`.
  Use `pretty_assertions` for diffs and `tempfile` for filesystem
  fixtures.
- Errors: library code returns `thiserror`-derived enums; binary entry
  points and glue can use `anyhow`. Don't `unwrap()` outside tests.
- Logging via `tracing`. Prefer structured fields (`tracing::info!(user_id, "…")`)
  over string interpolation.
- Async: Tokio for I/O and signal-cli; never block the glib mainloop.
  Bridge with `relm4`'s command/worker primitives.
- SQL: keep schema changes in `sqlx` migrations, not ad-hoc DDL.

## Commits and PRs

- Atomic commits. One logical change per commit, buildable at every
  step.
- [Conventional Commits](https://www.conventionalcommits.org/) format:
  `feat(vim): …`, `fix(dbus): …`, `refactor(ui): …`, `chore: …`.
- Rebase, don't merge. Force-push your own branch as needed.
- Before opening a PR: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
  `cargo nextest run`.
- PR description should say what changed, why, and how you tested it.
  Screenshots or a short screencap for UI changes.
- No "Co-Authored-By" trailers from AI tools.

## Reporting bugs

Open a GitHub issue with:

- Distro + GTK / libadwaita / signal-cli versions (`signal-cli --version`,
  `pkg-config --modversion gtk4 libadwaita-1`).
- Your `~/.config/kryptos/config.toml` (redact account info).
- Steps to reproduce and what you expected.
- Logs from `RUST_LOG=kryptos=debug cargo run` or the installed binary.

Crashes: a backtrace from `RUST_BACKTRACE=1` is gold.

## Scope

The project is in phase 0 — foundations. Right now the priorities are
signal-cli wiring, the vim modal engine, and the message view. Larger
proposals (new transports, plugin systems, alternate UIs) are welcome,
but please open an issue to discuss before coding so we can align on
the module boundaries.

## License

By contributing, you agree your work is dual-licensed under MIT or
Apache-2.0, matching the project.
