//! Small independent String oracle; no production movement/edit helpers in the model.
use filavim::{
    buffer::Buffer,
    config::Config,
    editor::{Editor, Mode},
    input::Action,
};
use proptest::{
    prelude::*,
    test_runner::{Config as PropertyConfig, FileFailurePersistence, RngSeed, TestRunner},
};
use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
enum Op {
    Insert(String),
    Left,
    Right,
    Backspace,
    Delete,
    Replace(u8, u8, String),
    Undo,
    Redo,
}
fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "a",
            " ",
            "\t",
            "\n",
            "\r\n",
            "é",
            "e\u{301}",
            "😀",
            "👩‍💻",
            "中",
        ]),
        0..4,
    )
    .prop_map(|parts| parts.concat())
}
fn operations() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            5 => text().prop_map(Op::Insert),
            2 => Just(Op::Left), 2 => Just(Op::Right),
            2 => Just(Op::Backspace), 2 => Just(Op::Delete),
            2 => (any::<u8>(), any::<u8>(), text()).prop_map(|(a,b,t)| Op::Replace(a,b,t)),
            1 => Just(Op::Undo), 1 => Just(Op::Redo),
        ],
        1..97,
    )
}
fn runner(name: &'static str) -> TestRunner {
    let seed = std::env::var("FILAVIM_PROPTEST_SEED")
        .map(|v| v.parse().expect("FILAVIM_PROPTEST_SEED must be u64"))
        .unwrap_or(0xF11A_2026);
    let cases = std::env::var("FILAVIM_PROPTEST_CASES")
        .map(|v| v.parse().expect("FILAVIM_PROPTEST_CASES must be u32"))
        .unwrap_or(64);
    eprintln!("{name}: seed={seed} cases={cases}; reproduce with FILAVIM_PROPTEST_SEED={seed}");
    TestRunner::new(PropertyConfig {
        cases,
        max_shrink_iters: 2048,
        max_shrink_time: 2000,
        rng_seed: RngSeed::Fixed(seed),
        source_file: Some(file!()),
        test_name: Some(name),
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions.txt",
        ))),
        ..PropertyConfig::default()
    })
}
#[derive(Clone, Default, Debug)]
struct State {
    text: String,
    cursor: usize,
}
fn boundaries(text: &str) -> Vec<usize> {
    let mut result: Vec<_> = text
        .grapheme_indices(true)
        .map(|(byte, _)| text[..byte].chars().count())
        .collect();
    result.push(text.chars().count());
    result
}
fn byte(text: &str, at: usize) -> usize {
    text.char_indices().nth(at).map_or(text.len(), |(b, _)| b)
}
fn replacement(
    state: &mut State,
    undo: &mut Vec<State>,
    redo: &mut Vec<State>,
    start: usize,
    end: usize,
    text: &str,
) {
    if start == end && text.is_empty() {
        return;
    }
    undo.push(state.clone());
    redo.clear();
    state
        .text
        .replace_range(byte(&state.text, start)..byte(&state.text, end), text);
    let inserted = start + text.chars().count();
    state.cursor = boundaries(&state.text)
        .into_iter()
        .find(|b| *b >= inserted)
        .unwrap();
}
#[test]
fn generated_unicode_edits_match_string_oracle_and_undo() {
    runner("generated_unicode_edits")
        .run(&operations(), |ops| {
            let mut buffer = Buffer::default();
            let mut state = State::default();
            let (mut undo, mut redo) = (Vec::new(), Vec::new());
            for op in ops {
                let stops = boundaries(&state.text);
                match op {
                    Op::Insert(text) => {
                        let at = state.cursor;
                        replacement(&mut state, &mut undo, &mut redo, at, at, &text);
                        buffer.insert(&text);
                    }
                    Op::Left => {
                        state.cursor = stops
                            .iter()
                            .rev()
                            .copied()
                            .find(|p| *p < state.cursor)
                            .unwrap_or(0);
                        buffer.horizontal(false);
                    }
                    Op::Right => {
                        state.cursor = stops
                            .iter()
                            .copied()
                            .find(|p| *p > state.cursor)
                            .unwrap_or(state.cursor);
                        buffer.horizontal(true);
                    }
                    Op::Backspace => {
                        let end = state.cursor;
                        let start = stops.iter().rev().copied().find(|p| *p < end).unwrap_or(0);
                        replacement(&mut state, &mut undo, &mut redo, start, end, "");
                        buffer.backspace();
                    }
                    Op::Delete => {
                        let start = state.cursor;
                        let end = stops.iter().copied().find(|p| *p > start).unwrap_or(start);
                        replacement(&mut state, &mut undo, &mut redo, start, end, "");
                        buffer.delete();
                    }
                    Op::Replace(a, b, text) => {
                        let (a, b) = (
                            stops[a as usize % stops.len()],
                            stops[b as usize % stops.len()],
                        );
                        replacement(&mut state, &mut undo, &mut redo, a.min(b), a.max(b), &text);
                        buffer.replace(a.min(b)..a.max(b), &text);
                    }
                    Op::Undo => {
                        if let Some(before) = undo.pop() {
                            redo.push(state);
                            state = before;
                        }
                        buffer.undo();
                    }
                    Op::Redo => {
                        if let Some(after) = redo.pop() {
                            undo.push(state);
                            state = after;
                        }
                        buffer.redo();
                    }
                }
                let actual = buffer.text.to_string();
                prop_assert_eq!(actual.as_str(), state.text.as_str());
                prop_assert_eq!(buffer.cursor, state.cursor);
                prop_assert!(boundaries(&state.text).contains(&buffer.cursor));
            }
            Ok(())
        })
        .unwrap();
}
#[test]
fn generated_modal_sequences_preserve_text_through_undo_redo() {
    runner("generated_modal_sequences")
        .run(&prop::collection::vec((0u8..10, text()), 1..65), |ops| {
            let mut config = Config::default();
            config.lsp.enabled = false;
            config.syntax = false;
            let mut editor = Editor::new(config);
            editor.set_buffer(Buffer::new(Rope::from_str("seed é😀\n"), None));
            for (action, text) in ops {
                match action {
                    0 => {
                        editor.set_mode(Mode::Insert);
                        editor.buffer.insert(&text);
                    }
                    1 => {
                        editor.act(Action::Left);
                    }
                    2 => {
                        editor.act(Action::Right);
                    }
                    3 => {
                        editor.act(Action::Up);
                    }
                    4 => {
                        editor.act(Action::Down);
                    }
                    5 => {
                        editor.act(Action::Backspace);
                    }
                    6 => {
                        editor.act(Action::Delete);
                    }
                    7 => {
                        editor.set_mode(Mode::Visual);
                        editor.act(Action::Right);
                        editor.act(Action::Copy);
                    }
                    8 => {
                        editor.set_mode(Mode::Normal);
                        editor.act(Action::Undo);
                    }
                    _ => {
                        editor.act(Action::Redo);
                    }
                }
                prop_assert!(editor.buffer.cursor <= editor.buffer.text.len_chars());
                prop_assert!(
                    boundaries(&editor.buffer.text.to_string()).contains(&editor.buffer.cursor)
                );
                if let Some(selection) = editor.selection() {
                    prop_assert!(
                        selection.start <= selection.end
                            && selection.end <= editor.buffer.text.len_chars()
                    );
                }
            }
            let final_text = editor.buffer.text.clone();
            let mut undone = 0;
            while editor.buffer.undo() {
                undone += 1;
            }
            prop_assert_eq!(editor.buffer.text.to_string(), "seed é😀\n");
            for _ in 0..undone {
                prop_assert!(editor.buffer.redo());
            }
            prop_assert_eq!(editor.buffer.text, final_text);
            Ok(())
        })
        .unwrap();
}

