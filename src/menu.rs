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
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    App, MENU_ITEMS, OPTION_TABS, OptionItem,
    config::{MAX_UPDATE_MS, MIN_UPDATE_MS},
    theme::*,
};

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
pub const HEADER_TEXT: &str = " WgcTUI ";
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
/// `keep` es la esquina derecha de la cabecera (contador y estrellas): se queda como está.
fn dim_background(frame: &mut Frame, area: Rect, appear: f32, keep: Option<Rect>) {
    let corner = keep;
    let keep = 1.0 + (DIM_KEEP - 1.0) * appear.clamp(0.0, 1.0);
    let (fg, bg) = (rgb_of(FG, (214, 218, 228)), rgb_of(BG, (22, 24, 29)));
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if corner.is_some_and(|rect| rect.contains((x, y).into())) {
                continue;
            }
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
    dim_background(frame, area, appear, app.header_keep);

    if fits_big_menu(area) {
        render_big_menu(
            frame,
            area,
            app.menu_selected,
            banner_width(),
            app.banner_animation(),
            appear,
            app.header_keep,
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
    keep: Option<Rect>,
) {
    for (row, line) in BANNER.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            // La esquina derecha de la cabecera (estrellas) queda por encima del banner.
            let (cx, cy) = (x + col as u16, top + row as u16);
            if keep.is_some_and(|rect| rect.contains((cx, cy).into())) {
                continue;
            }
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
    keep: Option<Rect>,
) {
    // Banner, versión, hueco y luego las palabras de 3 filas con 1 fila entre ellas.
    let total = big_menu_height();
    let top = area.y + area.height.saturating_sub(total) / 2;
    draw_banner(frame, centered_x(area, banner_w), top, banner_w, elapsed, appear, keep);

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

/// Lo que se muestra de un ajuste: nombre, valor (con su color) y descripción.
struct OptionText {
    label: &'static str,
    value: String,
    value_color: Color,
    description: String,
}

fn on_off(on: bool) -> (String, Color) {
    if on {
        ("ACTIVADA".to_string(), GREEN)
    } else {
        ("DESACTIVADA".to_string(), FG_MUTED)
    }
}

fn option_text(app: &App, item: OptionItem) -> OptionText {
    let animation = |label, description: &str| {
        let (value, value_color) = on_off(item.setting().is_some_and(|s| app.config.get(s)));
        OptionText { label, value, value_color, description: description.to_string() }
    };
    match item {
        OptionItem::UpdateInterval => OptionText {
            label: "Update interval",
            value: format!("{} ms", app.config.update_ms),
            value_color: YELLOW,
            description: format!(
                "Cada cuánto se leen los datos de WireGuard (estado, tráfico y gráfica). \
                 Por defecto 1000 ms; de {MIN_UPDATE_MS} a {MAX_UPDATE_MS}. Menos ms = datos \
                 más frescos y más CPU.\n\n\
                 + / − (o Enter y luego + / −): de 100 en 100 ms hasta 1000 y de 500 en 500 después."
            ),
        },
        OptionItem::GithubStars => {
            let (value, value_color) = on_off(app.config.show_stars);
            OptionText {
                label: "GitHub stars",
                value,
                value_color,
                description: format!(
                    "Muestra las estrellas de github.com/{} arriba a la derecha de la \
                     cabecera. Se consultan una vez por hora en segundo plano con curl y es la \
                     única petición de red de wgctui; DESACTIVADA las oculta y no hace ninguna.",
                    crate::stars::REPO
                ),
            }
        }
        OptionItem::HeaderAnimation => animation(
            "Header animation",
            "Texto «WGC TUI» animado de la esquina superior izquierda: barrido de luz y onda \
             infinita. DESACTIVADA: vuelve la etiqueta fija \" wgctui \", sin animar.",
        ),
        OptionItem::BannerAnimation => animation(
            "Banner animation",
            "Barrido de luz y onda del banner grande del menú de Esc. DESACTIVADA: el banner \
             se ve ya terminado y estático.",
        ),
        OptionItem::MenuAnimation => animation(
            "Menu animation",
            "Aparición del propio menú de Esc: el fondo se oscurece y el menú entra con un \
             fundido. DESACTIVADA: el menú aparece de golpe, sin transición.",
        ),
    }
}

/// Pantalla de opciones del menú de Esc, como la de runetui: pestañas arriba, los ajustes de
/// la pestaña a la izquierda y la descripción del seleccionado a la derecha.
pub fn render_options(frame: &mut Frame, area: Rect, app: &App) {
    if area.width < 30 || area.height < 8 {
        render_menu(frame, area, app);
        return;
    }
    dim_background(frame, area, 1.0, app.header_keep);

    let width = area.width.saturating_sub(4).min(84);
    let height = area.height.saturating_sub(2).min(18);
    let rect = Rect::new(
        centered_x(area, width),
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = framed("OPTIONS");
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    // Pestañas.
    let tab = app.options_tab.min(OPTION_TABS.len() - 1);
    let mut spans = vec![Span::raw(" ")];
    for (index, entry) in OPTION_TABS.iter().enumerate() {
        let style = if index == tab {
            Style::new().fg(BG).bg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(FG_DIM)
        };
        spans.push(Span::styled(format!(" {} ", entry.name), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height.saturating_sub(4));
    let list_width = (body.width * 40 / 100).max(32).min(body.width);
    let list = Rect::new(body.x, body.y, list_width, body.height);
    let detail = Rect::new(body.x + list_width, body.y, body.width - list_width, body.height);

    // Ajustes de la pestaña activa.
    let items = OPTION_TABS[tab].items;
    let selected = app.options_selected.min(items.len() - 1);
    for (index, item) in items.iter().enumerate() {
        let row = list.y + index as u16;
        if row >= list.y + list.height {
            break;
        }
        let mut text = option_text(app, *item);
        let is_selected = index == selected;
        if is_selected && app.adjusting && item.is_adjustable() {
            text.value = format!("◂ {} ▸", text.value);
        }
        let style = if is_selected {
            Style::new().fg(FG).bg(BG_SEL).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(FG_DIM)
        };
        let value_style = style.fg(text.value_color);
        let marker = if is_selected { "❯ " } else { "  " };
        let used = 2 + text.label.chars().count() + text.value.chars().count();
        let gap = (list.width as usize).saturating_sub(used + 1).max(1);
        let line = Line::from(vec![
            Span::styled(format!("{marker}{}", text.label), style),
            Span::styled(" ".repeat(gap), style),
            Span::styled(text.value, value_style),
            Span::styled(" ", style),
        ]);
        frame.render_widget(Paragraph::new(line).style(style), Rect::new(list.x, row, list.width, 1));
    }

    // Descripción del ajuste seleccionado.
    let text = option_text(app, items[selected]);
    let block = Block::new().borders(Borders::LEFT).border_style(Style::new().fg(BORDER));
    let text_area = block.inner(detail);
    frame.render_widget(block, detail);
    let mut lines = vec![
        Line::from(bold(text.label, ACCENT)),
        Line::default(),
    ];
    lines.extend(text.description.split('\n').map(|l| Line::from(sp(l.to_string(), FG_DIM))));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        Rect::new(
            text_area.x + 1,
            text_area.y,
            text_area.width.saturating_sub(1),
            text_area.height,
        ),
    );

    let (hint, color) = if app.adjusting {
        ("+ / − ajustar · Enter o Esc listo", YELLOW)
    } else {
        ("↑↓ mover · ←→ pestaña · Enter cambiar · + / − frecuencia · Esc volver", FG_MUTED)
    };
    let hint: String = hint.chars().take(inner.width.saturating_sub(2) as usize).collect();
    put_text(frame, inner.x + 1, inner.y + inner.height - 1, &hint, Style::new().fg(color));
}

// --------------------------------------------------------------- ayuda

pub fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    dim_background(frame, area, 1.0, app.header_keep);

    const KEYS: [(&str, &str); 10] = [
        ("↑ ↓  ·  j k", "Moverte por la lista de túneles"),
        ("g  ·  G", "Ir al primero / al último"),
        ("Enter  ·  Espacio", "Activar o desactivar el túnel"),
        ("/  ·  Ctrl+P", "Buscar túnel (búsqueda difusa)"),
        ("PgUp  ·  PgDn", "Desplazar el panel de detalle"),
        ("r", "Refrescar ahora"),
        ("Esc", "Abrir o cerrar el menú (OPTIONS · HELP · QUIT)"),
        ("En OPTIONS", "↑ ↓ mover · ← → pestaña · Enter cambiar"),
        ("+  ·  −", "Frecuencia de actualización (en OPTIONS)"),
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
        "★ N: estrellas de GitHub del repo (se ocultan en OPTIONS → General).",
        "Animaciones (OPTIONS): cabecera, banner y menú, cada una por su",
        "cuenta. Ajustes y frecuencia (100–20000 ms) en /etc/wgctui.conf.",
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
            ..Config::default()
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
            assert!(top.contains(HEADER_TEXT.trim()), "{w}x{h}: {top:?}");
            assert!(!second.contains(HEADER_TEXT.trim()), "{w}x{h}: la cabecera no debe ocupar 2 filas");
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
        assert!(top.starts_with(HEADER_TEXT), "{top:?}");
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

    fn options_app(config: Config, tab: usize, selected: usize) -> App {
        let mut app = app_with(config);
        app.overlay = Overlay::Options;
        app.options_tab = tab;
        app.options_selected = selected;
        app
    }

    #[test]
    fn options_has_tabs_on_top_with_the_active_one_highlighted() {
        for tab in 0..2usize {
            let mut app = options_app(Config::default(), tab, 0);
            let buf = render(&mut app, 100, 30);
            let y = (0..30u16).find(|&y| row_text(&buf, y).contains("General")).unwrap();
            let row = row_text(&buf, y);
            assert!(row.contains("General") && row.contains("Animaciones"), "{row:?}");
            // Solo la pestaña activa lleva el fondo de acento.
            let bg_at = |name: &str| buf[(col_of(&buf, y, name), y)].bg;
            let names = ["General", "Animaciones"];
            for (i, name) in names.iter().enumerate() {
                assert_eq!(bg_at(name) == ACCENT, i == tab, "pestaña {name} (activa: {tab})");
            }
        }
    }

    #[test]
    fn general_tab_shows_the_update_interval_with_its_description() {
        let mut app = options_app(Config::default(), 0, 0);
        let text = screen(&render(&mut app, 100, 30));
        assert!(text.contains("Update interval") && text.contains("1000 ms"), "{text}");
        assert!(text.contains("20000"));
        assert!(!text.contains("Header animation"), "las animaciones están en su pestaña");
    }

    // ---------------------------------------------------------------- estrellas

    fn with_stars(state: crate::stars::StarsState, config: Config) -> App {
        let mut app = app_with(config);
        app.stars.set_for_test(state);
        app
    }

    #[test]
    fn the_header_shows_the_star_count_at_the_far_right() {
        use crate::stars::StarsState::Count;
        for (w, h) in [(120u16, 30u16), (100, 24), (80, 24)] {
            let mut app = with_stars(Count(1234), Config::default());
            let top = row_text(&render(&mut app, w, h), 0);
            assert!(top.trim_end().ends_with("★ 1.2k"), "{w}x{h}: {top:?}");
            // Detrás del contador de túneles, con el hueco de margen de siempre.
            assert!(top.find("activos").unwrap() < top.find('★').unwrap(), "{top:?}");
            assert_eq!(top.chars().last(), Some(' '), "{top:?}");
        }
    }

    #[test]
    fn the_star_count_is_gold_and_bold() {
        let mut app = with_stars(crate::stars::StarsState::Count(42), Config::default());
        let buf = render(&mut app, 100, 24);
        let top = row_text(&buf, 0);
        let x = col_of(&buf, 0, "★ 42");
        assert_eq!(buf[(x, 0)].fg, STAR);
        assert!(buf[(x, 0)].modifier.contains(Modifier::BOLD));
        assert!(top.contains("★ 42"));
    }

    #[test]
    fn the_stars_follow_their_state() {
        use crate::stars::StarsState::*;
        for (state, expected) in [(Loading, "★ …"), (Unavailable, "★ —"), (Count(0), "★ 0"), (Count(15_900), "★ 15.9k")] {
            let mut app = with_stars(state, Config::default());
            let top = row_text(&render(&mut app, 100, 24), 0);
            assert!(top.trim_end().ends_with(expected), "{state:?}: {top:?}");
        }
    }

    #[test]
    fn hidden_stars_leave_no_trace_in_the_header() {
        let mut app = with_stars(crate::stars::StarsState::Count(42), Config::default());
        app.config.show_stars = false;
        app.stars.set_enabled(false);
        let top = row_text(&render(&mut app, 100, 24), 0);
        assert!(!top.contains('★'), "{top:?}");
        assert!(top.trim_end().ends_with("activos"), "{top:?}");
    }

    #[test]
    fn stars_are_the_first_thing_dropped_when_the_header_is_tight() {
        let mut app = with_stars(crate::stars::StarsState::Count(42), Config::default());
        let roomy = row_text(&render(&mut app, 60, 24), 0);
        assert!(roomy.contains('★') && roomy.contains("activos"), "{roomy:?}");
        // Con una cabecera muy corta se quitan las estrellas y se conserva el contador.
        let mut found_without_stars = false;
        for width in (24..60u16).rev() {
            let top = row_text(&render(&mut app, width, 24), 0);
            if top.contains("activos") && !top.contains('★') {
                found_without_stars = true;
                break;
            }
        }
        assert!(found_without_stars);
        // Nunca queda una estrella cortada o sola sin su número.
        for width in 10..60u16 {
            let top = row_text(&render(&mut app, width, 24), 0);
            if top.contains('★') {
                assert!(top.contains("★ 42"), "{width}: {top:?}");
            }
        }
    }

    #[test]
    fn the_esc_menu_leaves_the_header_corner_with_the_stars_undimmed() {
        let mut app = with_stars(crate::stars::StarsState::Count(42), all_off());
        let calm = render(&mut app, 120, 30);
        let x = col_of(&calm, 0, "★ 42");
        let (fg_before, bg_before) = (calm[(x, 0)].fg, calm[(x, 0)].bg);

        for overlay in [Overlay::Menu, Overlay::Options, Overlay::Help] {
            let mut app = with_stars(crate::stars::StarsState::Count(42), all_off());
            app.overlay = overlay;
            let buf = render(&mut app, 120, 30);
            let x = col_of(&buf, 0, "★ 42");
            assert_eq!((buf[(x, 0)].fg, buf[(x, 0)].bg), (fg_before, bg_before));
            assert!(row_text(&buf, 0).trim_end().ends_with("★ 42"));
            // El resto de la cabecera sí está oscurecido.
            assert_ne!(buf[(1, 0)].bg, calm[(1, 0)].bg);
        }
    }

    #[test]
    fn the_stars_also_show_during_the_menu_transition() {
        let mut app = with_stars(crate::stars::StarsState::Count(42), Config { banner_animation: false, header_animation: false, ..Config::default() });
        app.overlay = Overlay::Menu;
        app.menu_opened = Some(Instant::now());
        let buf = render(&mut app, 120, 30);
        let x = col_of(&buf, 0, "★ 42");
        assert_eq!(buf[(x, 0)].fg, STAR);
    }

    #[test]
    fn the_big_banner_never_paints_over_the_stars() {
        // Con 80x26 el banner empieza en la fila 0 y cruza la esquina de la cabecera.
        let mut app = with_stars(crate::stars::StarsState::Count(42), all_off());
        app.overlay = Overlay::Menu;
        let buf = render(&mut app, 80, 26);
        assert!(row_text(&buf, 0).trim_end().ends_with("★ 42"), "{:?}", row_text(&buf, 0));
    }

    #[test]
    fn the_stars_option_is_listed_in_the_general_tab() {
        let mut app = options_app(Config::default(), 0, 1);
        let buf = render(&mut app, 100, 24);
        let text = screen(&buf);
        assert!(text.contains("GitHub stars") && text.contains("Update interval"));
        let y = list_row(&buf, "GitHub stars");
        assert!(row_text(&buf, y).contains("❯ GitHub stars"));
        assert!(row_text(&buf, y).contains("ACTIVADA"));
        // Heading and description of the selected row.
        assert!(text.contains("fir3desert/WgcTUI"), "{text}");
        assert!(text.contains("curl"));
    }

    #[test]
    fn the_stars_option_shows_its_state() {
        let mut app = options_app(Config { show_stars: false, ..Config::default() }, 0, 1);
        let buf = render(&mut app, 100, 24);
        let y = list_row(&buf, "GitHub stars");
        assert!(row_text(&buf, y).contains("DESACTIVADA"));
    }

    #[test]
    fn the_stars_description_fits_a_24_row_terminal() {
        for width in [80u16, 100] {
            let mut app = options_app(Config::default(), 0, 1);
            let text = screen(&render(&mut app, width, 24));
            assert!(text.contains("ninguna."), "{width}:\n{text}");
        }
    }

    #[test]
    fn update_interval_value_follows_the_setting() {
        let mut app = options_app(Config { update_ms: 250, ..Config::default() }, 0, 0);
        assert!(screen(&render(&mut app, 100, 30)).contains("250 ms"));
        app.config.update_ms = 20_000;
        assert!(screen(&render(&mut app, 100, 30)).contains("20000 ms"));
    }

    #[test]
    fn adjusting_shows_arrows_around_the_value_and_a_different_hint() {
        let mut app = options_app(Config::default(), 0, 0);
        let calm = screen(&render(&mut app, 100, 30));
        assert!(calm.contains("Enter cambiar") && !calm.contains('◂'));
        app.adjusting = true;
        let buf = render(&mut app, 100, 30);
        let text = screen(&buf);
        assert!(text.contains("◂ 1000 ms ▸"), "{text}");
        assert!(text.contains("+ / − ajustar") && text.contains("Enter o Esc listo"));
        assert!(!text.contains("Enter cambiar"));
        // El aviso de ajuste va en amarillo.
        let y = (0..30u16).find(|&y| row_text(&buf, y).contains("ajustar")).unwrap();
        assert_eq!(buf[(col_of(&buf, y, "+ / −"), y)].fg, YELLOW);
    }

    #[test]
    fn animations_tab_lists_three_switches_with_their_state() {
        let mut app = options_app(Config { banner_animation: false, ..Config::default() }, 1, 0);
        let text = screen(&render(&mut app, 100, 30));
        for name in ["Header animation", "Banner animation", "Menu animation"] {
            assert!(text.contains(name), "falta {name}");
        }
        assert!(!text.contains("Update interval"));
        // En la lista (izquierda) hay dos ACTIVADA y una DESACTIVADA; la descripción
        // de la derecha menciona DESACTIVADA, pero no como valor.
        let value_rows: Vec<&str> = text
            .lines()
            .filter(|l| ["Header animation", "Banner animation", "Menu animation"].iter().any(|n| l.contains(n)))
            .collect();
        assert_eq!(value_rows.len(), 3);
        let on = value_rows.iter().filter(|l| l.contains(" ACTIVADA")).count();
        let off = value_rows.iter().filter(|l| l.contains("DESACTIVADA")).count();
        assert_eq!((on, off), (2, 1));
        let banner = value_rows.iter().find(|l| l.contains("Banner animation")).unwrap();
        assert!(banner.contains("DESACTIVADA"));
        // Orden: Header, Banner, Menu.
        let line_of = |needle: &str| text.lines().position(|l| l.contains(needle)).unwrap();
        assert!(line_of("Header animation") < line_of("Banner animation"));
        assert!(line_of("Banner animation") < line_of("Menu animation"));
    }

    #[test]
    fn animation_values_are_coloured_green_when_on_and_muted_when_off() {
        let mut app = options_app(Config { menu_animation: false, ..Config::default() }, 1, 0);
        let buf = render(&mut app, 100, 30);
        let colour_of_value = |label: &str, value: &str| {
            let y = (0..30u16).find(|&y| row_text(&buf, y).contains(label)).unwrap();
            let row = row_text(&buf, y);
            let byte = row.find(label).unwrap();
            let rest = &row[byte..];
            let at = rest.find(value).unwrap();
            let x = row[..byte].chars().count() + rest[..at].chars().count();
            buf[(x as u16, y)].fg
        };
        assert_eq!(colour_of_value("Header animation", "ACTIVADA"), GREEN);
        assert_eq!(colour_of_value("Menu animation", "DESACTIVADA"), FG_MUTED);
    }

    /// Fila de la lista (izquierda) del ajuste `label`: la que lleva su valor.
    fn list_row(buf: &Buffer, label: &str) -> u16 {
        (0..buf.area.height)
            .find(|&y| {
                // Primer tramo entre divisores = la lista; el segundo, la descripción.
                row_text(buf, y).split('│').nth(1).is_some_and(|list| {
                    list.contains(label) && (list.contains("ACTIVADA") || list.contains(" ms"))
                })
            })
            .unwrap_or_else(|| panic!("no hay fila para {label}"))
    }

    #[test]
    fn the_selected_row_has_the_marker_and_highlight() {
        let mut app = options_app(Config::default(), 1, 1);
        let buf = render(&mut app, 100, 30);
        let y = list_row(&buf, "Banner animation");
        assert!(row_text(&buf, y).contains("❯ Banner animation"));
        assert_eq!(buf[(col_of(&buf, y, "Banner"), y)].bg, BG_SEL);
        let other = list_row(&buf, "Header animation");
        assert!(!row_text(&buf, other).contains('❯'));
        assert_ne!(buf[(col_of(&buf, other, "Header"), other)].bg, BG_SEL);
    }

    #[test]
    fn the_description_on_the_right_follows_the_selected_item() {
        let labels = ["Header animation", "Banner animation", "Menu animation"];
        for (selected, label) in labels.iter().enumerate() {
            let mut app = options_app(Config::default(), 1, selected);
            let buf = render(&mut app, 110, 30);
            // La primera fila del cuerpo trae, a la derecha del divisor, el título del elegido.
            let row = row_text(&buf, list_row(&buf, "Header animation"));
            let segments: Vec<&str> = row.split('│').collect();
            assert!(segments[2].contains(label), "item {selected}: {row:?}");
        }
        // Y el texto de cada descripción es el de la opción.
        for (item, phrase) in [
            (OptionItem::HeaderAnimation, "etiqueta fija"),
            (OptionItem::BannerAnimation, "ya terminado y estático"),
            (OptionItem::MenuAnimation, "aparece de golpe"),
            (OptionItem::UpdateInterval, "de 100 en 100 ms"),
        ] {
            let app = app_with(Config::default());
            assert!(option_text(&app, item).description.contains(phrase), "{item:?}");
        }
    }

    #[test]
    fn long_descriptions_are_not_cut_off_in_a_24_row_terminal() {
        for (width, height) in [(80u16, 24u16), (100, 24), (120, 30)] {
            let mut app = options_app(Config::default(), 0, 0);
            let text = screen(&render(&mut app, width, height));
            // La última palabra de la descripción del intervalo tiene que verse entera.
            assert!(text.contains("después."), "{width}x{height}:\n{text}");
            for selected in 0..3 {
                let mut app = options_app(Config::default(), 1, selected);
                let text = screen(&render(&mut app, width, height));
                    assert!(
                    text.contains("animar.") || text.contains("estático.") || text.contains("transición."),
                    "{width}x{height} item {selected}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn options_screen_has_a_framed_title_and_a_hint_line() {
        let mut app = options_app(Config::default(), 0, 0);
        let text = screen(&render(&mut app, 100, 30));
        assert!(text.contains("OPTIONS"));
        assert!(text.contains("↑↓ mover") && text.contains("←→ pestaña") && text.contains("Esc volver"));
    }

    #[test]
    fn options_fit_a_24_row_terminal() {
        for tab in 0..2usize {
            let mut app = options_app(Config::default(), tab, 0);
            let text = screen(&render(&mut app, 80, 24));
            assert!(text.contains("General") && text.contains("Animaciones"));
            assert!(text.contains("Esc volver"), "tab {tab}");
        }
        let mut app = options_app(Config::default(), 1, 2);
        let text = screen(&render(&mut app, 80, 24));
        assert!(text.contains("Menu animation"));
    }

    #[test]
    fn options_on_a_tiny_terminal_falls_back_to_the_menu_instead_of_panicking() {
        let mut app = options_app(Config::default(), 1, 1);
        let _ = render(&mut app, 25, 7);
        let _ = render(&mut app, 12, 4);
        let _ = render(&mut app, 80, 9);
    }

    #[test]
    fn help_mentions_the_new_menu_and_the_settings() {
        let mut app = app_with(Config::default());
        app.overlay = Overlay::Help;
        let text = screen(&render(&mut app, 100, 30));
        assert!(text.contains("OPTIONS · HELP · QUIT"));
        assert!(text.contains("/etc/wgctui.conf"));
        assert!(text.contains("Frecuencia de actualización"));
        assert!(text.contains("100–20000 ms"));
        assert!(!text.contains("RESUME"));
    }

    /// Vuelca las pantallas a texto: `cargo test dump_screens -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_screens() {
        let show = |title: &str, app: &mut App, w: u16, h: u16| {
            println!("\n===== {title} ({w}x{h}) =====\n{}", screen(&render(app, w, h)));
        };
        let mut app = with_stars(crate::stars::StarsState::Count(1234), Config::default());
        app.header_start = at_secs(30);
        show("Cabecera con animación activada y estrellas", &mut app, 80, 6);
        app.config.show_stars = false;
        app.stars.set_enabled(false);
        show("Cabecera con estrellas ocultas", &mut app, 80, 6);
        let mut app = app_with(Config { header_animation: false, ..Config::default() });
        show("Cabecera con animación desactivada", &mut app, 80, 6);
        let mut app = with_stars(crate::stars::StarsState::Count(42), all_off());
        app.overlay = Overlay::Menu;
        show("Menú de Esc grande (26 filas)", &mut app, 100, 26);
        show("Menú de Esc compacto", &mut app, 60, 14);
        let mut app = options_app(Config::default(), 0, 0);
        show("OPTIONS · General", &mut app, 100, 24);
        app.adjusting = true;
        show("OPTIONS · General (ajustando)", &mut app, 100, 24);
        let mut app = options_app(Config { banner_animation: false, ..Config::default() }, 1, 1);
        show("OPTIONS · Animaciones", &mut app, 100, 24);
        show("OPTIONS · Animaciones (80x24)", &mut app, 80, 24);
        app.overlay = Overlay::Help;
        show("HELP", &mut app, 90, 26);
    }
}
