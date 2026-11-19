//! Dibujo de la pantalla principal: cabecera, lista, detalle y barra de estado.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::{
    App, Kind, Overlay, menu,
    theme::*,
    wg::{self, Tunnel, WG_DIR},
};

// ---------------------------------------------------------------- formato

fn fmt_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn fmt_rate(bps: f64) -> String {
    format!("{}/s", fmt_bytes(bps.max(0.0) as u64))
}

fn fmt_ago(ts: u64) -> (String, Color) {
    if ts == 0 {
        return ("nunca".into(), RED);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = now.saturating_sub(ts);
    let txt = if s < 60 {
        format!("hace {s} s")
    } else if s < 3600 {
        format!("hace {} min", s / 60)
    } else if s < 86400 {
        format!("hace {} h", s / 3600)
    } else {
        format!("hace {} d", s / 86400)
    };
    (txt, if s < 180 { GREEN } else { YELLOW })
}

fn short_key(k: &str) -> String {
    let c: Vec<char> = k.chars().collect();
    if c.len() > 16 {
        let head: String = c[..6].iter().collect();
        let tail: String = c[c.len() - 6..].iter().collect();
        format!("{head}…{tail}")
    } else {
        k.to_string()
    }
}

/// Mini gráfica de barras con las últimas muestras.
fn spark(vals: &[f64], width: usize) -> String {
    const L: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let take = vals.len().min(width);
    let slice = &vals[vals.len() - take..];
    let max = slice.iter().cloned().fold(1024.0_f64, f64::max);
    let mut s = " ".repeat(width - take);
    for v in slice {
        let idx = ((v / max) * 7.0).round().clamp(0.0, 7.0) as usize;
        s.push(L[idx]);
    }
    s
}

fn text_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// Une los grupos izquierdo y derecho de una barra; si no caben, se descarta el derecho.
fn bar_line(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let (lw, rw) = (text_width(&left), text_width(&right));
    let mut spans = left;
    if lw + rw < width as usize {
        spans.push(Span::raw(" ".repeat(width as usize - lw - rw)));
        spans.extend(right);
    }
    Line::from(spans)
}

// ------------------------------------------------------------------ draw

pub fn draw(app: &mut App, f: &mut Frame) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(FG)), area);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(app, f, rows[0]);
    draw_body(app, f, rows[1]);
    draw_status(app, f, rows[2]);

    match app.overlay {
        Overlay::None => {}
        Overlay::Menu => menu::render_menu(f, area, app),
        Overlay::Help => menu::render_help(f, area),
    }
}

