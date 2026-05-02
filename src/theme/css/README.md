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
- **Everforest** (dark medium / dark soft / light medium / light soft) —
  https://github.com/sainnhe/everforest
- **Rose Pine** (main / Moon / Dawn) — https://rosepinetheme.com

If a hex value here drifts from the upstream spec, the upstream spec wins —
file a bug.

## Everforest token mapping

Everforest groups its swatches by purpose (`bg0..bg5`, `grey0..grey2`,
`statusline1..3`, etc.). The mapping into Kryptos's 13 tokens is:

| Variant                  | bg        | mantle    | surface   | surface2  | overlay   | fg        | subtle    | accent / green | blue      | yellow    | red       | lavender  |
|--------------------------|-----------|-----------|-----------|-----------|-----------|-----------|-----------|----------------|-----------|-----------|-----------|-----------|
| `everforest-dark`        | `#2d353b` | `#272e33` | `#374145` | `#414b50` | `#475258` | `#d3c6aa` | `#9da9a0` | `#a7c080`      | `#7fbbb3` | `#dbbc7f` | `#e67e80` | `#d699b6` |
| `everforest-dark-soft`   | `#333c43` | `#2d353b` | `#3e4a51` | `#475258` | `#4f585e` | `#d3c6aa` | `#9da9a0` | `#a7c080`      | `#7fbbb3` | `#dbbc7f` | `#e67e80` | `#d699b6` |
| `everforest-light`       | `#fdf6e3` | `#f4f0d9` | `#efebd4` | `#e0dcc7` | `#e6e2cc` | `#5c6a72` | `#829181` | `#8da101`      | `#3a94c5` | `#dfa000` | `#f85552` | `#df69ba` |
| `everforest-light-soft`  | `#f3ead3` | `#eae4ca` | `#e5dfb8` | `#dad3ad` | `#d8d3ba` | `#5c6a72` | `#829181` | `#8da101`      | `#3a94c5` | `#dfa000` | `#f85552` | `#df69ba` |

`accent` is bound to Everforest's signature green so the focus stripe and
selection tint stay consistent across the family.
