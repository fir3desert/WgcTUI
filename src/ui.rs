//! Main screen rendering: header, list, details, and status bar.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::{
    App, Kind, Overlay, menu, stars,
    theme::*,
    wg::{self, Tunnel, WG_DIR},
};

// ---------------------------------------------------------------- formatting

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
        return ("never".into(), RED);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = now.saturating_sub(ts);
    let txt = if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86400)
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

/// Mini bar chart of recent samples.
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

/// Join the left and right groups of a bar; discard the right group if they do not fit.
fn bar_line(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let (lw, rw) = (text_width(&left), text_width(&right));
    let mut spans = left;
    if lw + rw < width as usize {
        spans.push(Span::raw(" ".repeat(width as usize - lw - rw)));
        spans.extend(right);
    }
    Line::from(spans)
}

// ------------------------------------------------------------------ drawing

pub fn draw(app: &mut App, f: &mut Frame) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(FG)), area);

    // The visible UI determines the loop cadence (see `App::frame_timeout`).
    app.menu_big = menu::fits_big_menu(area);
    app.header_fits = area.width >= menu::header_width(true);

    // The header is always one row high.
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let keep = draw_header(app, f, rows[0]);
    app.header_keep = keep;
    draw_body(app, f, rows[1]);
    draw_status(app, f, rows[2]);

    match app.overlay {
        Overlay::None => {}
        Overlay::Menu => menu::render_menu(f, area, app),
        Overlay::Options => menu::render_options(f, area, app),
        Overlay::Help => menu::render_help(f, area, app),
        // Search is rendered in the status bar (see `draw_status`).
        Overlay::Search => {}
    }
}

/// Draw the header and return the location of its right-hand group (tunnel count and stars),
/// which the Esc menu leaves undimmed; return `None` if it does not fit.
fn draw_header(app: &App, f: &mut Frame, area: Rect) -> Option<Rect> {
    let active = app.tunnels.iter().filter(|t| t.active).count();
    let total = app.tunnels.len();
    f.render_widget(Block::new().style(Style::new().bg(BG_ALT).fg(FG_DIM)), area);

    // One line: the (animated or static) label and, to its right, the selected tunnel.
    let bar = area;
    let mut left = menu::header_label(app.header_animation());
    if let Some(t) = app.selected() {
        left.push(sp("  › ", FG_MUTED));
        left.push(sp(t.name.clone(), FG_DIM));
    }
    let count = Span::styled(
        format!(" {active}/{total} active "),
        Style::new()
            .fg(if active > 0 { GREEN } else { FG_MUTED })
            .bg(BG_SEL)
            .add_modifier(ratatui::style::Modifier::BOLD),
    );
    // Tunnel count and, at the far right, GitHub stars (with a trailing margin). If they do not
    // fit, the stars are removed first.
    let stars = stars::label(app.stars.state()).map(|label| {
        Span::styled(
            format!("  {label}"),
            Style::new().fg(STAR).add_modifier(ratatui::style::Modifier::BOLD),
        )
    });
    let build_right = |with_stars: bool| {
        let mut right = vec![count.clone()];
        if let (true, Some(stars)) = (with_stars, &stars) {
            right.push(stars.clone());
        }
        right.push(Span::raw(" "));
        right
    };
    let lw = text_width(&left);
    let mut right = build_right(true);
    if lw + text_width(&right) >= bar.width as usize {
        right = build_right(false);
    }
    let rw = text_width(&right);
    let keep = (lw + rw < bar.width as usize)
        .then(|| Rect::new(bar.right() - rw as u16, bar.y, rw as u16, 1));

    f.render_widget(
        Paragraph::new(bar_line(left, right, bar.width)).style(Style::new().bg(BG_ALT).fg(FG_DIM)),
        bar,
    );

    // Center the clock only if it does not overlap either side.
    let clock = format!(" {} ", wg::clock());
    let cw = clock.chars().count() as u16;
    let x = area.x + area.width.saturating_sub(cw) / 2;
    let free_from = bar.x + lw as u16 + 1;
    let free_to = bar.right().saturating_sub(rw as u16);
    if x >= free_from && x + cw < free_to {
        f.render_widget(
            Paragraph::new(sp(clock, FG_MUTED)).style(Style::new().bg(BG_ALT)),
            Rect::new(x, area.y, cw, 1),
        );
    }
    keep
}

