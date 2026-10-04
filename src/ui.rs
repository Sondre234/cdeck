use crate::data::{self, Entry};
use crate::theme::{self, th};
use crate::{App, Focus, Mode, Row, Status, NEW_CHAT};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::hash::{Hash, Hasher};
use std::path::Path;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};


/// Rows the new-chat directory picker shows at once.
const PICKER_ROWS: usize = 8;
const SPINNER: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];

/// Every directory gets a stable colour so you can tell instances apart at a glance.
pub fn dir_color(p: &Path) -> Color {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    let dirs = th().dirs;
    dirs[(h.finish() % dirs.len() as u64) as usize]
}

/// Rows of the which-key strip (plus one for its rule). Fixed, so switching
/// modes never resizes the Claude instances.
const KEYMAP_ROWS: u16 = 3;

fn split_areas(area: Rect, keymap: bool) -> (Rect, Rect, Rect, Rect) {
    let map_h = if keymap { KEYMAP_ROWS + 1 } else { 0 };
    let [main, map, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(map_h), Constraint::Length(1)]).areas(area);
    let side_w = (area.width / 4).clamp(30, 48).min(area.width.saturating_sub(20));
    let [side, pane] = Layout::horizontal([Constraint::Length(side_w), Constraint::Fill(1)]).areas(main);
    (side, pane, map, footer)
}

/// (rows, cols) available to a Claude instance; the pane loses two rows to its title bar.
pub fn pane_size(area: Rect, keymap: bool) -> (u16, u16) {
    let (_, pane, _, _) = split_areas(area, keymap);
    (pane.height.saturating_sub(2).max(1), pane.width.max(1))
}

fn basename(p: &Path) -> String {
    if p == data::home() {
        return "~".into();
    }
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| data::tilde(p))
}

/// Cut to `w` columns, appending … when something was dropped.
fn trunc(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.into();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// Cut from the left: `~/dev/rust/very/deep` -> `…/very/deep`.
fn trunc_left(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.into();
    }
    let mut chars: Vec<char> = Vec::new();
    let mut used = 0;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        used += cw;
        chars.push(c);
    }
    std::iter::once('…').chain(chars.into_iter().rev()).collect()
}

fn pad_to(spans: &mut Vec<Span<'static>>, width: usize, right: Vec<Span<'static>>) {
    let used: usize = spans.iter().chain(&right).map(|s| s.content.width()).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    spans.extend(right);
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let (side, pane, map, footer) = split_areas(f.area(), app.keymap);
    draw_sidebar(f, app, side);
    draw_pane(f, app, pane);
    draw_footer(f, app, footer);
    if app.keymap {
        // The strip shows prefix continuations itself, like emacs which-key.
        draw_keymap(f, app, map);
        if app.help {
            draw_help(f);
        }
        return;
    }
    match app.mode {
        Mode::Space => popup(
            f,
            "space",
            &[
                ("n", "new chat…"),
                ("o", "new chat here, no prompt"),
                ("f", "search chats"),
                ("k", "kill instance"),
                ("l", "toggle live-only"),
                ("r", "rescan"),
                ("?", "help"),
                ("q", "quit"),
            ],
        ),
        Mode::Window => popup(f, "window", &[("←", "focus chat list"), ("→", "focus right pane"), ("w", "swap focus")]),
        Mode::Goto => popup(
            f,
            "goto",
            &[
                ("g", "first chat"),
                ("e", "last chat"),
                ("n", "next directory"),
                ("p", "previous directory"),
                ("←", "focus chat list"),
                ("→", "focus right pane"),
            ],
        ),
        _ => {}
    }
    if app.help {
        draw_help(f);
    }
}

