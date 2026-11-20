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

use crate::{App, MENU_ITEMS, config::Setting, theme::*};

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

/// Color de una celda según su posición en la diagonal (`d`, 0.0..=1.0), lo que ha avanzado
/// el barrido (`reveal`) y la onda `(fase, amplitud)`.
fn shade(d: f64, reveal: f64, wave: (f64, f64)) -> Color {
    let (phase, amplitude) = wave;
    let finale = lerp(BANNER_TOP, BANNER_BOTTOM, d as f32);
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

/// Posición en la diagonal (0.0..=1.0) de la celda `(row, col)` de un bloque `rows` x `width`.
fn diagonal(row: usize, col: usize, rows: usize, width: usize) -> f64 {
    let span = width as f64 + rows.saturating_sub(1) as f64 * ROW_WEIGHT;
    ((col as f64 + row as f64 * ROW_WEIGHT) / span.max(1.0)).clamp(0.0, 1.0)
}

/// Color de marca de una celda del banner en el instante de la animación.
pub fn banner_cell_color(row: usize, col: usize, elapsed: Option<Duration>) -> Color {
    let d = diagonal(row, col, BANNER.len(), banner_width() as usize);
    let (reveal, wave) = animation_state(elapsed);
    shade(d, reveal, wave)
}

// ------------------------------------------------------- texto de la cabecera

/// Texto de una sola línea de la esquina superior izquierda (con animación activada).
pub const HEADER_TEXT: &str = " WGC TUI ";
/// Etiqueta fija de siempre (con la animación desactivada).
pub const HEADER_STATIC: &str = " wgctui ";

/// Igual que la animación de Esc (barrido de luz y luego la onda), pero la onda no se
/// acaba nunca: entra suavemente y sigue girando mientras el programa esté abierto.
fn header_state(elapsed: Duration) -> (f64, (f64, f64)) {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms <= SWEEP_MS as f64 {
        (ms / SWEEP_MS as f64, (0.0, 0.0))
    } else {
        let t = (ms - SWEEP_MS as f64) / WAVE_MS as f64;
        let amplitude = (t / 0.15).min(1.0);
        (1.0, (t * WAVE_CYCLES, amplitude))
    }
}

/// ¿El texto de la cabecera todavía está en el barrido inicial? (para dibujar a más fotogramas)
pub fn header_sweeping(elapsed: Duration) -> bool {
    elapsed.as_millis() < u128::from(SWEEP_MS)
}

/// Ancho en columnas de lo que ocupa la cabecera a la izquierda en cada modo.
pub fn header_width(animated: bool) -> u16 {
    let text = if animated { HEADER_TEXT } else { HEADER_STATIC };
    text.chars().count() as u16
}

/// El rótulo de la esquina superior izquierda como spans de una línea.
/// `Some(elapsed)`: texto animado con el degradado rojo de la marca; `None` (animación
/// desactivada): la etiqueta `" wgctui "` con fondo ACCENT y texto oscuro, sin animar.
pub fn header_label(elapsed: Option<Duration>) -> Vec<Span<'static>> {
    let Some(elapsed) = elapsed else {
        return vec![badge(HEADER_STATIC, ACCENT)];
    };
    let (reveal, wave) = header_state(elapsed);
    let width = HEADER_TEXT.chars().count();
    HEADER_TEXT
        .chars()
        .enumerate()
        .map(|(col, ch)| {
            let style = Style::new()
                .fg(shade(diagonal(0, col, 1, width), reveal, wave))
                .bg(BG_ALT)
                .add_modifier(Modifier::BOLD);
            Span::styled(ch.to_string(), style)
        })
        .collect()
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
        'O' => ["╔═╗", "║ ║", "╚═╝"],
        'N' => ["╔╗╔", "║║║", "╝╚╝"],
        'W' => ["╦ ╦", "║║║", "╚╩╝"],
        'G' => ["╔═╗", "║ ╦", "╚═╝"],
        'C' => ["╔═╗", "║  ", "╚═╝"],
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
/// Duración de la animación de aparición del menú de Esc.
pub const APPEAR_MS: u64 = 240;

fn rgb_of(color: Color, fallback: Rgb) -> Rgb {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

fn darken(color: Color, fallback: Rgb, keep: f32) -> Color {
    let (r, g, b) = rgb_of(color, fallback);
    let scale = |v: u8| (f32::from(v) * keep) as u8;
    Color::Rgb(scale(r), scale(g), scale(b))
}

/// Cuánto ha aparecido el menú (0.0..=1.0) tras `elapsed`. `None` = ya está del todo
/// (animación del menú desactivada o terminada).
fn appear_amount(elapsed: Option<Duration>) -> f32 {
    let Some(elapsed) = elapsed else {
        return 1.0;
    };
    let t = (elapsed.as_secs_f32() * 1000.0 / APPEAR_MS as f32).clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) // frena al final
}

/// El color atenuado hacia el fondo oscurecido: con `appear = 1.0` no cambia nada.
fn fade(color: Color, appear: f32) -> Color {
    if appear >= 1.0 {
        return color;
    }
    let base = rgb_of(darken(BG, (22, 24, 29), DIM_KEEP), (9, 10, 12));
    let (r, g, b) = lerp(base, rgb_of(color, (214, 218, 228)), appear);
    Color::Rgb(r, g, b)
}

/// Oscurece todo lo ya dibujado, como btop detrás de su menú: los colores se mezclan
/// hacia el negro, así la interfaz sigue legible pero queda claramente al fondo.
/// Con `appear < 1.0` el oscurecido es parcial: así el menú "entra" suavemente.
fn dim_background(frame: &mut Frame, area: Rect, appear: f32) {
    let keep = 1.0 + (DIM_KEEP - 1.0) * appear.clamp(0.0, 1.0);
    let (fg, bg) = (rgb_of(FG, (214, 218, 228)), rgb_of(BG, (22, 24, 29)));
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                let (cfg, cbg) = (cell.fg, cell.bg);
                cell.set_fg(darken(cfg, fg, keep));
                cell.set_bg(darken(cbg, bg, keep));
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

/// Filas que necesita el menú grande: banner, versión, hueco y las palabras de 3 filas
/// con 1 fila entre ellas.
pub const fn big_menu_height() -> u16 {
    BANNER.len() as u16 + 3 + 4 * MENU_ITEMS.len() as u16
}

/// ¿Cabe el menú grande (con banner) en esta área? Si no, se usa el compacto.
pub fn fits_big_menu(area: Rect) -> bool {
    area.width >= banner_width() + 4 && area.height >= big_menu_height()
}

pub fn render_menu(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let appear = appear_amount(app.menu_appear());
    dim_background(frame, area, appear);

    if fits_big_menu(area) {
        render_big_menu(
            frame,
            area,
            app.menu_selected,
            banner_width(),
            app.banner_animation(),
            appear,
        );
    } else {
        render_compact_menu(frame, area, app.menu_selected, appear);
    }
    draw_footer(frame, area, appear);
}

/// El banner y la versión bajo su borde derecho, con colores de marca fijos.
/// Con `elapsed` se dibuja la animación en ese instante; `None` dibuja el banner terminado.
fn draw_banner(
    frame: &mut Frame,
    x: u16,
    top: u16,
    banner_w: u16,
    elapsed: Option<Duration>,
    appear: f32,
) {
    for (row, line) in BANNER.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let style = Style::new()
                .fg(fade(banner_cell_color(row, col, elapsed), appear))
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
            Style::new().fg(fade(FG_MUTED, appear)),
        );
    }
}

