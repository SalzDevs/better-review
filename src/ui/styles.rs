use ratatui::style::{Color, Modifier, Style};

pub const BASE_BG: Color = Color::Rgb(13, 17, 23);
pub const SURFACE: Color = Color::Rgb(18, 18, 18);
pub const SURFACE_RAISED: Color = Color::Rgb(28, 28, 28);
pub const BORDER_MUTED: Color = Color::Rgb(68, 68, 68);
pub const TEXT_PRIMARY: Color = Color::Rgb(245, 245, 245);
pub const TEXT_MUTED: Color = Color::Rgb(176, 176, 176);
pub const TEXT_SUBTLE: Color = Color::Rgb(112, 112, 112);
pub const ACCENT: Color = Color::Rgb(255, 255, 255);
pub const SUCCESS: Color = Color::Rgb(230, 230, 230);
pub const DANGER: Color = Color::Rgb(142, 142, 142);

pub fn title() -> Style {
    Style::default()
        .fg(TEXT_PRIMARY)
        .add_modifier(Modifier::BOLD)
}

pub fn muted() -> Style {
    Style::default().fg(TEXT_MUTED)
}

pub fn subtle() -> Style {
    Style::default().fg(TEXT_SUBTLE)
}