fn status_glyph(app: &App, id: &str) -> Span<'static> {
    let spin = SPINNER[app.tick / 2 % SPINNER.len()];
    match app.status(id) {
        Status::Busy => Span::styled(spin, Style::new().fg(th().accent).bold()),
        Status::Waiting(_) => Span::styled("◐", Style::new().fg(th().warning).bold()),
        Status::Idle { unseen: true } => Span::styled("●", Style::new().fg(th().success).bold()),
        Status::Idle { unseen: false } => Span::styled("●", Style::new().fg(th().success)),
        Status::External { status: "busy", .. } => Span::styled(spin, Style::new().fg(th().external)),
        Status::External { .. } => Span::styled("◆", Style::new().fg(th().external)),
        Status::Dormant => Span::raw(" "),
    }
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    app.hits.side = area;
    // The divider lights up in the chat's colour while the pane has focus.
    let divider = match (app.focus, app.selected_item()) {
        (Focus::Pane, Some(it)) => Style::new().fg(dir_color(&it.cwd)),
        (Focus::Pane, None) => Style::new().fg(th().accent),
        _ => Style::new().fg(th().border),
    };
    let block = Block::new().borders(Borders::RIGHT).border_style(divider).style(Style::new().bg(Color::Reset).fg(Color::Reset));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let sig = if th().name.is_some() { 1 } else { 0 };
    let [brand, list, _] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1), Constraint::Length(sig)]).areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(
            [Span::raw("  "), Span::styled("✦ ", Style::new().fg(th().shimmer).bold())]
                .into_iter()
                .chain(colored(theme::gradient("Claude deck", th().accent, th().lilac), true))
                .collect::<Vec<_>>(),
        )),
        brand,
    );
    // The active Claude Code theme's name, as a signature at the bottom.
    if let Some(name) = th().name {
        let r = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  ✧ ", Style::new().fg(th().lilac)),
                Span::styled(name, Style::new().fg(th().faint).italic()),
            ])),
            r,
        );
    }
    let h = list.height as usize;
    let w = list.width as usize;

    // Build every visual line, then window it around the selection.
    let sel_row = app.selected_row().unwrap_or(0);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut sel_line = 0;
    let mut group_color = th().faint;
    let mut line_rows: Vec<Option<usize>> = Vec::new();
    for (i, row) in app.rows.iter().enumerate() {
        let before = lines.len();
        let selected = i == sel_row;
        let sel_bg = if app.focus == Focus::Sidebar { th().sel } else { th().sel_dim };
        match row {
            Row::NewChat => {
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled("⊕", Style::new().fg(th().accent).bold()),
                    Span::styled("  New chat", Style::new().fg(Color::Reset).add_modifier(if selected { Modifier::BOLD } else { Modifier::empty() })),
                ];
                pad_to(&mut spans, w, vec![Span::styled("n ", Style::new().fg(th().faint))]);
                let mut line = Line::from(spans);
                if selected {
                    sel_line = lines.len();
                    line = line.style(Style::new().bg(sel_bg));
                }
                lines.push(line);
                if app.rows.len() == 1 {
                    lines.push(Line::from(""));
                    let msg = match (app.filter.is_empty(), app.search.running) {
                        (true, _) => "  no chats yet",
                        (false, true) => "  searching transcripts…",
                        (false, false) => "  no matches",
                    };
                    lines.push(Line::from(Span::styled(msg, Style::new().fg(th().faint))));
                }
            }
            Row::Header { cwd, count, live } => {
                let c = dir_color(cwd);
                group_color = c;
                lines.push(Line::from(""));
                let name = basename(cwd);
                let right = if *live > 0 { format!("●{live} {count} ") } else { format!("{count} ") };
                let name = trunc(&name, w.saturating_sub(6 + right.width()));
                let path = data::tilde(cwd);
                let parent = path.strip_suffix(name.as_str()).unwrap_or("").to_string();
                let room = w.saturating_sub(name.width() + 6 + right.width());
                let parent = if parent.is_empty() { String::new() } else { trunc_left(&parent, room) };
                let mut spans = vec![
                    Span::styled("  ● ", Style::new().fg(c)),
                    Span::styled(name, Style::new().fg(c).bold()),
                    Span::raw(" "),
                    Span::styled(parent, Style::new().fg(th().faint)),
                ];
                pad_to(&mut spans, w, vec![Span::styled(right, Style::new().fg(th().faint))]);
                lines.push(Line::from(spans));
            }
            Row::More { hidden } => {
                let text = if *hidden > 0 { format!("   … {hidden} more") } else { "   ▴ fold".into() };
                lines.push(Line::from(vec![
                    Span::styled("  │", Style::new().fg(group_color)),
                    Span::styled(text, Style::new().fg(th().faint).italic()),
                    Span::styled("  z", Style::new().fg(th().border)),
                ]));
            }
            Row::Item(it) => {
                let c = dir_color(&it.cwd);
                let live = app.live(&it.id).is_some();
                let unseen = matches!(app.status(&it.id), Status::Idle { unseen: true });
                let age = data::age(it.mtime);
                let title = trunc(&it.title, w.saturating_sub(8 + age.width()));
                let mut title_style = Style::new().fg(if live || selected { Color::Reset } else { th().muted });
                if unseen || selected {
                    title_style = title_style.bold();
                }
                let mut spans = vec![
                    Span::styled("  │ ", Style::new().fg(c)),
                    status_glyph(app, &it.id),
                    Span::raw(" "),
                    Span::styled(title, title_style),
                ];
                pad_to(&mut spans, w, vec![Span::styled(format!("{age} "), Style::new().fg(th().faint))]);
                let mut line = Line::from(spans);
                if selected {
                    sel_line = lines.len();
                    line = line.style(Style::new().bg(sel_bg));
                }
                lines.push(line);
                // Matched inside the transcript: show where.
                if let Some(snip) = &it.snippet {
                    let mut line = highlight(
                        &Line::from(vec![
                            Span::styled("  │   ", Style::new().fg(c)),
                            Span::styled(trunc(snip, w.saturating_sub(7)), Style::new().fg(th().faint).italic()),
                        ]),
                        &app.search.query,
                    );
                    if selected {
                        line = line.style(Style::new().bg(sel_bg));
                    }
                    lines.push(line);
                }
            }
        }
        // Remember which row each visual line belongs to, for mouse hits.
        for l in &lines[before..] {
            line_rows.push((l.width() > 0).then_some(i));
        }
    }
    // Keep the selection on screen, with its directory header when possible.
    if sel_line < app.side_offset + 2 {
        app.side_offset = sel_line.saturating_sub(2);
    } else if sel_line >= app.side_offset + h {
        app.side_offset = sel_line + 1 - h;
    }
    app.side_offset = app.side_offset.min(lines.len().saturating_sub(h));
    let visible: Vec<Line> = lines.into_iter().skip(app.side_offset).take(h).collect();
    app.hits.list = list;
    app.hits.rows = line_rows.into_iter().skip(app.side_offset).take(h).collect();
    f.render_widget(Paragraph::new(visible), list);
}