fn render_big_menu(
    frame: &mut Frame,
    area: Rect,
    selected: usize,
    banner_w: u16,
    elapsed: Option<Duration>,
    appear: f32,
) {
    // Banner, versión, hueco y luego las palabras de 3 filas con 1 fila entre ellas.
    let total = big_menu_height();
    let top = area.y + area.height.saturating_sub(total) / 2;
    draw_banner(frame, centered_x(area, banner_w), top, banner_w, elapsed, appear);

    let pw = PRODUCT.chars().count() as u16;
    if pw <= area.width {
        put_text(
            frame,
            centered_x(area, pw),
            top + BANNER.len() as u16 + 1,
            PRODUCT,
            Style::new().fg(fade(FG_MUTED, appear)),
        );
    }

    let mut y = top + BANNER.len() as u16 + 3;
    for (index, word) in MENU_ITEMS.iter().enumerate() {
        let rows = word_rows(word);
        let width = rows[0].chars().count() as u16;
        let x = centered_x(area, width);
        let style = if index == selected {
            Style::new().fg(fade(ACCENT, appear)).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(fade(FG_DIM, appear))
        };
        for (offset, row) in rows.iter().enumerate() {
            put_text(frame, x, y + offset as u16, row, style);
        }
        y += 4;
    }
}

