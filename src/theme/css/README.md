# Theme CSS

Built-in palettes for Kryptos. Each file is `include_str!`-embedded into the
binary by `src/theme/builtin.rs` and applied through a `gtk::CssProvider`.

Every theme defines the same set of GTK `@define-color` names so a user's
`~/.config/kryptos/custom.css` can override individual swatches, or pick a
built-in theme and tweak from there.

## Palette tokens

| Token              | Role                                                  |
|--------------------|-------------------------------------------------------|
| `kryptos_bg`       | Window background                                     |
| `kryptos_mantle`   | Sidebar / secondary surface                           |
| `kryptos_surface`  | Header bar, raised panels                             |
| `kryptos_surface2` | Hovered rows, separators                              |
| `kryptos_overlay`  | Tooltip / muted text                                  |
| `kryptos_fg`       | Primary text                                          |
| `kryptos_subtle`   | Subdued text (timestamps, metadata)                   |
| `kryptos_accent`   | Primary accent (links, focus ring)                    |
| `kryptos_blue`     | Insert-mode modeline                                  |
| `kryptos_green`    | Normal-mode modeline                                  |
| `kryptos_yellow`   | Search-mode modeline                                  |
| `kryptos_red`      | Errors, destructive actions                           |
| `kryptos_lavender` | Command-mode modeline                                 |

## Source palettes

- **Catppuccin** (Mocha / Latte / Frappé / Macchiato) — https://catppuccin.com
  Canonical hex values from the official palette spec.
- **Gruvbox** (dark / light medium-contrast) — https://github.com/morhetz/gruvbox
- **Tokyo Night** (default / Storm) — https://github.com/folke/tokyonight.nvim

If a hex value here drifts from the upstream spec, the upstream spec wins —
file a bug.