fn draw_pane(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(Block::new().style(Style::new().bg(Color::Reset).fg(Color::Reset)), area);
    let [head, body] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    app.hits.pane = area;
    if app.selected.as_deref() == Some(NEW_CHAT) || app.selected_item().is_none() {
        return draw_new_chat(f, app, area);
    }
    let item = app.selected_item().cloned().unwrap();
    let c = dir_color(&item.cwd);

    // Title bar: directory chip, path and branch, chat title, status.
    let (label, label_color) = match app.status(&item.id) {
        Status::Busy => ("working".to_string(), th().accent),
        Status::Waiting(w) => (format!("needs you: {}", w.unwrap_or("input")), th().warning),
        Status::Idle { .. } => ("idle".into(), th().success),
        Status::External { pid, status } => (format!("in another terminal · pid {pid} · {status}"), th().external),
        Status::Dormant => ("⏎ resume".into(), th().faint),
    };
    let scrolled = app.live(&item.id).map(|l| l.parser.lock().unwrap().screen().scrollback()).unwrap_or(0);
    let w = head.width as usize;
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(format!(" {} ", basename(&item.cwd)), Style::new().bg(c).fg(th().inverse).bold()),
        Span::styled(chip_path(&item.cwd), Style::new().fg(c)),
    ];
    if let Some(b) = &item.branch {
        spans.push(Span::styled(format!("   {b}"), Style::new().fg(th().faint)));
    }
    // "working" shimmers like Claude Code's own spinner text.
    let mut right: Vec<Span> = if matches!(app.status(&item.id), Status::Busy) {
        colored(theme::shimmer(&label, th().accent, th().shimmer, app.tick), false)
    } else {
        vec![Span::styled(label, Style::new().fg(label_color))]
    };
    if scrolled > 0 {
        right.push(Span::styled(format!(" · ↑{scrolled}"), Style::new().fg(th().muted)));
    }
    right.push(Span::styled(format!("  {} ", &item.id[..8.min(item.id.len())]), Style::new().fg(th().border)));
    let used: usize = spans.iter().chain(&right).map(|s| s.content.width()).sum();
    let title = trunc(&item.title, w.saturating_sub(used + 6));
    spans.push(Span::styled("  ·  ", Style::new().fg(th().border)));
    spans.push(Span::styled(title, Style::new().fg(Color::Reset).bold()));
    pad_to(&mut spans, w, right);
    let rule = Line::from(Span::styled("─".repeat(w), Style::new().fg(th().border)));
    f.render_widget(Paragraph::new(vec![Line::from(spans), rule]), head);

    if let Some(l) = app.live(&item.id) {
        let p = l.parser.lock().unwrap();
        let screen = p.screen();
        render_screen(screen, body, f.buffer_mut());
        if app.mode == Mode::Insert && !screen.hide_cursor() && screen.scrollback() == 0 {
            let (r, col) = screen.cursor_position();
            f.set_cursor_position(Position::new(body.x + col, body.y + r));
        }
    } else {
        draw_preview(f, app, &item.id, body);
    }
}

fn greeting() -> String {
    static HOUR: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let hour = *HOUR.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%H")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
            .unwrap_or(12)
    });
    let part = match hour {
        5..12 => "Good morning",
        12..18 => "Good afternoon",
        _ => "Good evening",
    };
    let user = std::env::var("USER").unwrap_or_default();
    let mut c = user.chars();
    match c.next() {
        Some(first) => format!("{part}, {}{}", first.to_uppercase(), c.as_str()),
        None => part.into(),
    }
}

