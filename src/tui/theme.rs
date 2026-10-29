//! Colors of the TUI.
//!
//! The UI never names a color directly: it asks for a role (`theme().red`,
//! `theme().selection`, …). The default `auto` theme only uses the terminal's
//! own palette (ANSI names, `reset` and text modifiers), so it follows any
//! terminal theme, light or dark. Other themes are built in (`catppuccin_*`)
//! or declared by the user under `[themes.<name>]`.

use indexmap::IndexMap;
use ratatui::style::{Color, Modifier, Style};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::OnceLock;

pub const AUTO: &str = "auto";

/// Resolved theme: every color the TUI draws with.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Default text.
    pub fg: Color,
    /// Secondary text (placeholders, "-" cells).
    pub muted: Color,
    /// Borders and empty parts of bars.
    pub faint: Color,
    pub red: Color,
    pub orange: Color,
    pub green: Color,
    pub yellow: Color,
    /// Daemons that are only declared in a config (not started yet).
    pub cyan: Color,
    /// Completed oneshot tasks.
    pub dim_green: Color,
    /// Whole-screen background. Unset in `auto`: the terminal's own is kept.
    pub background: Style,
    /// Row under the cursor.
    pub selection: Style,
    /// Row ticked with `Space`.
    pub marked: Style,
    /// Current search match.
    pub match_current: Style,
    /// Other search matches.
    pub match_other: Style,
    /// Background of overlays (help, details, editor, …).
    pub overlay: Style,
    /// Background of the confirmation overlay.
    pub danger: Style,
}

impl Theme {
    /// Terminal palette only: adapts to light and dark terminals.
    pub fn auto() -> Self {
        Self {
            fg: Color::Reset,
            muted: Color::DarkGray,
            faint: Color::DarkGray,
            red: Color::Red,
            orange: Color::LightRed,
            green: Color::Green,
            yellow: Color::Yellow,
            cyan: Color::Cyan,
            dim_green: Color::Green,
            // No background: ANSI colors are remapped by the terminal theme, so
            // no one of them is guaranteed to contrast with the text. Reverse
            // video is out too, it would fill colored empty cells (error column).
            background: Style::new(),
            selection: Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            marked: Style::new().add_modifier(Modifier::ITALIC),
            match_current: Style::new().add_modifier(Modifier::REVERSED),
            match_other: Style::new().add_modifier(Modifier::UNDERLINED),
            overlay: Style::new(),
            danger: Style::new(),
        }
    }

    fn catppuccin(p: &Palette) -> Self {
        let bg = |c: u32| Style::new().bg(hex(c));
        let panel = Style::new().fg(hex(p.text)).bg(hex(p.base));
        Self {
            fg: hex(p.text),
            muted: hex(p.subtext),
            faint: hex(p.surface),
            red: hex(p.red),
            orange: hex(p.peach),
            green: hex(p.green),
            yellow: hex(p.yellow),
            cyan: hex(p.sky),
            dim_green: hex(p.teal),
            background: panel,
            selection: bg(p.surface_alt),
            marked: bg(p.surface),
            match_current: bg(p.yellow_dim),
            match_other: bg(p.surface),
            overlay: panel,
            danger: panel,
        }
    }

