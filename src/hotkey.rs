// SPDX-License-Identifier: GPL-3.0-or-later
//! Conversion between text ("Ctrl+Alt+Up") and Windows modifiers/VKs.

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combo {
    pub mods: u32,
    pub vk: u32,
}

const NAMED: &[(&str, u32)] = &[
    ("Up", 0x26), ("Down", 0x28), ("Left", 0x25), ("Right", 0x27),
    ("PageUp", 0x21), ("PageDown", 0x22), ("Home", 0x24), ("End", 0x23),
    ("Insert", 0x2D), ("Delete", 0x2E), ("Space", 0x20),
    ("Plus", 0xBB), ("Minus", 0xBD), ("Comma", 0xBC), ("Period", 0xBE),
    ("NumAdd", 0x6B), ("NumSubtract", 0x6D), ("NumMultiply", 0x6A), ("NumDivide", 0x6F), ("NumDecimal", 0x6E),
    ("Tab", 0x09), ("Enter", 0x0D),
    ("Semicolon", 0xBA), ("Slash", 0xBF), ("Backquote", 0xC0), ("LeftBracket", 0xDB),
    ("Backslash", 0xDC), ("RightBracket", 0xDD), ("Quote", 0xDE),
];

fn key_to_vk(name: &str) -> Option<u32> {
    if name.len() == 1 {
        let c = name.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    if let Some(n) = name.strip_prefix("Num").and_then(|n| n.parse::<u32>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, vk)| *vk)
}

fn vk_to_key(vk: u32) -> Option<String> {
    if (0x30..=0x39).contains(&vk) || (0x41..=0x5A).contains(&vk) {
        return Some((vk as u8 as char).to_string());
    }
    if (0x70..=0x87).contains(&vk) {
        return Some(format!("F{}", vk - 0x6F));
    }
    if (0x60..=0x69).contains(&vk) {
        return Some(format!("Num{}", vk - 0x60));
    }
    NAMED.iter().find(|(_, v)| *v == vk).map(|(n, _)| n.to_string())
}

pub fn parse(text: &str) -> Option<Combo> {
    let mut mods = 0;
    let mut vk = None;
    for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "meta" | "super" => mods |= MOD_WIN,
            _ => {
                if vk.is_some() {
                    return None;
                }
                vk = Some(key_to_vk(part)?);
            }
        }
    }
    let vk = vk?;
    if mods == 0 && !(0x70..=0x87).contains(&vk) {
        return None; // requires at least one modifier (avoids hijacking ordinary keys)
    }
    Some(Combo { mods, vk })
}

pub fn format(c: Combo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.mods & MOD_CONTROL != 0 { parts.push("Ctrl".into()); }
    if c.mods & MOD_ALT != 0 { parts.push("Alt".into()); }
    if c.mods & MOD_SHIFT != 0 { parts.push("Shift".into()); }
    if c.mods & MOD_WIN != 0 { parts.push("Win".into()); }
    parts.push(vk_to_key(c.vk).unwrap_or_else(|| format!("0x{:02X}", c.vk)));
    parts.join("+")
}

