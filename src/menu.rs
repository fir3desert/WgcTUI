//! Menú de Esc al estilo de runetui: la pantalla de detrás se oscurece (sin borrarse),
//! y encima aparece el banner en bloques con la animación de apertura (un barrido de luz
//! en diagonal seguido de una onda brillante), la versión y las palabras del menú.
//!
//! El banner usa colores de marca fijos: aquí, un degradado rojo.

use std::time::Duration;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::{App, MENU_ITEMS, theme::*};

type Rgb = (u8, u8, u8);

/// Degradado de marca del banner, de arriba a la izquierda a abajo a la derecha.
pub const BANNER_TOP: Rgb = (0xff, 0x5a, 0x5f);
pub const BANNER_BOTTOM: Rgb = (0xb0, 0x1c, 0x3c);
/// Color de las letras que el barrido todavía no ha alcanzado.
const UNLIT: Rgb = (70, 70, 84);
/// Color del borde brillante del barrido.
const SWEEP_LIGHT: Rgb = (255, 255, 255);

/// Duración del barrido de luz y de la onda que le sigue.
pub const SWEEP_MS: u64 = 900;
pub const WAVE_MS: u64 = 2600;
/// Duración total de la animación; después el banner queda estático.
pub const ANIMATION_MS: u64 = SWEEP_MS + WAVE_MS;
/// Ancho del borde brillante (fracción de la diagonal).
const SWEEP_EDGE: f64 = 0.25;
/// Una celda es el doble de alta que de ancha: cada fila cuenta como dos columnas
/// para que la diagonal salga a 45 grados.
const ROW_WEIGHT: f64 = 2.0;
/// Vueltas de la onda, bandas visibles a la vez y brillo de sus valles.
const WAVE_CYCLES: f64 = 2.0;
const WAVE_BANDS: f64 = 1.5;
const WAVE_TROUGH: f32 = 0.3;

/// Wgc TUI en bloques finos.
const BANNER: [&str; 11] = [
    " █████   ███   █████                       ███████████ █████  █████ █████ ",
    "░░███   ░███  ░░███                        ░█░░░███░░░█░░███  ░░███ ░░███ ",
    " ░███   ░███   ░███   ███████   ██████     ░   ░███  ░  ░███   ░███  ░███ ",
    " ░███   ░███   ░███  ███░░███  ███░░███        ░███     ░███   ░███  ░███ ",
    " ░░███  █████  ███  ░███ ░███ ░███ ░░░         ░███     ░███   ░███  ░███ ",
    "  ░░░█████░█████░   ░███ ░███ ░███  ███        ░███     ░███   ░███  ░███ ",
    "    ░░███ ░░███     ░░███████ ░░██████         █████    ░░████████   █████",
    "     ░░░   ░░░       ░░░░░███  ░░░░░░          ░░░░░      ░░░░░░░░   ░░░░░ ",
    "                     ███ ░███                                              ",
    "                    ░░██████                                               ",
    "                                                                           ",
];

/// Nombre completo del proyecto, bajo el banner del menú.
pub const PRODUCT: &str = "";
const CREDIT: &str = "Made By - fir3";
const MENU_HINT: &str = " ↑ / ↓ elegir · Enter abrir · Esc cerrar ";