fn draw_header(app: &App, f: &mut Frame, area: Rect) {
    let active = app.tunnels.iter().filter(|t| t.active).count();
    let total = app.tunnels.len();

    let mut left = vec![badge(" wgctui ", ACCENT)];
    if let Some(t) = app.selected() {
        left.push(sp("  › ", FG_MUTED));
        left.push(sp(t.name.clone(), FG_DIM));
    }
    let right = vec![
        Span::styled(
            format!(" {active}/{total} activos "),
            Style::new()
                .fg(if active > 0 { GREEN } else { FG_MUTED })
                .bg(BG_SEL)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    let (lw, rw) = (text_width(&left), text_width(&right));

    let line = bar_line(left, right, area.width);
    f.render_widget(
        Paragraph::new(line).style(Style::new().bg(BG_ALT).fg(FG_DIM)),
        area,
    );

    // Reloj centrado, solo si no toca ninguno de los lados.
    let clock = format!(" {} ", wg::clock());
    let cw = clock.chars().count() as u16;
    let x = area.x + area.width.saturating_sub(cw) / 2;
    let free_from = area.x + lw as u16 + 1;
    let free_to = area.x + area.width.saturating_sub(rw as u16);
    if x >= free_from && x + cw < free_to {
        f.render_widget(
            Paragraph::new(sp(clock, FG_MUTED)).style(Style::new().bg(BG_ALT)),
            Rect::new(x, area.y, cw, 1),
        );
    }
}

fn draw_status(app: &App, f: &mut Frame, area: Rect) {
    let any_busy = app.tunnels.iter().any(|t| t.busy.is_some());
    let status = app.visible_status();

    let (badge_txt, badge_col) = match status {
        Some((_, Kind::Ok)) => (" ✔ OK ".to_string(), GREEN),
        Some((_, Kind::Err)) => (" ✘ ERROR ".to_string(), RED),
        Some((_, Kind::Info)) => (format!(" {} TRABAJANDO ", app.spinner()), YELLOW),
        None if any_busy => (format!(" {} TRABAJANDO ", app.spinner()), YELLOW),
        None => (" TÚNELES ".to_string(), ACCENT),
    };
    let mut left = vec![badge(badge_txt, badge_col)];
    if let Some((msg, kind)) = status {
        let color = if kind == Kind::Err { RED } else { FG_DIM };
        left.push(sp(format!("  {msg}"), color));
    } else if let Some(t) = app.selected() {
        left.push(sp(format!("  {}", t.name), FG_DIM));
    }

    // Atajos: se quitan de derecha a izquierda si no caben.
    const HINTS: [(&str, &str); 6] = [
        ("↑↓", "mover"),
        ("⏎", "alternar"),
        ("PgUp/PgDn", "detalle"),
        ("r", "refrescar"),
        ("Esc", "menú"),
        ("q", "salir"),
    ];
    let lw = text_width(&left);
    let mut right: Vec<Span<'static>> = Vec::new();
    for n in (0..=HINTS.len()).rev() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (key, label) in HINTS.iter().take(n) {
            spans.push(bold(*key, FG));
            spans.push(sp(format!(" {label}  "), FG_MUTED));
        }
        if lw + text_width(&spans) + 2 <= area.width as usize {
            right = spans;
            break;
        }
    }

    f.render_widget(
        Paragraph::new(bar_line(left, right, area.width))
            .style(Style::new().bg(BG_ALT).fg(FG_DIM)),
        area,
    );
}

fn draw_body(app: &mut App, f: &mut Frame, area: Rect) {
    let lw = (u32::from(area.width) * 34 / 100).clamp(34, 52).min(u32::from(area.width)) as u16;
    let cols = Layout::horizontal([Constraint::Length(lw), Constraint::Min(0)]).split(area);
    draw_list(app, f, cols[0]);
    draw_detail(app, f, cols[1]);
}

// ------------------------------------------------------------ lista

fn draw_list(app: &mut App, f: &mut Frame, area: Rect) {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(Style::new().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let parts = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);

    let active = app.tunnels.iter().filter(|t| t.active).count();
    f.render_widget(
        Paragraph::new(Line::from(bold(" TÚNELES", FG_MUTED))),
        parts[0],
    );
    f.render_widget(
        Paragraph::new(
            Line::from(vec![
                sp(format!("{active}"), if active > 0 { GREEN } else { FG_MUTED }),
                sp(format!("/{} ", app.tunnels.len()), FG_MUTED),
            ])
            .alignment(Alignment::Right),
        ),
        parts[0],
    );

    if app.tunnels.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(sp(format!(" No hay archivos .conf en {WG_DIR}"), FG_MUTED)),
            ]),
            parts[1],
        );
        return;
    }

    let width = parts[1].width as usize;
    let sel = app.state.selected();
    let items: Vec<ListItem> = app
        .tunnels
        .iter()
        .enumerate()
        .map(|(i, t)| tunnel_item(app, t, Some(i) == sel, width))
        .collect();
    let list = List::new(items).highlight_style(Style::new().bg(BG_SEL));
    f.render_stateful_widget(list, parts[1], &mut app.state);
}

