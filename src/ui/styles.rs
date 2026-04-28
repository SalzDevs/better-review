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
    pub syntax_keyword: Color,
    pub syntax_function: Color,
    pub syntax_string: Color,
    pub syntax_variable: Color,
    pub syntax_comment: Color,
    pub code_add: Color,
    pub code_remove: Color,
    pub code_add_bg: Color,
    pub code_remove_bg: Color,
    pub success: Color,
    pub danger: Color,
}

static CURRENT_PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

pub fn set_palette(palette: Palette) {
    let lock = CURRENT_PALETTE.get_or_init(|| RwLock::new(Palette::default_theme()));
    if let Ok(mut current) = lock.write() {
        *current = palette;
    }
}

pub fn current_palette() -> Palette {
    CURRENT_PALETTE
        .get_or_init(|| RwLock::new(Palette::default_theme()))
        .read()
        .map(|palette| *palette)
        .unwrap_or_else(|_| Palette::default_theme())
}

impl Palette {
    pub fn from_theme(theme: ThemePreset) -> Self {
        match theme {
            ThemePreset::Default => Self::default_theme(),
            ThemePreset::OneDarkPro => Self::one_dark_pro(),
            ThemePreset::Dracula => Self::dracula(),
            ThemePreset::TokyoNight => Self::tokyo_night(),
            ThemePreset::NightOwl => Self::night_owl(),
        }
    }