/// The claude.ai home screen: greeting, a prompt box, and the directory it'll run in.
fn draw_new_chat(f: &mut Frame, app: &App, area: Rect) {
    let composing = app.mode == Mode::Compose;
    let picker = app.picker.as_ref().filter(|_| composing);
    let dir = if composing || !app.compose_dir.as_os_str().is_empty() {
        app.compose_dir.clone()
    } else {
        app.selected_dir()
    };
    let c = dir_color(&dir);
    let bw = area.width.saturating_sub(8).min(76);
    let inner_w = bw.saturating_sub(4) as usize;
    let text_lines: Vec<String> = if app.compose.is_empty() {
        vec![String::new()]
    } else {
        app.compose.split('\n').flat_map(|l| wrap(l, inner_w.max(1))).collect()
    };
    let shown = text_lines.len().min(10);
    let bh = shown as u16 + 4;
    let list_h = picker.map(|p| p.items.len().clamp(1, PICKER_ROWS) as u16 + 2).unwrap_or(0);
    let total = bh + 5 + list_h;
    let x = area.x + (area.width.saturating_sub(bw)) / 2;
    let y = area.y + area.height.saturating_sub(total) / 2;

    let hello = greeting();
    let hw = hello.width() as u16 + 3;
    let gx = area.x + area.width.saturating_sub(hw) / 2;
    f.render_widget(
        Paragraph::new(Line::from(
            std::iter::once(Span::styled("✦  ", Style::new().fg(th().shimmer).bold()))
                .chain(colored(theme::gradient(&hello, th().accent, th().lilac), true))
                .collect::<Vec<_>>(),
        )),
        Rect::new(gx, y, hw.min(area.width), 1),
    );

    let boxr = Rect::new(x, y + 2, bw, bh.min(area.height.saturating_sub(2)));
    let border = if composing { th().accent } else { th().prompt_border };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .style(Style::new().bg(Color::Reset));
    let inner = block.inner(boxr).inner(ratatui::layout::Margin::new(1, 0));
    f.render_widget(block, boxr);
    let mut lines: Vec<Line> = Vec::new();
    if app.compose.is_empty() {
        let ph = if composing { "How can I help you today?" } else { "Press Enter to start a new chat" };
        lines.push(Line::from(Span::styled(ph, Style::new().fg(th().faint))));
    } else {
        let skip = text_lines.len() - shown;
        lines.extend(text_lines[skip..].iter().map(|l| Line::from(Span::styled(l.clone(), Style::new().fg(Color::Reset)))));
    }
    lines.push(Line::from(""));
    let chip_row = lines.len() as u16;
    let query_x;
    if let Some(p) = picker {
        // The directory chip turns into the picker's input.
        let label = " dir ";
        let q = trunc_left(&p.query, inner_w.saturating_sub(label.width() + 2));
        query_x = (label.width() + 1 + q.width()) as u16;
        let mut chip = vec![
            Span::styled(label, Style::new().bg(th().accent).fg(th().inverse).bold()),
            Span::raw(" "),
            Span::styled(q, Style::new().fg(Color::Reset)),
        ];
        if p.query.is_empty() {
            chip.push(Span::styled("type to search, or a path like ~/dev/", Style::new().fg(th().faint)));
        }
        lines.push(Line::from(chip));
    } else {
        query_x = 0;
        let mut chip = vec![
            Span::styled(format!(" {} ", basename(&dir)), Style::new().bg(c).fg(th().inverse).bold()),
            Span::styled(trunc_left(&chip_path(&dir), inner_w.saturating_sub(basename(&dir).width() + 14)), Style::new().fg(c)),
        ];
        pad_to(&mut chip, inner_w, vec![Span::styled(" ⏎ ", Style::new().bg(th().accent).fg(th().inverse).bold())]);
        lines.push(Line::from(chip));
    }
    f.render_widget(Paragraph::new(lines), inner);
    if picker.is_some() {
        f.set_cursor_position(Position::new(inner.x + query_x, inner.y + chip_row));
    } else if composing {
        let last = if app.compose.is_empty() { "" } else { text_lines.last().map(String::as_str).unwrap_or("") };
        let row = if app.compose.is_empty() { 0 } else { shown as u16 - 1 };
        f.set_cursor_position(Position::new(inner.x + last.width() as u16, inner.y + row));
    }

    let mut below = boxr.y + boxr.height;
    if let Some(p) = picker {
        let lr = Rect::new(x, below, bw, list_h.min((area.y + area.height).saturating_sub(below)));
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(th().border))
            .style(Style::new().bg(Color::Reset));
        let li = block.inner(lr);
        f.render_widget(block, lr);
        let w = li.width as usize;
        let rows = li.height as usize;
        let mut out: Vec<Line> = Vec::new();
        if p.items.is_empty() {
            out.push(Line::from(Span::styled(" no matching directory", Style::new().fg(th().faint).italic())));
        }
        // Keep the highlighted row on screen.
        let top = p.sel.saturating_sub(rows.saturating_sub(1));
        for (i, d) in p.items.iter().enumerate().skip(top).take(rows) {
            let selected = i == p.sel;
            let dc = dir_color(d);
            let name = basename(d);
            let parent = d.parent().filter(|_| name != "~").map(data::tilde).unwrap_or_default();
            let mut spans = vec![
                Span::styled(if selected { " ▸ " } else { "   " }, Style::new().fg(th().accent).bold()),
                Span::styled(name.clone(), Style::new().fg(dc).bold()),
                Span::raw("  "),
                Span::styled(trunc_left(&parent, w.saturating_sub(name.width() + 6)), Style::new().fg(th().faint)),
            ];
            pad_to(&mut spans, w, vec![]);
            let mut line = Line::from(spans);
            if selected {
                line = line.style(Style::new().bg(th().sel));
            }
            out.push(line);
        }
        f.render_widget(Paragraph::new(out), li);
        below = lr.y + lr.height;
    }

    let hint = if picker.is_some() {
        "↑↓ choose · ⏎ use it · Tab look inside · S-Tab up a level · Esc keep current"
    } else if composing {
        "Tab change directory · ⏎ start · Shift-⏎ newline · Esc back"
    } else {
        "⏎ or n new chat · o start without a prompt · O pick a directory"
    };
    let hy = below + 1;
    if hy < area.y + area.height {
        let hx = area.x + area.width.saturating_sub(hint.width() as u16) / 2;
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::new().fg(th().faint))),
            Rect::new(hx, hy, (hint.width() as u16).min(area.width), 1),
        );
    }
}

