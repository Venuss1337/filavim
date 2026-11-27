//! Bounded delimiter provenance and conservative local lexical pairing rules.
use crate::buffer::Buffer;
use serde::Deserialize;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub surround: bool,
    pub in_comments: bool,
    pub in_strings: bool,
    pub smart_delete: bool,
    pub overtype: bool,
    /// Extension -> two-character strings; an empty list disables that language.
    pub languages: BTreeMap<String, Vec<String>>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            surround: true,
            in_comments: false,
            in_strings: false,
            smart_delete: true,
            overtype: true,
            languages: BTreeMap::new(),
        }
    }
}
#[derive(Debug)]
struct Pair {
    open: usize,
    close: usize,
    left: char,
    right: char,
    untouched: bool,
}
#[derive(Default)]
pub struct Pairs {
    revision: u64,
    marks: Vec<Pair>,
}
impl Pairs {
    pub fn sync(&mut self, b: &Buffer) {
        if self.revision == b.revision {
            return;
        }
        if let Some(changes) = b.changes_since(self.revision) {
            for c in changes {
                let n = c.text.chars().count();
                let delta = n as isize - c.range.len() as isize;
                self.marks.retain_mut(|p| {
                    if c.range.contains(&p.open) || c.range.contains(&p.close) {
                        return false;
                    }
                    if c.range.start > p.open && c.range.start <= p.close && n > 0 {
                        p.untouched = false;
                    }
                    if c.range.end <= p.open {
                        p.open = p.open.saturating_add_signed(delta);
                    }
                    if c.range.end <= p.close {
                        p.close = p.close.saturating_add_signed(delta);
                    }
                    true
                });
            }
        } else {
            self.marks.clear();
        }
        self.revision = b.revision;
        self.marks.retain(|p| {
            p.close < b.text.len_chars()
                && b.text.char(p.open) == p.left
                && b.text.char(p.close) == p.right
        });
    }
    pub fn record(&mut self, b: &Buffer, open: usize, close: usize, left: char, right: char) {
        self.sync(b);
        if self.marks.len() >= 256 {
            self.marks.remove(0);
        }
        self.marks.push(Pair {
            open,
            close,
            left,
            right,
            untouched: true,
        });
        self.revision = b.revision;
    }
    pub fn completion_range(
        &mut self,
        b: &Buffer,
        mut range: std::ops::Range<usize>,
        text: &str,
    ) -> std::ops::Range<usize> {
        self.sync(b);
        if let Some(p) = self
            .marks
            .iter()
            .find(|p| p.open == range.end && p.close == p.open + 1 && p.untouched)
        {
            let mut end = text.chars().rev();
            if end.next() == Some(p.right) && end.next() == Some(p.left) {
                range.end = p.close + 1;
            }
        }
        range
    }
    pub fn overtype(&mut self, b: &mut Buffer, c: char) -> bool {
        self.sync(b);
        if matches!(c, '"' | '\'' | '`') {
            let mut at = b.cursor;
            let mut escapes = 0;
            while at > 0 && b.text.char(at - 1) == '\\' {
                at -= 1;
                escapes += 1;
            }
            if escapes % 2 == 1 {
                return false;
            }
        }
        if let Some(i) = self
            .marks
            .iter()
            .position(|p| p.close == b.cursor && p.right == c)
        {
            self.marks.remove(i);
            b.cursor = b.next(b.cursor);
            true
        } else {
            false
        }
    }
    pub fn delete(&mut self, b: &mut Buffer) -> bool {
        self.sync(b);
        if let Some(i) = self
            .marks
            .iter()
            .position(|p| p.open + 1 == b.cursor && p.close == b.cursor && p.untouched)
        {
            let p = self.marks.remove(i);
            b.replace(p.open..p.close + 1, "");
            self.sync(b);
            true
        } else {
            false
        }
    }
    pub fn newline(&mut self, b: &mut Buffer, indent: &str) -> bool {
        self.sync(b);
        if let Some(p) = self.marks.iter().find(|p| {
            p.open + 1 == b.cursor && p.close == b.cursor && matches!(p.left, '{' | '[' | '(')
        }) {
            let at = p.close;
            let start = b.text.line_to_char(b.row());
            let base: String = b
                .text
                .slice(start..p.open)
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .take(4096)
                .collect();
            let text = format!("{}{base}{indent}{}{base}", b.newline, b.newline);
            b.replace(at..at, &text);
            b.cursor =
                at + b.newline.chars().count() + base.chars().count() + indent.chars().count();
            self.sync(b);
            return true;
        }
        false
    }
}
pub fn surround_closer(b: &Buffer, c: char, config: &Config) -> Option<char> {
    if !config.enabled {
        return None;
    }
    let ext = b.path.as_ref()?.extension()?.to_str()?.to_ascii_lowercase();
    let custom = config.languages.get(&ext);
    let allowed = matches!(
        ext.as_str(),
        "rs" | "py"
            | "pyw"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "json"
            | "jsonc"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "go"
            | "lua"
            | "toml"
            | "yaml"
            | "yml"
            | "sh"
            | "bash"
            | "java"
            | "html"
            | "css"
            | "vue"
            | "md"
    );
    if !allowed && custom.is_none() {
        return None;
    }
    if let Some(pairs) = custom {
        pairs.iter().find_map(|p| {
            let mut chars = p.chars();
            (chars.next() == Some(c)).then(|| chars.next()).flatten()
        })
    } else {
        Some(match c {
            '(' => ')',
            '[' => ']',
            '{' => '}',
            '"' => '"',
            '\'' if ext != "json" => '\'',
            '`' if matches!(
                ext.as_str(),
                "js" | "jsx" | "ts" | "tsx" | "vue" | "md" | "sh" | "bash"
            ) =>
            {
                '`'
            }
            _ => return None,
        })
    }
}
pub fn closer(b: &Buffer, c: char, config: &Config) -> Option<char> {
    let right = surround_closer(b, c, config)?;
    let ext = b.path.as_ref()?.extension()?.to_str()?.to_ascii_lowercase();
    let at = b.cursor;
    let prev = at.checked_sub(1).map(|i| b.text.char(i));
    let next = (at < b.text.len_chars()).then(|| b.text.char(at));
    if next.is_some_and(|n| n.is_alphanumeric() || n == '_') {
        return None;
    }
    if matches!(c, '\'' | '"' | '`')
        && prev.is_some_and(|p| p.is_alphanumeric() || matches!(p, '_' | '\\'))
    {
        return None;
    }
    // Rust apostrophes also introduce lifetimes/labels, so default to conservative
    // literal pairing only after assignment or an opening delimiter.
    if c == '\'' && ext == "rs" && !prev.is_some_and(|p| matches!(p, '=' | '(' | '[' | ',')) {
        return None;
    }
    let (comment, string) = context(b, &ext);
    if comment && !config.in_comments || string && !config.in_strings {
        return None;
    }
    Some(right)
}
fn context(b: &Buffer, ext: &str) -> (bool, bool) {
    let at = b.cursor;
    let mut start = b.text.line_to_char(b.row().saturating_sub(32));
    if at - start > 8192 {
        start = b.text.line_to_char(b.row());
    }
    if at - start > 8192 {
        return (true, false);
    }
    // Unknown truncated lexical state is handled conservatively.
    if start > 0 && b.text.char(start - 1) != '\n' {
        return (true, false);
    }
    let mut chars = b.text.slice(start..at).chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut block = false;
    let mut line = false;
    let mut previous = None;
    let hash = matches!(ext, "py" | "pyw" | "toml" | "yaml" | "yml" | "sh" | "bash");
    while let Some(c) = chars.next() {
        let prev = previous.replace(c);
        if line {
            if c == '\n' {
                line = false;
            }
            continue;
        }
        if block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block = false;
            }
            continue;
        }
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            line = true;
            chars.next();
        } else if c == '/' && chars.peek() == Some(&'*') {
            block = true;
            chars.next();
        } else if hash && c == '#' {
            line = true;
        } else if ext == "lua" && c == '-' && chars.peek() == Some(&'-') {
            line = true;
            chars.next();
        } else if c == '\''
            && (prev.is_some_and(|p: char| p.is_alphanumeric() || p == '_')
                || ext == "rs"
                    && chars.peek().is_some_and(|c| c.is_alphabetic())
                    && chars.clone().nth(1) != Some('\''))
        {
            continue;
        } else if matches!(c, '"' | '\'' | '`') {
            quote = Some(c);
        }
    }
    (line || block, quote.is_some())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        editor::{Editor, Mode},
        gui::model::Model,
        input::Action,
    };
    fn editor(text: &str, ext: &str) -> Editor {
        let mut e = Editor::new(Config::default());
        e.set_buffer(Buffer::new(
            ropey::Rope::from_str(text),
            Some(format!("fixture.{ext}").into()),
        ));
        e.buffer.cursor = e.buffer.text.len_chars();
        e.set_mode(Mode::Insert);
        e
    }
    #[test]
    fn nested_owned_closers_delete_overtype_and_undo() {
        let mut e = editor("", "js");
        e.type_character('(');
        e.type_character('[');
        assert_eq!(e.buffer.text, "([])");
        assert_eq!(e.buffer.cursor, 2);
        e.act(Action::Backspace);
        assert_eq!(e.buffer.text, "()");
        e.type_character(')');
        assert_eq!(e.buffer.text, "()");
        assert_eq!(e.buffer.cursor, 2);
        e.act(Action::Undo);
        assert_eq!(e.buffer.text, "");
        e.act(Action::Redo);
        assert_eq!(e.buffer.text, "()");
        e.buffer.cursor = 1;
        e.type_character(')');
        assert_eq!(e.buffer.text, "())", "undo retires provenance");
    }
    #[test]
    fn escaped_quote_does_not_overtype_the_owned_string_terminator() {
        let mut e = editor("", "js");
        e.type_character('"');
        e.type_character('\\');
        e.type_character('"');
        assert_eq!(e.buffer.text, "\"\\\"\"");
        assert_eq!(e.buffer.cursor, 3);
        e.type_character('"');
        assert_eq!(e.buffer.text, "\"\\\"\"");
        assert_eq!(e.buffer.cursor, 4);
    }
    #[test]
    fn manual_closers_and_touched_pairs_are_not_deleted_or_skipped() {
        let mut e = editor(")", "js");
        e.buffer.cursor = 0;
        e.type_character(')');
        assert_eq!(e.buffer.text, "))");
        let mut e = editor("", "js");
        e.type_character('(');
        e.type_character('x');
        e.act(Action::Backspace);
        e.act(Action::Backspace);
        assert_eq!(
            e.buffer.text, ")",
            "a touched pair is not an empty auto-pair"
        );
    }
    #[test]
    fn context_quotes_apostrophes_templates_plain_text_and_overrides() {
        for (text, ext, c, expected) in [
            ("// comment ", "js", '(', "// comment ("),
            ("# comment ", "py", '"', "# comment \""),
            ("don", "js", '\'', "don'"),
            ("\"inside ", "js", '(', "\"inside ("),
            ("word ", "txt", '(', "word ("),
            ("", "ts", '`', "``"),
            ("", "json", '\'', "'"),
        ] {
            let mut e = editor(text, ext);
            e.type_character(c);
            assert_eq!(e.buffer.text, expected);
        }
        let mut e = editor("", "custom");
        e.config
            .pairs
            .languages
            .insert("custom".into(), vec!["<>".into()]);
        e.type_character('<');
        assert_eq!(e.buffer.text, "<>");
        let mut e = editor("let x: &'a str = ", "rs");
        e.type_character('"');
        assert_eq!(e.buffer.text, "let x: &'a str = \"\"");
        let mut e = editor("<Box title=", "tsx");
        e.type_character('"');
        assert_eq!(e.buffer.text, "<Box title=\"\"");
    }
    #[test]
    fn multiline_indent_crlf_tabs_and_atomic_undo() {
        for tabs in [false, true] {
            let mut e = editor("    fn f() ", "rs");
            e.buffer.newline = "\r\n";
            e.config.expand_tabs = !tabs;
            e.type_character('{');
            e.buffer.end_group();
            e.buffer.begin_group();
            e.act(Action::Newline);
            let indent = if tabs { "\t" } else { "    " };
            assert_eq!(
                e.buffer.text,
                format!("    fn f() {{\r\n    {indent}\r\n    }}")
            );
            assert_eq!(e.buffer.cursor, 12 + 2 + 4 + indent.len());
            e.act(Action::Undo);
            assert_eq!(e.buffer.text, "    fn f() {}");
        }
    }
    #[test]
    fn reverse_unicode_visual_surround_is_one_transaction() {
        let mut e = editor("é😀x", "js");
        e.buffer.cursor = 1;
        e.set_mode(Mode::Visual);
        e.buffer.cursor = 0;
        e.type_character('(');
        assert_eq!(e.buffer.text, "(é😀)x");
        assert_eq!(e.mode, Mode::Normal);
        e.act(Action::Undo);
        assert_eq!(e.buffer.text, "é😀x");
        e.act(Action::Redo);
        assert_eq!(e.buffer.text, "(é😀)x");
    }
    #[test]
    fn paste_ime_and_programmatic_changes_bypass_pairing() {
        let mut m = Model::new(Config::default()).unwrap();
        m.editor = editor("", "js");
        m.replace_input(None, "(", false, None);
        assert_eq!(m.editor.buffer.text, "(");
        m.replace_input(None, "[", true, None);
        m.replace_input(None, "[", false, None);
        assert_eq!(m.editor.buffer.text, "([");
        m.editor.paste_external("{\"😀");
        assert_eq!(m.editor.buffer.text, "([{\"😀");
        m.editor.type_character(')');
        assert_eq!(m.editor.buffer.text, "([{\"😀)");
    }
    #[test]
    fn provenance_projects_intervening_edits_and_current_syntax_blocks_pairing() {
        let mut e = editor("", "js");
        e.type_character('(');
        e.buffer.replace(0..0, "😀");
        e.buffer.cursor = 2;
        e.type_character(')');
        assert_eq!(e.buffer.text, "😀()");
        e.type_character_with_context('[', Some(crate::syntax::Token::Comment));
        assert_eq!(e.buffer.text, "😀()[");
        e.config.pairs.enabled = false;
        e.type_character('{');
        assert_eq!(e.buffer.text, "😀()[{");
    }
}