    pub fn default_theme() -> Self {
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
            syntax_keyword: Color::Rgb(221, 181, 248),
            syntax_function: Color::Rgb(154, 199, 238),
            syntax_string: Color::Rgb(154, 199, 165),
            syntax_variable: Color::Rgb(205, 205, 205),
            syntax_comment: Color::Rgb(85, 85, 85),
            code_add: Color::Rgb(154, 199, 165),
            code_remove: Color::Rgb(209, 148, 166),
            code_add_bg: Color::Rgb(22, 43, 31),
            code_remove_bg: Color::Rgb(48, 24, 35),
            success: Color::Rgb(184, 184, 184),
            danger: Color::Rgb(147, 147, 147),
        }
    }

    pub fn one_dark_pro() -> Self {
        Self {
            base_bg: Color::Rgb(40, 44, 52),
            surface: Color::Rgb(33, 37, 43),
            surface_raised: Color::Rgb(62, 68, 81),
            border_muted: Color::Rgb(92, 99, 112),
            text_primary: Color::Rgb(171, 178, 191),
            text_muted: Color::Rgb(130, 137, 151),
            text_subtle: Color::Rgb(92, 99, 112),
            accent: Color::Rgb(198, 120, 221),
            accent_bright: Color::Rgb(97, 175, 239),
            accent_dim: Color::Rgb(62, 68, 81),
            syntax_keyword: Color::Rgb(198, 120, 221),
            syntax_function: Color::Rgb(97, 175, 239),
            syntax_string: Color::Rgb(152, 195, 121),
            syntax_variable: Color::Rgb(224, 108, 117),
            syntax_comment: Color::Rgb(92, 99, 112),
            code_add: Color::Rgb(152, 195, 121),
            code_remove: Color::Rgb(224, 108, 117),
            code_add_bg: Color::Rgb(42, 58, 43),
            code_remove_bg: Color::Rgb(68, 45, 52),
            success: Color::Rgb(152, 195, 121),
            danger: Color::Rgb(224, 108, 117),
        }
    }

    pub fn dracula() -> Self {
        Self {
            base_bg: Color::Rgb(40, 42, 54),
            surface: Color::Rgb(33, 34, 44),
            surface_raised: Color::Rgb(68, 71, 90),
            border_muted: Color::Rgb(98, 114, 164),
            text_primary: Color::Rgb(248, 248, 242),
            text_muted: Color::Rgb(189, 193, 205),
            text_subtle: Color::Rgb(98, 114, 164),
            accent: Color::Rgb(255, 121, 198),
            accent_bright: Color::Rgb(189, 147, 249),
            accent_dim: Color::Rgb(68, 71, 90),
            syntax_keyword: Color::Rgb(255, 121, 198),
            syntax_function: Color::Rgb(80, 250, 123),
            syntax_string: Color::Rgb(241, 250, 140),
            syntax_variable: Color::Rgb(248, 248, 242),
            syntax_comment: Color::Rgb(98, 114, 164),
            code_add: Color::Rgb(80, 250, 123),
            code_remove: Color::Rgb(255, 85, 85),
            code_add_bg: Color::Rgb(38, 78, 58),
            code_remove_bg: Color::Rgb(81, 42, 55),
            success: Color::Rgb(80, 250, 123),
            danger: Color::Rgb(255, 85, 85),
        }
    }

    pub fn tokyo_night() -> Self {
        Self {
            base_bg: Color::Rgb(26, 27, 38),
            surface: Color::Rgb(22, 22, 31),
            surface_raised: Color::Rgb(36, 40, 59),
            border_muted: Color::Rgb(86, 95, 137),
            text_primary: Color::Rgb(169, 177, 214),
            text_muted: Color::Rgb(128, 138, 180),
            text_subtle: Color::Rgb(86, 95, 137),
            accent: Color::Rgb(187, 154, 247),
            accent_bright: Color::Rgb(167, 197, 255),
            accent_dim: Color::Rgb(46, 51, 75),
            syntax_keyword: Color::Rgb(187, 154, 247),
            syntax_function: Color::Rgb(122, 162, 247),
            syntax_string: Color::Rgb(158, 206, 106),
            syntax_variable: Color::Rgb(224, 175, 104),
            syntax_comment: Color::Rgb(86, 95, 137),
            code_add: Color::Rgb(158, 206, 106),
            code_remove: Color::Rgb(255, 158, 100),
            code_add_bg: Color::Rgb(40, 66, 49),
            code_remove_bg: Color::Rgb(72, 48, 42),
            success: Color::Rgb(158, 206, 106),
            danger: Color::Rgb(255, 158, 100),
        }
    }

    pub fn night_owl() -> Self {
        Self {
            base_bg: Color::Rgb(1, 4, 10),
            surface: Color::Rgb(3, 23, 40),
            surface_raised: Color::Rgb(10, 38, 63),
            border_muted: Color::Rgb(99, 119, 119),
            text_primary: Color::Rgb(214, 225, 237),
            text_muted: Color::Rgb(143, 164, 176),
            text_subtle: Color::Rgb(99, 119, 119),
            accent: Color::Rgb(199, 146, 234),
            accent_bright: Color::Rgb(130, 170, 255),
            accent_dim: Color::Rgb(35, 60, 84),
            syntax_keyword: Color::Rgb(199, 146, 234),
            syntax_function: Color::Rgb(130, 170, 255),
            syntax_string: Color::Rgb(173, 219, 103),
            syntax_variable: Color::Rgb(214, 225, 237),
            syntax_comment: Color::Rgb(99, 119, 119),
            code_add: Color::Rgb(173, 219, 103),
            code_remove: Color::Rgb(247, 140, 108),
            code_add_bg: Color::Rgb(23, 66, 53),
            code_remove_bg: Color::Rgb(70, 45, 50),
            success: Color::Rgb(173, 219, 103),
            danger: Color::Rgb(247, 140, 108),
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

pub fn code_add_bg() -> Color {
    current_palette().code_add_bg
}

pub fn code_remove_bg() -> Color {
    current_palette().code_remove_bg
}

pub fn syntax_keyword() -> Color {
    current_palette().syntax_keyword
}

pub fn syntax_function() -> Color {
    current_palette().syntax_function
}

pub fn syntax_string() -> Color {
    current_palette().syntax_string
}

pub fn syntax_variable() -> Color {
    current_palette().syntax_variable
}

pub fn syntax_comment() -> Color {
    current_palette().syntax_comment
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
        let default = Palette::from_theme(ThemePreset::Default);
        let one_dark = Palette::from_theme(ThemePreset::OneDarkPro);
        let dracula = Palette::from_theme(ThemePreset::Dracula);
        let tokyo = Palette::from_theme(ThemePreset::TokyoNight);
        let night = Palette::from_theme(ThemePreset::NightOwl);

        assert_ne!(default.base_bg, one_dark.base_bg);
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
        let tokyo = Palette::from_theme(ThemePreset::TokyoNight);
        let night_owl = Palette::from_theme(ThemePreset::NightOwl);

        assert_eq!(one_dark.code_add, Color::Rgb(152, 195, 121));
        assert_eq!(one_dark.code_remove, Color::Rgb(224, 108, 117));
        assert_eq!(dracula.code_add, Color::Rgb(80, 250, 123));
        assert_eq!(dracula.code_remove, Color::Rgb(255, 85, 85));
        assert_eq!(tokyo.code_add, Color::Rgb(158, 206, 106));
        assert_eq!(tokyo.code_remove, Color::Rgb(255, 158, 100));
        assert_eq!(night_owl.code_add, Color::Rgb(173, 219, 103));
        assert_eq!(night_owl.code_remove, Color::Rgb(247, 140, 108));
    }

    #[test]
    fn marked_diff_backgrounds_are_theme_specific() {
        let default = Palette::from_theme(ThemePreset::Default);
        let one_dark = Palette::from_theme(ThemePreset::OneDarkPro);
        let dracula = Palette::from_theme(ThemePreset::Dracula);

        assert_ne!(default.code_add_bg, one_dark.code_add_bg);
        assert_ne!(one_dark.code_add_bg, dracula.code_add_bg);
        assert_ne!(dracula.code_add_bg, dracula.code_remove_bg);
    }

    #[test]
    fn syntax_palette_matches_theme_specs() {
        let one_dark = Palette::from_theme(ThemePreset::OneDarkPro);
        let dracula = Palette::from_theme(ThemePreset::Dracula);
        let tokyo = Palette::from_theme(ThemePreset::TokyoNight);
        let night_owl = Palette::from_theme(ThemePreset::NightOwl);

        assert_eq!(one_dark.syntax_keyword, Color::Rgb(198, 120, 221));
        assert_eq!(one_dark.syntax_function, Color::Rgb(97, 175, 239));
        assert_eq!(dracula.syntax_keyword, Color::Rgb(255, 121, 198));
        assert_eq!(dracula.syntax_string, Color::Rgb(241, 250, 140));
        assert_eq!(tokyo.syntax_variable, Color::Rgb(224, 175, 104));
        assert_eq!(night_owl.syntax_comment, Color::Rgb(99, 119, 119));
    }
}