fn render_compact_menu(frame: &mut Frame, area: Rect, selected: usize, appear: f32) {
    let width = 26u16.min(area.width);
    let height = (MENU_ITEMS.len() as u16 + 4).min(area.height);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed_at("WGC TUI", appear);
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
            Style::new()
                .fg(fade(ACCENT, appear))
                .bg(fade(BG_SEL, appear))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(fade(FG_DIM, appear))
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
    framed_at(title, 1.0)
}

/// Marco redondeado; con `appear < 1.0` sus colores aún están entrando desde el fondo.
fn framed_at(title: &str, appear: f32) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(fade(ACCENT, appear)))
        .style(Style::new().bg(fade(BG, appear)))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(fade(ACCENT, appear)).add_modifier(Modifier::BOLD),
        ))
}

/// Última fila: se limpia (la barra de estado de detrás no debe mezclarse) y lleva
/// la ayuda de teclas centrada y el crédito a la derecha.
fn draw_footer(frame: &mut Frame, area: Rect, appear: f32) {
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
        put_text(frame, hx, y, MENU_HINT, Style::new().fg(fade(FG_MUTED, appear)));

        let credit_w = CREDIT.chars().count() as u16;
        let cx = area.right().saturating_sub(credit_w + 1);
        if credit_w + 2 <= area.width && cx > hx + hint_w {
            put_text(frame, cx, y, CREDIT, Style::new().fg(fade(FG_MUTED, appear)));
        }
    }
}

// ------------------------------------------------------------- opciones

/// Nombre y descripción (dos líneas) de cada interruptor de la pantalla OPTIONS.
fn option_text(setting: Setting) -> (&'static str, [&'static str; 2]) {
    match setting {
        Setting::Header => (
            "Header animation",
            [
                "Texto «WGC TUI» animado de la esquina superior izquierda.",
                "Si se desactiva, vuelve a la etiqueta fija \" wgctui \".",
            ],
        ),
        Setting::Banner => (
            "Banner animation",
            [
                "Barrido de luz y onda del banner grande del menú de Esc.",
                "Si se desactiva, el banner sale ya terminado y estático.",
            ],
        ),
        Setting::Menu => (
            "Menu animation",
            [
                "Transición de aparición del propio menú de Esc.",
                "Si se desactiva, el menú aparece de golpe, sin transición.",
            ],
        ),
    }
}

/// Pantalla de opciones del menú de Esc: un interruptor por animación.
pub fn render_options(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    dim_background(frame, area, 1.0);

    let mut lines: Vec<Line<'static>> = vec![Line::from("")];
    for (index, setting) in Setting::ALL.into_iter().enumerate() {
        let selected = index == app.options_selected;
        let (title, description) = option_text(setting);
        let value = if app.config.get(setting) {
            badge(" ACTIVADA ", GREEN)
        } else {
            Span::styled(" DESACTIVADA ", Style::new().fg(FG_MUTED).bg(BORDER))
        };
        let mut row = Line::from(vec![
            sp(if selected { " ▍ " } else { "   " }, ACCENT),
            bold(format!("{title:<18}"), if selected { FG } else { FG_DIM }),
            value,
        ]);
        if selected {
            row = row.style(Style::new().bg(BG_SEL));
        }
        lines.push(row);
        for text in description {
            lines.push(Line::from(sp(format!("   {text}"), FG_MUTED)));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(sp(
        "   ↑ ↓ moverse · Enter · Espacio · ← → cambiar · Esc volver",
        FG_DIM,
    )));

    let width = 66u16.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(framed("OPCIONES")), rect);
}

// --------------------------------------------------------------- ayuda

