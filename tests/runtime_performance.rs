use filavim::{
    buffer::{Buffer, display_width},
    git, setup,
};
use ropey::Rope;
use std::{process::Command, time::Duration};

#[test]
fn display_checkpoints_match_fresh_unicode_scans_across_edits_and_undo() {
    for unit in ["x\t", "e\u{301}", "👩‍💻", "🇵🇱🇯🇵", "\u{600}a"] {
        let mut buffer = Buffer::new(
            Rope::from_str(&format!("{}\nsecond", unit.repeat(4000))),
            None,
        );
        for tabs in [2, 4, 8] {
            buffer.cursor = buffer.line_end(0);
            assert_eq!(buffer.column(tabs), display_width(&buffer.line(0), tabs));
            for i in 0..12 {
                let at = if i % 2 == 0 {
                    3
                } else {
                    buffer.line_end(0).saturating_sub(10)
                };
                buffer.replace(at..at, if i % 3 == 0 { "\n" } else { "e\u{301}" });
                buffer.cursor = buffer.line_end(buffer.row());
                let row = buffer.row();
                let expected = display_width(&buffer.line(row), tabs);
                assert_eq!(buffer.column(tabs), expected);
                for column in [1, 10, expected / 2, expected.saturating_sub(2), expected] {
                    let (at, cells) = buffer.display_start(row, column, tabs);
                    assert!(cells <= column);
                    assert_eq!(
                        cells,
                        display_width(&buffer.line_slice(row).slice(..at).to_string(), tabs)
                    );
                    let fresh = Buffer::new(buffer.text.clone(), None);
                    assert_eq!((at, cells), fresh.display_start(row, column, tabs));
                }
            }
            buffer.undo();
            buffer.redo();
            assert_eq!(
                buffer.column(tabs),
                display_width(
                    &buffer
                        .line_slice(buffer.row())
                        .slice(..buffer.cursor - buffer.text.line_to_char(buffer.row()))
                        .to_string(),
                    tabs
                )
            );
        }
    }
}

#[test]
fn git_machine_data_above_old_capture_limit_is_complete_or_explicitly_unavailable() {
    if setup::executable("git").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let mut c = Command::new("git");
        c.current_dir(dir.path()).args(args);
        setup::run_data(c, Duration::from_secs(20), 32 * 1024 * 1024).unwrap()
    };
    run(&["init"]);
    let path = dir.path().join("large.txt");
    let text = "unchanged line\n".repeat(90_000);
    std::fs::write(&path, &text).unwrap();
    run(&["add", "large.txt"]);
    let state = git::inspect(dir.path(), Some(&path), Some(&text)).unwrap();
    assert!(state.lines_unavailable.is_none());
    assert!(state.lines.is_empty());
    for i in 0..2800 {
        std::fs::write(dir.path().join(format!("{i:04}_{}", "x".repeat(190))), "").unwrap();
    }
    let status = run(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    assert!(status.len() > 512 * 1024);
    let state = git::inspect(dir.path(), None, None).unwrap();
    assert_eq!(state.files.len(), 2801);
    let mut command = Command::new("git");
    command.current_dir(dir.path()).args(["show", ":large.txt"]);
    assert!(
        setup::run_data(command, Duration::from_secs(20), 512 * 1024)
            .unwrap_err()
            .to_string()
            .contains("exceeds")
    );
    let huge = "z".repeat(9 * 1024 * 1024);
    std::fs::write(&path, &huge).unwrap();
    run(&["add", "large.txt"]);
    let state = git::inspect(dir.path(), Some(&path), Some(&huge)).unwrap();
    assert!(state.lines.is_empty());
    assert!(state.lines_unavailable.is_some());
}
