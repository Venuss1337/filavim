//! Central mode-aware keymap. The engine only receives semantic actions.
use crate::key::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crate::{config::Config, editor::Mode};
use anyhow::{Result, bail};
use std::collections::HashMap;

/// Windows console Alt-code input may arrive only as a character release.
/// Keep paired releases out of the editor while retaining those text events.
pub struct KeyEvents {
    pressed: HashMap<KeyIdentity, KeyModifiers>,
    windows: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum KeyIdentity {
    #[cfg(windows)]
    Virtual(u8),
    Logical(KeyCode),
}
fn key_identity(code: KeyCode) -> KeyIdentity {
    // BackTab is the terminal's name for Shift+Tab, not a different held key.
    if code == KeyCode::BackTab {
        return KeyIdentity::Logical(KeyCode::Tab);
    }
    #[cfg(windows)]
    if let KeyCode::Char(c) = code
        && let Ok(ch) = u16::try_from(u32::from(c))
    {
        // SAFETY: VkKeyScanW accepts a UTF-16 value and has no pointer arguments.
        // The low byte identifies the key; the high byte describes modifiers.
        // Use the active layout, rather than hardcoding US pairs such as !/1.
        let key = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(ch) };
        if key != -1 {
            return KeyIdentity::Virtual(key as u8);
        }
    }
    KeyIdentity::Logical(match code {
        KeyCode::Char(c) if c.is_ascii_alphabetic() => KeyCode::Char(c.to_ascii_lowercase()),
        code => code,
    })
}
impl Default for KeyEvents {
    fn default() -> Self {
        Self {
            pressed: HashMap::new(),
            windows: cfg!(windows),
        }
    }
}
impl KeyEvents {
    pub fn accept(&mut self, event: &KeyEvent) -> bool {
        if !self.windows {
            return event.kind != KeyEventKind::Release;
        }
        self.accept_identity(event, key_identity(event.code))
    }
    fn accept_identity(&mut self, event: &KeyEvent, identity: KeyIdentity) -> bool {
        match event.kind {
            KeyEventKind::Press | KeyEventKind::Repeat => {
                // Surrogate-pair events can lack a matching release; bound bookkeeping.
                if self.pressed.len() > 256 {
                    self.pressed.clear();
                }
                let initial = self.pressed.entry(identity).or_insert(event.modifiers);
                // Keep ordinary repeat, but changing modifiers cannot turn an
                // already-held key into another binding before its release.
                *initial == event.modifiers
            }
            KeyEventKind::Release => {
                self.pressed.remove(&identity).is_none()
                    && matches!(event.code,KeyCode::Char(c) if !c.is_control())
            }
        }
    }
}
pub fn text_character(key: KeyEvent) -> Option<char> {
    let altgr = key
        .modifiers
        .contains(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if (altgr
        || !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT))
        && let KeyCode::Char(c) = key.code
        && !c.is_control()
    {
        Some(c)
    } else {
        None
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    #[test]
    fn windows_releases_do_not_duplicate_typing_but_alt_code_text_survives() {
        let mut state = KeyEvents {
            pressed: HashMap::new(),
            windows: true,
        };
        for c in ['a', '日', '本', '👩'] {
            assert!(state.accept(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
            assert!(!state.accept(&KeyEvent::new_with_kind(
                KeyCode::Char(c),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )));
        }
        assert!(state.accept(&KeyEvent::new_with_kind(
            KeyCode::Char('\u{200d}'),
            KeyModifiers::NONE,
            KeyEventKind::Release
        )));
        assert!(state.accept(&KeyEvent::new(KeyCode::Char('💻'), KeyModifiers::NONE)));
    }
    #[test]
    fn altgr_is_text_while_control_shortcuts_are_not() {
        let modifiers = KeyModifiers::CONTROL | KeyModifiers::ALT;
        for c in ['ą', 'ł', '€', '@', '\\'] {
            assert_eq!(
                text_character(KeyEvent::new(KeyCode::Char(c), modifiers)),
                Some(c)
            );
        }
        assert_eq!(
            text_character(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            None
        );
    }
    #[test]
    fn releasing_shift_does_not_reinterpret_a_held_letter() {
        for repeat_kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
            let mut state = KeyEvents {
                pressed: HashMap::new(),
                windows: true,
            };
            let event =
                |c, modifiers, kind| KeyEvent::new_with_kind(KeyCode::Char(c), modifiers, kind);
            assert!(state.accept(&event('S', KeyModifiers::SHIFT, KeyEventKind::Press)));
            assert!(state.accept(&event('S', KeyModifiers::SHIFT, repeat_kind)));
            // Releasing Shift can change subsequent repeats and the final
            // letter release to lowercase. Neither is a new lowercase press.
            assert!(!state.accept(&event('s', KeyModifiers::NONE, repeat_kind)));
            assert!(!state.accept(&event('s', KeyModifiers::NONE, repeat_kind)));
            assert!(!state.accept(&event('s', KeyModifiers::NONE, KeyEventKind::Release)));
            assert!(state.accept(&event('s', KeyModifiers::NONE, KeyEventKind::Press)));
            assert!(state.accept(&event('s', KeyModifiers::NONE, repeat_kind)));
            assert!(!state.accept(&event('s', KeyModifiers::NONE, KeyEventKind::Release)));
        }
    }
    #[test]
    fn pressing_shift_while_a_letter_is_held_also_waits_for_release() {
        let mut state = KeyEvents {
            pressed: HashMap::new(),
            windows: true,
        };
        assert!(state.accept(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
        assert!(!state.accept(&KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT)));
        assert!(!state.accept(&KeyEvent::new_with_kind(
            KeyCode::Char('S'),
            KeyModifiers::SHIFT,
            KeyEventKind::Release
        )));
        assert!(state.accept(&KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT)));
    }
    #[test]
    fn modifier_changes_do_not_rebind_navigation_function_or_tab_keys() {
        let pairs = [
            (KeyCode::Up, KeyCode::Up),
            (KeyCode::Down, KeyCode::Down),
            (KeyCode::Left, KeyCode::Left),
            (KeyCode::Right, KeyCode::Right),
            (KeyCode::Home, KeyCode::Home),
            (KeyCode::End, KeyCode::End),
            (KeyCode::PageUp, KeyCode::PageUp),
            (KeyCode::PageDown, KeyCode::PageDown),
            (KeyCode::Delete, KeyCode::Delete),
            (KeyCode::Backspace, KeyCode::Backspace),
            (KeyCode::Enter, KeyCode::Enter),
            (KeyCode::Esc, KeyCode::Esc),
            (KeyCode::F(1), KeyCode::F(1)),
            (KeyCode::F(12), KeyCode::F(12)),
            (KeyCode::BackTab, KeyCode::Tab),
            (KeyCode::Char(' '), KeyCode::Char(' ')),
        ];
        for (shifted, plain) in pairs {
            for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
                let mut state = KeyEvents {
                    pressed: HashMap::new(),
                    windows: true,
                };
                assert!(state.accept(&KeyEvent::new(shifted, KeyModifiers::SHIFT)));
                assert!(
                    !state.accept(&KeyEvent::new_with_kind(plain, KeyModifiers::NONE, kind)),
                    "{plain:?}"
                );
                assert!(!state.accept(&KeyEvent::new_with_kind(
                    plain,
                    KeyModifiers::NONE,
                    KeyEventKind::Release
                )));
                assert!(state.accept(&KeyEvent::new(plain, KeyModifiers::NONE)));
                assert!(state.accept(&KeyEvent::new_with_kind(plain, KeyModifiers::NONE, kind)));
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn shifted_symbols_use_the_active_layout_not_letter_case_or_us_pairs() {
        let mut pairs = 0;
        // Discover real shifted pairs in the active layout, including digits
        // and punctuation. This works on non-US Windows CI/user layouts too.
        for first in ' '..='~' {
            for second in ' '..='~' {
                if first == second || first.is_ascii_alphabetic() || second.is_ascii_alphabetic() {
                    continue;
                }
                if key_identity(KeyCode::Char(first)) != key_identity(KeyCode::Char(second)) {
                    continue;
                }
                pairs += 1;
                for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
                    let mut state = KeyEvents {
                        pressed: HashMap::new(),
                        windows: true,
                    };
                    assert!(
                        state.accept(&KeyEvent::new(KeyCode::Char(first), KeyModifiers::SHIFT))
                    );
                    assert!(
                        !state.accept(&KeyEvent::new_with_kind(
                            KeyCode::Char(second),
                            KeyModifiers::NONE,
                            kind
                        )),
                        "{first} -> {second}"
                    );
                    assert!(!state.accept(&KeyEvent::new_with_kind(
                        KeyCode::Char(second),
                        KeyModifiers::NONE,
                        KeyEventKind::Release
                    )));
                    assert!(
                        state.accept(&KeyEvent::new(KeyCode::Char(second), KeyModifiers::NONE))
                    );
                }
            }
        }
        assert!(
            pairs > 0,
            "active keyboard layout should have shifted symbol pairs"
        );
    }
    #[test]
    fn control_alt_and_shift_changes_are_per_key_and_do_not_block_other_keys() {
        for modifier in [
            KeyModifiers::SHIFT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
        ] {
            let mut state = KeyEvents {
                pressed: HashMap::new(),
                windows: true,
            };
            assert!(state.accept(&KeyEvent::new(KeyCode::F(2), modifier)));
            assert!(!state.accept(&KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)));
            assert!(state.accept(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)));
            assert!(!state.accept(&KeyEvent::new_with_kind(
                KeyCode::F(2),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )));
            assert!(state.accept(&KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ClearInput,
    Complete,
    Hover,
    Definition,
    Implementation,
    References,
    RenameSymbol,
    CodeActions,
    FormatDocument,
    SignatureHelp,
    LspInfo,
    Settings,
    Setup,
    GitStatus,
    GitDiff,
    GitStage,
    GitUnstage,
    Terminal,
    Snippets,
    Up,
    Left,
    Down,
    Right,
    WordBack,
    WordForward,
    Home,
    End,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Insert,
    Append,
    Visual,
    Normal,
    Command,
    Search,
    Delete,
    Backspace,
    DeleteLine,
    Newline,
    OpenBelow,
    Tab,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    Save,
    Quit,
    New,
    Help,
    Submit,
    SearchNext,
    Noop,
    FindFiles,
    LiveGrep,
    RecentFiles,
    Buffers,
    Palette,
    Projects,
    Explorer,
    SplitVertical,
    SplitHorizontal,
    FocusNext,
    FocusPrevious,
    GrowSplit,
    ShrinkSplit,
    CloseView,
    CloseBuffer,
    NextBuffer,
    PreviousBuffer,
    NewTab,
    NextTab,
    PreviousTab,
    Refresh,
    CreateFile,
    CreateDirectory,
    RenameFile,
    DeleteFile,
    ParentDirectory,
}
impl Action {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "clear_input" => Self::ClearInput,
            "complete" => Self::Complete,
            "hover" => Self::Hover,
            "definition" => Self::Definition,
            "implementation" => Self::Implementation,
            "references" => Self::References,
            "rename_symbol" => Self::RenameSymbol,
            "code_actions" => Self::CodeActions,
            "format_document" => Self::FormatDocument,
            "signature_help" => Self::SignatureHelp,
            "lsp_info" => Self::LspInfo,
            "settings" => Self::Settings,
            "setup" => Self::Setup,
            "git_status" => Self::GitStatus,
            "git_diff" => Self::GitDiff,
            "git_stage" => Self::GitStage,
            "git_unstage" => Self::GitUnstage,
            "terminal" => Self::Terminal,
            "snippets" => Self::Snippets,
            "up" => Self::Up,
            "left" => Self::Left,
            "down" => Self::Down,
            "right" => Self::Right,
            "word_back" => Self::WordBack,
            "word_forward" => Self::WordForward,
            "home" => Self::Home,
            "end" => Self::End,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "page_up" => Self::PageUp,
            "page_down" => Self::PageDown,
            "insert" => Self::Insert,
            "append" => Self::Append,
            "visual" => Self::Visual,
            "normal" => Self::Normal,
            "command" => Self::Command,
            "search" => Self::Search,
            "delete" => Self::Delete,
            "backspace" => Self::Backspace,
            "delete_line" => Self::DeleteLine,
            "newline" => Self::Newline,
            "open_below" => Self::OpenBelow,
            "tab" => Self::Tab,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            "copy" => Self::Copy,
            "cut" => Self::Cut,
            "paste" => Self::Paste,
            "save" => Self::Save,
            "quit" => Self::Quit,
            "new" => Self::New,
            "help" => Self::Help,
            "submit" => Self::Submit,
            "search_next" => Self::SearchNext,
            "noop" => Self::Noop,
            "find_files" => Self::FindFiles,
            "live_grep" => Self::LiveGrep,
            "recent_files" => Self::RecentFiles,
            "buffers" => Self::Buffers,
            "palette" => Self::Palette,
            "projects" => Self::Projects,
            "explorer" => Self::Explorer,
            "split_vertical" => Self::SplitVertical,
            "split_horizontal" => Self::SplitHorizontal,
            "focus_next" => Self::FocusNext,
            "focus_previous" => Self::FocusPrevious,
            "grow_split" => Self::GrowSplit,
            "shrink_split" => Self::ShrinkSplit,
            "close_view" => Self::CloseView,
            "close_buffer" => Self::CloseBuffer,
            "next_buffer" => Self::NextBuffer,
            "previous_buffer" => Self::PreviousBuffer,
            "new_tab" => Self::NewTab,
            "next_tab" => Self::NextTab,
            "previous_tab" => Self::PreviousTab,
            "refresh" => Self::Refresh,
            "create_file" => Self::CreateFile,
            "create_directory" => Self::CreateDirectory,
            "rename_file" => Self::RenameFile,
            "delete_file" => Self::DeleteFile,
            "parent_directory" => Self::ParentDirectory,
            _ => bail!("Unknown keybinding action: {name}"),
        })
    }
}

pub struct Keymap {
    bindings: HashMap<(Mode, String), Action>,
    pending: String,
}
impl Keymap {
    pub fn new(config: &Config) -> Result<Self> {
        let mut map = Self {
            bindings: HashMap::new(),
            pending: String::new(),
        };
        for mode in [Mode::Normal, Mode::Visual, Mode::Insert, Mode::Command] {
            for (key, action) in [
                ("esc", Action::Normal),
                ("ctrl-s", Action::Save),
                ("up", Action::Up),
                ("left", Action::Left),
                ("down", Action::Down),
                ("right", Action::Right),
                ("home", Action::Home),
                ("end", Action::End),
                ("pageup", Action::PageUp),
                ("pagedown", Action::PageDown),
                ("ctrl-home", Action::Top),
                ("ctrl-end", Action::Bottom),
                ("f1", Action::Help),
            ] {
                map.bind(mode, key, action);
            }
        }
        for mode in [Mode::Normal, Mode::Visual] {
            for (key, action) in [
                ("w", Action::Up),
                ("a", Action::Left),
                ("s", Action::Down),
                ("d", Action::Right),
                ("q", Action::WordBack),
                ("e", Action::WordForward),
                ("i", Action::Insert),
                ("v", Action::Visual),
                ("shift-i", Action::Append),
                ("x", Action::Delete),
                ("delete", Action::Delete),
                ("backspace", Action::Backspace),
                ("shift-x", Action::DeleteLine),
                ("o", Action::OpenBelow),
                ("u", Action::Undo),
                ("ctrl-r", Action::Redo),
                ("y", Action::Copy),
                ("c", Action::Cut),
                ("p", Action::Paste),
                (":", Action::Command),
                ("/", Action::Search),
                ("n", Action::SearchNext),
                ("space w", Action::Save),
                ("space q", Action::Quit),
                ("space n", Action::New),
                ("space ?", Action::Help),
                ("g g", Action::Top),
                ("shift-g", Action::Bottom),
                ("0", Action::Home),
                ("$", Action::End),
                ("space f f", Action::FindFiles),
                ("space f g", Action::LiveGrep),
                ("space f r", Action::RecentFiles),
                ("space f p", Action::Projects),
                ("space b b", Action::Buffers),
                ("space e", Action::Explorer),
                ("space p", Action::Palette),
                ("space s v", Action::SplitVertical),
                ("space s h", Action::SplitHorizontal),
                ("space s n", Action::FocusNext),
                ("space s p", Action::FocusPrevious),
                ("space s +", Action::GrowSplit),
                ("space s -", Action::ShrinkSplit),
                ("space s x", Action::CloseView),
                ("space b n", Action::NextBuffer),
                ("space b p", Action::PreviousBuffer),
                ("space b x", Action::CloseBuffer),
                ("space b t n", Action::NewTab),
                ("space b t d", Action::NextTab),
                ("space b t a", Action::PreviousTab),
                ("space c a", Action::CodeActions),
                ("space c r", Action::RenameSymbol),
                ("space c f", Action::FormatDocument),
                ("space c s", Action::Snippets),
                ("space g d", Action::Definition),
                ("space g i", Action::Implementation),
                ("space g r", Action::References),
                ("space g s", Action::GitStatus),
                ("space g h", Action::GitDiff),
                ("space t", Action::Terminal),
                ("space l", Action::LspInfo),
                ("shift-k", Action::Hover),
                ("space ,", Action::Settings),
            ] {
                map.bind(mode, key, action);
            }
        }
        for (key, action) in [
            ("ctrl-space", Action::Complete),
            ("ctrl-k", Action::SignatureHelp),
            ("ctrl-n", Action::Down),
            ("ctrl-p", Action::Up),
            ("enter", Action::Newline),
            ("tab", Action::Tab),
            ("backspace", Action::Backspace),
            ("delete", Action::Delete),
            ("ctrl-z", Action::Undo),
            ("ctrl-r", Action::Redo),
        ] {
            map.bind(Mode::Insert, key, action);
        }
        for (key, action) in [("enter", Action::Submit), ("backspace", Action::Backspace)] {
            map.bind(Mode::Command, key, action);
        }
        for mode in [Mode::Picker, Mode::Explorer, Mode::Dialog] {
            for (key, action) in [
                ("esc", Action::Normal),
                ("enter", Action::Submit),
                ("up", Action::Up),
                ("down", Action::Down),
                ("pageup", Action::PageUp),
                ("pagedown", Action::PageDown),
                ("backspace", Action::Backspace),
            ] {
                map.bind(mode, key, action);
            }
        }
        map.bind(Mode::Dialog, "ctrl-u", Action::ClearInput);
        map.bind(Mode::Command, "ctrl-u", Action::ClearInput);
        for (key, action) in [
            ("ctrl-w", Action::Up),
            ("ctrl-s", Action::Down),
            ("ctrl-p", Action::Up),
            ("ctrl-n", Action::Down),
            ("ctrl-u", Action::PageUp),
            ("ctrl-d", Action::PageDown),
            ("tab", Action::Down),
            ("ctrl-r", Action::Refresh),
        ] {
            map.bind(Mode::Picker, key, action);
        }
        for (key, action) in [
            ("w", Action::Up),
            ("s", Action::Down),
            ("a", Action::ParentDirectory),
            ("left", Action::ParentDirectory),
            ("d", Action::Submit),
            ("right", Action::Submit),
            ("i", Action::CreateFile),
            ("shift-i", Action::CreateDirectory),
            ("r", Action::RenameFile),
            ("x", Action::DeleteFile),
            ("ctrl-r", Action::Refresh),
            ("space e", Action::Explorer),
            ("space f f", Action::FindFiles),
            ("space p", Action::Palette),
        ] {
            map.bind(Mode::Explorer, key, action);
        }
        for (mode, bindings) in &config.keybindings {
            let mode = match mode.as_str() {
                "normal" => Mode::Normal,
                "insert" => Mode::Insert,
                "visual" => Mode::Visual,
                "command" => Mode::Command,
                "picker" => Mode::Picker,
                "explorer" => Mode::Explorer,
                "dialog" => Mode::Dialog,
                _ => bail!("Unknown keybinding mode: {mode}"),
            };
            for (key, action) in bindings {
                validate_key(key)?;
                map.bind(mode, key, Action::parse(action)?);
            }
        }
        Ok(map)
    }
    fn bind(&mut self, mode: Mode, key: &str, action: Action) {
        self.bindings.insert((mode, key.to_owned()), action);
    }
    pub fn clear(&mut self) {
        self.pending.clear();
    }
    pub fn pending(&self) -> &str {
        &self.pending
    }
    pub fn resolve(&mut self, mode: Mode, key: KeyEvent) -> Option<Action> {
        if key.code == KeyCode::Esc {
            self.pending.clear();
        }
        let key = key_name(key);
        let candidate = if self.pending.is_empty() {
            key.clone()
        } else {
            format!("{} {key}", self.pending)
        };
        if let Some(&action) = self.bindings.get(&(mode, candidate.clone())) {
            self.clear();
            return Some(action);
        }
        let prefix = format!("{candidate} ");
        if self
            .bindings
            .keys()
            .any(|(m, k)| *m == mode && k.starts_with(&prefix))
        {
            self.pending = candidate;
            return Some(Action::Noop);
        }
        let had_pending = !self.pending.is_empty();
        self.clear();
        if had_pending {
            self.bindings
                .get(&(mode, key))
                .copied()
                .or(Some(Action::Noop))
        } else {
            None
        }
    }
}
fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.split(' ').any(|part| {
            let part = part.strip_prefix("ctrl-").unwrap_or(part);
            let part = part.strip_prefix("alt-").unwrap_or(part);
            let part = part.strip_prefix("shift-").unwrap_or(part);
            part.chars().count() != 1
                && ![
                    "space",
                    "esc",
                    "enter",
                    "tab",
                    "backspace",
                    "delete",
                    "up",
                    "down",
                    "left",
                    "right",
                    "home",
                    "end",
                    "pageup",
                    "pagedown",
                    "f1",
                ]
                .contains(&part)
        })
    {
        bail!("Invalid key sequence: {key}");
    }
    Ok(())
}
pub fn key_name(event: KeyEvent) -> String {
    let mut name = String::new();
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        name.push_str("ctrl-");
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        name.push_str("alt-");
    }
    let printable_shift = matches!(event.code, KeyCode::Char(c) if c.is_ascii_uppercase());
    if printable_shift
        || (event.modifiers.contains(KeyModifiers::SHIFT)
            && !matches!(event.code, KeyCode::Char(_)))
    {
        name.push_str("shift-");
    }
    name.push_str(&match event.code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char(c) => {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase().to_string()
            } else {
                c.to_string()
            }
        }
        KeyCode::Esc => "esc".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::F(1) => "f1".into(),
        _ => "unsupported".into(),
    });
    name
}
