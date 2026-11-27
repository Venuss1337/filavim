//! Conservative, bounded fallback for TypeScript type positions, independent of LSP.
use crate::buffer::Buffer;

const PRIMITIVES: &[&str] = &[
    "string",
    "number",
    "boolean",
    "bigint",
    "symbol",
    "object",
    "unknown",
    "never",
    "undefined",
    "null",
    "void",
    "any",
];
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Block,
    Object,
    Type,
    Class,
    Parameters,
    Group,
    Generic,
    Array,
}

pub fn keywords(buffer: &Buffer) -> &'static [&'static str] {
    let language = buffer.path.as_deref().and_then(crate::lsp::language);
    if !matches!(language, Some("typescript" | "typescriptreact")) {
        return &[];
    }
    let anchor = super::completion::prefix(buffer);
    // Match the editor's conservative local lexical approach: bounded context, no
    // synchronous parser. This runs once per identifier session, not per keystroke.
    let start = buffer
        .text
        .line_to_char(buffer.text.char_to_line(anchor).saturating_sub(64));
    if anchor - start > 8192 {
        return &[];
    }
    let text = buffer.text.slice(start..anchor).to_string();
    let mut chars = text.char_indices().peekable();
    let mut tokens = Vec::with_capacity(64);
    while let Some((at, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '/' && chars.peek().is_some_and(|(_, c)| *c == '/' || *c == '*') {
            let block = chars.next().is_some_and(|(_, c)| c == '*');
            let mut ended = false;
            while let Some((_, c)) = chars.next() {
                if !block && c == '\n' {
                    ended = true;
                    break;
                }
                if block && c == '*' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                    chars.next();
                    ended = true;
                    break;
                }
            }
            if !ended {
                return &[];
            }
            continue;
        }
        if matches!(c, '\'' | '"' | '`') {
            let mut escaped = false;
            let mut ended = false;
            for (_, next) in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == c {
                    ended = true;
                    break;
                }
            }
            if !ended {
                return &[];
            }
            tokens.push("literal");
        } else if super::completion::identifier(c) || c == '$' {
            while chars
                .peek()
                .is_some_and(|(_, c)| super::completion::identifier(*c) || *c == '$')
            {
                chars.next();
            }
            let end = chars.peek().map_or(text.len(), |(at, _)| *at);
            tokens.push(&text[at..end]);
        } else {
            tokens.push(&text[at..at + c.len_utf8()]);
        }
    }
    let mut scopes = Vec::with_capacity(16);
    let mut typed = false;
    let mut variable = false;
    let mut alias = false;
    let mut interface = false;
    let mut class = false;
    let mut function = false;
    let mut previous = "";
    for (index, &token) in tokens.iter().enumerate() {
        let declaration = previous != "."
            && tokens.get(index + 1).is_some_and(|next| {
                next.chars()
                    .next()
                    .is_some_and(super::completion::identifier)
            });
        match token {
            "type" if declaration => alias = true,
            "interface" if declaration => interface = true,
            "class" if declaration => class = true,
            "function" if declaration || tokens.get(index + 1) == Some(&"(") => function = true,
            "let" | "const" | "var" => variable = true,
            "=" => {
                typed = alias;
                variable = false;
            }
            ":" => {
                typed = matches!(
                    scopes.last(),
                    Some(Scope::Type | Scope::Class | Scope::Parameters)
                ) || variable
                    || previous == ")";
            }
            "{" => {
                let scope = if interface || typed && (!function || previous == ":") {
                    Scope::Type
                } else if class {
                    Scope::Class
                } else if matches!(previous, "=" | "(" | "," | "[" | ":") {
                    Scope::Object
                } else {
                    Scope::Block
                };
                scopes.push(scope);
                typed = false;
                variable = false;
                alias = false;
                interface = false;
                class = false;
                function = false;
            }
            "(" => {
                let parameters = function
                    || previous == "="
                    || matches!(scopes.last(), Some(Scope::Type | Scope::Class));
                function |= parameters;
                scopes.push(if parameters {
                    Scope::Parameters
                } else {
                    Scope::Group
                });
                typed = false;
            }
            "<" if typed => scopes.push(Scope::Generic),
            "[" => {
                scopes.push(Scope::Array);
            }
            ")" | "]" | "}" | ">" => {
                let expected = match token {
                    ")" => Some(Scope::Parameters),
                    "]" => Some(Scope::Array),
                    "}" => None,
                    _ => Some(Scope::Generic),
                };
                let ended = scopes.pop();
                if token == ")" && ended == Some(Scope::Parameters) {
                    function = true;
                }
                typed = expected == Some(Scope::Generic) && ended == expected;
            }
            "," => typed = scopes.last() == Some(&Scope::Generic),
            ";" => {
                typed = false;
                variable = false;
                alias = false;
                function = false;
            }
            "." | "?" | "+" | "-" | "!" | "/" => typed = false,
            "|" | "&" => {}
            _ => {}
        }
        previous = token;
    }
    if typed { PRIMITIVES } else { &[] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;
    fn at(source: &str, extension: &str) -> Buffer {
        let cursor = source.find('§').unwrap();
        let mut b = Buffer::new(
            Rope::from_str(&source.replace('§', "")),
            Some(format!("a.{extension}").into()),
        );
        b.cursor = source[..cursor].chars().count();
        b
    }
    #[test]
    fn typescript_annotations_aliases_interfaces_and_generic_types() {
        for source in [
            "type Config = {\n    name: str§\n}",
            "interface User {\n name: str§\n age: number\n}",
            "interface User { name: string; age: num§; active: boolean }",
            "interface User { name: string; age: number; active: boo§ }",
            "type Settings = { theme: string; enabled: boo§; retries: number }",
            "type Settings = { theme: string; enabled: boolean; retries: num§ }",
            "let name: str§",
            "const callback = (value: str§) => value",
            "function get(value: string): str§ {}",
            "function get(value: { name: str§ }) {}",
            "interface X { get(): { name: str§ } }",
            "class C { name?: str§ }",
            "type X = str§",
            "type X = Array<str§>",
            "type X = [str§]",
            "type X = string | num§",
            "type X = { nested: { value: str§ } }",
            "type X = {\n name:§\n}",
            "type X = { name: \n    § }",
            "type X = { /* ignored : */ name: /* more */ str§ }",
        ] {
            let words = keywords(&at(source, "ts"));
            assert!(
                words.contains(&"string")
                    && words.contains(&"number")
                    && words.contains(&"boolean"),
                "{source}"
            );
        }
        let source = format!(
            "{}interface User {{ value: str§ }}",
            "// prior line\n".repeat(80)
        );
        assert!(keywords(&at(&source, "tsx")).contains(&"string"));
    }
    #[test]
    fn no_type_keywords_in_values_members_comments_strings_or_other_languages() {
        for source in [
            "const data = { name: str§ }",
            "const data = { type: str§ }",
            "obj.type = str§",
            "const type = str§",
            "const value = str§",
            "thing.str§",
            "thing?.str§",
            "import { str§ } from 'x'",
            "function f() { return str§ }",
            "class C { method(): string { return cond ? yes : str§ } }",
            "type X = { str§ }",
            "// type X = str§",
            "/* type X = { name: str§",
            "const text = 'name: str§'",
            "type X = { name: `str§` }",
            "const x = cond ? name : str§",
        ] {
            assert!(keywords(&at(source, "ts")).is_empty(), "{source}");
        }
        for ext in ["js", "jsx", "rs", "txt", "py"] {
            assert!(keywords(&at("let value: str§", ext)).is_empty());
        }
    }
}