fn tunnel_item(app: &App, t: &Tunnel, selected: bool, width: usize) -> ListItem<'static> {
    // Barra de selección, como en el explorador de runetui.
    let bar = if selected { sp("▍", ACCENT) } else { Span::raw(" ") };

    let (icon, icon_col) = match (t.busy, t.active) {
        (Some(_), _) => (app.spinner(), YELLOW),
        (None, true) => ("●", GREEN),
        (None, false) => ("○", FG_MUTED),
    };
    let badge_span = match (t.busy, t.active) {
        (Some(true), _) => badge(" activando ", YELLOW),
        (Some(false), _) => badge(" desactivando ", YELLOW),
        (None, true) => badge(" ACTIVA ", GREEN),
        (None, false) => sp(" inactiva ", FG_MUTED),
    };
    let name_span = if t.active {
        bold(t.name.clone(), FG)
    } else {
        sp(t.name.clone(), FG_DIM)
    };

    // barra + espacio + icono + espacio + nombre ... badge + margen
    let left_len = 4 + t.name.chars().count();
    let badge_len = badge_span.content.chars().count();
    let pad = width.saturating_sub(left_len + badge_len + 1).max(1);

    let line1 = Line::from(vec![
        bar.clone(),
        Span::raw(" "),
        sp(icon, icon_col),
        Span::raw(" "),
        name_span,
        Span::raw(" ".repeat(pad)),
        badge_span,
    ]);

    let mut detail: Vec<Span<'static>> = vec![bar.clone(), Span::raw("   ")];
    if t.busy.is_some() {
        detail.push(sp("trabajando…", FG_MUTED));
    } else if t.active {
        if t.last_handshake() == 0 {
            detail.push(sp("esperando handshake…", YELLOW));
        } else {
            let (rx, tx) = app.rates.get(&t.name).copied().unwrap_or((0.0, 0.0));
            detail.push(sp("↓ ", DOWN));
            detail.push(sp(fmt_rate(rx), FG_DIM));
            detail.push(sp("   ↑ ", UP));
            detail.push(sp(fmt_rate(tx), FG_DIM));
        }
    } else {
        let mut parts: Vec<String> = Vec::new();
        if !t.address.is_empty() {
            parts.push(t.address.clone());
        }
        if !t.endpoint.is_empty() {
            parts.push(t.endpoint.clone());
        }
        let joined = parts.join("  ·  ");
        let txt = if parts.is_empty() {
            "sin datos".to_string()
        } else if joined.chars().count() + 5 <= width {
            joined
        } else {
            parts[0].clone() // si no cabe, solo la dirección
        };
        detail.push(sp(txt, FG_MUTED));
    }

    ListItem::new(vec![line1, Line::from(detail), Line::from(bar)])
}

// ---------------------------------------------------------- detalle

fn draw_detail(app: &mut App, f: &mut Frame, area: Rect) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .split(inner);

    // Cabecera del panel: nombre y estado.
    let caption = match app.selected() {
        Some(t) => {
            let state = match (t.busy, t.active) {
                (Some(true), _) => badge(" ACTIVANDO ", YELLOW),
                (Some(false), _) => badge(" DESACTIVANDO ", YELLOW),
                (None, true) => badge(" ACTIVA ", GREEN),
                (None, false) => Span::styled(" INACTIVA ", Style::new().fg(FG_MUTED).bg(BG_SEL)),
            };
            Line::from(vec![bold(t.name.clone(), FG), Span::raw("  "), state])
        }
        None => Line::from(sp("Detalle", FG_MUTED)),
    };
    f.render_widget(Paragraph::new(caption), parts[0]);
    f.render_widget(
        Paragraph::new(Line::from(sp("─".repeat(inner.width as usize), BORDER))),
        parts[1],
    );

    let lines = detail_lines(app, inner.width);
    let max_scroll = lines.len().saturating_sub(parts[2].height as usize) as u16;
    app.scroll = app.scroll.min(max_scroll);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        parts[2],
    );
}

fn kv(indent: usize, label: &str, mut value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![sp(
        format!("{}{:<14}", " ".repeat(indent), label),
        FG_MUTED,
    )];
    spans.append(&mut value);
    Line::from(spans)
}

fn section(title: &str, width: u16) -> Line<'static> {
    let head = format!("{title} ");
    let rule = (width as usize).saturating_sub(head.chars().count());
    Line::from(vec![
        bold(head, FG_MUTED),
        sp("─".repeat(rule), BORDER),
    ])
}

fn style_conf_line(l: &str) -> Line<'static> {
    let t = l.trim();
    if t.is_empty() {
        return Line::from("");
    }
    if t.starts_with('#') {
        return Line::from(vec![Span::raw("  "), italic(t.to_string(), FG_MUTED)]);
    }
    if t.starts_with('[') {
        return Line::from(vec![Span::raw("  "), bold(t.to_string(), ACCENT)]);
    }
    if let Some((k, v)) = t.split_once('=') {
        let v = v.trim();
        let value = if v == "(oculta)" {
            italic("(oculta)", RED)
        } else {
            sp(v.to_string(), FG)
        };
        return Line::from(vec![
            Span::raw("  "),
            sp(k.trim().to_string(), UP),
            sp(" = ", FG_MUTED),
            value,
        ]);
    }
    Line::from(vec![Span::raw("  "), sp(t.to_string(), FG)])
}

