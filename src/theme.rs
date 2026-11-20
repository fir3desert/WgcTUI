//! Paleta y pequeños helpers de estilo.
//!
//! Base oscura igual que el tema por defecto de runetui, con el rojo de
//! WireGuard como color de acento.

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

pub const BG: Color = rgb(0x16181d);
pub const BG_ALT: Color = rgb(0x101217);
pub const BG_SEL: Color = rgb(0x262a33);
pub const FG: Color = rgb(0xd6dae4);
pub const FG_DIM: Color = rgb(0xa0a8b7);
pub const FG_MUTED: Color = rgb(0x69707e);
pub const BORDER: Color = rgb(0x2c313a);

pub const ACCENT: Color = rgb(0xf2556a);
pub const GREEN: Color = rgb(0x8bd49c);
pub const YELLOW: Color = rgb(0xe5c07b);
/// Contador de estrellas de GitHub de la cabecera.
pub const STAR: Color = rgb(0xf5c24b);
pub const RED: Color = rgb(0xe06c75);
/// Bajada (↓) y subida (↑).
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

/// Etiqueta con fondo de color, como las de la cabecera y barra de estado de runetui.
pub fn badge(text: impl Into<String>, bg: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(BG).bg(bg).add_modifier(Modifier::BOLD),
    )
}
