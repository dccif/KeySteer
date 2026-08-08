//! Theme palette, following neru's `[theme]` model.
//!
//! Five base colors per appearance; every component default is derived from
//! them. Explicit component colors in the config override the derivation.

use serde::{Deserialize, Serialize};

use crate::api::backend::Appearance;
use crate::api::overlay::Color;
pub use crate::api::theme::Palette;

/// A color that may differ between light and dark appearance.
///
/// Accepts either a bare string or `{ light = "...", dark = "..." }`, matching
/// neru's documented forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ThemedColor {
    Both(String),
    PerAppearance { light: String, dark: String },
}

impl ThemedColor {
    /// Whether every configured appearance uses canonical `#RRGGBBAA`.
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Both(value) => Color::parse(value).is_some(),
            Self::PerAppearance { light, dark } => {
                Color::parse(light).is_some() && Color::parse(dark).is_some()
            }
        }
    }

    pub fn resolve(&self, appearance: Appearance) -> Option<Color> {
        let raw = match (self, appearance) {
            (Self::Both(s), _) => s,
            (Self::PerAppearance { light, .. }, Appearance::Light) => light,
            (Self::PerAppearance { dark, .. }, Appearance::Dark) => dark,
        };
        Color::parse(raw)
    }
}

/// The five base colors, as documented for `[theme.light]` / `[theme.dark]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeColors {
    /// Translucent fills, badges, indicator backgrounds.
    pub surface: String,
    /// Borders, lines, primary chrome.
    pub accent: String,
    /// Active/emphasis states and highlights.
    pub accent_alt: String,
    /// Foreground on `accent_alt` surfaces.
    pub on_accent_alt: String,
    /// Foreground on `surface` backgrounds.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    #[serde(default = "Theme::default_light")]
    pub light: ThemeColors,
    #[serde(default = "Theme::default_dark")]
    pub dark: ThemeColors,
}

impl Theme {
    fn default_light() -> ThemeColors {
        ThemeColors {
            surface: "#EEF2FFFF".into(),
            accent: "#465FBCFF".into(),
            accent_alt: "#0B2377FF".into(),
            on_accent_alt: "#F8FAFFFF".into(),
            text: "#17327AFF".into(),
        }
    }

    fn default_dark() -> ThemeColors {
        ThemeColors {
            surface: "#0A1338FF".into(),
            accent: "#6E82D6FF".into(),
            accent_alt: "#8FA2F0FF".into(),
            on_accent_alt: "#081022FF".into(),
            text: "#E8EEFFFF".into(),
        }
    }

    /// Resolve to concrete colors for one appearance.
    pub fn palette(&self, appearance: Appearance) -> Palette {
        let raw = match appearance {
            Appearance::Light => &self.light,
            Appearance::Dark => &self.dark,
        };
        let fallback = match appearance {
            Appearance::Light => Self::default_light(),
            Appearance::Dark => Self::default_dark(),
        };
        let pick = |value: &str, default: &str| {
            Color::parse(value)
                .or_else(|| Color::parse(default))
                .unwrap_or(Color::rgb(0x80, 0x80, 0x80))
        };
        Palette {
            appearance,
            surface: pick(&raw.surface, &fallback.surface),
            accent: pick(&raw.accent, &fallback.accent),
            accent_alt: pick(&raw.accent_alt, &fallback.accent_alt),
            on_accent_alt: pick(&raw.on_accent_alt, &fallback.on_accent_alt),
            text: pick(&raw.text, &fallback.text),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            light: Self::default_light(),
            dark: Self::default_dark(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_resolves_documented_defaults() {
        let dark = Theme::default().palette(Appearance::Dark);
        assert_eq!(dark.surface, Color::rgb(0x0A, 0x13, 0x38));
        assert_eq!(dark.accent, Color::rgb(0x6E, 0x82, 0xD6));

        let light = Theme::default().palette(Appearance::Light);
        assert_eq!(light.text, Color::rgb(0x17, 0x32, 0x7A));
    }

    #[test]
    fn derived_shades_match_alpha_reference() {
        let p = Theme::default().palette(Appearance::Dark);
        assert_eq!(p.surface_label().a, 0xF2);
        assert_eq!(p.surface_cell().a, 0xB3);
        assert_eq!(p.accent_border().a, 0x99);
        assert_eq!(p.highlight().a, 0x4D);
    }

    #[test]
    fn invalid_color_falls_back_instead_of_panicking() {
        let theme = Theme {
            dark: ThemeColors {
                surface: "not-a-color".into(),
                ..Theme::default().dark
            },
            ..Theme::default()
        };
        assert_eq!(
            theme.palette(Appearance::Dark).surface,
            Color::rgb(0x0A, 0x13, 0x38)
        );
    }

    #[test]
    fn themed_color_accepts_both_toml_forms() {
        #[derive(Deserialize)]
        struct Holder {
            c: ThemedColor,
        }
        let bare: Holder = toml::from_str(r##"c = "#FF0000FF""##).unwrap();
        assert_eq!(
            bare.c.resolve(Appearance::Dark),
            Some(Color::rgb(255, 0, 0))
        );

        let split: Holder =
            toml::from_str(r##"c = { light = "#FF0000FF", dark = "#00FF00FF" }"##).unwrap();
        assert_eq!(
            split.c.resolve(Appearance::Dark),
            Some(Color::rgb(0, 255, 0))
        );
    }
}
