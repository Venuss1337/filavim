//! Small lossless JSONC editor. Only selected value spans are replaced; comments,
//! ordering and unrelated bytes survive. Ambiguous duplicate keys are rejected.
use anyhow::{Result, bail, ensure};
use std::ops::Range;

#[derive(Debug)]
pub struct Node {
    pub span: Range<usize>,
    pub properties: Vec<(String, Node)>,
    pub items: Vec<Node>,
    pub object: bool,
    pub array: bool,
}
impl Node {
    pub fn get(&self, name: &str) -> Option<&Node> {
        self.properties
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, n)| n)
    }
    pub fn string(&self, source: &str) -> Option<String> {
        serde_json::from_str(&source[self.span.clone()]).ok()
    }
}
struct Parser<'a> {
    source: &'a str,
    at: usize,
}
impl Parser<'_> {
    fn space(&mut self) -> Result<()> {
        let b = self.source.as_bytes();
        loop {
            while b.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
            if b.get(self.at..self.at + 2) == Some(b"//") {
                while b.get(self.at).is_some_and(|c| *c != b'\n') {
                    self.at += 1;
                }
            } else if b.get(self.at..self.at + 2) == Some(b"/*") {
                self.at += 2;
                while b.get(self.at..self.at + 2).is_some_and(|s| s != b"*/") {
                    self.at += 1;
                }
                ensure!(
                    b.get(self.at..self.at + 2) == Some(b"*/"),
                    "Unterminated JSONC comment"
                );
                self.at += 2;
            } else {
                return Ok(());
            }
        }
    }
    fn string(&mut self) -> Result<String> {
        let start = self.at;
        ensure!(
            self.source.as_bytes().get(self.at) == Some(&b'"'),
            "Expected JSON string"
        );
        self.at += 1;
        let b = self.source.as_bytes();
        loop {
            match b.get(self.at) {
                Some(b'\\') => self.at += 2,
                Some(b'"') => {
                    self.at += 1;
                    return Ok(serde_json::from_str(&self.source[start..self.at])?);
                }
                Some(_) => self.at += 1,
                None => bail!("Unterminated JSON string"),
            }
        }
    }
    fn value(&mut self, depth: usize) -> Result<Node> {
        ensure!(depth < 128, "Terminal settings are too deeply nested");
        self.space()?;
        let start = self.at;
        let mut n = Node {
            span: start..start,
            properties: Vec::new(),
            items: Vec::new(),
            object: false,
            array: false,
        };
        match self.source.as_bytes().get(self.at) {
            Some(c @ (b'{' | b'[')) => {
                n.object = *c == b'{';
                n.array = *c == b'[';
                let end = if n.object { b'}' } else { b']' };
                self.at += 1;
                self.space()?;
                while self.source.as_bytes().get(self.at) != Some(&end) {
                    if n.object {
                        let key = self.string()?;
                        ensure!(
                            n.get(&key).is_none(),
                            "Duplicate JSONC key {key}; edit settings manually"
                        );
                        self.space()?;
                        ensure!(
                            self.source.as_bytes().get(self.at) == Some(&b':'),
                            "Expected colon"
                        );
                        self.at += 1;
                        n.properties.push((key, self.value(depth + 1)?));
                    } else {
                        n.items.push(self.value(depth + 1)?);
                    }
                    self.space()?;
                    if self.source.as_bytes().get(self.at) == Some(&end) {
                        break;
                    }
                    ensure!(
                        self.source.as_bytes().get(self.at) == Some(&b','),
                        "Expected comma"
                    );
                    self.at += 1;
                    self.space()?;
                }
                self.at += 1;
            }
            Some(b'"') => {
                self.string()?;
            }
            Some(_) => {
                while self
                    .source
                    .as_bytes()
                    .get(self.at)
                    .is_some_and(|c| !c.is_ascii_whitespace() && !b",]} /".contains(c))
                {
                    self.at += 1;
                }
                let _: serde_json::Value = serde_json::from_str(&self.source[start..self.at])?;
            }
            None => bail!("Expected JSONC value"),
        }
        n.span.end = self.at;
        Ok(n)
    }
}
pub fn parse(source: &str) -> Result<Node> {
    let mut p = Parser {
        source,
        at: usize::from(source.starts_with('\u{feff}')) * 3,
    };
    let node = p.value(0)?;
    p.space()?;
    ensure!(
        p.at == source.len() && node.object,
        "Expected a JSONC settings object"
    );
    Ok(node)
}
/// Set a nested object property, creating absent objects without reserializing.
pub fn set(source: &str, node: &Node, path: &[&str], value: &str) -> Result<String> {
    ensure!(node.object && !path.is_empty(), "Expected settings object");
    let key = path[0];
    if let Some(child) = node.get(key) {
        if path.len() > 1 {
            return set(source, child, &path[1..], value);
        }
        let mut result = source.to_owned();
        result.replace_range(child.span.clone(), value);
        return Ok(result);
    }
    let mut nested = value.to_owned();
    for key in path[1..].iter().rev() {
        nested = format!("{{{}: {nested}}}", serde_json::to_string(key)?);
    }
    let line_start = source[..node.span.start].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..node.span.start]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let comma = if node.properties.is_empty() { "" } else { "," };
    let insertion = format!(
        "{newline}{indent}    {}: {nested}{comma}{newline}{indent}",
        serde_json::to_string(key)?
    );
    let mut result = source.to_owned();
    result.insert_str(node.span.start + 1, &insertion);
    parse(&result)?;
    Ok(result)
}
