//! Native LSP-style numbered snippet fields; no scripting or transformations.
use anyhow::{Result, bail};
use std::{collections::BTreeMap, ops::Range};
pub struct Expansion {
    pub text: String,
    pub fields: Vec<Range<usize>>,
    pub groups: Vec<Vec<Range<usize>>>,
}
pub fn normalize(mut expansion: Expansion, newline: &str) -> Expansion {
    let convert = |s: &str| {
        s.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', newline)
    };
    for group in &mut expansion.groups {
        for range in group {
            range.start = convert(&expansion.text.chars().take(range.start).collect::<String>())
                .chars()
                .count();
            range.end = convert(&expansion.text.chars().take(range.end).collect::<String>())
                .chars()
                .count();
        }
    }
    expansion.fields = expansion
        .groups
        .iter()
        .filter_map(|g| g.first().cloned())
        .collect();
    expansion.text = convert(&expansion.text);
    expansion
}
#[derive(Clone)]
enum Part {
    Text(String),
    Field(u32, Vec<Part>),
}
fn parse(chars: &[char], index: &mut usize, nested: bool, depth: usize) -> Result<Vec<Part>> {
    anyhow::ensure!(depth < 32, "Snippet nesting exceeds limit");
    let mut parts = Vec::new();
    let mut text = String::new();
    while *index < chars.len() {
        let ch = chars[*index];
        *index += 1;
        if ch == '}' && nested {
            if !text.is_empty() {
                parts.push(Part::Text(text));
            }
            return Ok(parts);
        }
        if ch == '\\' {
            if let Some(ch) = chars.get(*index) {
                text.push(*ch);
                *index += 1;
            }
            continue;
        }
        if ch != '$' {
            text.push(ch);
            continue;
        }
        if !text.is_empty() {
            parts.push(Part::Text(std::mem::take(&mut text)));
        }
        let brace = chars.get(*index) == Some(&'{');
        if brace {
            *index += 1;
        }
        let start = *index;
        while chars.get(*index).is_some_and(char::is_ascii_digit) {
            *index += 1;
        }
        if start == *index {
            if brace
                || chars
                    .get(*index)
                    .is_some_and(|c| c.is_alphabetic() || *c == '_')
            {
                bail!("Snippet variables and transformations are unsupported");
            }
            text.push('$');
            continue;
        }
        let number = chars[start..*index].iter().collect::<String>().parse()?;
        let default = if brace {
            match chars.get(*index) {
                Some(':') => {
                    *index += 1;
                    parse(chars, index, true, depth + 1)?
                }
                Some('}') => {
                    *index += 1;
                    Vec::new()
                }
                _ => bail!("Snippet choices/transformations or malformed placeholder unsupported"),
            }
        } else {
            Vec::new()
        };
        parts.push(Part::Field(number, default));
    }
    anyhow::ensure!(!nested, "Unterminated snippet placeholder");
    if !text.is_empty() {
        parts.push(Part::Text(text));
    }
    Ok(parts)
}
pub fn expand(source: &str) -> Result<Expansion> {
    anyhow::ensure!(source.len() <= 1024 * 1024, "Snippet exceeds size limit");
    let parts = parse(&source.chars().collect::<Vec<_>>(), &mut 0, false, 0)?;
    fn defaults(parts: &[Part], values: &mut BTreeMap<u32, Vec<Part>>) {
        for part in parts {
            if let Part::Field(number, body) = part {
                if !body.is_empty() {
                    values.entry(*number).or_insert_with(|| body.clone());
                }
                defaults(body, values);
            }
        }
    }
    fn render(
        parts: &[Part],
        values: &BTreeMap<u32, Vec<Part>>,
        text: &mut String,
        groups: &mut BTreeMap<u32, Vec<Range<usize>>>,
        stack: &mut Vec<u32>,
    ) -> Result<()> {
        for part in parts {
            match part {
                Part::Text(value) => text.push_str(value),
                Part::Field(number, body) => {
                    anyhow::ensure!(!stack.contains(number), "Recursive snippet placeholder");
                    let start = text.chars().count();
                    stack.push(*number);
                    render(
                        values.get(number).unwrap_or(body),
                        values,
                        text,
                        groups,
                        stack,
                    )?;
                    stack.pop();
                    anyhow::ensure!(
                        text.len() <= 1024 * 1024,
                        "Expanded snippet exceeds size limit"
                    );
                    groups
                        .entry(*number)
                        .or_default()
                        .push(start..text.chars().count());
                    anyhow::ensure!(groups.len() <= 4096, "Too many snippet placeholders");
                }
            }
        }
        Ok(())
    }
    let mut values = BTreeMap::new();
    defaults(&parts, &mut values);
    let mut text = String::new();
    let mut groups = BTreeMap::new();
    render(&parts, &values, &mut text, &mut groups, &mut Vec::new())?;
    let final_stop = groups
        .remove(&0)
        .unwrap_or_else(|| std::iter::once(text.chars().count()..text.chars().count()).collect());
    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.push(final_stop);
    let fields = groups.iter().map(|g| g[0].clone()).collect();
    Ok(Expansion {
        text,
        fields,
        groups,
    })
}
/// One document's scalar placeholder ranges; text/history remain owned by Buffer.
pub struct Session {
    groups: Vec<Vec<Range<usize>>>,
    current: usize,
    revision: u64,
    selected: bool,
}
impl Session {
    pub fn new(
        groups: Vec<Vec<Range<usize>>>,
        start: usize,
        buffer: &crate::buffer::Buffer,
    ) -> Self {
        Self {
            groups: groups
                .into_iter()
                .map(|g| {
                    g.into_iter()
                        .map(|r| start + r.start..start + r.end)
                        .collect()
                })
                .collect(),
            current: 0,
            revision: buffer.revision,
            selected: true,
        }
    }
    pub fn selection(&self, buffer: &crate::buffer::Buffer) -> Option<Range<usize>> {
        let range = self.groups.get(self.current)?.first()?;
        (self.selected && self.revision == buffer.revision && buffer.cursor == range.start)
            .then(|| range.clone())
    }
    fn advance(&mut self, buffer: &crate::buffer::Buffer) -> bool {
        let Some(changes) = buffer.changes_since(self.revision) else {
            return false;
        };
        for change in changes {
            for group in &mut self.groups {
                for range in group {
                    if change.range.end <= range.start {
                        let delta = change.text.chars().count() as isize
                            - (change.range.end - change.range.start) as isize;
                        range.start = range.start.saturating_add_signed(delta);
                        range.end = range.end.saturating_add_signed(delta);
                    } else if change.range.start < range.end {
                        return false;
                    }
                }
            }
        }
        self.revision = buffer.revision;
        true
    }
    pub fn navigate(
        &mut self,
        buffer: &mut crate::buffer::Buffer,
        backwards: bool,
    ) -> Option<bool> {
        if !self.advance(buffer) {
            return None;
        }
        self.current = if backwards {
            self.current.saturating_sub(1)
        } else {
            (self.current + 1).min(self.groups.len() - 1)
        };
        buffer.end_group();
        buffer.begin_group();
        self.selected = true;
        buffer.cursor = self.groups[self.current].first()?.start;
        buffer.snap_cursor();
        Some(self.current + 1 < self.groups.len())
    }
    pub fn replace(
        &mut self,
        buffer: &mut crate::buffer::Buffer,
        range: Range<usize>,
        text: &str,
    ) -> bool {
        if !self.advance(buffer) {
            return false;
        }
        let Some(active) = self
            .groups
            .get(self.current)
            .and_then(|g| g.first())
            .cloned()
        else {
            return false;
        };
        if range.start < active.start || range.end > active.end {
            return false;
        }
        let replacement = format!(
            "{}{}{}",
            buffer.text.slice(active.start..range.start),
            text,
            buffer.text.slice(range.end..active.end)
        );
        let cursor = active.start + range.start - active.start + text.chars().count();
        let length = replacement.chars().count();
        let mut edits = self.groups[self.current].clone();
        edits.sort_by_key(|r| r.start);
        for edit in edits.into_iter().rev() {
            let delta = length as isize - (edit.end - edit.start) as isize;
            buffer.replace(edit.clone(), &replacement);
            for (index, group) in self.groups.iter_mut().enumerate() {
                group.retain_mut(|field| {
                    if index == self.current && *field == edit {
                        field.end = field.start + length;
                        return true;
                    }
                    if field.start >= edit.end {
                        field.start = field.start.saturating_add_signed(delta);
                        field.end = field.end.saturating_add_signed(delta);
                        true
                    } else {
                        field.end <= edit.start
                    }
                });
            }
        }
        let removed_before = self.groups[..self.current]
            .iter()
            .filter(|g| g.is_empty())
            .count();
        self.groups.retain(|g| !g.is_empty());
        self.current = self.current.saturating_sub(removed_before);
        buffer.cursor = cursor.min(buffer.text.len_chars());
        buffer.snap_cursor();
        self.revision = buffer.revision;
        self.selected = false;
        true
    }
}
pub fn defaults(language: &str) -> Vec<(&'static str, &'static str)> {
    match language {
        "rust" => vec![
            ("fn", "fn ${1:name}(${2}) ${3:-> ()} {\n    $0\n}"),
            (
                "struct",
                "struct ${1:Name} {\n    ${2:field}: ${3:Type},\n}\n$0",
            ),
            ("test", "#[test]\nfn ${1:test_name}() {\n    $0\n}"),
        ],
        "python" => vec![
            ("def", "def ${1:name}(${2}):\n    $0"),
            ("class", "class ${1:Name}:\n    $0"),
        ],
        "javascript" | "typescript" | "typescriptreact" | "javascriptreact" => vec![
            ("fn", "function ${1:name}(${2}) {\n    $0\n}"),
            ("const", "const ${1:name} = ${2:value};$0"),
        ],
        "go" => vec![("func", "func ${1:name}(${2}) {\n    $0\n}")],
        _ => vec![("for", "for (${1}) {\n    $0\n}")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    fn fixture(source: &str) -> (Buffer, Session) {
        let expansion = expand(source).unwrap();
        let mut buffer = Buffer::new(ropey::Rope::from_str(&expansion.text), None);
        buffer.cursor = expansion.fields[0].start;
        let session = Session::new(expansion.groups, 0, &buffer);
        (buffer, session)
    }
    #[test]
    fn mirrored_unicode_fields_navigate_and_undo_together() {
        let (mut buffer, mut session) = fixture("${1:é} = $1; ${2:value}$0");
        assert_eq!(session.selection(&buffer), Some(0..1));
        buffer.begin_group();
        assert!(session.replace(&mut buffer, 0..1, "名前"));
        assert_eq!(buffer.text.to_string(), "名前 = 名前; value");
        assert_eq!(buffer.cursor, 2);
        assert_eq!(session.navigate(&mut buffer, false), Some(true));
        assert_eq!(session.selection(&buffer), Some(9..14));
        assert_eq!(session.navigate(&mut buffer, true), Some(true));
        assert_eq!(session.selection(&buffer), Some(0..2));
        buffer.end_group();
        buffer.undo();
        assert_eq!(buffer.text.to_string(), "é = é; value");
        buffer.redo();
        assert_eq!(buffer.text.to_string(), "名前 = 名前; value");
    }
    #[test]
    fn nested_defaults_final_stop_and_crlf_offsets() {
        let ex = normalize(expand("${1:outer ${2:é}}\n$0").unwrap(), "\r\n");
        assert_eq!(ex.text, "outer é\r\n");
        assert_eq!(ex.fields, vec![0..7, 6..7, 9..9]);
        let (mut buffer, mut session) = fixture("${1:outer ${2:x}}$0");
        assert!(session.replace(&mut buffer, 0..7, "new"));
        assert_eq!(buffer.text.to_string(), "new");
        assert_eq!(session.navigate(&mut buffer, false), Some(false));
        assert_eq!(buffer.cursor, 3);
    }
    #[test]
    fn surrounding_edits_rebase_and_intersecting_edits_cancel() {
        let (mut buffer, mut session) = fixture("before ${1:field} $0");
        buffer.replace(0..0, "é");
        assert_eq!(session.navigate(&mut buffer, true), Some(true));
        assert_eq!(session.selection(&buffer), Some(8..13));
        buffer.replace(9..10, "x");
        assert_eq!(session.navigate(&mut buffer, false), None);
        for invalid in ["${1", "${1:$1}", "${1/regex/replace/}", "${TM_FILENAME}"] {
            assert!(expand(invalid).is_err(), "{invalid}");
        }
    }
}
