//! Compile-time RGB swatches mirroring the CSS palettes in `css/*.css`.
//!
//! The settings preview cards need raw RGB triples to draw in Cairo, but the
//! source of truth for palette values is the CSS itself (parsed by GTK at
//! runtime). Rather than parse CSS at compile time, we keep a parallel table
//! here and lean on the `palettes_match_css_definitions` test to guard
//! against drift.

/// Six tokens that survive being shrunk to a 96×60 thumbnail. We pick the
/// surfaces that read as "the chat" plus the accent that brands it.
#[derive(Debug, Clone, Copy)]
pub struct PaletteSwatch {
    pub name: &'static str,
    pub bg: [u8; 3],
    pub mantle: [u8; 3],
    pub surface: [u8; 3],
    pub fg: [u8; 3],
    pub subtle: [u8; 3],
    pub accent: [u8; 3],
}

pub const ALL: &[PaletteSwatch] = &[
    PaletteSwatch {
        name: "catppuccin-mocha",
        bg: [0x1e, 0x1e, 0x2e],
        mantle: [0x18, 0x18, 0x25],
        surface: [0x31, 0x32, 0x44],
        fg: [0xcd, 0xd6, 0xf4],
        subtle: [0xa6, 0xad, 0xc8],
        accent: [0xcb, 0xa6, 0xf7],
    },
    PaletteSwatch {
        name: "catppuccin-latte",
        bg: [0xef, 0xf1, 0xf5],
        mantle: [0xe6, 0xe9, 0xef],
        surface: [0xcc, 0xd0, 0xda],
        fg: [0x4c, 0x4f, 0x69],
        subtle: [0x6c, 0x6f, 0x85],
        accent: [0x88, 0x39, 0xef],
    },
    PaletteSwatch {
        name: "catppuccin-frappe",
        bg: [0x30, 0x34, 0x46],
        mantle: [0x29, 0x2c, 0x3c],
        surface: [0x41, 0x45, 0x59],
        fg: [0xc6, 0xd0, 0xf5],
        subtle: [0xa5, 0xad, 0xce],
        accent: [0xca, 0x9e, 0xe6],
    },
    PaletteSwatch {
        name: "catppuccin-macchiato",
        bg: [0x24, 0x27, 0x3a],
        mantle: [0x1e, 0x20, 0x30],
        surface: [0x36, 0x3a, 0x4f],
        fg: [0xca, 0xd3, 0xf5],
        subtle: [0xa5, 0xad, 0xcb],
        accent: [0xc6, 0xa0, 0xf6],
    },
    PaletteSwatch {
        name: "gruvbox",
        bg: [0x28, 0x28, 0x28],
        mantle: [0x1d, 0x20, 0x21],
        surface: [0x3c, 0x38, 0x36],
        fg: [0xeb, 0xdb, 0xb2],
        subtle: [0xa8, 0x99, 0x84],
        accent: [0xd3, 0x86, 0x9b],
    },
    PaletteSwatch {
        name: "gruvbox-light",
        bg: [0xfb, 0xf1, 0xc7],
        mantle: [0xf9, 0xf5, 0xd7],
        surface: [0xeb, 0xdb, 0xb2],
        fg: [0x3c, 0x38, 0x36],
        subtle: [0x7c, 0x6f, 0x64],
        accent: [0xb1, 0x62, 0x86],
    },
    PaletteSwatch {
        name: "tokyo-night",
        bg: [0x1a, 0x1b, 0x26],
        mantle: [0x16, 0x16, 0x1e],
        surface: [0x29, 0x2e, 0x42],
        fg: [0xc0, 0xca, 0xf5],
        subtle: [0xa9, 0xb1, 0xd6],
        accent: [0xbb, 0x9a, 0xf7],
    },
    PaletteSwatch {
        name: "tokyo-night-storm",
        bg: [0x24, 0x28, 0x3b],
        mantle: [0x1f, 0x23, 0x35],
        surface: [0x2e, 0x34, 0x50],
        fg: [0xc0, 0xca, 0xf5],
        subtle: [0xa9, 0xb1, 0xd6],
        accent: [0xbb, 0x9a, 0xf7],
    },
    PaletteSwatch {
        name: "everforest-dark",
        bg: [0x2d, 0x35, 0x3b],
        mantle: [0x27, 0x2e, 0x33],
        surface: [0x37, 0x41, 0x45],
        fg: [0xd3, 0xc6, 0xaa],
        subtle: [0x9d, 0xa9, 0xa0],
        accent: [0xa7, 0xc0, 0x80],
    },
    PaletteSwatch {
        name: "everforest-dark-soft",
        bg: [0x33, 0x3c, 0x43],
        mantle: [0x2d, 0x35, 0x3b],
        surface: [0x3e, 0x4a, 0x51],
        fg: [0xd3, 0xc6, 0xaa],
        subtle: [0x9d, 0xa9, 0xa0],
        accent: [0xa7, 0xc0, 0x80],
    },
    PaletteSwatch {
        name: "everforest-light",
        bg: [0xfd, 0xf6, 0xe3],
        mantle: [0xf4, 0xf0, 0xd9],
        surface: [0xef, 0xeb, 0xd4],
        fg: [0x5c, 0x6a, 0x72],
        subtle: [0x82, 0x91, 0x81],
        accent: [0x8d, 0xa1, 0x01],
    },
    PaletteSwatch {
        name: "everforest-light-soft",
        bg: [0xf3, 0xea, 0xd3],
        mantle: [0xea, 0xe4, 0xca],
        surface: [0xe5, 0xdf, 0xb8],
        fg: [0x5c, 0x6a, 0x72],
        subtle: [0x82, 0x91, 0x81],
        accent: [0x8d, 0xa1, 0x01],
    },
    PaletteSwatch {
        name: "rose-pine",
        bg: [0x19, 0x17, 0x24],
        mantle: [0x1f, 0x1d, 0x2e],
        surface: [0x26, 0x23, 0x3a],
        fg: [0xe0, 0xde, 0xf4],
        subtle: [0x90, 0x8c, 0xaa],
        accent: [0xc4, 0xa7, 0xe7],
    },
    PaletteSwatch {
        name: "rose-pine-moon",
        bg: [0x23, 0x21, 0x36],
        mantle: [0x2a, 0x27, 0x3f],
        surface: [0x39, 0x35, 0x52],
        fg: [0xe0, 0xde, 0xf4],
        subtle: [0x90, 0x8c, 0xaa],
        accent: [0xc4, 0xa7, 0xe7],
    },
    PaletteSwatch {
        name: "rose-pine-dawn",
        bg: [0xfa, 0xf4, 0xed],
        mantle: [0xff, 0xfa, 0xf3],
        surface: [0xf2, 0xe9, 0xe1],
        fg: [0x57, 0x52, 0x79],
        subtle: [0x79, 0x75, 0x93],
        accent: [0x90, 0x7a, 0xa9],
    },
    PaletteSwatch {
        name: "nord",
        bg: [0x2e, 0x34, 0x40],
        mantle: [0x3b, 0x42, 0x52],
        surface: [0x43, 0x4c, 0x5e],
        fg: [0xec, 0xef, 0xf4],
        subtle: [0xd8, 0xde, 0xe9],
        accent: [0x88, 0xc0, 0xd0],
    },
];

