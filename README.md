# SigVim

A vim-first Signal desktop client for Linux. Native GTK4 + libadwaita.
No Electron, no webview. Built for NixOS + Hyprland; works anywhere
GTK4 runs.

> **Status:** phase 0 — foundation. Window opens, modules compile.
> Signal-cli wiring, vim modal engine, and message view come next.

## Tech stack (fixed)

| Layer        | Choice                                     |
| ------------ | ------------------------------------------ |
| Language     | Rust 2021 (stable)                         |
| GUI          | gtk4-rs + libadwaita-rs + relm4            |
| IPC          | zbus → signal-cli (D-Bus)                  |
| Async        | Tokio + glib mainloop                      |
| Config       | serde + toml, hot-reloaded via `notify`    |
| Local cache  | sqlx + SQLite                              |
| Notifications| notify-rust (libnotify with reply action)  |

## Build / run

### Nix (recommended)

```sh
nix develop          # enter dev shell with all deps + signal-cli
cargo run            # launch
cargo nextest run    # tests
```

`shell.nix` is provided for users on classic `nix-shell`.

A release binary:

```sh
nix build .#         # → ./result/bin/sigvim
```

### Without Nix

You need: rustc ≥ 1.80, gtk4 ≥ 4.14, libadwaita ≥ 1.5, pkg-config,
glib, sqlite, openssl, dbus, signal-cli on `$PATH`.

```sh
cargo run
```

## Configuration

SigVim reads `~/.config/sigvim/config.toml`. A complete, commented
example lives at [`config.example.toml`](./config.example.toml). Copy
it and edit. Changes are picked up live — no restart.

```sh
mkdir -p ~/.config/sigvim
cp config.example.toml ~/.config/sigvim/config.toml
```

## Repo layout

```
src/
├── main.rs          # entry point
├── lib.rs           # module root
├── config/          # toml schema + hot-reload
├── core/            # error type, logging
├── dbus/            # signal-cli zbus proxy
├── vim/             # modal state machine, keymaps
└── ui/              # gtk4 / libadwaita view layer
```

## Contributing

- Atomic commits, [Conventional Commits](https://www.conventionalcommits.org/) format.
- Tests next to the code they cover (`#[cfg(test)] mod tests`).
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` clean.
- Feature work touches one module at a time when possible.

## License

Dual-licensed under MIT or Apache-2.0, at your option.
