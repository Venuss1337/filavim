//! Compact completion and signature overlays in the active rendered text viewport.
use crate::{
    animation::CursorTarget,
    app::App,
    contextual::{self, Placement},
    ide::{Choice, PanelKind},
    theme::Theme,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn surface(frame: &mut Frame, area: Rect, title: &str, theme: &Theme, unicode: bool) -> Rect {
    frame.render_widget(Clear, area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.muted))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    if !title.is_empty() {
        block = block.title(title.to_owned());
    }
    if !unicode {
        block = block.border_set(crate::ui::ASCII_BORDER);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

pub fn completion_anchor(editor: &crate::editor::Editor, target: CursorTarget) -> u16 {
    let cursor = editor.buffer.cursor;
    let mut start = cursor;
    while start > 0
        && (editor.buffer.text.char(start - 1).is_alphanumeric()
            || editor.buffer.text.char(start - 1) == '_')
    {
        start -= 1;
    }
    let row = editor.buffer.text.char_to_line(start);
    let col = crate::buffer::display_width(
        &editor
            .buffer
            .text
            .slice(editor.buffer.text.line_to_char(row)..start)
            .to_string(),
        editor.config.tab_width,
    );
    target.viewport.x
        + col
            .saturating_sub(target.origin.0)
            .min(target.viewport.width.saturating_sub(1) as usize) as u16
}

fn kind(
    value: &serde_json::Value,
    theme: &Theme,
    nerd: bool,
    unicode: bool,
) -> (&'static str, Color) {
    let k = value["kind"].as_u64().unwrap_or(1);
    let color = match k {
        2..=4 => theme.pine,
        7 | 8 | 13 | 22 | 25 => theme.foam,
        14 => theme.iris,
        15 => theme.gold,
        12 | 20 | 21 => theme.love,
        _ => theme.text,
    };
    let icon = if nerd && unicode {
        match k {
            2 | 3 => "󰊕",
            4 => "",
            5 => "󰜢",
            6 => "󰀫",
            7 => "󰠱",
            9 => "",
            14 => "󰌋",
            15 => "",
            21 => "󰏿",
            _ => "󰈙",
        }
    } else {
        match k {
            2 => "m",
            3 | 4 if unicode => "ƒ",
            3 | 4 => "fn",
            5 => "f",
            6 => "v",
            7 => "C",
            8 => "I",
            9 => "M",
            12 | 20 | 21 => "K",
            13 => "E",
            14 => "k",
            15 => "S",
            22 => "T",
            _ => ".",
        }
    };
    (icon, color)
}

pub fn draw(frame: &mut Frame, app: &mut App, theme: &Theme, target: Option<CursorTarget>) {
    let Some(target) = target else { return };
    if app.ide.focused() || app.picker.is_some() || app.dialog.is_some() || app.explorer.focused {
        return;
    }
    let viewport = target.viewport.intersection(frame.area());
    let unicode = app.workspace.config.unicode;
    let anchor = if app
        .ide
        .panel
        .as_ref()
        .is_some_and(|p| p.kind == PanelKind::Completion)
    {
        completion_anchor(app.workspace.editor(), target)
    } else {
        target.position.0
    };
    let mut occupied = Vec::with_capacity(2);
    if let Some(panel) = &mut app.ide.panel
        && panel.kind == PanelKind::Completion
        && app.workspace.editor().mode == crate::editor::Mode::Insert
    {
        let desired = panel.completion_width();
        panel.popup_size.0 = panel.popup_size.0.max(desired);
        let count_rows = panel.visible().len();
        panel.popup_size.1 = panel.popup_size.1.max(count_rows.min(8) as u16 + 2);
        let rows = panel.visible();
        if !rows.is_empty()
            && let Some(area) = contextual::place(
                viewport,
                target.position,
                anchor.saturating_sub(1),
                panel.popup_size,
                Placement::Below,
                &[],
            )
        {
            let inner = surface(frame, area, "", theme, unicode);
            let count = inner.height as usize;
            let start = panel.selected.saturating_sub(count.saturating_sub(1));
            for (i, row) in rows.iter().enumerate().skip(start).take(count) {
                let y = inner.y + (i - start) as u16;
                let selected = i == panel.selected;
                let bg = if selected {
                    theme.overlay
                } else {
                    theme.surface
                };
                frame.render_widget(
                    Block::default().style(Style::default().bg(bg)),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                let value = match &row.choice {
                    Choice::Completion(v) => v,
                    _ => &serde_json::Value::Null,
                };
                let (icon, color) = kind(value, theme, app.workspace.config.nerd_fonts, unicode);
                let label = clean(&row.label);
                let needle = panel.query.to_lowercase();
                let matched = label
                    .to_lowercase()
                    .find(&needle)
                    .filter(|_| !needle.is_empty());
                let mut spans = Vec::new();
                let mut cells = 0;
                for (byte, g) in label.grapheme_indices(true) {
                    if cells + g.width() > inner.width.saturating_sub(4) as usize {
                        break;
                    }
                    cells += g.width();
                    let hit =
                        matched.is_some_and(|start| byte >= start && byte < start + needle.len());
                    spans.push(Span::styled(
                        g,
                        if hit {
                            Style::default().fg(theme.gold).bg(bg).bold()
                        } else {
                            Style::default().fg(color).bg(bg)
                        },
                    ));
                }
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(inner.x, y, inner.width.saturating_sub(4), 1),
                );
                let icon_width = icon.width().min(inner.width as usize) as u16;
                frame.render_widget(
                    Paragraph::new(icon).style(
                        Style::default()
                            .fg(if selected { color } else { theme.muted })
                            .bg(bg),
                    ),
                    Rect::new(inner.right() - icon_width, y, icon_width, 1),
                );
            }
            if rows.len() > count && area.width > 2 {
                let y = inner.y
                    + ((panel.selected.min(rows.len() - 1) * count.saturating_sub(1))
                        / (rows.len() - 1)) as u16;
                frame.buffer_mut()[(area.right() - 1, y)]
                    .set_symbol(if unicode { "┃" } else { "#" })
                    .set_fg(theme.iris);
            }
            occupied.push(area);
            if let Some(row) = rows.get(panel.selected)
                && !row.detail.trim().is_empty()
            {
                let right = viewport.right().saturating_sub(area.right());
                let left = area.x.saturating_sub(viewport.x);
                let (x, available) = if right >= 18 {
                    (area.right(), right)
                } else {
                    (viewport.x, left)
                };
                if available >= 18 {
                    let width = available.min(42);
                    let x = if right >= 18 { x } else { area.x - width };
                    let doc = Rect::new(x, area.y, width, area.height);
                    let inside = surface(frame, doc, "", theme, unicode);
                    frame.render_widget(
                        Paragraph::new(row.detail.as_str())
                            .wrap(Wrap { trim: false })
                            .style(Style::default().fg(theme.text)),
                        inside,
                    );
                    occupied.push(doc);
                }
            }
        }
    }
    if let Some(signature) = &app.ide.signature
        && signature.buffer == app.workspace.active_buffer()
        && signature.document == app.workspace.editor().document_id
        && let Some(label) = contextual::signature_label(&signature.help)
    {
        let width =
            (label.label.width().saturating_add(2).clamp(8, 100) as u16).min(viewport.width);
        let content_width = width.saturating_sub(2).max(1) as usize;
        let mut lines: Vec<Line<'_>> = vec![Line::default()];
        let (mut cells, mut active_line) = (0, 0);
        for (byte, g) in label.label.grapheme_indices(true).take(2048) {
            let g = clean(g);
            let w = g.width();
            if cells + w > content_width && cells > 0 {
                lines.push(Line::default());
                cells = 0;
            }
            let active = label.active.as_ref().is_some_and(|r| r.contains(&byte));
            if active {
                active_line = lines.len() - 1;
            }
            lines.last_mut().unwrap().spans.push(Span::styled(
                g,
                if active {
                    Style::default().fg(theme.gold).bold().underlined()
                } else {
                    Style::default().fg(theme.text)
                },
            ));
            cells += w;
        }
        let height = lines.len().min(3) as u16 + 2;
        if let Some(area) = contextual::place(
            viewport,
            target.position,
            target.position.0.saturating_sub(1),
            (width, height),
            Placement::Above,
            &occupied,
        ) {
            let title = if label.total > 1 {
                format!(" {}/{} ", label.index + 1, label.total)
            } else {
                String::new()
            };
            let inner = surface(frame, area, &title, theme, unicode);
            let start = active_line.saturating_sub(inner.height.saturating_sub(1) as usize);
            frame.render_widget(
                Paragraph::new(lines).scroll((start.min(u16::MAX as usize) as u16, 0)),
                inner,
            );
        }
    }
}

pub(crate) fn label_width(label: &str) -> usize {
    clean(label).width()
}
