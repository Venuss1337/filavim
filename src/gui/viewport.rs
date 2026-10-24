//! A clipped shaping string plus explicit scalar-to-shaped-byte boundaries.
use crate::buffer::{Buffer, grapheme_width, rope_graphemes};

#[derive(Clone, Debug)]
pub struct VisibleLine {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub column: usize,
    pub text: String,
    pub boundaries: Vec<(usize, usize)>,
}
impl VisibleLine {
    pub fn new(buffer: &Buffer, row: usize, left: usize, columns: usize, tabs: usize) -> Self {
        let line = buffer.line_slice(row);
        let origin = buffer.text.line_to_char(row);
        let (offset, mut column) = buffer.display_start(row, left, tabs);
        let mut result = Self {
            row,
            start: origin + offset,
            end: origin + offset,
            column,
            text: String::new(),
            boundaries: vec![(origin + offset, 0)],
        };
        let mut at = origin + offset;
        for g in rope_graphemes(line.slice(offset..)) {
            if column >= left.saturating_add(columns).saturating_add(2) {
                break;
            }
            let w = grapheme_width(&g, column, tabs);
            if g == "\t" {
                result.text.extend(std::iter::repeat_n(' ', w));
            } else {
                for c in g.chars() {
                    result.text.push(if c.is_control() { '�' } else { c });
                }
            }
            at += g.chars().count();
            result.boundaries.push((at, result.text.len()));
            column += w;
        }
        result.end = at;
        result
    }
    pub fn byte_for_char(&self, at: usize) -> usize {
        let i = self
            .boundaries
            .partition_point(|p| p.0 <= at)
            .saturating_sub(1);
        self.boundaries[i].1
    }
    pub fn char_for_byte(&self, byte: usize) -> usize {
        let i = self
            .boundaries
            .partition_point(|p| p.1 <= byte)
            .saturating_sub(1);
        self.boundaries[i].0
    }
}
