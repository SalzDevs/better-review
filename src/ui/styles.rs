use ratatui_core::style::{Color, Modifier, Style};

pub const BASE_BG: Color = Color::Rgb(13, 17, 23);
pub const SURFACE: Color = Color::Rgb(18, 18, 18);
pub const SURFACE_RAISED: Color = Color::Rgb(28, 28, 28);
pub const BORDER_MUTED: Color = Color::Rgb(66, 73, 79);
pub const TEXT_PRIMARY: Color = Color::Rgb(245, 245, 245);
pub const TEXT_MUTED: Color = Color::Rgb(176, 176, 176);
pub const TEXT_SUBTLE: Color = Color::Rgb(112, 112, 112);
pub const ACCENT: Color = Color::Rgb(118, 152, 166);
pub const ACCENT_BRIGHT: Color = Color::Rgb(176, 205, 214);
pub const ACCENT_DIM: Color = Color::Rgb(85, 110, 120);
pub const SUCCESS: Color = Color::Rgb(137, 180, 156);
pub const DANGER: Color = Color::Rgb(180, 138, 145);

pub fn title() -> Style {
    Style::default()
        .fg(TEXT_PRIMARY)
        .add_modifier(Modifier::BOLD)
}

pub fn accent_bold() -> Style {
    Style::default()
        .fg(ACCENT_BRIGHT)
        .add_modifier(Modifier::BOLD)
}

pub fn keybind() -> Style {
    Style::default()
        .fg(ACCENT_BRIGHT)
        .add_modifier(Modifier::BOLD)
}

pub fn soft_accent() -> Style {
    Style::default().fg(ACCENT)
}

pub fn muted() -> Style {
    Style::default().fg(TEXT_MUTED)
}

pub fn subtle() -> Style {
    Style::default().fg(TEXT_SUBTLE)
}