fn conv(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn render_screen(screen: &vt100::Screen, area: Rect, buf: &mut Buffer) {
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut style = Style::new().fg(conv(cell.fgcolor(), Color::Reset)).bg(conv(cell.bgcolor(), Color::Reset));
            if cell.bold() {
                style = style.bold();
            }
            if cell.dim() {
                style = style.dim();
            }
            if cell.italic() {
                style = style.italic();
            }
            if cell.underline() {
                style = style.underlined();
            }
            if cell.inverse() {
                style = style.reversed();
            }
            let sym = if cell.has_contents() { cell.contents() } else { " " };
            if let Some(out) = buf.cell_mut(Position::new(area.x + col, area.y + row)) {
                out.set_symbol(sym).set_style(style);
            }
        }
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        let mut lw = 0;
        for word in para.split(' ') {
            let ww = word.width();
            if lw > 0 && lw + 1 + ww > width {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            if lw > 0 {
                line.push(' ');
                lw += 1;
            }
            // Hard-split words longer than the line.
            for ch in word.chars() {
                let cw = ch.width().unwrap_or(0);
                if lw + cw > width && lw > 0 {
                    out.push(std::mem::take(&mut line));
                    lw = 0;
                }
                line.push(ch);
                lw += cw;
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Inline markdown: **bold** and `code`. State carries across wrapped lines.
fn inline_md(line: &str, bold: &mut bool, code: &mut bool, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut cur = String::new();
    let style = |b: bool, c: bool| {
        let mut s = base;
        if c {
            s = s.fg(th().code);
        }
        if b {
            s = s.bold();
        }
        s
    };
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '`' {
            spans.push(Span::styled(std::mem::take(&mut cur), style(*bold, *code)));
            *code = !*code;
        } else if ch == '*' && chars.peek() == Some(&'*') && !*code {
            chars.next();
            spans.push(Span::styled(std::mem::take(&mut cur), style(*bold, *code)));
            *bold = !*bold;
        } else {
            cur.push(ch);
        }
    }
    spans.push(Span::styled(cur, style(*bold, *code)));
    spans
}

/// Assistant text: light markdown (headings, bullets, fences, inline styles).
fn render_markdown(text: &str, width: usize, out: &mut Vec<Line<'static>>, indent: &str) {
    let mut fence = false;
    for raw in text.lines() {
        if raw.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            let t = trunc(raw, width.saturating_sub(2));
            let pad = width.saturating_sub(t.width() + 2);
            out.push(Line::from(vec![
                Span::raw(indent.to_string()),
                Span::styled(format!(" {t}{} ", " ".repeat(pad)), Style::new().bg(th().bubble).fg(th().muted)),
            ]));
            continue;
        }
        let (body, base, bullet) = if let Some(h) = raw.trim_start().strip_prefix('#') {
            (h.trim_start_matches('#').trim().to_string(), Style::new().fg(Color::Reset).bold(), "")
        } else if let Some(b) = raw.strip_prefix("- ").or_else(|| raw.strip_prefix("* ")) {
            (b.to_string(), Style::new().fg(Color::Reset), "• ")
        } else {
            (raw.to_string(), Style::new().fg(Color::Reset), "")
        };
        let (mut bold, mut code) = (false, false);
        for (i, l) in wrap(&body, width.saturating_sub(bullet.width())).into_iter().enumerate() {
            let lead = if i == 0 { bullet.to_string() } else { " ".repeat(bullet.width()) };
            let mut spans = vec![Span::raw(indent.to_string()), Span::styled(lead, Style::new().fg(th().accent))];
            spans.extend(inline_md(&l, &mut bold, &mut code, base));
            out.push(Line::from(spans));
        }
    }
}

fn draw_preview(f: &mut Frame, app: &mut App, id: &str, area: Rect) {
    let Some(file) = app.store.sessions.get(id).map(|s| s.file.clone()) else {
        f.render_widget(Paragraph::new(Span::styled("  (no transcript yet)", Style::new().fg(th().faint))), area);
        return;
    };
    let len = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    if !matches!(&app.preview, Some((pid, plen, _)) if pid == id && *plen == len) {
        app.preview = Some((id.into(), len, data::transcript(&file)));
    }
    // A centred reading column, like the website.
    let col_w = area.width.saturating_sub(4).min(96);
    let margin = " ".repeat(((area.width - col_w) / 2) as usize);
    let cw = col_w as usize;
    if !matches!(&app.wrapped, Some((wid, wlen, ww, _, _)) if wid == id && *wlen == len && *ww == area.width) {
        let entries = &app.preview.as_ref().unwrap().2;
        let initial = std::env::var("USER").ok().and_then(|u| u.chars().next()).unwrap_or('U').to_ascii_uppercase();
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut owners: Vec<Option<usize>> = Vec::new();
        let mut after_user = true;
        for (ei, e) in entries.iter().enumerate() {
            match e {
                Entry::User(t) => {
                    lines.push(Line::from(""));
                    let bubble_w = cw.saturating_sub(6).min(t.lines().map(|l| l.width()).max().unwrap_or(0) + 2).max(4);
                    let wrapped: Vec<String> = t.lines().flat_map(|l| wrap(l, bubble_w - 2)).collect();
                    for (i, l) in wrapped.iter().enumerate() {
                        let avatar = if i == 0 {
                            Span::styled(format!(" {initial} "), Style::new().bg(th().sel).fg(th().user).bold())
                        } else {
                            Span::raw("   ")
                        };
                        let pad = bubble_w.saturating_sub(l.width() + 2);
                        lines.push(Line::from(vec![
                            Span::raw(margin.clone()),
                            avatar,
                            Span::raw(" "),
                            Span::styled(format!(" {l}{} ", " ".repeat(pad)), Style::new().bg(th().bubble).fg(th().user)),
                        ]));
                    }
                    lines.push(Line::from(""));
                    after_user = true;
                }
                Entry::Assistant(t) => {
                    let mut block = Vec::new();
                    render_markdown(t, cw.saturating_sub(4), &mut block, "");
                    for (i, mut l) in block.into_iter().enumerate() {
                        let lead = if i == 0 && after_user {
                            Span::styled("✻   ", Style::new().fg(th().accent).bold())
                        } else {
                            Span::raw("    ")
                        };
                        l.spans.insert(0, lead);
                        l.spans.insert(0, Span::raw(margin.clone()));
                        lines.push(l);
                    }
                    lines.push(Line::from(""));
                    after_user = false;
                }
                Entry::Tool(t) => {
                    let lead = if after_user { "✻   " } else { "    " };
                    lines.push(Line::from(vec![
                        Span::raw(margin.clone()),
                        Span::styled(lead, Style::new().fg(th().accent).bold()),
                        Span::styled(format!("⚙ {}", trunc(t, cw.saturating_sub(6))), Style::new().fg(th().faint)),
                    ]));
                    after_user = false;
                }
            }
            let owner = (!matches!(e, Entry::Tool(_))).then_some(ei);
            owners.resize(lines.len(), owner);
        }
        app.wrapped = Some((id.into(), len, area.width, lines, owners));
    }
    let (_, _, _, lines, owners) = app.wrapped.as_ref().unwrap();
    let h = area.height as usize;
    let max_scroll = lines.len().saturating_sub(h);
    if app.jump_to_match.as_deref() == Some(id) {
        let q = &app.search.query;
        if app.search.hits.contains_key(id) {
            // Put the match near the top, with a little context above it.
            if let Some(m) = match_line(app.preview.as_ref().unwrap().2.as_slice(), lines, owners, q) {
                app.preview_scroll = lines.len().saturating_sub(m.saturating_sub(2) + h);
            }
            app.jump_to_match = None;
        } else if !app.search.running || q.chars().count() < crate::search::MIN_QUERY {
            app.jump_to_match = None;
        }
    }
    app.preview_scroll = app.preview_scroll.min(max_scroll);
    let end = lines.len() - app.preview_scroll;
    let start = end.saturating_sub(h);
    let q = &app.search.query;
    let searching = app.search.hits.contains_key(id);
    let visible = (start..end)
        .map(|i| if searching && owners[i].is_some() { highlight(&lines[i], q) } else { lines[i].clone() })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(visible), area);
}

/// Mark every case-insensitive occurrence of `needle` (lowercase) in the
/// line. Matches split across spans (e.g. half bold) aren't marked.
fn highlight(line: &Line<'static>, needle: &str) -> Line<'static> {
    if needle.is_empty() {
        return line.clone();
    }
    let mark = Style::new().bg(th().warning).fg(th().inverse).bold();
    let mut spans = Vec::new();
    for span in &line.spans {
        let text = span.content.as_ref();
        // Lowercase char by char, remembering each original char's byte range,
        // since lowercasing can change byte lengths.
        let mut lower = String::new();
        let mut orig: Vec<(usize, usize)> = Vec::new();
        for (i, c) in text.char_indices() {
            for l in c.to_lowercase() {
                for _ in 0..l.len_utf8() {
                    orig.push((i, i + c.len_utf8()));
                }
                lower.push(l);
            }
        }
        let mut done = 0;
        for (at, _) in lower.match_indices(needle) {
            let (s, _) = orig[at];
            let (_, e) = orig[at + needle.len() - 1];
            if s < done {
                continue;
            }
            if s > done {
                spans.push(Span::styled(text[done..s].to_string(), span.style));
            }
            spans.push(Span::styled(text[s..e].to_string(), span.style.patch(mark)));
            done = e;
        }
        if done == 0 {
            spans.push(span.clone());
        } else if done < text.len() {
            spans.push(Span::styled(text[done..].to_string(), span.style));
        }
    }
    Line { spans, ..line.clone() }
}

/// First rendered line showing `needle` (lowercase) in a user or assistant
/// message. A match split by wrapping falls back to the top of its message.
fn match_line(entries: &[Entry], lines: &[Line], owners: &[Option<usize>], needle: &str) -> Option<usize> {
    let hit = |i: usize| {
        owners[i].is_some() && lines[i].spans.iter().map(|s| s.content.as_ref()).collect::<String>().to_lowercase().contains(needle)
    };
    (0..lines.len()).find(|&i| hit(i)).or_else(|| {
        let e = entries.iter().position(|e| match e {
            Entry::User(t) | Entry::Assistant(t) => t.to_lowercase().contains(needle),
            Entry::Tool(_) => false,
        })?;
        owners.iter().position(|&o| o == Some(e))
    })
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let (badge, color) = match app.mode {
        Mode::Normal => ("NOR", th().accent),
        Mode::Insert => ("INS", th().success),
        Mode::Compose => ("NEW", th().accent),
        Mode::Command => ("CMD", th().lilac),
        Mode::Search => ("FIND", th().warning),
        Mode::Space => ("SPC", th().external),
        Mode::Goto => ("GOTO", th().external),
        Mode::Window => ("WIN", th().external),
    };
    let hint = Style::new().fg(th().faint);
    let mut spans = vec![Span::styled(format!(" {badge} "), Style::new().bg(color).fg(th().inverse).bold()), Span::raw(" ")];
    let mut cursor = None;
    match app.mode {
        Mode::Command => {
            spans.push(Span::styled(format!(":{}", app.cmdline), Style::new().fg(Color::Reset)));
            cursor = Some(app.cmdline.width() + 1);
        }
        Mode::Search => {
            spans.push(Span::styled(format!("/{}", app.filter), Style::new().fg(Color::Reset)));
            cursor = Some(app.filter.width() + 1);
            if app.search.running {
                spans.push(Span::styled("  searching transcripts…", hint));
            }
        }
        _ => match &app.msg {
            Some((m, true)) => spans.push(Span::styled(m.clone(), Style::new().fg(th().error))),
            Some((m, false)) => spans.push(Span::styled(m.clone(), Style::new().fg(th().muted))),
            None if app.keymap => {}
            None => spans.push(Span::styled(
                match (app.mode, app.focus) {
                    (Mode::Insert, _) => "typing into claude · Ctrl-\\ back to chats",
                    (Mode::Compose, _) => "writing a new chat · ⏎ start · Esc back",
                    (_, Focus::Pane) => "↑↓ scroll · shift ↑↓ half page · Home/End · ⏎ type · ← back to chats",
                    _ => "↑↓ chat · ←→ directory · ⏎ open · n new chat · Ctrl-→ pane · d kill · / search · space menu · ? keys",
                },
                hint,
            )),
        },
    }
    let mut right = format!("{} live · {} chats ", app.lives.len(), app.store.visible().count());
    if !app.filter.is_empty() && app.mode != Mode::Search {
        right = format!("/{} · {right}", app.filter);
    }
    if app.live_only {
        right = format!("live-only · {right}");
    }
    // Messages and hints give way to the counters on narrow terminals.
    let room = (area.width as usize).saturating_sub(badge.len() + 3 + right.width());
    if let Some(last) = spans.last_mut().filter(|_| app.mode != Mode::Command && app.mode != Mode::Search) {
        let t = trunc(&last.content, room);
        last.content = t.into();
    }
    pad_to(&mut spans, area.width as usize, vec![Span::styled(right, hint)]);
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(Color::Reset)), area);
    if let Some(c) = cursor {
        f.set_cursor_position(Position::new(area.x + badge.len() as u16 + 3 + c as u16, area.y));
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th().prompt_border))
        .title(Span::styled(format!(" {title} "), Style::new().fg(th().accent).bold()))
        .style(Style::new().bg(Color::Reset).fg(Color::Reset))
}