#[test]
fn generated_literal_replace_matches_string_oracle_and_is_one_undo() {
    runner("literal replacement and history")
        .run(&(text(), text(), text()), |(source, query, replacement)| {
            if query.is_empty() {
                return Ok(());
            }
            let rope = Rope::from_str(&source);
            let results = filavim::search::run(
                &rope,
                &query,
                Default::default(),
                0,
                false,
                Some((&replacement, true)),
            )
            .unwrap();
            let expected = source.replace(&query, &replacement);
            let mut buffer = Buffer::new(rope, None);
            let untouched = Buffer::new(Rope::from_str("independent"), None);
            if let Some(output) = results.replacement {
                prop_assert_eq!(&output, &expected);
                buffer.replace(0..buffer.text.len_chars(), &output);
                prop_assert!(boundaries(&buffer.text.to_string()).contains(&buffer.cursor));
                prop_assert!(buffer.undo());
                prop_assert_eq!(buffer.text.to_string(), source);
                prop_assert!(buffer.redo());
                prop_assert_eq!(buffer.text.to_string(), expected);
            } else {
                prop_assert_eq!(expected, source);
            }
            prop_assert_eq!(untouched.text.to_string(), "independent");
            Ok(())
        })
        .unwrap();
}
#[test]
fn generated_zero_width_regex_replacement_uses_valid_scalar_boundaries() {
    runner("zero width scalar replacement")
        .run(&text(), |source| {
            let result = filavim::search::run(
                &Rope::from_str(&source),
                "(?:)",
                filavim::search::Options {
                    regex: true,
                    ..Default::default()
                },
                0,
                false,
                Some(("_", true)),
            )
            .unwrap();
            let mut expected = String::from("_");
            for ch in source.chars() {
                expected.push(ch);
                expected.push('_');
            }
            prop_assert_eq!(result.replacement.unwrap(), expected);
            prop_assert_eq!(result.replaced, source.chars().count() + 1);
            Ok(())
        })
        .unwrap();
}
