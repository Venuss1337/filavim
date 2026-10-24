//! Caret-relative geometry and protocol-aware signature presentation, without I/O.
use lsp_types::{ParameterLabel, SignatureHelp};
use ratatui::layout::Rect;
use ropey::Rope;
use std::ops::Range;

#[derive(Clone, Copy)]
pub enum Placement {
    Above,
    Below,
}

/// Neither candidate covers the caret row. Tiny viewports suppress the popup.
pub fn place(
    viewport: Rect,
    caret: (u16, u16),
    x: u16,
    size: (u16, u16),
    preference: Placement,
    avoid: &[Rect],
) -> Option<Rect> {
    if viewport.width < 4 || !viewport.contains(caret.into()) {
        return None;
    }
    let width = size.0.max(4).min(viewport.width);
    let x = x.clamp(viewport.x, viewport.right().saturating_sub(width));
    let above = caret.1.saturating_sub(viewport.y);
    let below = viewport.bottom().saturating_sub(caret.1 + 1);
    let candidates = match preference {
        Placement::Above => [(true, above), (false, below)],
        Placement::Below => [(false, below), (true, above)],
    };
    // Prefer a full fit; then use the side with more space and crop rows.
    for full in [true, false] {
        let mut sides = candidates;
        if !full && sides[1].1 > sides[0].1 {
            sides.swap(0, 1);
        }
        for (up, available) in sides {
            if available < 3 || (full && available < size.1) {
                continue;
            }
            let height = size.1.max(3).min(available);
            let rect = Rect::new(
                x,
                if up { caret.1 - height } else { caret.1 + 1 },
                width,
                height,
            );
            if avoid.iter().all(|other| !rect.intersects(*other)) {
                return Some(rect);
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub open: usize,
    pub parameter: usize,
}

/// A bounded lexical hint, not a language parser. The LSP server remains authoritative.
pub fn call_site(text: &Rope, cursor: usize) -> Option<CallSite> {
    let end = cursor.min(text.len_chars());
    let start = end.saturating_sub(8192);
    let chars: Vec<_> = text.slice(start..end).chars().collect();
    let mut stack: Vec<(char, usize, usize)> = Vec::new();
    let (mut i, mut quote, mut comment) = (0, None, 0usize);
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if comment > 0 {
            if c == '/' && next == Some('*') {
                comment += 1;
                i += 2;
                continue;
            }
            if c == '*' && next == Some('/') {
                comment -= 1;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            comment = 1;
            i += 2;
            continue;
        }
        // Avoid treating Rust lifetime syntax as a string literal.
        if c == '"' || c == '`' || (c == '\'' && chars[i + 1..].iter().take(5).any(|c| *c == '\''))
        {
            quote = Some(c);
            i += 1;
            continue;
        }
        match c {
            '(' | '[' | '{' => stack.push((c, start + i, 0)),
            ')' | ']' | '}' => {
                let expected = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if stack.last().is_some_and(|s| s.0 == expected) {
                    stack.pop();
                } else {
                    stack.clear();
                }
            }
            ',' => {
                if let Some(s) = stack.last_mut() {
                    s.2 += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let (_, open, parameter) = stack.into_iter().rev().find(|s| s.0 == '(')?;
    Some(CallSite { open, parameter })
}

#[derive(Clone, Debug)]
pub struct Signature {
    pub help: SignatureHelp,
    pub buffer: u64,
    pub document: u64,
    pub call: Option<CallSite>,
}
pub struct SignatureLabel<'a> {
    pub label: &'a str,
    /// Byte range in the label; protocol offset labels are always UTF-16.
    pub active: Option<Range<usize>>,
    pub index: usize,
    pub total: usize,
}
pub fn signature_label(help: &SignatureHelp) -> Option<SignatureLabel<'_>> {
    let index = help.active_signature.unwrap_or(0) as usize;
    let index = if index < help.signatures.len() {
        index
    } else {
        0
    };
    let signature = help.signatures.get(index)?;
    let parameter = signature
        .active_parameter
        .or(help.active_parameter)
        .unwrap_or(0) as usize;
    let mut active = None;
    if let Some(parameters) = &signature.parameters {
        let mut after = signature.label.find('(').map_or(0, |i| i + 1);
        for (i, p) in parameters.iter().enumerate() {
            let range = match &p.label {
                ParameterLabel::Simple(s) => signature
                    .label
                    .get(after..)
                    .and_then(|label| label.find(s))
                    .map(|offset| after + offset..after + offset + s.len()),
                ParameterLabel::LabelOffsets([start, end]) => utf16_byte(&signature.label, *start)
                    .zip(utf16_byte(&signature.label, *end))
                    .filter(|(s, e)| s <= e)
                    .map(|(s, e)| s..e),
            };
            if let Some(r) = &range {
                after = r.end;
            }
            if i == parameter {
                active = range;
                break;
            }
        }
    }
    Some(SignatureLabel {
        label: &signature.label,
        active,
        index,
        total: help.signatures.len(),
    })
}
fn utf16_byte(text: &str, offset: u32) -> Option<usize> {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units == offset {
            return Some(byte);
        }
        units += c.len_utf16() as u32;
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.len())
}