    /// Per-role colors of `def` replace the ones of `self`.
    fn apply(mut self, name: &str, def: &ThemeDef) -> miette::Result<Self> {
        let color = |key: &str, v: &Option<String>| -> miette::Result<Option<Color>> {
            v.as_deref()
                .map(|s| {
                    Color::from_str(s).map_err(|_| {
                        miette::miette!(
                            "theme '{name}': invalid color '{s}' for '{key}' \
                             (use #rrggbb, an ANSI name such as bright_black, or reset)"
                        )
                    })
                })
                .transpose()
        };
        macro_rules! fg {
            ($($f:ident),*) => {$(
                if let Some(c) = color(stringify!($f), &def.$f)? { self.$f = c; }
            )*};
        }
        macro_rules! bg {
            ($($f:ident => $key:ident),*) => {$(
                if let Some(c) = color(stringify!($key), &def.$key)? {
                    self.$f = Style::new().bg(c);
                }
            )*};
        }
        fg!(
            fg, muted, faint, red, orange, green, yellow, cyan, dim_green
        );
        bg!(
            background => bg,
            selection => selection_bg,
            marked => marked_bg,
            match_current => match_current_bg,
            match_other => match_other_bg,
            overlay => overlay_bg,
            danger => danger_bg
        );
        // Cells are merged onto the buffer and `Clear` wipes it, so every
        // full-area style must carry the foreground itself.
        if def.bg.is_some() {
            if def.overlay_bg.is_none() {
                self.overlay = self.background;
            }
            if def.danger_bg.is_none() {
                self.danger = self.background;
            }
        }
        for panel in [&mut self.background, &mut self.overlay, &mut self.danger] {
            *panel = panel.fg(self.fg);
        }
        Ok(self)
    }
}

/// A theme declared in `[themes.<name>]`. Only the roles set here differ from
/// `base`; colors are `#rrggbb`, ANSI names (`red`, `bright_black`, …),
/// `reset` (the terminal's own color) or a 0-255 palette index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemeDef {
    /// Theme to start from: `auto`, a built-in theme or another custom theme.
    /// Defaults to `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Default text color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    /// Secondary text color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<String>,
    /// Borders and empty parts of bars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub red: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orange: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub green: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yellow: Option<String>,
    /// Daemons declared in a config but not started yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cyan: Option<String>,
    /// Completed oneshot tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim_green: Option<String>,
    /// Background of the whole screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    /// Background of the row under the cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_bg: Option<String>,
    /// Background of rows ticked with `Space`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marked_bg: Option<String>,
    /// Background of the current search match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_current_bg: Option<String>,
    /// Background of the other search matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_other_bg: Option<String>,
    /// Background of overlays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_bg: Option<String>,
    /// Background of the confirmation overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub danger_bg: Option<String>,
}

/// Resolves `name` against the built-in themes and the user's `themes`.
pub fn resolve(name: &str, custom: &IndexMap<String, ThemeDef>) -> miette::Result<Theme> {
    resolve_inner(name, custom, &mut Vec::new())
}

fn resolve_inner<'a>(
    name: &'a str,
    custom: &'a IndexMap<String, ThemeDef>,
    seen: &mut Vec<&'a str>,
) -> miette::Result<Theme> {
    if seen.contains(&name) {
        miette::bail!("theme '{name}' is its own base ({})", seen.join(" -> "));
    }
    seen.push(name);
    // A custom theme shadows a built-in one of the same name.
    if let Some(def) = custom.get(name) {
        let base = match def.base.as_deref() {
            Some(base) => resolve_inner(base, custom, seen)?,
            // `[themes.auto]` customizes the built-in `auto`, not itself.
            None if name == AUTO => Theme::auto(),
            None => resolve_inner(AUTO, custom, seen)?,
        };
        return base.apply(name, def);
    }
    if name == AUTO {
        return Ok(Theme::auto());
    }
    if let Some(p) = PALETTES.iter().find(|p| p.name == name) {
        return Ok(Theme::catppuccin(p));
    }
    let builtin = PALETTES.iter().map(|p| p.name).chain([AUTO]);
    let known: Vec<_> = builtin.chain(custom.keys().map(String::as_str)).collect();
    miette::bail!(
        "unknown TUI theme '{name}' (available: {})",
        known.join(", ")
    );
}

/// Name of the theme to use: `theme` unless it is `auto` and a
/// `light`/`dark` variant is configured for the terminal's background.
///
/// `is_dark` is only called when it matters, as it queries the terminal.
pub fn pick_name<'a>(
    theme: &'a str,
    light: &'a str,
    dark: &'a str,
    is_dark: impl FnOnce() -> Option<bool>,
) -> &'a str {
    if theme != AUTO || (light.is_empty() && dark.is_empty()) {
        return theme;
    }
    let variant = match is_dark() {
        Some(true) => dark,
        Some(false) => light,
        None => "",
    };
    if variant.is_empty() { theme } else { variant }
}