fn detail_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(t) = app.selected() else {
        return vec![Line::from(sp("Selecciona un túnel", FG_MUTED))];
    };
    let mut out: Vec<Line<'static>> = vec![Line::from("")];

    // ---- estado
    out.push(section("ESTADO", width));
    let state = match (t.busy, t.active) {
        (Some(true), _) => sp(format!("{} Activando…", app.spinner()), YELLOW),
        (Some(false), _) => sp(format!("{} Desactivando…", app.spinner()), YELLOW),
        (None, true) => bold("● Activa", GREEN),
        (None, false) => sp("○ Inactiva", FG_MUTED),
    };
    out.push(kv(2, "Estado", vec![state]));
    if !t.address.is_empty() {
        out.push(kv(2, "Dirección", vec![sp(t.address.clone(), FG)]));
    }
    if t.active {
        if !t.listen_port.is_empty() && t.listen_port != "0" {
            out.push(kv(2, "Puerto", vec![sp(t.listen_port.clone(), FG)]));
        }
        if !t.public_key.is_empty() {
            out.push(kv(2, "Clave pública", vec![sp(short_key(&t.public_key), FG)]));
        }
    } else if !t.endpoint.is_empty() {
        out.push(kv(2, "Endpoint", vec![sp(t.endpoint.clone(), FG)]));
    }

    // ---- tráfico con gráfica
    if t.active {
        let (rr, tr) = app.rates.get(&t.name).copied().unwrap_or((0.0, 0.0));
        let hist = app.history.get(&t.name);
        let down: Vec<f64> = hist.map(|h| h.iter().map(|s| s.0).collect()).unwrap_or_default();
        let up: Vec<f64> = hist.map(|h| h.iter().map(|s| s.1).collect()).unwrap_or_default();

        // La gráfica ocupa lo que sobra de la fila; si no cabe, se omite.
        let spark_w = (width as usize).saturating_sub(37).min(24);
        let spark_w = if spark_w >= 8 { spark_w } else { 0 };

        out.push(Line::from(""));
        out.push(section("TRÁFICO", width));
        for (arrow, label, color, total, rate, series) in [
            ("↓", "Descarga", DOWN, t.rx(), rr, &down),
            ("↑", "Subida", UP, t.tx(), tr, &up),
        ] {
            out.push(Line::from(vec![
                sp("  ", FG),
                bold(arrow, color),
                sp(format!(" {label:<9}"), FG_MUTED),
                sp(format!("{:<10}", fmt_bytes(total)), FG),
                sp(format!("{:<12}", fmt_rate(rate)), FG_DIM),
                sp(spark(series, spark_w), color),
            ]));
        }

        // ---- peers
        for (i, p) in t.peers.iter().enumerate() {
            out.push(Line::from(""));
            out.push(section(&format!("PEER {}", i + 1), width));
            out.push(kv(2, "Clave", vec![sp(short_key(&p.public_key), FG)]));
            if p.endpoint != "(none)" {
                out.push(kv(2, "Endpoint", vec![sp(p.endpoint.clone(), FG)]));
            }
            out.push(kv(
                2,
                "AllowedIPs",
                vec![sp(p.allowed_ips.replace(',', ", "), FG)],
            ));
            let (ago, col) = fmt_ago(p.handshake);
            out.push(kv(2, "Handshake", vec![sp(ago, col)]));
            out.push(kv(
                2,
                "Transferido",
                vec![
                    sp("↓ ", DOWN),
                    sp(fmt_bytes(p.rx), FG),
                    sp("    ↑ ", UP),
                    sp(fmt_bytes(p.tx), FG),
                ],
            ));
            if p.keepalive != "off" {
                out.push(kv(2, "Keepalive", vec![sp(format!("cada {} s", p.keepalive), FG)]));
            }
        }
    } else if t.busy.is_none() {
        out.push(Line::from(""));
        out.push(Line::from(italic("  Pulsa ⏎ para activar este túnel", FG_MUTED)));
    }

    // ---- configuración
    out.push(Line::from(""));
    out.push(section("CONFIGURACIÓN", width));
    for l in t.conf.lines() {
        out.push(style_conf_line(l));
    }
    out
}
