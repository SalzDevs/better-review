use std::sync::{OnceLock, RwLock};

use ratatui_core::style::{Color, Modifier, Style};

use crate::settings::ThemePreset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub base_bg: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub border_muted: Color,
    pub text_primary: Color,
    pub text_muted: Color,
    pub text_subtle: Color,
    pub accent: Color,
    pub accent_bright: Color,
    pub accent_dim: Color,
    pub code_add: Color,
    pub code_remove: Color,
    pub success: Color,
    pub danger: Color,
}

static CURRENT_PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

pub fn set_palette(palette: Palette) {
    let lock = CURRENT_PALETTE.get_or_init(|| RwLock::new(Palette::one_dark_pro()));
    if let Ok(mut current) = lock.write() {
        *current = palette;
    }
}

pub fn current_palette() -> Palette {
    CURRENT_PALETTE
        .get_or_init(|| RwLock::new(Palette::one_dark_pro()))
        .read()
        .map(|palette| *palette)
        .unwrap_or_else(|_| Palette::one_dark_pro())
}

impl Palette {
    pub fn from_theme(theme: ThemePreset) -> Self {
        match theme {
            ThemePreset::OneDarkPro => Self::one_dark_pro(),
            ThemePreset::Dracula => Self::dracula(),
            ThemePreset::TokyoNight => Self::tokyo_night(),
            ThemePreset::NightOwl => Self::night_owl(),
        }
    }

    pub fn one_dark_pro() -> Self {
        Self {
            base_bg: Color::Rgb(0, 0, 0),
            surface: Color::Rgb(10, 8, 18),
            surface_raised: Color::Rgb(21, 16, 39),
            border_muted: Color::Rgb(47, 47, 47),
            text_primary: Color::Rgb(205, 205, 205),
            text_muted: Color::Rgb(133, 133, 133),
            text_subtle: Color::Rgb(85, 85, 85),
            accent: Color::Rgb(105, 48, 199),
            accent_bright: Color::Rgb(221, 181, 248),
            accent_dim: Color::Rgb(58, 47, 102),
            code_add: Color::Rgb(154, 199, 165),
            code_remove: Color::Rgb(209, 148, 166),
            success: Color::Rgb(184, 184, 184),
            danger: Color::Rgb(147, 147, 147),
        }
    }

    pub fn dracula() -> Self {
        Self {
            base_bg: Color::Rgb(8, 8, 13),
            surface: Color::Rgb(23, 23, 33),
            surface_raised: Color::Rgb(35, 33, 49),
            border_muted: Color::Rgb(72, 72, 92),
            text_primary: Color::Rgb(232, 234, 247),
            text_muted: Color::Rgb(167, 172, 204),
            text_subtle: Color::Rgb(113, 118, 149),
            accent: Color::Rgb(189, 147, 249),
            accent_bright: Color::Rgb(218, 196, 255),
            accent_dim: Color::Rgb(78, 62, 108),
            code_add: Color::Rgb(141, 206, 170),
            code_remove: Color::Rgb(245, 160, 171),
            success: Color::Rgb(170, 220, 193),
            danger: Color::Rgb(222, 152, 173),
        }
    }

    pub fn tokyo_night() -> Self {
        Self {
            base_bg: Color::Rgb(4, 8, 15),
            surface: Color::Rgb(24, 30, 46),
            surface_raised: Color::Rgb(34, 41, 65),
            border_muted: Color::Rgb(70, 83, 117),
            text_primary: Color::Rgb(199, 206, 255),
            text_muted: Color::Rgb(154, 168, 206),
            text_subtle: Color::Rgb(101, 120, 168),
            accent: Color::Rgb(122, 162, 247),
            accent_bright: Color::Rgb(167, 197, 255),
            accent_dim: Color::Rgb(58, 79, 117),
            code_add: Color::Rgb(157, 216, 174),
            code_remove: Color::Rgb(231, 154, 165),
            success: Color::Rgb(161, 205, 229),
            danger: Color::Rgb(222, 145, 160),
        }
    }

    pub fn night_owl() -> Self {
        Self {
            base_bg: Color::Rgb(1, 4, 10),
            surface: Color::Rgb(9, 15, 26),
            surface_raised: Color::Rgb(15, 24, 41),
            border_muted: Color::Rgb(44, 62, 86),
            text_primary: Color::Rgb(214, 225, 237),
            text_muted: Color::Rgb(144, 169, 196),
            text_subtle: Color::Rgb(94, 123, 155),
            accent: Color::Rgb(130, 170, 255),
            accent_bright: Color::Rgb(179, 208, 255),
            accent_dim: Color::Rgb(48, 70, 104),
            code_add: Color::Rgb(146, 209, 183),
            code_remove: Color::Rgb(247, 139, 144),
            success: Color::Rgb(152, 210, 191),
            danger: Color::Rgb(225, 147, 160),
        }
    }
}