/// Whether the terminal has a dark background, `None` if it does not say.
pub fn terminal_is_dark() -> Option<bool> {
    use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};
    match theme_mode(QueryOptions::default()) {
        Ok(ThemeMode::Dark) => Some(true),
        Ok(ThemeMode::Light) => Some(false),
        Err(_) => None,
    }
}

static THEME: OnceLock<Theme> = OnceLock::new();

/// Installs the theme for this process. Later calls are ignored.
pub fn init(theme: Theme) {
    let _ = THEME.set(theme);
}

/// The active theme; `auto` until [`init`] is called.
pub fn theme() -> &'static Theme {
    THEME.get_or_init(Theme::auto)
}

fn hex(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

struct Palette {
    name: &'static str,
    text: u32,
    /// Subtext0: secondary text, which must stay readable on `surface_alt`.
    subtext: u32,
    /// Surface1: borders, marked rows, secondary matches.
    surface: u32,
    /// Surface0: cursor row.
    surface_alt: u32,
    base: u32,
    red: u32,
    peach: u32,
    green: u32,
    yellow: u32,
    sky: u32,
    teal: u32,
    /// Current search match: yellow blended into the background.
    yellow_dim: u32,
}

const PALETTES: &[Palette] = &[
    Palette {
        name: "catppuccin_latte",
        text: 0x4c4f69,
        subtext: 0x6c6f85,
        surface: 0xbcc0cc,
        surface_alt: 0xccd0da,
        base: 0xeff1f5,
        red: 0xd20f39,
        peach: 0xfe640b,
        green: 0x40a02b,
        yellow: 0xdf8e1d,
        sky: 0x04a5e5,
        teal: 0x179299,
        yellow_dim: 0xf2d8a6,
    },
    Palette {
        name: "catppuccin_frappe",
        text: 0xc6d0f5,
        subtext: 0xa5adce,
        surface: 0x51576d,
        surface_alt: 0x414559,
        base: 0x303446,
        red: 0xe78284,
        peach: 0xef9f76,
        green: 0xa6d189,
        yellow: 0xe5c890,
        sky: 0x99d1db,
        teal: 0x81c8be,
        yellow_dim: 0x6b624a,
    },
    Palette {
        name: "catppuccin_macchiato",
        text: 0xcad3f5,
        subtext: 0xa5adcb,
        surface: 0x494d64,
        surface_alt: 0x363a4f,
        base: 0x24273a,
        red: 0xed8796,
        peach: 0xf5a97f,
        green: 0xa6da95,
        yellow: 0xeed49f,
        sky: 0x91d7e3,
        teal: 0x8bd5ca,
        yellow_dim: 0x645a41,
    },
    Palette {
        name: "catppuccin_mocha",
        text: 0xcdd6f4,
        subtext: 0xa6adc8,
        surface: 0x45475a,
        surface_alt: 0x313244,
        base: 0x1e1e2e,
        red: 0xf38ba8,
        peach: 0xfab387,
        green: 0xa6e3a1,
        yellow: 0xf9e2af,
        sky: 0x89dceb,
        teal: 0x94e2d5,
        yellow_dim: 0x5f573f,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(toml: &str) -> IndexMap<String, ThemeDef> {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn auto_only_uses_the_terminal_palette() {
        let t = resolve(AUTO, &IndexMap::new()).unwrap();
        assert_eq!(t.fg, Color::Reset);
        assert_eq!(t.selection.bg, None);
        assert!(t.selection.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn builtin_catppuccin_is_available() {
        let t = resolve("catppuccin_mocha", &IndexMap::new()).unwrap();
        assert_eq!(t.fg, Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(t.selection.bg, Some(Color::Rgb(0x31, 0x32, 0x44)));
        assert_eq!(t.background.bg, Some(Color::Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(Theme::auto().background.bg, None);
    }

    #[test]
    fn custom_theme_overrides_only_what_it_sets() {
        let c = custom(
            r##"
            [mine]
            base = "catppuccin_latte"
            red = "bright_red"
            selection_bg = "#ff0000"
            "##,
        );
        let t = resolve("mine", &c).unwrap();
        assert_eq!(t.red, Color::LightRed);
        assert_eq!(t.selection.bg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(t.green, Color::Rgb(0x40, 0xa0, 0x2b));
    }

    #[test]
    fn custom_theme_without_base_starts_from_auto() {
        let c = custom(r#"mine = { yellow = "reset" }"#);
        let t = resolve("mine", &c).unwrap();
        assert_eq!(t.yellow, Color::Reset);
        assert_eq!(t.red, Color::Red);
    }

    #[test]
    fn custom_themes_can_chain_bases() {
        let c = custom(
            r#"
            a = { red = "blue" }
            b = { base = "a", green = "magenta" }
            "#,
        );
        let t = resolve("b", &c).unwrap();
        assert_eq!((t.red, t.green), (Color::Blue, Color::Magenta));
    }

    #[test]
    fn a_custom_auto_without_base_customizes_the_builtin() {
        let c = custom(r#"auto = { red = "magenta" }"#);
        let t = resolve(AUTO, &c).unwrap();
        assert_eq!((t.red, t.green), (Color::Magenta, Color::Green));
    }

    #[test]
    fn full_area_styles_carry_the_foreground() {
        let t = resolve("catppuccin_latte", &IndexMap::new()).unwrap();
        for s in [t.background, t.overlay, t.danger] {
            assert_eq!(s.fg, Some(t.fg));
        }
        let c = custom(r##"mine = { fg = "#111111", bg = "#eeeeee" }"##);
        let t = resolve("mine", &c).unwrap();
        for s in [t.background, t.overlay, t.danger] {
            assert_eq!((s.fg, s.bg), (Some(t.fg), t.background.bg));
        }
    }

    #[test]
    fn base_cycles_are_rejected() {
        let c = custom(
            r#"
            a = { base = "b" }
            b = { base = "a" }
            "#,
        );
        let err = resolve("a", &c).unwrap_err().to_string();
        assert!(err.contains("own base"), "{err}");
    }

    #[test]
    fn unknown_theme_lists_the_available_ones() {
        let c = custom(r#"mine = {}"#);
        let err = resolve("nope", &c).unwrap_err().to_string();
        assert!(
            err.contains("catppuccin_mocha") && err.contains("mine"),
            "{err}"
        );
    }

    #[test]
    fn invalid_color_names_the_theme_and_key() {
        let c = custom(r#"mine = { red = "blurple" }"#);
        let err = resolve("mine", &c).unwrap_err().to_string();
        assert!(err.contains("'mine'") && err.contains("'red'"), "{err}");
    }

    #[test]
    fn pick_name_follows_the_terminal_only_for_auto() {
        let pick = |theme, light, dark, is_dark| pick_name(theme, light, dark, || is_dark);
        assert_eq!(pick("auto", "l", "d", Some(false)), "l");
        assert_eq!(pick("auto", "l", "d", Some(true)), "d");
        assert_eq!(pick("auto", "l", "", Some(true)), "auto");
        assert_eq!(pick("auto", "l", "d", None), "auto");
        assert_eq!(pick("mine", "l", "d", Some(true)), "mine");
    }

    #[test]
    fn pick_name_does_not_query_the_terminal_when_unneeded() {
        let never = || -> Option<bool> { panic!("terminal queried") };
        assert_eq!(pick_name("auto", "", "", never), "auto");
        assert_eq!(pick_name("mine", "l", "d", never), "mine");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<IndexMap<String, ThemeDef>>("a = { blue = \"red\" }").is_err());
    }
}
