//! Shared native floating chrome; content and input remain owned by the editor.
use super::{FONT_FAMILY, theme::Theme};
use gpui::{prelude::*, *};
use std::ops::Range;

pub fn frame(theme: Theme, id: &'static str, font_size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .flex_col()
        .font_family(FONT_FAMILY)
        .text_size(px(font_size))
        .text_color(theme.text)
        .bg(theme.overlay)
        .border_1()
        .border_color(theme.popup_border.opacity(0.55))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
pub fn indicator(theme: Theme, icon: &'static str) -> Div {
    div()
        .absolute()
        .left(px(5.))
        .top(px(-7.))
        .h(px(14.))
        .line_height(px(14.))
        .text_size(px(12.))
        .text_color(if icon == "⚡" {
            theme.warning
        } else if icon == "!" {
            theme.error
        } else {
            theme.matched
        })
        .child(if icon == "⚡" { "⚡\u{fe0e}" } else { icon })
}

pub fn documentation(text: String, theme: Theme) -> StyledText {
    let first = text.lines().next().unwrap_or("");
    if first.contains('(') && first.contains(')') {
        signature(&text, &[], theme)
    } else {
        StyledText::new(text)
    }
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
