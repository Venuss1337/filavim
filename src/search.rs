//! Document search evaluated on snapshots by the existing file/search executor.
use anyhow::Result;
use regex::RegexBuilder;
use ropey::Rope;
use std::ops::Range;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub regex: bool,
    pub ignore_case: bool,
    pub whole_word: bool,
}
pub struct Results {
    pub ranges: Vec<Range<usize>>,
    pub current: Option<usize>,
    pub replacement: Option<String>,
    pub cursor: usize,
    pub replaced: usize,
}
pub fn run(
    text: &Rope,
    query: &str,
    options: Options,
    start: usize,
    backwards: bool,
    replace: Option<(&str, bool)>,
) -> Result<Results> {
    run_cancellable(text, query, options, start, backwards, replace, || false)
}
/// Cancellation is checked between matches, not inside a single regex engine scan.
pub fn run_cancellable(
    text: &Rope,
    query: &str,
    options: Options,
    start: usize,
    backwards: bool,
    replace: Option<(&str, bool)>,
    cancelled: impl Fn() -> bool,
) -> Result<Results> {
    let check = || -> Result<()> {
        if cancelled() {
            anyhow::bail!("Search superseded");
        }
        Ok(())
    };
    check()?;
    if query.is_empty() {
        return Ok(Results {
            ranges: vec![],
            current: None,
            replacement: None,
            cursor: start,
            replaced: 0,
        });
    }
    let pattern = if options.regex {
        query.to_owned()
    } else {
        regex::escape(query)
    };
    let pattern = if options.whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(options.ignore_case)
        .multi_line(true)
        .crlf(true)
        .build()?;
    let source = text.to_string();
    check()?;
    let mut ranges = Vec::new();
    for matched in regex.find_iter(&source) {
        check()?;
        ranges.push(text.byte_to_char(matched.start())..text.byte_to_char(matched.end()));
    }
    check()?;
    let current = if backwards {
        ranges
            .iter()
            .rposition(|r| r.start < start)
            .or_else(|| ranges.len().checked_sub(1))
    } else {
        ranges
            .iter()
            .position(|r| r.start >= start)
            .or_else(|| (!ranges.is_empty()).then_some(0))
    };
    let mut result = Results {
        cursor: current.map_or(start, |i| ranges[i].start),
        ranges,
        current,
        replacement: None,
        replaced: 0,
    };
    if let Some((replacement, all)) = replace
        && let Some(current) = current
    {
        let mut output = String::with_capacity(source.len());
        let mut copied = 0;
        for (i, captures) in regex.captures_iter(&source).enumerate() {
            check()?;
            if !all && i != current {
                continue;
            }
            let matched = captures.get(0).unwrap();
            output.push_str(&source[copied..matched.start()]);
            if options.regex {
                captures.expand(replacement, &mut output);
            } else {
                output.push_str(replacement);
            }
            copied = matched.end();
            result.replaced += 1;
            if !all {
                break;
            }
        }
        result.cursor = output.chars().count();
        output.push_str(&source[copied..]);
        result.replacement = Some(output);
        // Replacement changes offsets; highlights are recomputed on the next search.
        result.ranges.clear();
        result.current = None;
    }
    Ok(result)
}
/// Preserve small, unaffected highlight sets during debounce. These are visual
/// hints only: navigation/replacement always use a fresh validated snapshot.
/// Large result sets are left hidden until their background rescan completes.
pub fn project(ranges: &mut Vec<Range<usize>>, changes: &[crate::buffer::TextChange]) {
    if ranges.len() > 4096 || changes.len() > 64 {
        ranges.clear();
        return;
    }
    for change in changes {
        let inserted = change.text.chars().count();
        let removed = change.range.end - change.range.start;
        ranges.retain_mut(|range| {
            if range.end <= change.range.start
                && !(range.start == range.end && range.start == change.range.start)
            {
                return true;
            }
            if range.start >= change.range.end {
                let shift = inserted as isize - removed as isize;
                range.start = range.start.saturating_add_signed(shift);
                range.end = range.end.saturating_add_signed(shift);
                return true;
            }
            false
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_unicode_case_word_and_wrap() {
        let text = Rope::from_str("α cat Cat scatter α");
        let result = run(
            &text,
            "cat",
            Options {
                ignore_case: true,
                whole_word: true,
                ..Options::default()
            },
            8,
            false,
            None,
        )
        .unwrap();
        assert_eq!(result.ranges, vec![2..5, 6..9]);
        assert_eq!(result.current, Some(0));
        assert_eq!(
            run(&text, "α", Options::default(), 0, true, None)
                .unwrap()
                .current,
            Some(1)
        );
    }
    #[test]
    fn regex_captures_zero_width_and_line_endings() {
        let options = Options {
            regex: true,
            ..Options::default()
        };
        let result = run(
            &Rope::from_str("α=1\r\nβ=2\n"),
            r"(\p{L})=(\d)",
            options,
            0,
            false,
            Some(("${2}:$1", true)),
        )
        .unwrap();
        assert_eq!(result.replacement.as_deref(), Some("1:α\r\n2:β\n"));
        let result = run(
            &Rope::from_str("αβ"),
            r"",
            options,
            0,
            false,
            Some(("x", true)),
        )
        .unwrap();
        assert_eq!(result.replaced, 0);
        let result = run(
            &Rope::from_str("αβ"),
            r"(?:)",
            options,
            0,
            false,
            Some(("x", true)),
        )
        .unwrap();
        assert_eq!(result.replacement.as_deref(), Some("xαxβx"));
        assert!(run(&Rope::new(), "[", options, 0, false, None).is_err());
    }
    #[test]
    fn cancellation_interrupts_enumeration_and_projection_keeps_only_unaffected_ranges() {
        let calls = std::cell::Cell::new(0);
        let result = run_cancellable(
            &Rope::from_str(&"match ".repeat(10000)),
            "match",
            Options::default(),
            0,
            false,
            None,
            || {
                let count = calls.get() + 1;
                calls.set(count);
                count >= 20
            },
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 20);
        let mut buffer = crate::buffer::Buffer::new(Rope::from_str("match and match"), None);
        let mut ranges = vec![0..5, 10..15];
        let revision = buffer.revision;
        buffer.insert("x");
        project(&mut ranges, &buffer.changes_since(revision).unwrap());
        assert_eq!(ranges, vec![1..6, 11..16]);
        let revision = buffer.revision;
        buffer.replace(2..3, "");
        project(&mut ranges, &buffer.changes_since(revision).unwrap());
        assert_eq!(ranges, vec![10..15]);
    }
}