pub fn palette_for(name: &str) -> Option<&'static PaletteSwatch> {
    let needle = name.trim().to_ascii_lowercase();
    ALL.iter().find(|p| p.name == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtin;
    use pretty_assertions::assert_eq;

    #[test]
    fn every_builtin_has_a_swatch() {
        for b in builtin::ALL {
            assert!(
                palette_for(b.canonical_name).is_some(),
                "missing swatch entry for {}",
                b.canonical_name
            );
        }
        assert_eq!(
            ALL.len(),
            builtin::ALL.len(),
            "swatch table size diverged from builtin table"
        );
    }

    #[test]
    fn palettes_match_css_definitions() {
        // Guards against drift: every RGB triple in `ALL` must be the
        // literal value of the matching `@define-color kryptos_<token>`
        // line in the corresponding CSS file.
        for sw in ALL {
            let css = builtin::lookup(sw.name)
                .unwrap_or_else(|| panic!("no css for swatch {}", sw.name))
                .css;
            for (token, expected) in [
                ("bg", sw.bg),
                ("mantle", sw.mantle),
                ("surface", sw.surface),
                ("fg", sw.fg),
                ("subtle", sw.subtle),
                ("accent", sw.accent),
            ] {
                let got = parse_define_color(css, token).unwrap_or_else(|| {
                    panic!("{}: token kryptos_{token} not parseable", sw.name)
                });
                assert_eq!(
                    got, expected,
                    "{} kryptos_{token} swatch drift (css={got:02x?}, swatch={expected:02x?})",
                    sw.name
                );
            }
        }
    }

    fn parse_define_color(css: &str, token: &str) -> Option<[u8; 3]> {
        let needle = format!("@define-color kryptos_{token}");
        let line = css.lines().find(|l| l.trim_start().starts_with(&needle))?;
        let hex = line.split('#').nth(1)?.trim().trim_end_matches(';').trim();
        if hex.len() < 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some([r, g, b])
    }

    #[test]
    fn palette_for_is_case_insensitive() {
        assert!(palette_for("Rose-Pine").is_some());
        assert!(palette_for("  NORD  ").is_some());
        assert!(palette_for("not-a-theme").is_none());
        assert!(palette_for("system").is_none());
    }
}