fn draw_status(app: &App, f: &mut Frame, area: Rect) {
    let any_busy = app.tunnels.iter().any(|t| t.busy.is_some());
    let status = app.visible_status();

    let (badge_txt, badge_col) = match status {
        Some((_, Kind::Ok)) => (" ✔ OK ".to_string(), GREEN),
        Some((_, Kind::Err)) => (" ✘ ERROR ".to_string(), RED),
        Some((_, Kind::Info)) => (format!(" {} WORKING ", app.spinner()), YELLOW),
        None if any_busy => (format!(" {} WORKING ", app.spinner()), YELLOW),
        None => (" TUNNELS ".to_string(), ACCENT),
    };
    let searching = app.overlay == Overlay::Search;
    let (badge_txt, badge_col) = if searching {
        (" SEARCH ".to_string(), ACCENT)
    } else {
        (badge_txt, badge_col)
    };
    let mut left = vec![badge(badge_txt, badge_col)];
    let mut found: Option<(usize, usize)> = None;
    if searching {
        // Like runetui's `/` search: type the query directly in the bar.
        left.push(sp("  /", YELLOW));
        left.push(bold(app.finder.query.clone(), FG));
        left.push(sp("▏", ACCENT));
        if app.finder.query.is_empty() {
            left.push(sp(" type to search tunnels", FG_MUTED));
        } else {
            let total = app.finder.hits(&app.tunnels).len();
            found = Some((app.finder.selected.min(total.saturating_sub(1)) + 1, total));
        }
    } else if let Some((msg, kind)) = status {
        let color = if kind == Kind::Err { RED } else { FG_DIM };
        left.push(sp(format!("  {msg}"), color));
    } else if let Some(t) = app.selected() {
        left.push(sp(format!("  {}", t.name), FG_DIM));
    }

    // Hints are removed from right to left if they do not fit.
    const SEARCH_HINTS: [(&str, &str); 3] = [("↑↓", "next"), ("⏎", "go"), ("Esc", "cancel")];
    const HINTS: [(&str, &str); 7] = [
        ("↑↓", "move"),
        ("⏎", "toggle"),
        ("/", "search"),
        ("PgUp/PgDn", "details"),
        ("r", "refresh"),
        ("Esc", "menu"),
        ("q", "quit"),
    ];
    let hints: &[(&str, &str)] = if searching { &SEARCH_HINTS } else { &HINTS };
    // Match counter, like runetui's `1/34`.
    let counter: Vec<Span<'static>> = match found {
        Some((_, 0)) => vec![sp(" no matches ", RED)],
        Some((cur, total)) => vec![sp(format!(" {cur}/{total} "), YELLOW)],
        None => Vec::new(),
    };
    let lw = text_width(&left);
    let mut right: Vec<Span<'static>> = Vec::new();
    for n in (0..=hints.len()).rev() {
        let mut spans: Vec<Span<'static>> = counter.clone();
        for (key, label) in hints.iter().take(n) {
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

// ------------------------------------------------------------ list

fn draw_list(app: &mut App, f: &mut Frame, area: Rect) {
    let block = Block::new()
        .borders(Borders::RIGHT)
        .border_style(Style::new().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let parts = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);

    let active = app.tunnels.iter().filter(|t| t.active).count();
    f.render_widget(
        Paragraph::new(Line::from(bold(" TUNNELS", FG_MUTED))),
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
                Line::from(sp(format!(" No .conf files found in {WG_DIR}"), FG_MUTED)),
            ]),
            parts[1],
        );
        return;
    }

    let width = parts[1].width as usize;
    let sel = app.state.selected();
    // When search is open, matching letters are highlighted and non-matches are dimmed.
    let hits = if app.overlay == Overlay::Search && !app.finder.query.trim().is_empty() {
        Some(app.finder.hits(&app.tunnels))
    } else {
        None
    };
    let items: Vec<ListItem> = app
        .tunnels
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let found = hits
                .as_ref()
                .map(|h| h.iter().find(|hit| hit.index == i).map(|hit| hit.positions.as_slice()));
            tunnel_item(app, t, Some(i) == sel, width, found)
        })
        .collect();
    let list = List::new(items).highlight_style(Style::new().bg(BG_SEL));
    f.render_stateful_widget(list, parts[1], &mut app.state);
}

/// `found`: `None` = not searching; `Some(None)` = no match; `Some(Some(letters))` = match.
fn tunnel_item(
    app: &App,
    t: &Tunnel,
    selected: bool,
    width: usize,
    found: Option<Option<&[usize]>>,
) -> ListItem<'static> {
    // Selection bar, like runetui's file finder.
    let bar = if selected { sp("▍", ACCENT) } else { Span::raw(" ") };

    let (icon, icon_col) = match (t.busy, t.active) {
        (Some(_), _) => (app.spinner(), YELLOW),
        (None, true) => ("●", GREEN),
        (None, false) => ("○", FG_MUTED),
    };
    let badge_span = match (t.busy, t.active) {
        (Some(true), _) => badge(" enabling ", YELLOW),
        (Some(false), _) => badge(" disabling ", YELLOW),
        (None, true) => badge(" ACTIVE ", GREEN),
        (None, false) => sp(" inactive ", FG_MUTED),
    };
    let base = if t.active {
        Style::new().fg(FG).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_DIM)
    };
    let name_spans_vec: Vec<Span<'static>> = match found {
        None => vec![Span::styled(t.name.clone(), base)],
        Some(None) => vec![Span::styled(t.name.clone(), Style::new().fg(FG_MUTED))],
        Some(Some(pos)) => name_spans(&t.name, pos, base),
    };

    // bar + space + icon + space + name ... badge + margin
    let left_len = 4 + t.name.chars().count();
    let badge_len = badge_span.content.chars().count();
    let pad = width.saturating_sub(left_len + badge_len + 1).max(1);

    let mut first = vec![bar.clone(), Span::raw(" "), sp(icon, icon_col), Span::raw(" ")];
    first.extend(name_spans_vec);
    first.push(Span::raw(" ".repeat(pad)));
    first.push(badge_span);
    let line1 = Line::from(first);

    let mut detail: Vec<Span<'static>> = vec![bar.clone(), Span::raw("   ")];
    if t.busy.is_some() {
        detail.push(sp("working…", FG_MUTED));
    } else if t.active {
        if t.last_handshake() == 0 {
            detail.push(sp("waiting for handshake…", YELLOW));
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
            "no data".to_string()
        } else if joined.chars().count() + 5 <= width {
            joined
        } else {
            parts[0].clone() // If it does not fit, show only the address.
        };
        detail.push(sp(txt, FG_MUTED));
    }

    ListItem::new(vec![line1, Line::from(detail), Line::from(bar)])
}

