//! Interface glyphs from the bundled Agave Nerd Font; no image assets or icon fonts.
//!
//! Every glyph here is verified present in `assets/fonts/AgaveNerdFontMono-Regular.ttf`.
//! Use these names rather than literal codepoints so the set stays auditable. Chrome
//! actions use Codicons (`U+EA60..`); file types use Seti glyphs (`U+E5FA..`).
//! Icons accompany text or have a stated purpose; they never replace state wording.
use super::theme::Hue;

pub const CLOSE: &str = "\u{ea76}";
pub const MINIMIZE: &str = "\u{eaba}";
pub const MAXIMIZE: &str = "\u{eab9}";
pub const RESTORE: &str = "\u{eabb}";
pub const SIDEBAR: &str = "\u{ebf3}";
pub const CHEVRON_RIGHT: &str = "\u{eab6}";
pub const CHEVRON_DOWN: &str = "\u{eab4}";
pub const FOLDER: &str = "\u{ea83}";
pub const FOLDER_OPEN: &str = "\u{eaf7}";
pub const FILE: &str = "\u{ea7b}";
pub const ERROR: &str = "\u{ea87}";
pub const WARNING: &str = "\u{ea6c}";
pub const INFO: &str = "\u{ea74}";
pub const HINT: &str = "\u{ea61}";
pub const CHECK: &str = "\u{eab2}";
pub const ZAP: &str = "\u{ea86}";
pub const SEARCH: &str = "\u{ea6d}";
pub const SYNC: &str = "\u{ea77}";
pub const HISTORY: &str = "\u{ea82}";
pub const EDIT: &str = "\u{ea73}";
pub const TERMINAL: &str = "\u{ea85}";
pub const PACKAGE: &str = "\u{eb29}";
pub const SIGNATURE: &str = "\u{ea8c}";
pub const DOT: &str = "\u{ea71}";
/// Git branch (devicons), for the current branch in the status pane.
pub const BRANCH: &str = "\u{e725}";
/// Settings gear (codicons), marking attached language servers.
pub const GEAR: &str = "\u{eb51}";

/// File-type glyph and identity hue. `None` means neutral (secondary text).
pub fn file(name: &str) -> (&'static str, Option<Hue>) {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match (lower.as_str(), extension) {
        ("cargo.lock", _) | (_, "lock") => ("\u{e672}", None),
        ("license" | "license.md" | "license.txt" | "copying", _) => {
            ("\u{e60a}", Some(Hue::Yellow))
        }
        (".gitignore" | ".gitattributes" | ".gitmodules", _) => ("\u{e65d}", Some(Hue::Orange)),
        (_, "rs") => ("\u{e68b}", Some(Hue::Orange)),
        (_, "toml" | "ini" | "cfg") => ("\u{e615}", Some(Hue::Purple)),
        (_, "md" | "markdown") => ("\u{e609}", Some(Hue::Blue)),
        (_, "json" | "jsonc") => ("\u{e60b}", Some(Hue::Yellow)),
        (_, "py" | "pyi") => ("\u{e606}", Some(Hue::Blue)),
        (_, "js" | "mjs" | "cjs" | "jsx") => ("\u{e60c}", Some(Hue::Yellow)),
        (_, "ts" | "mts" | "cts" | "tsx") => ("\u{e628}", Some(Hue::Blue)),
        (_, "c" | "h") => ("\u{e649}", Some(Hue::Blue)),
        (_, "cc" | "cpp" | "cxx" | "hpp" | "hh") => ("\u{e646}", Some(Hue::Blue)),
        (_, "go") => ("\u{e627}", Some(Hue::Cyan)),
        (_, "html" | "htm") => ("\u{e60e}", Some(Hue::Orange)),
        (_, "css" | "scss") => ("\u{e614}", Some(Hue::Blue)),
        (_, "sh" | "bash" | "zsh" | "ps1" | "psm1" | "bat" | "cmd") => {
            ("\u{e691}", Some(Hue::Green))
        }
        (_, "yml" | "yaml") => ("\u{e615}", Some(Hue::Purple)),
        (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico") => {
            ("\u{e60d}", Some(Hue::Magenta))
        }
        (_, "ttf" | "otf" | "woff" | "woff2") => ("\u{e64e}", None),
        (_, "txt" | "") => (FILE, None),
        _ => ("\u{e64e}", None),
    }
}

/// LSP `CompletionItemKind` numbers mapped to symbol glyphs.
pub fn completion_kind(kind: u64) -> &'static str {
    match kind {
        1 => "\u{ea93}",       // text
        2..=4 => "\u{ea8c}",   // method, function, constructor
        5 => "\u{eb5f}",       // field
        6 => "\u{ea88}",       // variable
        7 => "\u{eb5b}",       // class
        8 => "\u{eb61}",       // interface
        9 => "\u{ea8b}",       // module
        10 => "\u{eb65}",      // property
        11 => "\u{ea96}",      // unit
        12 | 13 => "\u{ea95}", // value, enum
        14 => "\u{eb62}",      // keyword
        15 => "\u{eb66}",      // snippet
        16 => "\u{eb5c}",      // color
        17 => "\u{ea7b}",      // file
        18 => "\u{eb36}",      // reference
        19 => FOLDER,          // folder
        20 => "\u{eb5e}",      // enum member
        21 => "\u{eb5d}",      // constant
        22 => "\u{ea91}",      // struct
        23 => ZAP,             // event
        24 => "\u{eb64}",      // operator
        25 => "\u{ea92}",      // type parameter
        _ => "\u{eb63}",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_types_and_completion_kinds_resolve() {
        assert_eq!(super::file("main.rs").1, Some(super::Hue::Orange));
        assert_eq!(super::file("Cargo.lock").1, None);
        assert_eq!(super::file("README.MD").1, Some(super::Hue::Blue));
        assert_eq!(super::file("noext").0, super::FILE);
        assert_ne!(super::completion_kind(3), super::completion_kind(14));
    }
}
