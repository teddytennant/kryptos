//! Embedded built-in theme CSS. Sourced from `src/theme/css/*.css` at
//! compile time via `include_str!`.

use adw::ColorScheme;

pub const CATPPUCCIN_MOCHA: &str = include_str!("css/catppuccin-mocha.css");
pub const CATPPUCCIN_LATTE: &str = include_str!("css/catppuccin-latte.css");
pub const CATPPUCCIN_FRAPPE: &str = include_str!("css/catppuccin-frappe.css");
pub const CATPPUCCIN_MACCHIATO: &str = include_str!("css/catppuccin-macchiato.css");
pub const GRUVBOX: &str = include_str!("css/gruvbox.css");
pub const GRUVBOX_LIGHT: &str = include_str!("css/gruvbox-light.css");
pub const TOKYO_NIGHT: &str = include_str!("css/tokyo-night.css");
pub const TOKYO_NIGHT_STORM: &str = include_str!("css/tokyo-night-storm.css");
pub const EVERFOREST_DARK: &str = include_str!("css/everforest-dark.css");
pub const EVERFOREST_DARK_SOFT: &str = include_str!("css/everforest-dark-soft.css");
pub const EVERFOREST_LIGHT: &str = include_str!("css/everforest-light.css");
pub const EVERFOREST_LIGHT_SOFT: &str = include_str!("css/everforest-light-soft.css");
pub const ROSE_PINE: &str = include_str!("css/rose-pine.css");
pub const ROSE_PINE_MOON: &str = include_str!("css/rose-pine-moon.css");
pub const ROSE_PINE_DAWN: &str = include_str!("css/rose-pine-dawn.css");
pub const NORD: &str = include_str!("css/nord.css");

/// Resolved built-in theme: which CSS blob, and which adw color scheme to
/// nudge libadwaita-aware widgets toward.
#[derive(Debug, Clone, Copy)]
pub struct Builtin {
    pub canonical_name: &'static str,
    pub css: &'static str,
    pub color_scheme: ColorScheme,
}

/// Every name `apply()` accepts other than `"system"`. Order is the order
/// shown in the "unknown theme" error message.
pub const ALL: &[Builtin] = &[
    Builtin {
        canonical_name: "catppuccin-mocha",
        css: CATPPUCCIN_MOCHA,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "catppuccin-latte",
        css: CATPPUCCIN_LATTE,
        color_scheme: ColorScheme::ForceLight,
    },
    Builtin {
        canonical_name: "catppuccin-frappe",
        css: CATPPUCCIN_FRAPPE,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "catppuccin-macchiato",
        css: CATPPUCCIN_MACCHIATO,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "gruvbox",
        css: GRUVBOX,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "gruvbox-light",
        css: GRUVBOX_LIGHT,
        color_scheme: ColorScheme::ForceLight,
    },
    Builtin {
        canonical_name: "tokyo-night",
        css: TOKYO_NIGHT,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "tokyo-night-storm",
        css: TOKYO_NIGHT_STORM,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "everforest-dark",
        css: EVERFOREST_DARK,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "everforest-dark-soft",
        css: EVERFOREST_DARK_SOFT,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "everforest-light",
        css: EVERFOREST_LIGHT,
        color_scheme: ColorScheme::ForceLight,
    },
    Builtin {
        canonical_name: "everforest-light-soft",
        css: EVERFOREST_LIGHT_SOFT,
        color_scheme: ColorScheme::ForceLight,
    },
    Builtin {
        canonical_name: "rose-pine",
        css: ROSE_PINE,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "rose-pine-moon",
        css: ROSE_PINE_MOON,
        color_scheme: ColorScheme::ForceDark,
    },
    Builtin {
        canonical_name: "rose-pine-dawn",
        css: ROSE_PINE_DAWN,
        color_scheme: ColorScheme::ForceLight,
    },
    Builtin {
        canonical_name: "nord",
        css: NORD,
        color_scheme: ColorScheme::ForceDark,
    },
];

/// Case-insensitive lookup. `"system"` is *not* a built-in — callers must
/// treat that name specially.
pub fn lookup(name: &str) -> Option<Builtin> {
    let needle = name.trim().to_ascii_lowercase();
    ALL.iter().copied().find(|b| b.canonical_name == needle)
}

pub fn known_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = ALL.iter().map(|b| b.canonical_name).collect();
    v.push("system");
    v
}
