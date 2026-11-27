//! Predictable line editing. Edits use the existing Buffer history and scalar offsets.
use crate::{buffer::Buffer, editor::Editor};
use std::ops::Range;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indentation {
    pub tabs: bool,
    pub width: usize,
}
pub fn detect(text: &ropey::Rope) -> Option<Indentation> {
    let mut tabs = 0;
    let mut spaces = 0;
    let mut width = 0;
    for line in text.lines().take(2000) {
        let prefix: String = line
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        if line.chars().all(char::is_whitespace) {
            continue;
        }
        if prefix.starts_with('\t') {
            tabs += 1;
        } else if !prefix.is_empty() {
            spaces += 1;
            let count = prefix.chars().take_while(|c| *c == ' ').count();
            width = if width == 0 { count } else { gcd(width, count) };
        }
    }
    if tabs > spaces {
        Some(Indentation {
            tabs: true,
            width: 4,
        })
    } else if spaces > 0 && (2..=8).contains(&width) {
        Some(Indentation { tabs: false, width })
    } else {
        None
    }
}
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn rebase(position: usize, edits: &[(Range<usize>, String)]) -> usize {
    let mut delta: isize = 0;
    for (range, text) in edits {
        if position < range.start {
            break;
        }
        let length = text.chars().count();
        if position <= range.end {
            return range.start.saturating_add_signed(delta) + length;
        }
        delta += length as isize - (range.end - range.start) as isize;
    }
    position.saturating_add_signed(delta)
}
fn apply(buffer: &mut Buffer, edits: &[(Range<usize>, String)]) {
    let cursor = rebase(buffer.cursor, edits);
    buffer.end_group();
    buffer.begin_group();
    for (range, text) in edits.iter().rev() {
        buffer.replace(range.clone(), text);
    }
    buffer.end_group();
    buffer.cursor = cursor.min(buffer.text.len_chars());
    buffer.snap_cursor();
}
impl Editor {
    pub fn indentation(&self) -> Indentation {
        if self.config.detect_indentation
            && let Some(mut style) = self.buffer.indentation
        {
            if style.tabs {
                style.width = self.config.tab_width;
            }
            return style;
        }
        Indentation {
            tabs: !self.config.expand_tabs,
            width: self.config.tab_width,
        }
    }
    pub fn indent_unit(&self) -> String {
        let style = self.indentation();
        if style.tabs {
            "\t".into()
        } else {
            " ".repeat(style.width)
        }
    }
    fn selected_rows(&self) -> Range<usize> {
        let range = self
            .selection()
            .unwrap_or(self.buffer.cursor..self.buffer.cursor);
        self.buffer.text.char_to_line(range.start)
            ..self
                .buffer
                .text
                .char_to_line(range.end.saturating_sub(1).max(range.start))
                + 1
    }
    pub fn toggle_comment(&mut self) {
        let extension = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let marker = match extension {
            "rs" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "c" | "h" | "cpp" | "cc"
            | "cxx" | "hpp" | "jsonc" => "//",
            "py" | "pyw" => "#",
            _ => {
                self.notify("Line comments are unavailable for this language", true);
                return;
            }
        };
        let rows = self.selected_rows();
        let mut lines = Vec::new();
        for row in rows {
            let start = self.buffer.text.line_to_char(row);
            let line = self.buffer.text.line(row).to_string();
            let indent = line.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
            let byte = line
                .char_indices()
                .nth(indent)
                .map_or(line.len(), |(i, _)| i);
            if line[byte..].trim().is_empty() {
                continue;
            }
            lines.push((start + indent, line[byte..].to_owned()));
        }
        let uncomment = !lines.is_empty() && lines.iter().all(|(_, line)| line.starts_with(marker));
        let edits: Vec<_> = lines
            .into_iter()
            .map(|(at, line)| {
                if uncomment {
                    let n = marker.len() + usize::from(line[marker.len()..].starts_with(' '));
                    (at..at + n, String::new())
                } else {
                    (at..at, format!("{marker} "))
                }
            })
            .collect();
        self.anchor = self.anchor.map(|a| rebase(a, &edits));
        apply(&mut self.buffer, &edits);
        if self.mode == crate::editor::Mode::Insert {
            self.buffer.begin_group();
        }
    }
    pub fn indent_lines(&mut self, backwards: bool) {
        let style = self.indentation();
        let unit = self.indent_unit();
        let mut edits = Vec::new();
        for row in self.selected_rows() {
            let start = self.buffer.text.line_to_char(row);
            if backwards {
                let mut chars = self.buffer.text.line(row).chars();
                let count = match chars.next() {
                    Some('\t') => 1,
                    Some(' ') => {
                        1 + chars
                            .take(style.width.saturating_sub(1))
                            .take_while(|c| *c == ' ')
                            .count()
                    }
                    _ => 0,
                };
                if count > 0 {
                    edits.push((start..start + count, String::new()));
                }
            } else {
                edits.push((start..start, unit.clone()));
            }
        }
        self.anchor = self.anchor.map(|a| rebase(a, &edits));
        apply(&mut self.buffer, &edits);
        if self.mode == crate::editor::Mode::Insert {
            self.buffer.begin_group();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, editor::Mode};
    use ropey::Rope;
    fn editor(text: &str, extension: &str) -> Editor {
        let mut e = Editor::new(Config::default());
        e.set_buffer(Buffer::new(
            Rope::from_str(text),
            Some(format!("test.{extension}").into()),
        ));
        e
    }
    #[test]
    fn comments_preserve_indent_crlf_and_group_undo() {
        for extension in ["rs", "js", "ts", "tsx", "c", "cpp"] {
            let text = "  one\r\n  two\r\n";
            let mut e = editor(text, extension);
            e.mode = Mode::Visual;
            e.anchor = Some(0);
            e.buffer.cursor = 10;
            e.toggle_comment();
            assert_eq!(e.buffer.text.to_string(), "  // one\r\n  // two\r\n");
            e.toggle_comment();
            assert_eq!(e.buffer.text.to_string(), text);
            e.buffer.undo();
            assert!(e.buffer.text.to_string().contains("//"));
        }
        let mut e = editor("  x\n", "py");
        e.toggle_comment();
        assert_eq!(e.buffer.text.to_string(), "  # x\n");
        let mut e = editor("{}", "json");
        e.toggle_comment();
        assert!(e.error);
        assert_eq!(e.buffer.text.to_string(), "{}");
    }
    #[test]
    fn detection_and_selected_unindent() {
        assert_eq!(
            detect(&Rope::from_str("if x:\n  a\n    b\n")),
            Some(Indentation {
                tabs: false,
                width: 2
            })
        );
        assert!(detect(&Rope::from_str("if x:\n\ta\n\t\tb\n")).unwrap().tabs);
        let mut e = editor("    x\n    y", "py");
        e.mode = Mode::Visual;
        e.anchor = Some(0);
        e.buffer.cursor = 10;
        e.indent_lines(true);
        assert_eq!(e.buffer.text.to_string(), "x\ny");
        e.buffer.undo();
        assert_eq!(e.buffer.text.to_string(), "    x\n    y");
    }
    #[test]
    fn comment_transaction_keeps_subsequent_insert_typing_grouped() {
        let mut e = editor("x", "rs");
        e.set_mode(Mode::Insert);
        e.toggle_comment();
        e.type_character('a');
        e.type_character('b');
        e.set_mode(Mode::Normal);
        e.buffer.undo();
        assert_eq!(e.buffer.text.to_string(), "// x");
        e.buffer.undo();
        assert_eq!(e.buffer.text.to_string(), "x");
    }
    #[test]
    fn mixed_comments_preserve_blank_lines_and_round_trip_as_one_transaction() {
        let text = "  // one\r\n  two\r\n\r\n";
        let mut e = editor(text, "ts");
        e.set_mode(Mode::Visual);
        e.anchor = Some(0);
        e.buffer.cursor = e.buffer.text.len_chars() - 1;
        e.toggle_comment();
        assert_eq!(e.buffer.text.to_string(), "  // // one\r\n  // two\r\n\r\n");
        e.toggle_comment();
        assert_eq!(e.buffer.text.to_string(), text);
        assert!(e.buffer.undo());
        assert!(e.buffer.redo());
        assert_eq!(e.buffer.text.to_string(), text);
    }
    #[test]
    fn explicit_configuration_overrides_detected_indentation() {
        let mut e = editor("if ready:", "py");
        e.buffer.indentation = Some(Indentation {
            tabs: false,
            width: 2,
        });
        assert_eq!(e.indent_unit(), "  ");
        e.config.detect_indentation = false;
        e.config.expand_tabs = false;
        e.config.tab_width = 8;
        assert_eq!(e.indent_unit(), "\t");
        e.buffer.cursor = e.buffer.text.len_chars();
        e.set_mode(Mode::Insert);
        e.act(crate::input::Action::Newline);
        assert_eq!(e.buffer.text.to_string(), "if ready:\n\t");
    }
    #[test]
    fn python_comment_ending_in_colon_does_not_open_a_block() {
        let mut e = editor("    # explanation:", "py");
        e.buffer.cursor = e.buffer.text.len_chars();
        e.set_mode(Mode::Insert);
        e.act(crate::input::Action::Newline);
        assert_eq!(e.buffer.text.to_string(), "    # explanation:\n    ");
    }
}