// ---------------------------------------------------------- details

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

    // Panel header: name and status.
    let caption = match app.selected() {
        Some(t) => {
            let state = match (t.busy, t.active) {
                (Some(true), _) => badge(" ENABLING ", YELLOW),
                (Some(false), _) => badge(" DISABLING ", YELLOW),
                (None, true) => badge(" ACTIVE ", GREEN),
                (None, false) => Span::styled(" INACTIVE ", Style::new().fg(FG_MUTED).bg(BG_SEL)),
            };
            Line::from(vec![bold(t.name.clone(), FG), Span::raw("  "), state])
        }
        None => Line::from(sp("Details", FG_MUTED)),
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
        let value = if v == "(hidden)" {
            italic("(hidden)", RED)
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
        return vec![Line::from(sp("Select a tunnel", FG_MUTED))];
    };
    let mut out: Vec<Line<'static>> = vec![Line::from("")];

    // ---- status
    out.push(section("STATUS", width));
    let state = match (t.busy, t.active) {
        (Some(true), _) => sp(format!("{} Enabling…", app.spinner()), YELLOW),
        (Some(false), _) => sp(format!("{} Disabling…", app.spinner()), YELLOW),
        (None, true) => bold("● Active", GREEN),
        (None, false) => sp("○ Inactive", FG_MUTED),
    };
    out.push(kv(2, "Status", vec![state]));
    if !t.address.is_empty() {
        out.push(kv(2, "Address", vec![sp(t.address.clone(), FG)]));
    }
    if t.active {
        if !t.listen_port.is_empty() && t.listen_port != "0" {
            out.push(kv(2, "Port", vec![sp(t.listen_port.clone(), FG)]));
        }
        if !t.public_key.is_empty() {
            out.push(kv(2, "Public key", vec![sp(short_key(&t.public_key), FG)]));
        }
    } else if !t.endpoint.is_empty() {
        out.push(kv(2, "Endpoint", vec![sp(t.endpoint.clone(), FG)]));
    }

    // ---- traffic graph
    if t.active {
        let (rr, tr) = app.rates.get(&t.name).copied().unwrap_or((0.0, 0.0));
        let hist = app.history.get(&t.name);
        let down: Vec<f64> = hist.map(|h| h.iter().map(|s| s.0).collect()).unwrap_or_default();
        let up: Vec<f64> = hist.map(|h| h.iter().map(|s| s.1).collect()).unwrap_or_default();

        // The graph uses the remaining row width; omit it if it does not fit.
        let spark_w = (width as usize).saturating_sub(37).min(24);
        let spark_w = if spark_w >= 8 { spark_w } else { 0 };

        out.push(Line::from(""));
        out.push(section("TRAFFIC", width));
        for (arrow, label, color, total, rate, series) in [
            ("↓", "Download", DOWN, t.rx(), rr, &down),
            ("↑", "Upload", UP, t.tx(), tr, &up),
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
            out.push(kv(2, "Key", vec![sp(short_key(&p.public_key), FG)]));
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
                "Transferred",
                vec![
                    sp("↓ ", DOWN),
                    sp(fmt_bytes(p.rx), FG),
                    sp("    ↑ ", UP),
                    sp(fmt_bytes(p.tx), FG),
                ],
            ));
            if p.keepalive != "off" {
                out.push(kv(2, "Keepalive", vec![sp(format!("every {} s", p.keepalive), FG)]));
            }
        }
    } else if t.busy.is_none() {
        out.push(Line::from(""));
        out.push(Line::from(italic("  Press ⏎ to enable this tunnel", FG_MUTED)));
    }

    // ---- configuration
    out.push(Line::from(""));
    out.push(section("CONFIGURATION", width));
    for l in t.conf.lines() {
        out.push(style_conf_line(l));
    }
    out
}

// ----------------------------------------------------------- search

/// The name with letters matching the query highlighted.
fn name_spans(name: &str, positions: &[usize], base: Style) -> Vec<Span<'static>> {
    name.chars()
        .enumerate()
        .map(|(i, c)| {
            let style = if positions.contains(&i) {
                Style::new()
                    .fg(YELLOW)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                base
            };
            Span::styled(c.to_string(), style)
        })
        .collect()
}
