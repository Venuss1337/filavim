//! Floating content helpers (documentation and signature text) plus the shared
//! frame used by every contextual or centered popup. Content and input remain
//! owned by the editor; surface styling comes from [`super::design`].
use super::{
    design::{self, Elevation},
    theme::Theme,
};
use gpui::{prelude::*, *};
use std::ops::Range;

/// A floating glass surface that occludes the editor and swallows its clicks.
pub fn frame(theme: Theme, id: &'static str, font_size: f32) -> Stateful<Div> {
    design::typeface(
        design::glass(&theme, Elevation::Floating),
        &theme,
        font_size,
    )
    .id(id)
    .relative()
    .flex()
    .flex_col()
    .occlude()
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// Documentation prose with light, line-level structure. This is not a Markdown
/// engine: it recognizes the shapes servers commonly send (ATX headings, rules,
/// bullets, fenced and inline code) so their markers do not appear as text.
/// A function-like first line keeps signature coloring; prose is `secondary`.
pub fn documentation(text: String, theme: Theme) -> Div {
    let mut column = div().flex().flex_col().gap(px(design::space::XXS));
    let mut fenced = false;
    let mut gap = false;
    let mut first = true;
    for line in text.lines() {
        let line = line.trim_end();
        let content = line.trim_start();
        if content.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            column = column.child(div().text_color(theme.text).child(line.to_owned()));
            first = false;
            continue;
        }
        if content.is_empty() {
            gap = !first;
            continue;
        }
        let spaced = |d: Div| d.when(gap, |d| d.mt(px(design::space::SM)));
        if matches!(content, "---" | "***" | "___") {
            column = column.child(design::divider(&theme).my(px(design::space::XS)));
            gap = false;
            continue;
        }
        let heading = content.trim_start_matches('#');
        let element = if heading.len() < content.len() && heading.starts_with(' ') {
            div()
                .text_color(theme.text)
                .child(inline_code(heading.trim_start(), theme))
        } else if first && content.contains('(') && content.contains(')') {
            div().child(signature(content, &[], theme))
        } else if let Some(item) = content
            .strip_prefix("- ")
            .or_else(|| content.strip_prefix("* "))
        {
            div()
                .flex()
                .gap(px(design::space::SM))
                .text_color(theme.secondary)
                .child(div().flex_none().text_color(theme.muted).child("•"))
                .child(div().flex_1().min_w(px(0.)).child(inline_code(item, theme)))
        } else {
            div()
                .text_color(theme.secondary)
                .child(inline_code(content, theme))
        };
        column = column.child(spaced(element));
        gap = false;
        first = false;
    }
    column
}
/// Removes backtick delimiters and draws code spans in the text color.
fn inline_code(line: &str, theme: Theme) -> StyledText {
    let (text, spans) = code_spans(line);
    let style = HighlightStyle {
        color: Some(theme.text),
        ..Default::default()
    };
    StyledText::new(text).with_highlights(spans.into_iter().map(|r| (r, style)))
}
/// Text without backtick delimiters plus UTF-8 ranges of the code spans. An
/// unbalanced trailing backtick is kept literally rather than opening a span.
fn code_spans(line: &str) -> (String, Vec<Range<usize>>) {
    if line.matches('`').count() < 2 {
        return (line.to_owned(), Vec::new());
    }
    let parts: Vec<_> = line.split('`').collect();
    let balanced = parts.len() % 2 == 1;
    let mut text = String::with_capacity(line.len());
    let mut spans = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if !balanced && i == parts.len() - 1 {
            text.push('`');
        }
        let start = text.len();
        text.push_str(part);
        let code = i % 2 == 1 && (balanced || i < parts.len() - 1);
        if code && !part.is_empty() {
            spans.push(start..text.len());
        }
    }
    (text, spans)
}

pub fn signature(text: &str, active: &[Range<usize>], theme: Theme) -> StyledText {
    use crate::syntax::Token;
    let label = text.lines().next().unwrap_or("");
    let mut chars = label.char_indices().peekable();
    let mut syntax = Vec::new();
    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut end = start + c.len_utf8();
        let color = if c.is_alphanumeric() || c == '_' {
            while chars
                .peek()
                .is_some_and(|(_, c)| c.is_alphanumeric() || *c == '_')
            {
                if let Some((at, c)) = chars.next() {
                    end = at + c.len_utf8();
                }
            }
            let word = &label[start..end];
            theme.token(
                if matches!(word, "fn" | "function" | "async" | "const" | "mut") {
                    Token::Keyword
                } else if matches!(
                    word,
                    "string"
                        | "number"
                        | "boolean"
                        | "void"
                        | "any"
                        | "unknown"
                        | "never"
                        | "str"
                        | "bool"
                        | "i32"
                        | "u32"
                        | "usize"
                        | "Self"
                ) || c.is_uppercase()
                {
                    Token::Type
                } else if c.is_numeric() {
                    Token::Number
                } else if label[end..].trim_start().starts_with('(') {
                    Token::Function
                } else {
                    Token::Parameter
                },
            )
        } else {
            theme.token(Token::Operator)
        };
        syntax.push((start..end, color));
    }
    let mut active: Vec<_> = active
        .iter()
        .filter(|r| {
            r.start < r.end
                && r.end <= text.len()
                && text.is_char_boundary(r.start)
                && text.is_char_boundary(r.end)
        })
        .cloned()
        .collect();
    active.sort_by_key(|r| r.start);
    let mut edges: Vec<_> = syntax
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .chain(active.iter().flat_map(|r| [r.start, r.end]))
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let mut token = 0;
    let mut parameter = 0;
    let highlights = edges
        .windows(2)
        .map(|edge| {
            let range = edge[0]..edge[1];
            while token < syntax.len() && syntax[token].0.end <= range.start {
                token += 1;
            }
            while parameter < active.len() && active[parameter].end <= range.start {
                parameter += 1;
            }
            let selected = active
                .get(parameter)
                .is_some_and(|r| r.contains(&range.start));
            (
                range.clone(),
                HighlightStyle {
                    color: Some(if selected {
                        theme.matched
                    } else {
                        syntax
                            .get(token)
                            .filter(|(r, _)| r.contains(&range.start))
                            .map_or(theme.text, |(_, color)| *color)
                    }),
                    font_weight: selected.then_some(FontWeight::BOLD),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(text.to_string()).with_highlights(highlights)
}

#[cfg(test)]
mod tests {
    #[test]
    fn code_spans_strip_delimiters_and_keep_unbalanced_ticks() {
        let (text, spans) = super::code_spans("returns `Résumé` or `none`");
        assert_eq!(text, "returns Résumé or none");
        assert_eq!(&text[spans[0].clone()], "Résumé");
        assert_eq!(&text[spans[1].clone()], "none");
        let (text, spans) = super::code_spans("a `b` c `d");
        assert_eq!(text, "a b c `d");
        assert_eq!(spans.len(), 1);
        assert_eq!(super::code_spans("plain `").0, "plain `");
    }
}