/// Shortcut text ("Ctrl+Alt+Up") in the XDG portal format ("CTRL+ALT+Up", XKB key names).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn portal_trigger(text: &str) -> Option<String> {
    let c = parse(text)?;
    let mut parts: Vec<String> = Vec::new();
    if c.mods & MOD_CONTROL != 0 { parts.push("CTRL".into()); }
    if c.mods & MOD_ALT != 0 { parts.push("ALT".into()); }
    if c.mods & MOD_SHIFT != 0 { parts.push("SHIFT".into()); }
    if c.mods & MOD_WIN != 0 { parts.push("LOGO".into()); }
    let key = vk_to_key(c.vk)?;
    let sym = match key.as_str() {
        "PageUp" => "Prior".to_string(),
        "PageDown" => "Next".to_string(),
        "Plus" => "plus".into(),
        "Minus" => "minus".into(),
        "Comma" => "comma".into(),
        "Period" => "period".into(),
        "Space" => "space".into(),
        "Enter" => "Return".into(),
        "Semicolon" => "semicolon".into(),
        "Slash" => "slash".into(),
        "Backquote" => "grave".into(),
        "LeftBracket" => "bracketleft".into(),
        "RightBracket" => "bracketright".into(),
        "Backslash" => "backslash".into(),
        "Quote" => "apostrophe".into(),
        "NumAdd" => "KP_Add".into(),
        "NumSubtract" => "KP_Subtract".into(),
        "NumMultiply" => "KP_Multiply".into(),
        "NumDivide" => "KP_Divide".into(),
        "NumDecimal" => "KP_Decimal".into(),
        k if k.len() == 4 && k.starts_with("Num") => format!("KP_{}", &k[3..]),
        k if k.len() == 1 => k.to_ascii_lowercase(),
        k => k.to_string(),
    };
    parts.push(sym);
    Some(parts.join("+"))
}

/// Candidate keys when capturing a shortcut (Windows virtual-key codes).
pub fn candidate_vks() -> Vec<u32> {
    let mut v: Vec<u32> = vec![0x08, 0x09, 0x0D, 0x1B];
    v.extend(0x20..=0x2E);
    v.extend(0x30..=0x39);
    v.extend(0x41..=0x5A);
    v.extend(0x60..=0x87);
    v.extend(0xBA..=0xC0);
    v.extend(0xDB..=0xDE);
    v
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// still waiting for a key (shows the modifiers already pressed)
    Pending(u32),
    Done(Combo),
    Cancel,
    Clear,
}

/// Decides the capture result from the modifiers and the keys pressed right now.
pub fn decide(mods: u32, down: &[u32]) -> Capture {
    let Some(&vk) = down.first() else { return Capture::Pending(mods) };
    if mods == 0 {
        return match vk {
            0x1B => Capture::Cancel,
            0x08 => Capture::Clear,
            0x70..=0x87 => Capture::Done(Combo { mods, vk }), // F keys are valid on their own
            _ => Capture::Pending(mods),                       // the others require a modifier
        };
    }
    if vk_to_key(vk).is_none() {
        return Capture::Pending(mods);
    }
    Capture::Done(Combo { mods, vk })
}

