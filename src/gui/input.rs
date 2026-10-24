use crate::key::{KeyCode, KeyEvent, KeyModifiers};
use gpui::Keystroke;
/// Text arrives through GPUI's native input handler. This translates bindings only.
pub fn translate(k: &Keystroke) -> Option<KeyEvent> {
    if k.modifiers.platform {
        return None;
    }
    let mut m = KeyModifiers::NONE;
    if k.modifiers.control {
        m |= KeyModifiers::CONTROL;
    }
    if k.modifiers.alt {
        m |= KeyModifiers::ALT;
    }
    if k.modifiers.shift {
        m |= KeyModifiers::SHIFT;
    }
    let code = match k.key.as_str() {
        "escape" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        "f1" => KeyCode::F(1),
        s => {
            let s = k
                .key_char
                .as_deref()
                .filter(|s| s.chars().count() == 1)
                .unwrap_or(s);
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(
                if m.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                },
            )
        }
    };
    Some(KeyEvent::new(code, m))
}