fn popup(f: &mut Frame, title: &str, items: &[(&str, &str)]) {
    let w = items.iter().map(|(k, d)| k.width() + d.width() + 5).max().unwrap_or(10) as u16 + 2;
    let h = items.len() as u16 + 2;
    let a = f.area();
    let r = Rect::new(a.width.saturating_sub(w + 1), a.height.saturating_sub(h + 1), w.min(a.width), h.min(a.height));
    let lines: Vec<Line> = items
        .iter()
        .map(|(k, d)| {
            Line::from(vec![Span::styled(format!(" {k:<2} "), Style::new().fg(th().accent).bold()), Span::styled(*d, Style::new().fg(Color::Reset))])
        })
        .collect();
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(panel(title)), r);
}

fn draw_help(f: &mut Frame) {
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "chat list",
            &[
                ("↑ ↓", "move between chats"),
                ("→ ←", "next / previous directory"),
                ("Home End", "first / last chat"),
                ("⏎", "open: type into it / resume it"),
                ("n  Ctrl-n", "new chat (write the first message)"),
                ("o", "new chat in this dir, no prompt"),
                ("O", "new chat, pick the directory first"),
                ("d", "kill live instance"),
                ("z", "expand / fold a directory"),
                ("Tab S-Tab", "cycle live chats"),
                ("Ctrl-→ Ctrl-←", "focus pane / chat list"),
                ("Ctrl-w ← →", "same, helix window style (also g← g→)"),
                ("/", "search titles, dirs and chat text"),
                ("space", "menu"),
            ],
        ),
        (
            "pane focused",
            &[
                ("↑ ↓", "scroll 3 lines (shift: half page)"),
                ("Home End", "top / bottom"),
                ("⏎", "start typing into claude"),
                ("← Esc", "back to chat list"),
            ],
        ),
        ("typing", &[("Ctrl-\\", "back to chats (everything else goes to claude)")]),
        (
            "mouse",
            &[
                ("wheel on list", "next / previous chat"),
                ("click", "select · click again to open"),
                ("wheel on pane", "scroll the pane"),
                ("Shift-drag", "select text (terminal's own)"),
            ],
        ),
        (
            "commands",
            &[
                (":new [dir]", "new chat; dir can be fuzzy (:new cdeck)"),
                (":open [dir]", "start claude there, no prompt (fuzzy too)"),
                (":kill  :live", "kill instance / live-only view"),
                (":resume!", "resume even if running elsewhere"),
                (":q  :q!", "quit / quit killing instances"),
            ],
        ),
        (
            "status",
            &[
                ("✻", "working"),
                ("◐", "waiting on you (permission/input)"),
                ("●", "idle (bold title: finished while away)"),
                ("◆", "running in another terminal"),
            ],
        ),
    ];
    let mut lines = Vec::new();
    for (name, items) in sections {
        lines.push(Line::from(Span::styled(format!(" {name}"), Style::new().fg(th().muted).bold())));
        for (k, d) in *items {
            lines.push(Line::from(vec![
                Span::styled(format!("   {k:<15}"), Style::new().fg(th().accent)),
                Span::styled(*d, Style::new().fg(Color::Reset)),
            ]));
        }
        lines.push(Line::from(""));
    }
    let a = f.area();
    let w = 66.min(a.width);
    let h = (lines.len() as u16 + 2).min(a.height);
    let r = Rect::new((a.width - w) / 2, (a.height - h) / 2, w, h);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(panel("cdeck — any key closes")), r);
}