pub fn render_help(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    dim_background(frame, area, 1.0);

    const KEYS: [(&str, &str); 9] = [
        ("↑ ↓  ·  j k", "Moverte por la lista de túneles"),
        ("g  ·  G", "Ir al primero / al último"),
        ("Enter  ·  Espacio", "Activar o desactivar el túnel"),
        ("/  ·  Ctrl+P", "Buscar túnel (búsqueda difusa)"),
        ("PgUp  ·  PgDn", "Desplazar el panel de detalle"),
        ("r", "Refrescar ahora"),
        ("Esc", "Abrir o cerrar el menú (OPTIONS · HELP · QUIT)"),
        ("En OPTIONS", "↑ ↓ elegir · Enter / Espacio / ← → cambiar"),
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
    for note in [
        "Puedes tener varios túneles activos a la vez.",
        "Animaciones (OPTIONS): cabecera, banner y menú, cada una por su",
        "cuenta; se guardan en /etc/wgctui.conf.",
        "Pulsa cualquier tecla para volver.",
    ] {
        lines.push(Line::from(vec![Span::raw("  "), sp(note, FG_MUTED)]));
    }

    let width = 74u16.min(area.width);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use crate::{Overlay, config::Config};

    fn app_with(config: Config) -> App {
        App::with_config(config)
    }

    fn all_off() -> Config {
        Config {
            header_animation: false,
            banner_animation: false,
            menu_animation: false,
        }
    }

    fn render(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| crate::ui::draw(app, f)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
    }

    fn screen(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| row_text(buf, y).trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Columna (en celdas) donde empieza `needle` en la fila `y`.
    fn col_of(buf: &Buffer, y: u16, needle: &str) -> u16 {
        let row = row_text(buf, y);
        let byte = row.find(needle).expect("texto no encontrado en la fila");
        row[..byte].chars().count() as u16
    }

    /// Fila donde está la primera línea de la palabra grande `word`.
    fn word_row(buf: &Buffer, word: &str) -> u16 {
        let needle = &word_rows(word)[0];
        (0..buf.area.height)
            .find(|&y| row_text(buf, y).contains(needle.as_str()))
            .unwrap_or_else(|| panic!("no está {word}"))
    }

    /// Los dos búferes son idénticos salvo la fila de la cabecera (lleva el reloj).
    fn same_below_header(a: &Buffer, b: &Buffer) -> bool {
        (1..a.area.height).all(|y| (0..a.area.width).all(|x| a[(x, y)] == b[(x, y)]))
    }

    fn at_secs(secs: u64) -> Instant {
        Instant::now().checked_sub(Duration::from_secs(secs)).unwrap()
    }

    // ---------------------------------------------------------------- cabecera

    #[test]
    fn header_is_always_one_row_with_the_list_right_below() {
        for (w, h) in [(120u16, 40u16), (80, 24), (60, 16), (40, 10)] {
            let mut app = app_with(Config::default());
            let buf = render(&mut app, w, h);
            let top = row_text(&buf, 0);
            let second = row_text(&buf, 1);
            assert!(top.contains("WGC TUI"), "{w}x{h}: {top:?}");
            assert!(!second.contains("WGC TUI"), "{w}x{h}: la cabecera no debe ocupar 2 filas");
            // La fila 1 ya es el cuerpo (borde del panel), no más cabecera.
            assert!(second.contains('┌') || second.contains('╭') || second.contains('│'), "{w}x{h}: {second:?}");
        }
    }

    #[test]
    fn animated_header_is_one_line_of_gradient_text() {
        let mut app = app_with(Config::default());
        app.header_start = at_secs(30); // ya en la onda infinita
        let buf = render(&mut app, 80, 24);
        let top = row_text(&buf, 0);
        assert!(top.starts_with(" WGC TUI "), "{top:?}");
        // Las letras llevan colores de la marca (rojos), no el fondo rojo de la etiqueta fija.
        let mut colors = Vec::new();
        for x in 1..8u16 {
            let cell = &buf[(x, 0)];
            if cell.symbol() != " " {
                assert_eq!(cell.bg, BG_ALT, "x={x}");
                colors.push(cell.fg);
            }
        }
        assert_eq!(colors.len(), 6); // W G C T U I
        assert!(colors.iter().all(|c| matches!(c, Color::Rgb(r, _, _) if *r > 40)));
    }

    #[test]
    fn the_wave_never_stops_so_frames_keep_changing() {
        let mut app = app_with(Config::default());
        let mut seen = std::collections::HashSet::new();
        for secs in [10u64, 11, 12, 13] {
            app.header_start = at_secs(secs) + Duration::from_millis(37 * secs);
            let buf = render(&mut app, 80, 24);
            let colors: Vec<Color> = (1..8u16).map(|x| buf[(x, 0)].fg).collect();
            seen.insert(format!("{colors:?}"));
        }
        assert!(seen.len() > 1, "la onda debería seguir moviéndose");
    }

    #[test]
    fn header_sweep_starts_unlit_and_ends_with_the_gradient() {
        let mut app = app_with(Config::default());
        app.header_start = Instant::now();
        let first = render(&mut app, 80, 24);
        assert_eq!(first[(1, 0)].fg, Color::Rgb(UNLIT.0, UNLIT.1, UNLIT.2));
        app.header_start = at_secs(2); // barrido terminado, onda en marcha
        let later = render(&mut app, 80, 24);
        assert_ne!(later[(1, 0)].fg, Color::Rgb(UNLIT.0, UNLIT.1, UNLIT.2));
    }

    #[test]
    fn disabled_header_is_the_old_static_red_label() {
        let mut app = app_with(Config { header_animation: false, ..Config::default() });
        let buf = render(&mut app, 80, 24);
        assert!(row_text(&buf, 0).starts_with(" wgctui "), "{:?}", row_text(&buf, 0));
        for x in 0..8u16 {
            let cell = &buf[(x, 0)];
            assert_eq!(cell.bg, ACCENT, "x={x}");
            assert_eq!(cell.fg, BG, "x={x}"); // texto oscuro
        }
        // Y no cambia con el tiempo.
        let again = render(&mut app, 80, 24);
        assert_eq!(row_text(&buf, 0), row_text(&again, 0));
        assert_eq!(buf[(1, 0)].fg, again[(1, 0)].fg);
    }

    #[test]
    fn header_label_has_the_expected_widths() {
        let animated: usize = header_label(Some(Duration::from_secs(5)))
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let fixed: usize = header_label(None).iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(animated as u16, header_width(true));
        assert_eq!(fixed as u16, header_width(false));
    }

    // ------------------------------------------------------------------- menú

    #[test]
    fn big_menu_height_needs_fewer_rows_than_with_four_entries() {
        assert_eq!(big_menu_height(), BANNER.len() as u16 + 3 + 4 * 3);
        assert_eq!(big_menu_height(), 26);
        let wide = banner_width() + 4;
        assert!(fits_big_menu(Rect::new(0, 0, wide, 26)));
        assert!(!fits_big_menu(Rect::new(0, 0, wide, 25)));
        assert!(!fits_big_menu(Rect::new(0, 0, wide - 1, 40)));
    }

    #[test]
    fn big_menu_shows_options_help_quit_and_no_resume() {
        let mut app = app_with(all_off());
        app.overlay = Overlay::Menu;
        let buf = render(&mut app, 100, 30);
        let text = screen(&buf);
        for word in ["OPTIONS", "HELP", "QUIT"] {
            let first_row = &word_rows(word)[0];
            assert!(text.contains(first_row.as_str()), "falta {word}");
        }
        assert!(!text.contains(word_rows("RESUME")[0].as_str()));
        // En orden: OPTIONS arriba, luego HELP, luego QUIT.
        let line_of = |w: &str| {
            let needle = word_rows(w)[0].clone();
            (0..buf.area.height).find(|&y| row_text(&buf, y).contains(&needle)).unwrap()
        };
        assert!(line_of("OPTIONS") < line_of("HELP") && line_of("HELP") < line_of("QUIT"));
    }

    #[test]
    fn big_menu_fits_in_the_new_smaller_minimum() {
        // 26 filas bastan ahora; antes hacían falta 30 para el menú grande.
        let mut app = app_with(all_off());
        app.overlay = Overlay::Menu;
        let text = screen(&render(&mut app, 100, 26));
        assert!(text.contains(word_rows("QUIT")[0].as_str()));
        assert!(text.contains("█████"), "se ve el banner");
        // Con 25 filas pasa al menú compacto.
        let compact = screen(&render(&mut app, 100, 25));
        assert!(compact.contains("OPTIONS") && compact.contains("HELP") && compact.contains("QUIT"));
        assert!(!compact.contains("RESUME"));
    }

    #[test]
    fn options_item_is_selected_by_default_in_both_menus() {
        // Grande: la palabra seleccionada va en ACCENT y las demás en gris.
        let mut app = app_with(all_off());
        app.overlay = Overlay::Menu;
        let buf = render(&mut app, 100, 30);
        let colour_of = |word: &str| {
            let y = word_row(&buf, word);
            buf[(col_of(&buf, y, &word_rows(word)[0]), y)].fg
        };
        assert_eq!(colour_of("OPTIONS"), ACCENT);
        assert_eq!(colour_of("HELP"), FG_DIM);
        assert_eq!(colour_of("QUIT"), FG_DIM);

        // Compacto: el cursor ❯ está en OPTIONS.
        let buf = render(&mut app, 100, 20);
        let y = (0..20u16).find(|&y| row_text(&buf, y).contains("❯ OPTIONS")).unwrap();
        assert!(row_text(&buf, y).contains("OPTIONS"));
        assert!(!screen(&buf).contains("❯ HELP") && !screen(&buf).contains("❯ QUIT"));
    }

    #[test]
    fn disabled_banner_is_already_finished_and_static() {
        let mut app = app_with(Config { banner_animation: false, ..all_off() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now());
        let a = render(&mut app, 100, 30);
        app.menu_opened = Some(at_secs(1));
        let b = render(&mut app, 100, 30);
        assert!(same_below_header(&a, &b));
        // La versión ya está visible bajo el banner.
        assert!(screen(&a).contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
        // Y el color de una celda del banner es el del degradado final, no el apagado.
        let (row, col) = (0usize, 2usize);
        let top = (30 - big_menu_height()) / 2;
        let x = (100 - banner_width()) / 2 + col as u16;
        assert_eq!(a[(x, top + row as u16)].fg, banner_cell_color(row, col, None));
    }

    #[test]
    fn enabled_banner_starts_unlit_and_has_no_version_yet() {
        let mut app = app_with(Config { menu_animation: false, ..Config::default() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now());
        let buf = render(&mut app, 100, 30);
        assert!(!screen(&buf).contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
        let top = (30 - big_menu_height()) / 2;
        let x = (100 - banner_width()) / 2 + 2;
        assert_eq!(buf[(x, top)].fg, Color::Rgb(UNLIT.0, UNLIT.1, UNLIT.2));
    }

    #[test]
    fn disabled_menu_animation_shows_the_menu_at_once() {
        let mut app = app_with(Config { menu_animation: false, ..all_off() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now()); // recién abierto
        let fresh = render(&mut app, 100, 30);
        app.menu_opened = Some(at_secs(10));
        let settled = render(&mut app, 100, 30);
        // Sin transición: el primer fotograma ya es el definitivo.
        assert!(same_below_header(&fresh, &settled));
    }

    #[test]
    fn enabled_menu_animation_fades_in_and_ends_in_the_final_look() {
        let mut app = app_with(Config { menu_animation: true, ..all_off() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now());
        let fresh = render(&mut app, 100, 30);
        app.menu_opened = Some(at_secs(10));
        let settled = render(&mut app, 100, 30);
        assert!(!same_below_header(&fresh, &settled), "al abrir, el menú aún no está del todo");

        // Mismo texto, solo cambian los colores.
        assert_eq!(
            (1..30u16).map(|y| row_text(&fresh, y)).collect::<Vec<_>>(),
            (1..30u16).map(|y| row_text(&settled, y)).collect::<Vec<_>>()
        );

        // La palabra OPTIONS (ACCENT al final) empieza casi invisible y acaba en ACCENT.
        let colour = |buf: &Buffer| {
            let y = word_row(buf, "OPTIONS");
            buf[(col_of(buf, y, &word_rows("OPTIONS")[0]), y)].fg
        };
        assert_eq!(colour(&settled), ACCENT);
        assert_ne!(colour(&fresh), ACCENT);
        // A mitad de camino queda entre los dos extremos.
        app.menu_opened = Some(Instant::now().checked_sub(Duration::from_millis(APPEAR_MS / 3)).unwrap());
        let mid = render(&mut app, 100, 30);
        assert_ne!(colour(&mid), colour(&fresh));
        assert_ne!(colour(&mid), ACCENT);
    }

    #[test]
    fn each_menu_animation_works_without_the_others() {
        // Solo banner: el banner arranca apagado y el menú no se atenúa (OPTIONS ya en ACCENT).
        let mut app = app_with(Config { banner_animation: true, ..all_off() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now());
        let buf = render(&mut app, 100, 30);
        let y = word_row(&buf, "OPTIONS");
        assert_eq!(buf[(col_of(&buf, y, &word_rows("OPTIONS")[0]), y)].fg, ACCENT);
        let top = (30 - big_menu_height()) / 2;
        let x = (100 - banner_width()) / 2 + 2;
        assert_eq!(buf[(x, top)].fg, Color::Rgb(UNLIT.0, UNLIT.1, UNLIT.2));

        // Solo transición del menú: el banner sale terminado (ya con su versión visible).
        let mut app = app_with(Config { menu_animation: true, ..all_off() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now());
        let buf = render(&mut app, 100, 30);
        assert!(screen(&buf).contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn appear_amount_goes_from_zero_to_one() {
        assert_eq!(appear_amount(None), 1.0);
        assert!(appear_amount(Some(Duration::ZERO)) < 0.01);
        let mid = appear_amount(Some(Duration::from_millis(APPEAR_MS / 2)));
        assert!(mid > 0.3 && mid < 1.0);
        assert_eq!(appear_amount(Some(Duration::from_millis(APPEAR_MS))), 1.0);
        assert_eq!(appear_amount(Some(Duration::from_secs(9))), 1.0);
    }

    #[test]
    fn fade_is_the_identity_when_fully_shown() {
        assert_eq!(fade(ACCENT, 1.0), ACCENT);
        assert_ne!(fade(ACCENT, 0.0), ACCENT);
    }

    // ---------------------------------------------------------------- opciones

    #[test]
    fn options_lists_three_switches_with_their_state() {
        let mut app = app_with(Config { banner_animation: false, ..Config::default() });
        app.overlay = Overlay::Options;
        let text = screen(&render(&mut app, 100, 30));
        for name in ["Header animation", "Banner animation", "Menu animation"] {
            assert!(text.contains(name), "falta {name}");
        }
        assert_eq!(text.matches(" ACTIVADA ").count(), 2);
        assert_eq!(text.matches("DESACTIVADA").count(), 1);
        let line_of = |needle: &str| text.lines().position(|l| l.contains(needle)).unwrap();
        assert!(line_of("Header animation") < line_of("Banner animation"));
        assert!(line_of("Banner animation") < line_of("Menu animation"));
        // El banner es el desactivado.
        assert!(text.lines().nth(line_of("Banner animation")).unwrap().contains("DESACTIVADA"));
        assert!(text.contains("etiqueta fija"));
        assert!(text.contains("de golpe"));
    }

    #[test]
    fn options_highlights_the_selected_switch() {
        let mut app = app_with(Config::default());
        app.overlay = Overlay::Options;
        app.options_selected = 1;
        let buf = render(&mut app, 100, 30);
        let y = (0..30u16).find(|&y| row_text(&buf, y).contains("Banner animation")).unwrap();
        assert!(row_text(&buf, y).contains('▍'));
        let other = (0..30u16).find(|&y| row_text(&buf, y).contains("Header animation")).unwrap();
        assert!(!row_text(&buf, other).contains('▍'));
        let x = (0..100u16).find(|&x| buf[(x, y)].symbol() == "B").unwrap();
        assert_eq!(buf[(x, y)].bg, BG_SEL);
    }

    #[test]
    fn options_fit_a_24_row_terminal() {
        let mut app = app_with(Config::default());
        app.overlay = Overlay::Options;
        let text = screen(&render(&mut app, 80, 24));
        assert!(text.contains("Menu animation") && text.contains("Esc volver"));
    }

    #[test]
    fn help_mentions_the_new_menu_and_the_settings() {
        let mut app = app_with(Config::default());
        app.overlay = Overlay::Help;
        let text = screen(&render(&mut app, 100, 30));
        assert!(text.contains("OPTIONS · HELP · QUIT"));
        assert!(text.contains("/etc/wgctui.conf"));
        assert!(!text.contains("RESUME"));
    }

    /// Vuelca las pantallas a texto: `cargo test dump_screens -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_screens() {
        let show = |title: &str, app: &mut App, w: u16, h: u16| {
            println!("\n===== {title} ({w}x{h}) =====\n{}", screen(&render(app, w, h)));
        };
        let mut app = app_with(Config::default());
        app.header_start = at_secs(30);
        show("Cabecera con animación activada", &mut app, 80, 6);
        let mut app = app_with(Config { header_animation: false, ..Config::default() });
        show("Cabecera con animación desactivada", &mut app, 80, 6);
        let mut app = app_with(all_off());
        app.overlay = Overlay::Menu;
        show("Menú de Esc grande (26 filas)", &mut app, 100, 26);
        show("Menú de Esc compacto", &mut app, 60, 14);
        let mut app = app_with(Config { banner_animation: false, ..Config::default() });
        app.overlay = Overlay::Options;
        app.options_selected = 1;
        show("OPTIONS", &mut app, 80, 24);
        app.overlay = Overlay::Help;
        show("HELP", &mut app, 90, 26);
    }
}