/// Converts the Slint keyboard event into canonical shortcut text.
/// Returns `None` for keys that are only modifiers or unknown.
pub fn from_slint_event(text: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> Option<String> {
    use slint::platform::Key;
    let c = text.chars().next()?;
    let special: &[(Key, &str)] = &[
        (Key::UpArrow, "Up"), (Key::DownArrow, "Down"), (Key::LeftArrow, "Left"), (Key::RightArrow, "Right"),
        (Key::PageUp, "PageUp"), (Key::PageDown, "PageDown"), (Key::Home, "Home"), (Key::End, "End"),
        (Key::Insert, "Insert"), (Key::Delete, "Delete"),
        (Key::F1, "F1"), (Key::F2, "F2"), (Key::F3, "F3"), (Key::F4, "F4"), (Key::F5, "F5"), (Key::F6, "F6"),
        (Key::F7, "F7"), (Key::F8, "F8"), (Key::F9, "F9"), (Key::F10, "F10"), (Key::F11, "F11"), (Key::F12, "F12"),
    ];
    let key: String = if let Some((_, n)) = special.iter().find(|(k, _)| char::from(*k) == c) {
        (*n).to_string()
    } else if c == ' ' {
        "Space".into()
    } else if c == '+' || c == '=' {
        "Plus".into()
    } else if c == '-' || c == '_' {
        "Minus".into()
    } else if c.is_ascii_alphanumeric() {
        c.to_ascii_uppercase().to_string()
    } else {
        // Modifier keys themselves (Shift, Control, Alt, AltGr, Meta...) arrive as private codes
        // U+0010..U+0018: they are not a final key, so keep capturing.
        return None;
    };
    let mut mods = 0;
    if ctrl { mods |= MOD_CONTROL; }
    if alt { mods |= MOD_ALT; }
    if shift { mods |= MOD_SHIFT; }
    if meta { mods |= MOD_WIN; }
    let vk = key_to_vk(&key)?;
    if mods == 0 {
        return None;
    }
    Some(format(Combo { mods, vk }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_keys_alone_do_not_finish_capture() {
        // Slint delivers Shift/Control/Alt/Meta presses as private codes U+0010..U+0018
        for c in ['\u{10}', '\u{11}', '\u{12}', '\u{13}', '\u{15}', '\u{16}', '\u{17}', '\u{18}'] {
            assert_eq!(from_slint_event(&c.to_string(), true, true, true, true), None);
        }
        assert_eq!(from_slint_event("q", true, false, false, false).as_deref(), Some("Ctrl+Q"));
        assert_eq!(from_slint_event("s", false, false, false, true).as_deref(), Some("Win+S"));
    }

    #[test]
    fn roundtrip() {
        for s in ["Ctrl+Alt+Up", "Ctrl+Shift+F5", "Alt+M", "Ctrl+Alt+PageDown", "Ctrl+Win+Num3"] {
            let c = parse(s).unwrap_or_else(|| panic!("{s}"));
            assert_eq!(format(c), s);
        }
    }

    #[test]
    fn portal_triggers() {
        assert_eq!(portal_trigger("Ctrl+Alt+Up").as_deref(), Some("CTRL+ALT+Up"));
        assert_eq!(portal_trigger("Ctrl+Alt+PageDown").as_deref(), Some("CTRL+ALT+Next"));
        assert_eq!(portal_trigger("Ctrl+Alt+Plus").as_deref(), Some("CTRL+ALT+plus"));
        assert_eq!(portal_trigger("Ctrl+Alt+S").as_deref(), Some("CTRL+ALT+s"));
        assert_eq!(portal_trigger("Ctrl+Win+Num3").as_deref(), Some("CTRL+LOGO+KP_3"));
        assert_eq!(portal_trigger("nonsense"), None);
    }

    #[test]
    fn rejects_invalid() {
        assert!(parse("Up").is_none());
        assert!(parse("Ctrl+").is_none());
        assert!(parse("Ctrl+A+B").is_none());
        assert!(parse("Ctrl+Banana").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn slint_events() {
        let up = char::from(slint::platform::Key::UpArrow).to_string();
        assert_eq!(from_slint_event(&up, true, true, false, false).as_deref(), Some("Ctrl+Alt+Up"));
        assert_eq!(from_slint_event("m", true, true, false, false).as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(from_slint_event("a", false, false, false, false), None);
    }

    #[test]
    fn capture_decisions() {
        assert_eq!(decide(MOD_CONTROL, &[]), Capture::Pending(MOD_CONTROL));
        assert_eq!(decide(MOD_CONTROL | MOD_ALT, &[0x51]), Capture::Done(Combo { mods: MOD_CONTROL | MOD_ALT, vk: 0x51 }));
        assert_eq!(decide(MOD_ALT, &[0x09]), Capture::Done(Combo { mods: MOD_ALT, vk: 0x09 })); // Tab
        assert_eq!(decide(0, &[0x1B]), Capture::Cancel);
        assert_eq!(decide(0, &[0x08]), Capture::Clear);
        assert_eq!(decide(0, &[0x41]), Capture::Pending(0));
        assert!(matches!(decide(0, &[0x7B]), Capture::Done(_))); // F12 alone
        assert_eq!(parse("Alt+Tab").map(format).as_deref(), Some("Alt+Tab"));
    }
}

#[cfg(test)]
mod default_tests {
    #[test]
    fn all_default_hotkeys_parse_and_are_unique() {
        let d = crate::config::default_hotkeys();
        let mut seen = std::collections::HashSet::new();
        for (id, text) in &d {
            let c = super::parse(text).unwrap_or_else(|| panic!("{id}: {text}"));
            assert!(seen.insert((c.mods, c.vk)), "duplicate default: {text}");
        }
    }
}