/// The full path shown after a directory chip, unless the chip already says it all (`~`).
fn chip_path(p: &Path) -> String {
    let t = data::tilde(p);
    if t == basename(p) { String::new() } else { format!(" {t}") }
}

fn colored(chars: Vec<(String, Color)>, bold: bool) -> Vec<Span<'static>> {
    chars
        .into_iter()
        .map(|(s, c)| Span::styled(s, if bold { Style::new().fg(c).bold() } else { Style::new().fg(c) }))
        .collect()
}

/// What the which-key strip lists for the current mode and focus.
fn keymap_entries(app: &App) -> (&'static str, &'static [(&'static str, &'static str)]) {
    match (app.mode, app.focus) {
        (Mode::Insert, _) => (
            "typing into claude",
            &[("Ctrl-\\", "back to chats"), ("Esc", "interrupt claude"), ("Shift-⏎", "newline (claude)"), ("everything else", "goes to claude")],
        ),
        (Mode::Compose, _) if app.picker.is_some() => (
            "pick directory",
            &[
                ("type", "fuzzy recent dirs"),
                ("~/ / ./", "browse a path"),
                ("↑ ↓", "choose"),
                ("⏎", "use it"),
                ("Tab", "look inside"),
                ("S-Tab C-w", "up a level"),
                ("Ctrl-u", "clear"),
                ("Esc", "keep current"),
            ],
        ),
        (Mode::Compose, _) => (
            "new chat",
            &[
                ("⏎", "start chat"),
                ("Shift-⏎ C-j", "newline"),
                ("Tab", "change directory…"),
                ("Ctrl-u", "clear"),
                ("Ctrl-w", "delete word"),
                ("Esc", "back"),
            ],
        ),
        (Mode::Command, _) => (
            ": command",
            &[
                ("new [dir]", "new chat"),
                ("open [dir]", "start, no prompt"),
                ("kill", "kill instance"),
                ("live", "live-only view"),
                ("resume!", "resume anyway"),
                ("q / q!", "quit / force"),
                ("Tab", "complete dir"),
                ("⏎ / Esc", "run / cancel"),
            ],
        ),
        (Mode::Search, _) => (
            "/ search",
            &[("type", "titles, dirs + chat text"), ("⏎", "keep filter"), ("Esc", "clear filter"), ("Ctrl-u", "clear text")],
        ),
        (Mode::Space, _) => (
            "space",
            &[
                ("n", "new chat…"),
                ("o", "new here, no prompt"),
                ("f", "search"),
                ("k", "kill instance"),
                ("l", "live-only"),
                ("r", "rescan"),
                ("?", "full help"),
                ("q", "quit"),
            ],
        ),
        (Mode::Goto, _) => (
            "g",
            &[("g", "first chat"), ("e", "last chat"), ("n", "next dir"), ("p", "prev dir"), ("←", "focus list"), ("→", "focus pane")],
        ),
        (Mode::Window, _) => ("Ctrl-w", &[("←", "focus list"), ("→", "focus pane"), ("w", "swap focus")]),
        (Mode::Normal, Focus::Pane) => (
            "pane",
            &[
                ("↑ ↓", "scroll"),
                ("Shift-↑↓", "half page"),
                ("Home End", "top / bottom"),
                ("⏎", "type into claude"),
                ("← Esc", "back to list"),
                ("Tab", "next live chat"),
                ("n", "new chat"),
                ("d", "kill"),
                ("space", "menu…"),
                ("?", "hide this map"),
            ],
        ),
        (Mode::Normal, Focus::Sidebar) => (
            "chats",
            &[
                ("↑ ↓", "chat"),
                ("← →", "directory"),
                ("⏎", "open / resume"),
                ("n", "new chat"),
                ("o", "new, no prompt"),
                ("O", "new, pick dir"),
                ("d", "kill"),
                ("z", "expand dir"),
                ("Tab", "next live chat"),
                ("Ctrl-→", "focus pane"),
                ("/", "search"),
                ("space", "menu…"),
                ("g  Ctrl-w", "goto… window…"),
                (":", "command…"),
                (":q", "quit"),
                ("?", "hide this map"),
            ],
        ),
    }
}

