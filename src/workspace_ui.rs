//! Shared floating surfaces and the workspace compositor; no filesystem access.
use crate::{
    animation::Visuals,
    app::{App, Picker},
    config::Preset,
    navigation::Preview,
    syntax::Highlight,
    theme::Theme,
    ui,
    workspace::BufferId,
};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use std::{collections::BTreeMap, time::Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn border<'a>(
    title: impl Into<Line<'a>>,
    theme: &Theme,
    unicode: bool,
    minimal: bool,
    focused: bool,
) -> Block<'a> {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(if unicode && !minimal {
            BorderType::Rounded
        } else {
            BorderType::Plain
        })
        .border_style(Style::default().fg(if focused { theme.iris } else { theme.overlay }))
        .style(Style::default().bg(theme.surface).fg(theme.text));
    if unicode {
        block
    } else {
        block.border_set(crate::ui::ASCII_BORDER)
    }
}
fn floating(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    theme: &Theme,
    unicode: bool,
    minimal: bool,
) -> Rect {
    frame.render_widget(Clear, area);
    let block = border(format!(" {title} "), theme, unicode, minimal, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}
pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    theme: &Theme,
    spans: &BTreeMap<BufferId, Vec<Highlight>>,
    visuals: &mut Visuals,
    pending: &str,
    now: Instant,
) {
    app.workspace.stash();
    let area = frame.area();
    ui::chrome(frame, app.workspace.editor(), theme, pending);
    if area.width < 12 || area.height < 6 {
        visuals.reset();
        return;
    }
    let unicode = app.workspace.config.unicode;
    let minimal = app.workspace.config.preset == Preset::Minimal;
    let focused = app.workspace.focused();
    let active = app.workspace.active_buffer();
    let mode = app.mode();
    let mut tabs = vec![Span::styled(
        " FILAVIM ",
        Style::default().fg(theme.base).bg(theme.iris).bold(),
    )];
    for (i, tab) in app.workspace.tabs.iter().enumerate() {
        let editor = &app.workspace.buffers[&app.workspace.views[&tab.focused].buffer];
        let name = editor
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| "Untitled".into(), |n| clean(&n.to_string_lossy()));
        let selected = i == app.workspace.active_tab;
        tabs.push(Span::styled(
            format!(
                "  {} {name}{}  ",
                i + 1,
                if editor.buffer.dirty() { " +" } else { "" }
            ),
            Style::default()
                .fg(if selected { theme.text } else { theme.muted })
                .bg(if selected {
                    theme.overlay
                } else {
                    theme.surface
                }),
        ));
    }
    frame.render_widget(Clear, Rect::new(0, 0, area.width, 1));
    frame.render_widget(
        Paragraph::new(Line::from(tabs)).style(Style::default().bg(theme.surface)),
        Rect::new(0, 0, area.width, 1),
    );
    let ids: Vec<_> = app.workspace.buffers.keys().copied().collect();
    let active_at = ids.iter().position(|id| *id == active).unwrap_or(0);
    let mut buffers = vec![Span::styled(" ", Style::default())];
    let start = active_at.saturating_sub(2);
    for id in &ids[start..] {
        let editor = &app.workspace.buffers[id];
        let name = editor
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(
                || format!("Untitled #{id}"),
                |n| clean(&n.to_string_lossy()),
            );
        buffers.push(Span::styled(
            format!(" {name}{} ", if editor.buffer.dirty() { " +" } else { "" }),
            Style::default()
                .fg(if *id == active {
                    theme.foam
                } else {
                    theme.muted
                })
                .bg(theme.base),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(buffers)),
        Rect::new(0, 1, area.width, 1),
    );
    let mut body = Rect::new(0, 2, area.width, area.height - 4);
    body = crate::ide_ui::terminal_panel(frame, app, body, theme);
    if app.explorer.visible && area.width >= 42 {
        let width = app
            .workspace
            .config
            .navigation
            .explorer_width
            .min(area.width / 2);
        explorer(
            frame,
            Rect::new(body.x, body.y, width, body.height),
            app,
            theme,
        );
        body.x += width;
        body.width -= width;
    }
    let mut panels = Vec::new();
    app.workspace.tabs[app.workspace.active_tab]
        .layout
        .layout(body, &mut panels);
    let context_area = panels
        .iter()
        .find(|(id, _)| *id == focused)
        .map_or(body, |(_, a)| *a);
    let editing = app.picker.is_none()
        && app.dialog.is_none()
        && !app.explorer.focused
        && !app.workspace.editor().help;
    let editing = editing && !app.ide.focused();
    if editing {
        visuals.context(focused, active, mode as u8, context_area);
    } else {
        visuals.reset();
    }
    let mut cursor = None;
    for (view, rect) in panels {
        app.workspace.load(view);
        let buffer = app.workspace.views[&view].buffer;
        let editor = app.workspace.buffers.get_mut(&buffer).unwrap();
        if editor.dashboard {
            ui::dashboard(frame, rect, theme, unicode);
        } else {
            let highlights = spans.get(&buffer).map_or(&[][..], Vec::as_slice);
            let position = ui::panel(
                frame,
                rect,
                editor,
                theme,
                highlights,
                view == focused && editing,
                &mut |top, row, height| {
                    if editing {
                        visuals.scroll(view, top, row, height, now)
                    } else {
                        top
                    }
                },
            );
            if view == focused {
                cursor = position;
            }
            crate::ide_ui::markers(
                frame,
                editor,
                &app.ide,
                rect,
                position.map_or(editor.top, |p| p.origin.1),
                theme,
            );
        }
        app.workspace.capture(view);
    }
    app.workspace.load(focused);
    crate::ide_ui::draw(frame, app, theme);
    if editing {
        crate::contextual_ui::draw(frame, app, theme, cursor);
    }
    if editing {
        visuals.cache_scene(frame.buffer_mut(), cursor);
        visuals.cursor(cursor, now, theme, frame);
    }
    if app.explorer.focused || app.picker.is_some() || app.dialog.is_some() {
        frame.render_widget(
            Paragraph::new(format!("  {}  ", mode.label()))
                .style(Style::default().bg(theme.iris).fg(theme.base).bold()),
            Rect::new(
                0,
                area.height - 2,
                (mode.label().len() as u16 + 4).min(area.width),
                1,
            ),
        );
    }
    if app.explorer.focused && area.width < 42 {
        explorer(frame, body, app, theme);
    }
    if app.workspace.editor().help {
        ui::help(
            frame,
            area,
            theme,
            unicode,
            app.workspace.editor().help_page,
        );
    }
    if let Some(p) = &mut app.picker {
        picker(frame, area, p, theme, unicode, minimal, app.indexing);
    }
    if let Some(dialog) = &app.dialog {
        let width = area.width.saturating_sub(4).clamp(1, 82);
        let height = area.height.saturating_sub(2).clamp(1, 9);
        let popup = Rect::new(
            (area.width - width) / 2,
            (area.height - height) / 2,
            width,
            height,
        );
        let inner = floating(frame, popup, &dialog.title, theme, unicode, minimal);
        if inner.height > 0 {
            frame.render_widget(
                Paragraph::new(clean(&dialog.prompt))
                    .style(Style::default().fg(theme.gold))
                    .wrap(ratatui::widgets::Wrap { trim: false }),
                inner,
            );
        }
        if inner.height >= 3 {
            input_line(
                frame,
                Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
                &dialog.input,
                theme,
            );
        }
        if inner.height >= 4 {
            frame.render_widget(
                Paragraph::new(" Enter confirm  ·  Esc cancel")
                    .style(Style::default().fg(theme.muted)),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
        }
    }
}
fn explorer(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let unicode = app.workspace.config.unicode;
    let root = app.workspace.root.file_name().map_or_else(
        || app.workspace.root.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    let block = border(
        format!(" {}{} ", clean(&root), if app.indexing { " …" } else { "" }),
        theme,
        unicode,
        app.workspace.config.preset == Preset::Minimal,
        app.explorer.focused,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let height = inner.height.saturating_sub(2).max(1) as usize;
    if app.explorer.selected < app.explorer.top {
        app.explorer.top = app.explorer.selected;
    }
    if app.explorer.selected >= app.explorer.top + height {
        app.explorer.top = app.explorer.selected + 1 - height;
    }
    for (offset, row) in app
        .explorer
        .rows
        .iter()
        .skip(app.explorer.top)
        .take(height)
        .enumerate()
    {
        let at = app.explorer.top + offset;
        let selected = at == app.explorer.selected;
        let icon = if unicode && app.workspace.config.nerd_fonts {
            if row.entry.directory {
                "\u{f07b} "
            } else {
                "\u{f15b} "
            }
        } else if row.entry.directory {
            if row.expanded {
                if unicode { "▾ " } else { "- " }
            } else if unicode {
                "▸ "
            } else {
                "+ "
            }
        } else {
            "  "
        };
        let marker = if selected {
            if unicode { "›" } else { ">" }
        } else {
            " "
        };
        let name = row
            .entry
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let line = format!(
            "{marker}{}{icon}{}{}",
            "  ".repeat(row.depth.min(12)),
            clean(&name),
            app.ide
                .git
                .normalized_files
                .get(&crate::lsp::path_key(&row.entry.path))
                .map_or(String::new(), |status| format!(" [{status}]"))
        );
        let style = Style::default()
            .fg(if selected {
                theme.text
            } else if row.entry.directory {
                theme.foam
            } else {
                theme.muted
            })
            .bg(if selected {
                theme.overlay
            } else {
                theme.surface
            });
        frame.render_widget(
            Paragraph::new(line).style(style),
            Rect::new(inner.x, inner.y + offset as u16, inner.width, 1),
        );
    }
    if app.explorer.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.indexing {
                " Indexing…"
            } else {
                " Empty workspace"
            })
            .style(Style::default().fg(theme.muted)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    if inner.height >= 3 {
        frame.render_widget(
            Paragraph::new(" I new  R rename  X delete\n WASD move · Esc editor")
                .style(Style::default().fg(theme.muted)),
            Rect::new(inner.x, inner.bottom() - 2, inner.width, 2),
        );
    }
}
fn input_line(frame: &mut Frame, area: Rect, text: &str, theme: &Theme) {
    if area.width < 2 || area.height == 0 {
        return;
    }
    let available = area.width.saturating_sub(3) as usize;
    let mut tail = String::new();
    let mut width = 0;
    for g in text.graphemes(true).rev() {
        let w = UnicodeWidthStr::width(g).max(1);
        if width + w > available {
            break;
        }
        tail.insert_str(0, g);
        width += w;
    }
    frame.render_widget(
        Paragraph::new(format!(" > {}", clean(&tail)))
            .style(Style::default().fg(theme.foam).bg(theme.surface)),
        area,
    );
    frame.set_cursor_position((
        area.x + 3.min(area.width - 1) + (width as u16).min(area.width.saturating_sub(4)),
        area.y,
    ));
}
fn marked(
    text: &str,
    indices: &[u32],
    theme: &Theme,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let graphemes: Vec<_> = text.graphemes(true).collect();
    let last = indices.last().copied().unwrap_or(0) as usize;
    let start = if last > width.saturating_sub(6) {
        last.saturating_sub(width.saturating_sub(6))
    } else {
        0
    };
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled("…", Style::default().fg(theme.muted)));
    }
    for (i, g) in graphemes.iter().enumerate().skip(start).take(width) {
        let hit = indices.binary_search(&(i as u32)).is_ok();
        let mut style = Style::default().fg(if hit {
            theme.gold
        } else if selected {
            theme.text
        } else {
            theme.muted
        });
        if hit {
            style = style.bold();
        }
        spans.push(Span::styled(clean(g), style));
    }
    Line::from(spans)
}
fn picker(
    frame: &mut Frame,
    area: Rect,
    p: &mut Picker,
    theme: &Theme,
    unicode: bool,
    minimal: bool,
    indexing: bool,
) {
    let width = area
        .width
        .saturating_sub(if area.width > 30 { 6 } else { 0 })
        .min(150);
    let height = area
        .height
        .saturating_sub(if area.height > 12 { 4 } else { 0 });
    let popup = Rect::new(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    );
    let inner = floating(frame, popup, p.kind.title(), theme, unicode, minimal);
    if inner.width < 3 || inner.height < 3 {
        return;
    }
    input_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        &p.query,
        theme,
    );
    let show_preview = inner.width >= 75 && inner.height >= 8;
    let list_width = if show_preview {
        inner.width * 53 / 100
    } else {
        inner.width
    };
    let list = Rect::new(
        inner.x,
        inner.y + 2,
        list_width,
        inner.height.saturating_sub(4),
    );
    let page = list.height.max(1) as usize;
    if p.selected < p.top {
        p.top = p.selected;
    }
    if p.selected >= p.top + page {
        p.top = p.selected + 1 - page;
    }
    for (offset, result) in p
        .results
        .iter()
        .skip(p.top)
        .take(list.height as usize)
        .enumerate()
    {
        let selected = p.top + offset == p.selected;
        let marker = if selected {
            if unicode { "› " } else { "> " }
        } else {
            "  "
        };
        let mut line = marked(
            &result.item.label,
            &result.indices,
            theme,
            selected,
            list.width.saturating_sub(3) as usize,
        );
        line.spans
            .insert(0, Span::styled(marker, Style::default().fg(theme.iris)));
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(if selected {
                theme.overlay
            } else {
                theme.surface
            })),
            Rect::new(list.x, list.y + offset as u16, list.width, 1),
        );
    }
    if p.results.is_empty() && list.height > 0 {
        frame.render_widget(
            Paragraph::new(if p.busy || indexing {
                " Searching…"
            } else if p.query.is_empty() {
                " Start typing to search"
            } else {
                " No results"
            })
            .style(Style::default().fg(theme.muted)),
            list,
        );
    }
    if show_preview {
        let preview_area = Rect::new(
            inner.x + list_width + 1,
            inner.y + 1,
            inner.width - list_width - 1,
            inner.height - 3,
        );
        preview(frame, preview_area, &p.preview, theme, unicode, minimal);
    }
    let state = if p.busy { "working" } else { "ready" };
    let detail = if p.message.is_empty() {
        format!(
            "{} results · {state}{}",
            p.results.len(),
            if indexing { " · indexing" } else { "" }
        )
    } else {
        p.message.clone()
    };
    frame.render_widget(
        Paragraph::new(format!(" {detail}")).style(Style::default().fg(theme.muted)),
        Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(" ↑↓ / Ctrl-W,S navigate  ·  Enter open  ·  Ctrl-R refresh  ·  Esc close")
            .style(Style::default().fg(theme.muted)),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
}
fn preview(
    frame: &mut Frame,
    area: Rect,
    p: &Preview,
    theme: &Theme,
    unicode: bool,
    minimal: bool,
) {
    let title = if p.title.is_empty() {
        " PREVIEW ".into()
    } else {
        format!(" {} ", clean(&p.title))
    };
    let block = border(title, theme, unicode, minimal, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if p.textual {
        frame.render_widget(
            Paragraph::new(
                p.lines
                    .iter()
                    .map(|line| clean(line))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(Style::default().fg(theme.text)),
            inner,
        );
        return;
    }
    for (offset, line) in p
        .lines
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
    {
        let row = p.first_line + offset;
        let selected = row == p.focused_line;
        let mut spans = vec![Span::styled(
            format!("{:>4} ", row + 1),
            Style::default().fg(if selected { theme.iris } else { theme.muted }),
        )];
        for (byte, g) in line.grapheme_indices(true) {
            let hit = p
                .ranges
                .iter()
                .any(|(r, range)| *r == offset && range.start < byte + g.len() && range.end > byte);
            spans.push(Span::styled(
                if g == "\t" { "    ".into() } else { clean(g) },
                Style::default()
                    .fg(if hit { theme.gold } else { theme.text })
                    .bg(if hit {
                        theme.overlay
                    } else if selected {
                        theme.base
                    } else {
                        theme.surface
                    }),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(if selected {
                theme.base
            } else {
                theme.surface
            })),
            Rect::new(inner.x, inner.y + offset as u16, inner.width, 1),
        );
    }
    if !p.message.is_empty() {
        frame.render_widget(
            Paragraph::new(clean(&p.message))
                .style(Style::default().fg(theme.muted))
                .alignment(Alignment::Left),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}
