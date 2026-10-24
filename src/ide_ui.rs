//! Headless-testable Rosé Pine IDE surfaces; rendering never starts processes.
use crate::{
    app::App,
    editor::Editor,
    ide::{Ide, PanelKind},
    theme::Theme,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;
pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}
fn surface(frame: &mut Frame, area: Rect, title: &str, theme: &Theme, unicode: bool) -> Rect {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(format!(" {} ", clean(title)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.iris))
        .style(Style::default().bg(theme.surface).fg(theme.text));
    let block = if unicode {
        block
    } else {
        block.border_set(crate::ui::ASCII_BORDER)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2)).max(1);
    let h = height.min(area.height.saturating_sub(2)).max(1);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}
pub fn draw(frame: &mut Frame, app: &mut App, theme: &Theme) {
    let area = frame.area();
    if area.width < 8 || area.height < 4 {
        return;
    }
    let unicode = app.workspace.config.unicode;
    if let Some(panel) = app
        .ide
        .panel
        .as_ref()
        .filter(|p| p.kind != PanelKind::Completion)
    {
        let popup = centered(area, 110, 28);
        let inner = surface(frame, popup, &panel.title, theme, unicode);
        if inner.width > 0 && inner.height > 0 {
            let rows = panel.visible();
            let query_height = u16::from(!panel.rows.is_empty());
            if query_height > 0 {
                frame.render_widget(
                    Paragraph::new(format!(" > {}", clean(&panel.query)))
                        .style(Style::default().fg(theme.gold)),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
            }
            let content = Rect::new(
                inner.x,
                inner.y + query_height,
                inner.width,
                inner.height.saturating_sub(query_height),
            );
            if panel.kind == PanelKind::Font && !rows.is_empty() && content.height >= 3 {
                let count = (rows.len() as u16).min(8).min(content.height / 3).max(1);
                let start = panel
                    .selected
                    .saturating_sub(count.saturating_sub(1) as usize);
                let lines: Vec<Line> = rows
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(count as usize)
                    .map(|(i, r)| {
                        Line::styled(
                            format!(
                                " {} {}",
                                if i == panel.selected { ">" } else { " " },
                                clean(&r.label)
                            ),
                            Style::default()
                                .fg(if i == panel.selected {
                                    theme.base
                                } else {
                                    theme.text
                                })
                                .bg(if i == panel.selected {
                                    theme.iris
                                } else {
                                    theme.surface
                                }),
                        )
                    })
                    .collect();
                frame.render_widget(
                    Paragraph::new(lines),
                    Rect::new(content.x, content.y, content.width, count),
                );
                let detail = rows.get(panel.selected).map_or("", |r| r.detail.as_str());
                frame.render_widget(
                    Paragraph::new(clean(detail))
                        .wrap(Wrap { trim: false })
                        .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0)),
                    Rect::new(
                        content.x,
                        content.y + count,
                        content.width,
                        content.height.saturating_sub(count + 1),
                    ),
                );
                if content.height > count {
                    frame.render_widget(
                        Paragraph::new(
                            " Up/Down select | Enter choose | PgUp/PgDn read | Esc cancel",
                        )
                        .style(Style::default().fg(theme.muted)),
                        Rect::new(content.x, content.bottom() - 1, content.width, 1),
                    );
                }
            } else if panel.rows.is_empty() {
                frame.render_widget(
                    Paragraph::new(clean(&panel.text))
                        .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0))
                        .wrap(Wrap { trim: false }),
                    content,
                );
            } else {
                let list_width = if content.width > 58 {
                    content.width * 2 / 5
                } else {
                    content.width
                };
                let visible = content.height as usize;
                let start = panel.selected.saturating_sub(visible.saturating_sub(1));
                let lines = rows
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(visible)
                    .map(|(i, r)| {
                        Line::from(Span::styled(
                            format!(
                                " {} {}",
                                if i == panel.selected { ">" } else { " " },
                                clean(&r.label)
                            ),
                            Style::default()
                                .fg(if i == panel.selected {
                                    theme.base
                                } else {
                                    theme.text
                                })
                                .bg(if i == panel.selected {
                                    theme.iris
                                } else {
                                    theme.surface
                                }),
                        ))
                    })
                    .collect::<Vec<_>>();
                frame.render_widget(
                    Paragraph::new(lines),
                    Rect::new(content.x, content.y, list_width, content.height),
                );
                if list_width < content.width {
                    let x = content.x + list_width;
                    for y in content.y..content.bottom() {
                        frame.buffer_mut()[(x, y)]
                            .set_symbol(if unicode { "│" } else { "|" })
                            .set_fg(theme.overlay);
                    }
                    if let Some(row) = rows.get(panel.selected) {
                        frame.render_widget(
                            Paragraph::new(clean(&row.detail))
                                .wrap(Wrap { trim: false })
                                .scroll((panel.scroll.min(u16::MAX as usize) as u16, 0)),
                            Rect::new(
                                x + 2,
                                content.y,
                                content.width.saturating_sub(list_width + 2),
                                content.height,
                            ),
                        );
                    }
                }
            }
        }
    }
    if let Some(prompt) = &app.ide.prompt {
        let popup = centered(area, 90, 10);
        let inner = surface(frame, popup, &prompt.title, theme, unicode);
        if inner.height > 1 {
            let shown = clean(&prompt.input);
            frame.render_widget(
                Paragraph::new(shown.clone())
                    .style(Style::default().fg(theme.gold))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            let x = (inner.x + shown.width().min(inner.width.saturating_sub(1) as usize) as u16)
                .min(inner.right().saturating_sub(1));
            frame.set_cursor_position((x, inner.y));
            frame.render_widget(
                Paragraph::new(" Enter confirms  |  Esc cancels")
                    .style(Style::default().fg(theme.muted)),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
        }
    }
    let status = if app.ide.terminal_focus {
        "TERMINAL INPUT - Ctrl-] returns to editor"
    } else if app.ide.busy > 0 {
        "BACKGROUND TASK RUNNING"
    } else {
        ""
    };
    if !status.is_empty() {
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(theme.base).bg(theme.foam)),
            Rect::new(
                0,
                area.height.saturating_sub(2),
                status.len().min(area.width as usize) as u16,
                1,
            ),
        );
    }
    if !app.ide.git.branch.is_empty() && app.workspace.config.status_bar && area.width > 65 {
        let text = format!(" git:{} ", clean(&app.ide.git.branch));
        let w = text.width().min((area.width / 3) as usize) as u16;
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(theme.foam).bg(theme.surface)),
            Rect::new(area.width / 3, area.height - 2, w, 1),
        );
    }
}
pub fn terminal_panel(frame: &mut Frame, app: &mut App, body: Rect, theme: &Theme) -> Rect {
    if !app.ide.terminal_visible
        || app.ide.terminals.is_empty()
        || body.width < 12
        || body.height < 5
    {
        return body;
    }
    let (terminal, editor) = if app.ide.terminal_vertical && body.width >= 36 {
        let w = body.width / 2;
        (
            Rect::new(body.x + w, body.y, body.width - w, body.height),
            Rect::new(body.x, body.y, w, body.height),
        )
    } else {
        let h = app
            .workspace
            .config
            .terminal
            .height
            .min(body.height.saturating_sub(3))
            .max(3);
        (
            Rect::new(body.x, body.bottom() - h, body.width, h),
            Rect::new(body.x, body.y, body.width, body.height - h),
        )
    };
    let title = format!(
        "TERMINAL {}/{} | Ctrl-] editor | Ctrl-PageUp/Down sessions | Ctrl-Shift-X close",
        app.ide.terminal_active + 1,
        app.ide.terminals.len()
    );
    let inner = surface(frame, terminal, &title, theme, app.workspace.config.unicode);
    let session = &mut app.ide.terminals[app.ide.terminal_active];
    session.resize(inner.height, inner.width);
    {
        let screen = &session.display;
        for y in 0..inner.height {
            for x in 0..inner.width {
                if let Some(cell) = screen.cell(y, x) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let mut fg = color(cell.fgcolor(), theme.text);
                    let mut bg = color(cell.bgcolor(), theme.base);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg)
                    }
                    let mut style = Style::default().fg(fg).bg(bg);
                    if cell.bold() {
                        style = style.add_modifier(Modifier::BOLD)
                    }
                    if cell.italic() {
                        style = style.add_modifier(Modifier::ITALIC)
                    }
                    if cell.underline() {
                        style = style.add_modifier(Modifier::UNDERLINED)
                    }
                    if cell.dim() {
                        style = style.add_modifier(Modifier::DIM)
                    }
                    frame.buffer_mut().set_stringn(
                        inner.x + x,
                        inner.y + y,
                        if cell.contents().is_empty() {
                            " "
                        } else {
                            cell.contents()
                        },
                        (inner.width - x) as usize,
                        style,
                    );
                }
            }
        }
        if app.ide.terminal_focus && !screen.hide_cursor() && screen.scrollback() == 0 {
            let (y, x) = screen.cursor_position();
            if y < inner.height && x < inner.width {
                frame.set_cursor_position((inner.x + x, inner.y + y));
            }
        }
    }
    if session.ended.load(std::sync::atomic::Ordering::Acquire) {
        let message = session
            .error
            .try_lock()
            .ok()
            .and_then(|s| s.clone())
            .unwrap_or_else(|| "Process exited. :TerminalNew starts another shell.".into());
        if inner.height > 0 {
            frame.render_widget(
                Paragraph::new(clean(&message)).style(Style::default().fg(theme.love)),
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
            );
        }
    }
    editor
}
fn color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
pub fn markers(
    frame: &mut Frame,
    editor: &Editor,
    ide: &Ide,
    area: Rect,
    top: usize,
    theme: &Theme,
) {
    if area.width < 4 || area.height < 3 {
        return;
    }
    let Some(path) = editor.buffer.path.as_deref() else {
        return;
    };
    let key = crate::lsp::path_key(path);
    let diagnostics = ide.diagnostics.get(&key);
    let git_matches = ide
        .git
        .path
        .as_ref()
        .is_some_and(|p| crate::lsp::path_key(p) == key);
    for y in area.y + 1..area.bottom() - 1 {
        let row = top + usize::from(y - area.y - 1);
        let diagnostic = diagnostics.and_then(|d| {
            let start = d.partition_point(|d| (d.range.start.line as usize) < row);
            let end = d.partition_point(|d| (d.range.start.line as usize) <= row);
            d[start..end]
                .iter()
                .filter(|d| d.revision == editor.buffer.revision)
                .min_by_key(|d| d.severity)
        });
        let git = if git_matches {
            ide.git.lines.get(&row).copied()
        } else {
            None
        };
        if let Some(d) = diagnostic {
            frame.buffer_mut()[(area.x + 1, y)]
                .set_symbol(if editor.config.unicode {
                    if d.severity == 1 { "●" } else { "▲" }
                } else if d.severity == 1 {
                    "E"
                } else {
                    "W"
                })
                .set_fg(if d.severity == 1 {
                    theme.love
                } else {
                    theme.gold
                });
        } else if let Some(mark) = git {
            frame.buffer_mut()[(area.x + 1, y)]
                .set_symbol(match mark {
                    '-' => "-",
                    '~' => "~",
                    _ => "+",
                })
                .set_fg(match mark {
                    '-' => theme.love,
                    '~' => theme.gold,
                    _ => theme.foam,
                });
        }
    }
    if let Some(d) = diagnostics.and_then(|d| {
        let row = editor.buffer.row();
        let start = d.partition_point(|d| (d.range.start.line as usize) < row);
        let end = d.partition_point(|d| (d.range.start.line as usize) <= row);
        d[start..end]
            .iter()
            .find(|d| d.revision == editor.buffer.revision)
    }) {
        let area = frame.area();
        if editor.mode != crate::editor::Mode::Command && area.height > 0 {
            frame.render_widget(
                Paragraph::new(clean(&d.message.chars().take(1024).collect::<String>())).style(
                    Style::default().fg(if d.severity == 1 {
                        theme.love
                    } else {
                        theme.gold
                    }),
                ),
                Rect::new(1, area.height - 1, area.width.saturating_sub(2), 1),
            );
        }
    }
}
