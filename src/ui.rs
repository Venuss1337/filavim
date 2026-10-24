use crate::{
    buffer::{display_width, grapheme_width, rope_graphemes},
    config::Preset,
    editor::{Editor, Mode},
    syntax::{Highlight, Token, language_name},
    theme::Theme,
};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;

pub(crate) const ASCII_BORDER: ratatui::symbols::border::Set = ratatui::symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

fn rect(x: u16, y: u16, width: u16, height: u16) -> Rect {
    Rect::new(x, y, width, height)
}
fn safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
pub fn draw(
    frame: &mut Frame,
    editor: &mut Editor,
    theme: &Theme,
    highlights: &[Highlight],
    pending: &str,
) {
    chrome(frame, editor, theme, pending);
    let area = frame.area();
    if area.height < 4 || area.width < 12 {
        return;
    }
    let body = rect(0, 1, area.width, area.height - 3);
    if editor.dashboard {
        dashboard(frame, body, theme, editor.config.unicode);
    } else if let Some(cursor) = panel(
        frame,
        body,
        editor,
        theme,
        highlights,
        true,
        &mut |top, _, _| top,
    ) {
        frame.set_cursor_position(cursor.position);
    }
    if editor.help {
        help(frame, area, theme, editor.config.unicode, editor.help_page);
    }
}
pub(crate) fn chrome(frame: &mut Frame, editor: &Editor, theme: &Theme, pending: &str) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.base).fg(theme.text)),
        area,
    );
    if area.width == 0 || area.height == 0 {
        return;
    }
    if area.height < 4 || area.width < 12 {
        frame.render_widget(
            Paragraph::new("FILAVIM · resize terminal").style(Style::default().fg(theme.iris)),
            area,
        );
        return;
    }
    let name = editor
        .buffer
        .path
        .as_ref()
        .map_or_else(|| "Untitled".into(), |p| safe(&p.to_string_lossy()));
    let modified = if editor.buffer.dirty() { " [+]" } else { "" };
    let header = Line::from(vec![
        Span::styled(
            "  FILAVIM  ",
            Style::default().fg(theme.base).bg(theme.iris).bold(),
        ),
        Span::styled(
            format!("  {name}{modified}"),
            Style::default().fg(theme.text).bg(theme.surface),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(Style::default().bg(theme.surface)),
        rect(0, 0, area.width, 1),
    );
    let mode_color = match editor.mode {
        Mode::Normal => theme.iris,
        Mode::Insert => theme.foam,
        Mode::Visual => theme.love,
        Mode::Command => theme.gold,
        _ => theme.iris,
    };
    let row = editor.buffer.row() + 1;
    let col = editor.buffer.column(editor.config.tab_width) + 1;
    let status_y = area.height - 2;
    let right = format!(
        "  {row}:{col}  {}  UTF-8  ",
        if editor.buffer.newline == "\r\n" {
            "CRLF"
        } else {
            "LF"
        }
    );
    let left = Line::from(vec![
        Span::styled(
            format!("  {}  ", editor.mode.label()),
            Style::default().fg(theme.base).bg(mode_color).bold(),
        ),
        Span::styled(
            format!(
                "  {}{modified}",
                safe(
                    &editor
                        .buffer
                        .path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map_or_else(|| "Untitled".into(), |p| p.to_string_lossy())
                )
            ),
            Style::default().fg(theme.text),
        ),
    ]);
    if editor.config.status_bar {
        frame.render_widget(
            Paragraph::new(left).style(Style::default().bg(theme.surface)),
            rect(0, status_y, area.width, 1),
        );
        if area.width > 45 {
            let width = (right.len() as u16).min(area.width / 2);
            frame.render_widget(
                Paragraph::new(right)
                    .alignment(Alignment::Right)
                    .style(Style::default().fg(theme.muted).bg(theme.surface)),
                rect(area.width - width, status_y, width, 1),
            );
        }
    }
    let message_y = area.height - 1;
    if editor.mode == Mode::Command {
        let prefix = if editor.searching { "/" } else { ":" };
        let available = area.width.saturating_sub(3) as usize;
        let mut tail = String::new();
        let mut width = 0;
        for g in editor.command.graphemes(true).rev() {
            let w = display_width(g, editor.config.tab_width);
            if width + w > available {
                break;
            }
            width += w;
            tail.insert_str(0, g);
        }
        frame.render_widget(
            Paragraph::new(format!(" {prefix}{}", safe(&tail)))
                .style(Style::default().fg(theme.gold)),
            rect(0, message_y, area.width, 1),
        );
        frame.set_cursor_position((2 + width as u16, message_y));
    } else {
        let message = if pending.is_empty() {
            safe(&editor.message)
        } else {
            format!("{pending} …")
        };
        frame.render_widget(
            Paragraph::new(format!(" {message}")).style(Style::default().fg(if editor.error {
                theme.love
            } else {
                theme.muted
            })),
            rect(0, message_y, area.width, 1),
        );
    }
}
pub(crate) fn panel(
    frame: &mut Frame,
    area: Rect,
    editor: &mut Editor,
    theme: &Theme,
    highlights: &[Highlight],
    focused: bool,
    scroll: &mut dyn FnMut(usize, usize, usize) -> usize,
) -> Option<crate::animation::CursorTarget> {
    let border = match (editor.config.unicode, editor.config.preset, focused) {
        (true, Preset::Beautiful, true) => BorderType::Double,
        (true, Preset::Beautiful | Preset::Balanced, _) => BorderType::Rounded,
        _ => BorderType::Plain,
    };
    let block = Block::default()
        .borders(if editor.config.borders {
            Borders::ALL
        } else {
            Borders::NONE
        })
        .border_type(border)
        .border_style(Style::default().fg(if focused { theme.iris } else { theme.overlay }))
        .title(
            Line::from(format!(
                " {} / {} ",
                editor
                    .buffer
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or_else(|| "Untitled".into(), |n| safe(&n.to_string_lossy())),
                language_name(editor.buffer.path.as_deref())
            ))
            .fg(theme.muted),
        );
    let block = if editor.config.unicode {
        block
    } else {
        block.border_set(ASCII_BORDER)
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 2 || inner.height == 0 {
        return None;
    }
    let gutter = if editor.config.line_numbers {
        (editor.buffer.text.len_lines().to_string().len() + 2)
            .min(inner.width.saturating_sub(2) as usize)
    } else {
        1
    };
    let width = (inner.width as usize).saturating_sub(gutter + 1);
    if width == 0 {
        return None;
    }
    let row = editor.buffer.row();
    editor.page_height = inner.height as usize;
    let margin = 3.min((editor.page_height - 1) / 2);
    if row < editor.top + margin {
        editor.top = row.saturating_sub(margin);
    }
    if row + margin >= editor.top + editor.page_height {
        editor.top = (row + margin + 1).saturating_sub(editor.page_height);
    }
    let render_top = scroll(editor.top, row, editor.page_height);
    let col = editor.buffer.column(editor.config.tab_width);
    if col < editor.left {
        editor.left = col;
    }
    if col >= editor.left + width {
        editor.left = col + 1 - width;
    }
    let selection = editor.selection();
    let buffer = frame.buffer_mut();
    for offset in 0..inner.height as usize {
        let line_no = render_top + offset;
        let y = inner.y + offset as u16;
        if line_no >= editor.buffer.text.len_lines() {
            buffer.set_string(inner.x, y, "~", Style::default().fg(theme.overlay));
            continue;
        }
        let active = focused && line_no == row && editor.config.preset != Preset::Minimal;
        let bg = if active { theme.surface } else { theme.base };
        for x in inner.x..inner.right() {
            buffer[(x, y)].set_bg(bg);
        }
        if editor.config.line_numbers {
            let number = format!("{:>width$} ", line_no + 1, width = gutter.saturating_sub(1));
            buffer.set_stringn(
                inner.x,
                y,
                number,
                gutter,
                Style::default()
                    .fg(if active { theme.iris } else { theme.muted })
                    .bg(bg),
            );
        }
        let (start_char, mut visual_col) =
            editor
                .buffer
                .display_start(line_no, editor.left, editor.config.tab_width);
        let text = editor.buffer.line_slice(line_no).slice(start_char..);
        let mut char_at = editor.buffer.text.line_to_char(line_no) + start_char;
        let mut byte_at = editor.buffer.text.char_to_byte(char_at);
        for g in rope_graphemes(text) {
            let w = grapheme_width(&g, visual_col, editor.config.tab_width);
            let start = visual_col;
            visual_col += w;
            let selected = selection.as_ref().is_some_and(|r| r.contains(&char_at));
            if visual_col > editor.left && start < editor.left + width {
                let index = highlights.partition_point(|h| h.range.end <= byte_at);
                let token = highlights
                    .get(index)
                    .filter(|h| h.range.contains(&byte_at))
                    .map(|h| h.token);
                let fg = match token {
                    Some(Token::Comment) => theme.muted,
                    Some(Token::String) => theme.gold,
                    Some(Token::Number | Token::Constant) => theme.love,
                    Some(Token::Keyword) => theme.iris,
                    Some(Token::Type) => theme.foam,
                    Some(Token::Function) => theme.pine,
                    None => theme.text,
                };
                let style = Style::default()
                    .fg(fg)
                    .bg(if selected { theme.overlay } else { bg });
                let x = inner.x + gutter as u16 + start.saturating_sub(editor.left) as u16;
                let visible = visual_col.min(editor.left + width) - start.max(editor.left);
                if g.as_ref() == "\t" || start < editor.left || start + w > editor.left + width {
                    buffer.set_stringn(x, y, " ".repeat(visible), visible, style);
                } else {
                    let rendered = if g.chars().any(char::is_control) {
                        "�"
                    } else {
                        g.as_ref()
                    };
                    buffer.set_stringn(x, y, rendered, visible, style);
                }
            }
            char_at += g.chars().count();
            byte_at += g.len();
            if visual_col >= editor.left + width {
                break;
            }
        }
        // A newline is a selectable cell too, including empty lines.
        if visual_col >= editor.left
            && visual_col < editor.left + width
            && selection.as_ref().is_some_and(|r| r.contains(&char_at))
        {
            buffer[(
                inner.x + gutter as u16 + (visual_col - editor.left) as u16,
                y,
            )]
                .set_bg(theme.overlay);
        }
    }
    if focused && editor.mode != Mode::Command && !editor.help {
        let x = inner.x + gutter as u16 + (col - editor.left) as u16;
        let y = inner.y + (row - render_top) as u16;
        return Some(crate::animation::CursorTarget {
            position: (x, y),
            viewport: Rect::new(inner.x + gutter as u16, inner.y, width as u16, inner.height),
            origin: (editor.left, render_top),
            logical: editor.buffer.cursor,
            revision: editor.buffer.revision,
        });
    }
    None
}
const LOGO: [&str; 6] = [
    "███████╗██╗██╗      █████╗ ██╗   ██╗██╗███╗   ███╗",
    "██╔════╝██║██║     ██╔══██╗██║   ██║██║████╗ ████║",
    "█████╗  ██║██║     ███████║██║   ██║██║██╔████╔██║",
    "██╔══╝  ██║██║     ██╔══██║╚██╗ ██╔╝██║██║╚██╔╝██║",
    "██║     ██║███████╗██║  ██║ ╚████╔╝ ██║██║ ╚═╝ ██║",
    "╚═╝     ╚═╝╚══════╝╚═╝  ╚═╝  ╚═══╝  ╚═╝╚═╝     ╚═╝",
];
const ASCII_LOGO: [&str; 6] = [
    " _____ ___ _        _ __     _____ __  __",
    "|  ___|_ _| |      / \\ \\   / /_ _|  \\/  |",
    "| |_   | || |     / _ \\ \\ / / | || |\\/| |",
    "|  _|  | || |___ / ___ \\ V /  | || |  | |",
    "|_|   |___|_____/_/   \\_\\_/  |___|_|  |_|",
    "",
];
pub(crate) fn dashboard(frame: &mut Frame, area: Rect, theme: &Theme, unicode: bool) {
    let full = area.width >= 56 && area.height >= 19;
    let mut lines: Vec<Line> = Vec::new();
    if full {
        for line in if unicode { &LOGO } else { &ASCII_LOGO } {
            lines.push(Line::from(Span::styled(
                *line,
                Style::default().fg(theme.iris),
            )));
        }
        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(Span::styled(
            "F I L A V I M",
            Style::default().fg(theme.iris).bold(),
        )));
    }
    lines.push(Line::from(Span::styled(
        "A quiet place to build something.",
        Style::default().fg(theme.muted),
    )));
    lines.push(Line::from(""));
    for (key, label) in [
        ("i", "Create a new file"),
        (":e <path>", "Open a file"),
        ("Space f f", "Find project files"),
        ("Space p", "Command palette"),
        (":help / F1", "Explore the controls"),
        (":q", "Leave Filavim"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:>13}"), Style::default().fg(theme.foam)),
            Span::styled(format!("    {label:<24}"), Style::default().fg(theme.text)),
        ]));
    }
    if full {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "RUST AT THE CORE  /  ROSÉ PINE MOON",
            Style::default().fg(theme.muted),
        )));
    }
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        rect(area.x, y, area.width, h.min(area.height)),
    );
}
pub(crate) fn help(frame: &mut Frame, area: Rect, theme: &Theme, unicode: bool, page: usize) {
    let width = area.width.saturating_sub(2).min(72);
    let height = area.height.saturating_sub(2).min(24);
    let popup = rect(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if unicode {
            BorderType::Rounded
        } else {
            BorderType::Plain
        })
        .title(if page == 0 {
            " FILAVIM / EDITING / PgDn for navigation "
        } else {
            " FILAVIM / NAVIGATION / PgUp for editing "
        })
        .border_style(Style::default().fg(theme.iris))
        .style(Style::default().bg(theme.surface).fg(theme.text));
    let lines = if page == 0 {
        [
            " Esc / F1       Close this guide",
            " W A S D        Up / left / down / right",
            " I / Shift-I    Insert / append at end of line",
            " V, then WASD   Select text; Esc returns to Normal",
            " Q / E          Previous / next word",
            " 0 / $          Start / end of line (Home / End)",
            " gg / Shift-G   First / last line",
            " PgUp / PgDn    Move a page; arrows work in all edit modes",
            " X / Shift-X    Delete grapheme / line",
            " O              Open an indented line below",
            " Y / C / P      Copy / cut / paste (selection or line)",
            " U / Ctrl-R     Undo / redo; Ctrl-Z also in Insert",
            " /text, N       Find in buffer / next result (wraps)",
            " Space W        Save; Ctrl-S works in editing modes",
            " Space N / Q    New buffer / quit",
            " :w [path]      Save; :wq saves and quits",
            " :e <path>      Open file; :new creates a buffer",
            " :q / :q!       Quit / discard changes and quit",
            " :e! / :new!    Discard changes and open / create",
            " :42            Go to line 42",
            " Terminal paste: enter Insert, then Ctrl-Shift-V / Cmd-V",
            " Config: FILAVIM_CONFIG or platform config/filavim/config.toml",
        ]
    } else {
        [
            " Esc / F1       Close guide; PgUp/PgDn switches pages",
            " Space f f      Fuzzy find project files",
            " Space f g      Live regex grep (smart case)",
            " Space f r / p  Recent files / recent projects",
            " Space b b      Search and switch open buffers",
            " Space b n / p  Next / previous buffer",
            " Space b x      Close buffer (confirms modified text)",
            " Space p        Fuzzy command palette",
            " Picker: type query, arrows or Ctrl-W/S move, Enter opens",
            " Space e        File explorer; Esc returns to editor",
            " Tree: WASD, Enter open; I new file; Shift-I directory",
            " Tree: R rename, X delete (type delete to confirm)",
            " Ctrl-R         Refresh explorer / file picker index",
            " Space s v / h  Vertical / horizontal split",
            " Space s n / p  Next / previous split",
            " Space s + / -  Grow / shrink split",
            " Space s x      Close split or current tab",
            " Space b t n    New tab; Space b t A/D changes tabs",
            " :cd <path>     Switch workspace; :projects shows recents",
            " :e <path>      Open buffer and retain existing edits",
            " :bd / :bd!     Close buffer / discard its changes",
            " :qa / :qa!     Quit all / discard all buffers and quit",
        ]
    };
    let block = if unicode {
        block
    } else {
        block.border_set(ASCII_BORDER)
    };
    frame.render_widget(
        Paragraph::new(lines.map(Line::from).to_vec()).block(block),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn renders_dashboard_document_and_tiny_sizes() {
        for (w, h) in [(100, 32), (40, 12), (12, 4), (5, 2), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let mut editor = Editor::new(Config::default());
            terminal
                .draw(|f| draw(f, &mut editor, &Theme::default(), &[], ""))
                .unwrap();
            editor.act(crate::input::Action::Insert);
            editor.type_text("日本\t👩‍💻\nhello");
            terminal
                .draw(|f| draw(f, &mut editor, &Theme::default(), &[], ""))
                .unwrap();
            editor.help = true;
            terminal
                .draw(|f| draw(f, &mut editor, &Theme::default(), &[], ""))
                .unwrap();
        }
    }
}