fn lerp(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

/// Dónde va la animación en `elapsed`: cuánto avanzó el barrido (0.0..=1.0) y la onda como
/// `(fase, amplitud)`. `None` significa banner estático.
fn animation_state(elapsed: Option<Duration>) -> (f64, (f64, f64)) {
    let Some(elapsed) = elapsed else {
        return (1.0, (0.0, 0.0));
    };
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms >= ANIMATION_MS as f64 {
        (1.0, (0.0, 0.0))
    } else if ms <= SWEEP_MS as f64 {
        (ms / SWEEP_MS as f64, (0.0, 0.0))
    } else {
        let t = (ms - SWEEP_MS as f64) / WAVE_MS as f64;
        // Entra y sale suavemente para que el último fotograma sea exactamente el degradado final.
        let amplitude = ((t / 0.15).min(1.0) * ((1.0 - t) / 0.25).min(1.0)).max(0.0);
        (1.0, (t * WAVE_CYCLES, amplitude))
    }
}

fn banner_width() -> u16 {
    BANNER.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16
}

/// Color de marca de una celda del banner en el instante de la animación.
pub fn banner_cell_color(row: usize, col: usize, elapsed: Option<Duration>) -> Color {
    let rows = BANNER.len();
    let width = banner_width() as usize;
    let span = width as f64 + rows.saturating_sub(1) as f64 * ROW_WEIGHT;
    let d = ((col as f64 + row as f64 * ROW_WEIGHT) / span.max(1.0)).clamp(0.0, 1.0);
    let finale = lerp(BANNER_TOP, BANNER_BOTTOM, d as f32);

    let (reveal, (phase, amplitude)) = animation_state(elapsed);
    let lead = reveal * (1.0 + SWEEP_EDGE);
    let k = lead - d;
    let mut color = if k >= SWEEP_EDGE {
        finale
    } else if k >= 0.0 {
        lerp(SWEEP_LIGHT, finale, (k / SWEEP_EDGE) as f32) // borde brillante del barrido
    } else {
        UNLIT
    };
    if amplitude > 0.0 {
        // La cresta (el color del degradado) viaja hacia abajo a la derecha.
        let w = 0.5 + 0.5 * (std::f64::consts::TAU * (d * WAVE_BANDS - phase)).sin();
        let w = (w * w) as f32;
        let dark = lerp((0, 0, 0), color, WAVE_TROUGH);
        color = lerp(color, lerp(dark, color, w), amplitude as f32);
    }
    Color::Rgb(color.0, color.1, color.2)
}

/// Letras de 3 filas en doble trazo para las palabras del menú.
fn glyph(c: char) -> [&'static str; 3] {
    match c {
        'R' => ["╦═╗", "╠╦╝", "╩╚═"],
        'E' => ["╔═╗", "║╣ ", "╚═╝"],
        'S' => ["╔═╗", "╚═╗", "╚═╝"],
        'U' => ["╦ ╦", "║ ║", "╚═╝"],
        'M' => ["╔╦╗", "║║║", "╩ ╩"],
        'H' => ["╦ ╦", "╠═╣", "╩ ╩"],
        'L' => ["╦  ", "║  ", "╩═╝"],
        'P' => ["╔═╗", "╠═╝", "╩  "],
        'Q' => ["╔═╗ ", "║═╬╗", "╚═╝╚"],
        'I' => ["╦", "║", "╩"],
        'T' => ["╔╦╗", " ║ ", " ╩ "],
        _ => [" ", " ", " "],
    }
}

fn word_rows(word: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for c in word.chars() {
        for (row, part) in rows.iter_mut().zip(glyph(c)) {
            row.push_str(part);
        }
    }
    rows
}

// ------------------------------------------------------- fondo oscurecido

/// Cuánto de su color conserva el fondo mientras hay un menú abierto.
const DIM_KEEP: f32 = 0.42;

fn rgb_of(color: Color, fallback: Rgb) -> Rgb {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

fn darken(color: Color, fallback: Rgb) -> Color {
    let (r, g, b) = rgb_of(color, fallback);
    let scale = |v: u8| (f32::from(v) * DIM_KEEP) as u8;
    Color::Rgb(scale(r), scale(g), scale(b))
}

/// Oscurece todo lo ya dibujado, como btop detrás de su menú: los colores se mezclan
/// hacia el negro, así la interfaz sigue legible pero queda claramente al fondo.
fn dim_background(frame: &mut Frame, area: Rect) {
    let (fg, bg) = (rgb_of(FG, (214, 218, 228)), rgb_of(BG, (22, 24, 29)));
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                let (cfg, cbg) = (cell.fg, cell.bg);
                cell.set_fg(darken(cfg, fg));
                cell.set_bg(darken(cbg, bg));
            }
        }
    }
}

fn centered_x(area: Rect, width: u16) -> u16 {
    area.x + area.width.saturating_sub(width) / 2
}

/// Pone un carácter sin tocar los huecos: los espacios dejan ver lo que hay detrás.
fn put_cell(frame: &mut Frame, x: u16, y: u16, ch: char, style: Style) {
    let area = frame.area();
    if ch == ' ' || x >= area.right() || y >= area.bottom() {
        return;
    }
    if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
        cell.set_symbol(&ch.to_string());
        cell.modifier = style.add_modifier;
        if let Some(fg) = style.fg {
            cell.set_fg(fg);
        }
        if let Some(bg) = style.bg {
            cell.set_bg(bg);
        }
    }
}

fn put_text(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    for (i, ch) in text.chars().enumerate() {
        put_cell(frame, x.saturating_add(i as u16), y, ch, style);
    }
}

// --------------------------------------------------------------- menú

pub fn render_menu(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    dim_background(frame, area);

    let banner_w = banner_width();
    let needed_height = BANNER.len() as u16 + 3 + 4 * MENU_ITEMS.len() as u16;
    if area.width >= banner_w + 4 && area.height >= needed_height {
        render_big_menu(frame, area, app.menu_selected, banner_w, app.menu_animation());
    } else {
        render_compact_menu(frame, area, app.menu_selected);
    }
    draw_footer(frame, area);
}