/// Emacs which-key style cheat sheet: a rule, then `key → action` in columns.
fn draw_keymap(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }
    let (title, entries) = keymap_entries(app);
    let w = area.width as usize;
    let head = format!("─ {title} ");
    let rule = Line::from(vec![
        Span::styled("─ ", Style::new().fg(th().border)),
        Span::styled(title, Style::new().fg(th().accent).bold()),
        Span::styled(format!(" {}", "─".repeat(w.saturating_sub(head.width()))), Style::new().fg(th().border)),
    ]);
    let rows = KEYMAP_ROWS as usize;
    let key_w = entries.iter().map(|(k, _)| k.width()).max().unwrap_or(1);
    let cell_w = entries.iter().map(|(_, d)| d.width()).max().unwrap_or(1) + key_w + 6;
    // Fill column by column; drop whole columns that don't fit.
    let cols = (w.saturating_sub(1) / cell_w).max(1).min(entries.len().div_ceil(rows));
    let col_w = w.saturating_sub(1) / cols;
    let mut lines = vec![rule];
    for r in 0..rows {
        let mut spans = vec![Span::raw(" ")];
        for c in 0..cols {
            let Some((k, d)) = entries.get(c * rows + r) else { continue };
            let desc = trunc(d, col_w.saturating_sub(key_w + 5));
            spans.push(Span::styled(format!(" {k:>key_w$}"), Style::new().fg(th().accent).bold()));
            spans.push(Span::styled(" → ", Style::new().fg(th().border)));
            spans.push(Span::styled(desc.clone(), Style::new().fg(th().muted)));
            spans.push(Span::raw(" ".repeat(col_w.saturating_sub(key_w + 4 + desc.width()))));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), area);
}
