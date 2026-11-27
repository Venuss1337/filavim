//! Scalar-to-protocol coordinate bookkeeping for Buffer's edit journal.
//!
//! Positions use line-relative UTF-8/UTF-16/UTF-32 units. Native LSP response/edit
//! conversion also exists in `lsp.rs`; GPUI IME conversion belongs to `gui::model`.
use lsp_types::{Position, Range};
use ropey::Rope;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}
pub fn position(text: &Rope, index: usize, encoding: Encoding) -> Position {
    let index = index.min(text.len_chars());
    let row = text.char_to_line(index);
    let start = text.line_to_char(row);
    let character = match encoding {
        Encoding::Utf8 => text.char_to_byte(index) - text.char_to_byte(start),
        Encoding::Utf16 => text.char_to_utf16_cu(index) - text.char_to_utf16_cu(start),
        Encoding::Utf32 => index - start,
    };
    Position::new(row as u32, character as u32)
}
pub fn selection_range(text: &Rope, range: std::ops::Range<usize>, encoding: Encoding) -> Range {
    Range::new(
        position(text, range.start, encoding),
        position(text, range.end, encoding),
    )
}