/// El banner y la versión bajo su borde derecho, con colores de marca fijos.
/// Con `elapsed` se dibuja la animación en ese instante; `None` dibuja el banner terminado.
fn draw_banner(frame: &mut Frame, x: u16, top: u16, banner_w: u16, elapsed: Option<Duration>) {
    for (row, line) in BANNER.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let style = Style::new()
                .fg(banner_cell_color(row, col, elapsed))
                .add_modifier(Modifier::BOLD);
            put_cell(frame, x + col as u16, top + row as u16, ch, style);
        }
    }
    // La versión aparece cuando el barrido ya ha iluminado todo el logo.
    let sweep_done = elapsed.is_none_or(|t| t.as_millis() as u64 >= SWEEP_MS);
    if sweep_done {
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        let vw = version.chars().count() as u16;
        put_text(
            frame,
            x + banner_w.saturating_sub(vw),
            top + BANNER.len() as u16,
            &version,
            Style::new().fg(FG_MUTED),
        );
    }
}

fn render_big_menu(
    frame: &mut Frame,
    area: Rect,
    selected: usize,
    banner_w: u16,
    elapsed: Option<Duration>,
) {
    // Banner, versión, hueco y luego las palabras de 3 filas con 1 fila entre ellas.
    let total = BANNER.len() as u16 + 3 + 4 * MENU_ITEMS.len() as u16;
    let top = area.y + area.height.saturating_sub(total) / 2;
    draw_banner(frame, centered_x(area, banner_w), top, banner_w, elapsed);

    let pw = PRODUCT.chars().count() as u16;
    if pw <= area.width {
        put_text(
            frame,
            centered_x(area, pw),
            top + BANNER.len() as u16 + 1,
            PRODUCT,
            Style::new().fg(FG_MUTED),
        );
    }

    let mut y = top + BANNER.len() as u16 + 3;
    for (index, word) in MENU_ITEMS.iter().enumerate() {
        let rows = word_rows(word);
        let width = rows[0].chars().count() as u16;
        let x = centered_x(area, width);
        let style = if index == selected {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(FG_DIM)
        };
        for (offset, row) in rows.iter().enumerate() {
            put_text(frame, x, y + offset as u16, row, style);
        }
        y += 4;
    }
}

fn render_compact_menu(frame: &mut Frame, area: Rect, selected: usize) {
    let width = 26u16.min(area.width);
    let height = (MENU_ITEMS.len() as u16 + 4).min(area.height);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed("WGC TUI");
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    for (index, label) in MENU_ITEMS.iter().enumerate() {
        let row = inner.y + 1 + index as u16;
        if row >= inner.y + inner.height {
            break;
        }
        let is_selected = index == selected;
        let style = if is_selected {
            Style::new().fg(ACCENT).bg(BG_SEL).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(FG_DIM)
        };
        let text = format!(
            "{} {label:<w$}",
            if is_selected { "❯" } else { " " },
            w = (inner.width as usize).saturating_sub(2)
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))),
            Rect::new(inner.x, row, inner.width, 1),
        );
    }
}

fn framed(title: &str) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(BG))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

/// Última fila: se limpia (la barra de estado de detrás no debe mezclarse) y lleva
/// la ayuda de teclas centrada y el crédito a la derecha.
fn draw_footer(frame: &mut Frame, area: Rect) {
    if area.height < 2 {
        return;
    }
    let y = area.bottom() - 1;
    {
        let buffer = frame.buffer_mut();
        for x in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol(" ");
            }
        }
    }
    let hint_w = MENU_HINT.chars().count() as u16;
    if hint_w <= area.width {
        let hx = centered_x(area, hint_w);
        put_text(frame, hx, y, MENU_HINT, Style::new().fg(FG_MUTED));

        let credit_w = CREDIT.chars().count() as u16;
        let cx = area.right().saturating_sub(credit_w + 1);
        if credit_w + 2 <= area.width && cx > hx + hint_w {
            put_text(frame, cx, y, CREDIT, Style::new().fg(FG_MUTED));
        }
    }
}

// --------------------------------------------------------------- ayuda

pub fn render_help(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    dim_background(frame, area);

    const KEYS: [(&str, &str); 7] = [
        ("↑ ↓  ·  j k", "Moverte por la lista de túneles"),
        ("g  ·  G", "Ir al primero / al último"),
        ("Enter  ·  Espacio", "Activar o desactivar el túnel"),
        ("PgUp  ·  PgDn", "Desplazar el panel de detalle"),
        ("r", "Refrescar ahora"),
        ("Esc", "Abrir o cerrar el menú"),
        ("q  ·  Ctrl+C", "Salir (los túneles quedan como estén)"),
    ];

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(bold(PRODUCT, ACCENT)),
        Line::from(sp(
            format!("v{} · {}", env!("CARGO_PKG_VERSION"), CREDIT),
            FG_MUTED,
        )),
        Line::from(""),
    ];
    for (key, desc) in KEYS.iter() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            bold(format!("{key:<20}"), FG),
            sp(*desc, FG_DIM),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        sp("Puedes tener varios túneles activos a la vez.", FG_MUTED),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        sp("Pulsa cualquier tecla para volver.", FG_MUTED),
    ]));

    let width = 66u16.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed("AYUDA");
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}
