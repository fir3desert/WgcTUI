//! Color palette and small styling helpers.
//!
//! Dark base, like runetui's default theme, with WireGuard red as the accent color.

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

pub const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// Transparent background: use the terminal's own background so the interface fills the
/// window without differently colored edges (including pixels left over when the window size
/// is not an exact multiple of the cell size).
pub const BG: Color = Color::Reset;
pub const BG_ALT: Color = Color::Reset;
/// Dark text on colored badges.
pub const BADGE_FG: Color = rgb(0x0b0c10);
pub const BG_SEL: Color = rgb(0x262a33);
pub const FG: Color = rgb(0xd6dae4);
pub const FG_DIM: Color = rgb(0xa0a8b7);
pub const FG_MUTED: Color = rgb(0x69707e);
pub const BORDER: Color = rgb(0x2c313a);

pub const ACCENT: Color = rgb(0xf2556a);
pub const GREEN: Color = rgb(0x8bd49c);
pub const YELLOW: Color = rgb(0xe5c07b);
/// GitHub star count in the header.
pub const STAR: Color = rgb(0xf5c24b);
pub const RED: Color = rgb(0xe06c75);
/// Download (↓) and upload (↑).
pub const DOWN: Color = rgb(0x56b6c2);
pub const UP: Color = rgb(0x6cb6ff);

pub fn sp(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(color))
}

pub fn bold(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )
}

pub fn italic(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(color).add_modifier(Modifier::ITALIC),
    )
}

/// Colored-background badge, like the ones in runetui's header and status bar.
pub fn badge(text: impl Into<String>, bg: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(BADGE_FG).bg(bg).add_modifier(Modifier::BOLD),
    )
}