pub fn title() -> Style {
    let palette = current_palette();
    Style::default()
        .fg(palette.text_primary)
        .add_modifier(Modifier::BOLD)
}

pub fn accent_bold() -> Style {
    let palette = current_palette();
    Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD)
}

pub fn keybind() -> Style {
    let palette = current_palette();
    Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD)
}

pub fn soft_accent() -> Style {
    let palette = current_palette();
    Style::default().fg(palette.accent)
}

pub fn muted() -> Style {
    let palette = current_palette();
    Style::default().fg(palette.text_muted)
}

pub fn subtle() -> Style {
    let palette = current_palette();
    Style::default().fg(palette.text_subtle)
}

pub fn base_bg() -> Color {
    current_palette().base_bg
}

pub fn surface() -> Color {
    current_palette().surface
}

pub fn surface_raised() -> Color {
    current_palette().surface_raised
}

pub fn border_muted() -> Color {
    current_palette().border_muted
}

pub fn text_primary() -> Color {
    current_palette().text_primary
}

pub fn text_muted() -> Color {
    current_palette().text_muted
}

pub fn accent() -> Color {
    current_palette().accent
}

pub fn accent_bright_color() -> Color {
    current_palette().accent_bright
}

pub fn accent_dim() -> Color {
    current_palette().accent_dim
}

pub fn code_add() -> Color {
    current_palette().code_add
}

pub fn code_remove() -> Color {
    current_palette().code_remove
}

pub fn success() -> Color {
    current_palette().success
}

pub fn danger() -> Color {
    current_palette().danger
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bold(style: Style) {
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn palette_from_theme_selects_each_theme() {
        let one_dark = Palette::from_theme(ThemePreset::OneDarkPro);
        let dracula = Palette::from_theme(ThemePreset::Dracula);
        let tokyo = Palette::from_theme(ThemePreset::TokyoNight);
        let night = Palette::from_theme(ThemePreset::NightOwl);

        assert_ne!(one_dark.accent, dracula.accent);
        assert_ne!(dracula.accent, tokyo.accent);
        assert_ne!(tokyo.accent, night.accent);
    }

    #[test]
    fn title_style_matches_palette() {
        let palette = Palette::one_dark_pro();
        set_palette(palette);
        let style = title();
        assert_eq!(style.fg, Some(palette.text_primary));
        assert_bold(style);
    }

    #[test]
    fn accent_bold_style_matches_palette() {
        let palette = Palette::one_dark_pro();
        set_palette(palette);
        let style = accent_bold();
        assert_eq!(style.fg, Some(palette.accent));
        assert_bold(style);
    }

    #[test]
    fn keybind_style_matches_palette() {
        let palette = Palette::one_dark_pro();
        set_palette(palette);
        let style = keybind();
        assert_eq!(style.fg, Some(palette.accent));
        assert_bold(style);
    }

    #[test]
    fn soft_accent_style_matches_palette() {
        let palette = Palette::one_dark_pro();
        set_palette(palette);
        let style = soft_accent();
        assert_eq!(style.fg, Some(palette.accent));
        assert!(!style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn muted_style_matches_palette() {
        let palette = Palette::one_dark_pro();
        set_palette(palette);
        let style = muted();
        assert_eq!(style.fg, Some(palette.text_muted));
    }

    #[test]
    fn subtle_style_matches_palette() {
        let palette = Palette::one_dark_pro();
        set_palette(palette);
        let style = subtle();
        assert_eq!(style.fg, Some(palette.text_subtle));
    }

    #[test]
    fn code_diff_colors_preserve_readability_by_theme() {
        let one_dark = Palette::from_theme(ThemePreset::OneDarkPro);
        let dracula = Palette::from_theme(ThemePreset::Dracula);

        assert_eq!(one_dark.code_add, Color::Rgb(154, 199, 165));
        assert_eq!(one_dark.code_remove, Color::Rgb(209, 148, 166));
        assert_eq!(dracula.code_add, Color::Rgb(141, 206, 170));
        assert_eq!(dracula.code_remove, Color::Rgb(245, 160, 171));
    }
}
