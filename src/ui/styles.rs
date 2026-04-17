use ratatui::style::{Color, Modifier, Style};

pub const BASE_BG: Color = Color::Rgb(13, 17, 23);
pub const SURFACE: Color = Color::Rgb(19, 26, 34);
pub const SURFACE_RAISED: Color = Color::Rgb(25, 35, 45);
pub const BORDER_MUTED: Color = Color::Rgb(38, 52, 67);
pub const TEXT_PRIMARY: Color = Color::Rgb(238, 243, 248);
pub const TEXT_MUTED: Color = Color::Rgb(154, 171, 184);
pub const TEXT_SUBTLE: Color = Color::Rgb(111, 130, 146);
pub const ACCENT: Color = Color::Rgb(242, 181, 68);
pub const SUCCESS: Color = Color::Rgb(89, 201, 165);
pub const DANGER: Color = Color::Rgb(239, 111, 108);

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
