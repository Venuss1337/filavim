//! Native LSP-style numbered snippet fields; no scripting or transformations.
use anyhow::{Result, bail};
use std::{collections::BTreeMap, ops::Range};
pub struct Expansion {
    pub text: String,
    pub fields: Vec<Range<usize>>,
}
pub fn normalize(mut expansion: Expansion, newline: &str) -> Expansion {
    let convert = |s: &str| {
        s.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', newline)
    };
    for field in &mut expansion.fields {
        field.start = convert(&expansion.text.chars().take(field.start).collect::<String>())
            .chars()
            .count();
        field.end = convert(&expansion.text.chars().take(field.end).collect::<String>())
            .chars()
            .count();
    }
    expansion.text = convert(&expansion.text);
    expansion
}
pub fn expand(source: &str) -> Result<Expansion> {
    let chars = source.chars().collect::<Vec<_>>();
    let mut i = 0;
    let mut text = String::new();
    let mut fields: BTreeMap<u32, Range<usize>> = BTreeMap::new();
    let mut count = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c == '\\' && i < chars.len() {
            text.push(chars[i]);
            i += 1;
            count += 1;
            continue;
        }
        if c != '$' {
            text.push(c);
            count += 1;
            continue;
        }
        let brace = i < chars.len() && chars[i] == '{';
        if brace {
            i += 1
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1
        }
        if i == start {
            if brace {
                bail!("Snippet variables/transformations are unsupported")
            };
            text.push('$');
            count += 1;
            continue;
        }
        let number = chars[start..i].iter().collect::<String>().parse::<u32>()?;
        let mut value = String::new();
        if brace {
            if i < chars.len() && chars[i] == ':' {
                i += 1;
                while i < chars.len() && chars[i] != '}' {
                    if chars[i] == '$' || chars[i] == '{' {
                        bail!("Nested snippet fields are unsupported")
                    };
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1
                    };
                    value.push(chars[i]);
                    i += 1;
                }
            }
            if i >= chars.len() || chars[i] != '}' {
                bail!("Unsupported or unterminated snippet field")
            };
            i += 1;
        }
        let begin = count;
        text.push_str(&value);
        count += value.chars().count();
        if fields.insert(number, begin..count).is_some() {
            bail!("Mirrored snippet fields are unsupported; no text inserted")
        }
    }
    let final_stop = fields.remove(&0);
    let mut fields = fields.into_values().collect::<Vec<_>>();
    fields.push(final_stop.unwrap_or(count..count));
    Ok(Expansion { text, fields })
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
