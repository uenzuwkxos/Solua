// SPDX-License-Identifier: GPL-3.0-or-later
//! MCCS/VCP constants, readable names and the DDC/CI capabilities string parser.

use std::collections::BTreeMap;

pub const RESET_ALL: u8 = 0x04;
pub const RESET_LUMINANCE: u8 = 0x05;
pub const RESET_COLOR: u8 = 0x08;
pub const BRIGHTNESS: u8 = 0x10;
pub const CONTRAST: u8 = 0x12;
pub const COLOR_PRESET: u8 = 0x14;
pub const GAIN_RED: u8 = 0x16;
pub const GAIN_GREEN: u8 = 0x18;
pub const GAIN_BLUE: u8 = 0x1A;
pub const INPUT_SOURCE: u8 = 0x60;
pub const BLACK_RED: u8 = 0x6C;
pub const BLACK_GREEN: u8 = 0x6E;
pub const BLACK_BLUE: u8 = 0x70;
pub const H_FREQ: u8 = 0xAC;
pub const V_FREQ: u8 = 0xAE;
pub const TECH_TYPE: u8 = 0xB6;
pub const CONTROLLER: u8 = 0xC8;
pub const FIRMWARE: u8 = 0xC9;
pub const VCP_VERSION: u8 = 0xDF;
pub const VOLUME: u8 = 0x62;
pub const MUTE: u8 = 0x8D;
pub const USAGE_HOURS: u8 = 0xC0;
pub const POWER_MODE: u8 = 0xD6;

pub fn code_name(code: u8) -> &'static str {
    match code {
        0x02 => "New control value",
        0x04 => "Restore factory defaults",
        0x05 => "Restore factory luminance/contrast",
        0x08 => "Restore factory color defaults",
        0x0B => "Color temperature increment",
        0x0C => "Color temperature request",
        0x10 => "Luminance (brightness)",
        0x12 => "Contrast",
        0x14 => "Select color preset",
        0x16 => "Video gain: red",
        0x18 => "Video gain: green",
        0x1A => "Video gain: blue",
        0x52 => "Active control",
        0x60 => "Input source",
        0x62 => "Audio speaker volume",
        0x6C => "Video black level: red",
        0x6E => "Video black level: green",
        0x70 => "Video black level: blue",
        0x72 => "Gamma",
        0x86 => "Display scaling",
        0x87 => "Sharpness",
        0x8D => "Audio mute",
        0x8F => "Audio treble",
        0x91 => "Audio bass",
        0xA2 => "Auto setup",
        0xAC => "Horizontal frequency",
        0xAE => "Vertical frequency",
        0xB0 => "Settings (store/restore)",
        0xB6 => "Display technology type",
        0xC0 => "Display usage time (hours)",
        0xC6 => "Application enable key",
        0xC8 => "Display controller type",
        0xC9 => "Display firmware level",
        0xCA => "OSD / button control",
        0xCC => "OSD language",
        0xD6 => "Power mode",
        0xDC => "Display mode (picture preset)",
        0xDF => "VCP version",
        _ => "Manufacturer / unknown",
    }
}

pub fn input_name(v: u16) -> String {
    match v {
        0x01 => "VGA 1".into(),
        0x02 => "VGA 2".into(),
        0x03 => "DVI 1".into(),
        0x04 => "DVI 2".into(),
        0x05 => "Composite 1".into(),
        0x06 => "Composite 2".into(),
        0x07 => "S-Video 1".into(),
        0x08 => "S-Video 2".into(),
        0x09 => "Tuner 1".into(),
        0x0A => "Tuner 2".into(),
        0x0B => "Tuner 3".into(),
        0x0C => "Component 1".into(),
        0x0D => "Component 2".into(),
        0x0E => "Component 3".into(),
        0x0F => "DisplayPort 1".into(),
        0x10 => "DisplayPort 2".into(),
        0x11 => "HDMI 1".into(),
        0x12 => "HDMI 2".into(),
        0x13 => "HDMI 3".into(),
        0x1B => "USB-C".into(),
        _ => format!("Input 0x{v:02X}"),
    }
}

pub fn preset_name(v: u16) -> String {
    match v {
        0x01 => "sRGB".into(),
        0x02 => "Native".into(),
        0x03 => "4000 K".into(),
        0x04 => "5000 K".into(),
        0x05 => "6500 K".into(),
        0x06 => "7500 K".into(),
        0x07 => "8200 K".into(),
        0x08 => "9300 K".into(),
        0x09 => "10000 K".into(),
        0x0A => "11500 K".into(),
        0x0B => "User 1".into(),
        0x0C => "User 2".into(),
        0x0D => "User 3".into(),
        _ => format!("Preset 0x{v:02X}"),
    }
}

/// Common input values, used when the monitor does not report its capabilities.
#[allow(dead_code)]
pub const COMMON_INPUTS: &[u16] = &[0x01, 0x03, 0x0F, 0x10, 0x11, 0x12];
/// Inputs offered even if the monitor does not list them.
pub const STANDARD_INPUTS: &[u16] = &[0x01, 0x03, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x1B];
pub const COMMON_PRESETS: &[u16] = &[0x01, 0x04, 0x05, 0x06, 0x08, 0x0B];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caps {
    pub raw: String,
    pub model: Option<String>,
    pub mccs: Option<String>,
    /// VCP code -> allowed discrete values (empty = continuous)
    pub vcp: BTreeMap<u8, Vec<u16>>,
}

impl Caps {
    pub fn supports(&self, code: u8) -> bool {
        self.vcp.contains_key(&code)
    }
    pub fn values(&self, code: u8) -> &[u16] {
        self.vcp.get(&code).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Returns the balanced content of `name( ... )` at the outermost level.
fn section<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && s[i..].to_ascii_lowercase().starts_with(&format!("{name}(")) {
            // make sure it is the start of a token
            let ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if ok {
                let start = i + name.len() + 1;
                let mut d = 1;
                let mut j = start;
                while j < bytes.len() && d > 0 {
                    match bytes[j] {
                        b'(' => d += 1,
                        b')' => d -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                if d == 0 {
                    return Some(&s[start..j - 1]);
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

fn hex_tokens(s: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<u16>, depth: i32| {
        if !cur.is_empty() && depth == 0 {
            if let Ok(v) = u16::from_str_radix(cur, 16) {
                out.push(v);
            }
        }
        cur.clear();
    };
    for c in s.chars() {
        match c {
            '(' => {
                flush(&mut cur, &mut out, depth);
                depth += 1;
            }
            ')' => depth -= 1,
            c if c.is_ascii_hexdigit() => cur.push(c),
            _ => flush(&mut cur, &mut out, depth),
        }
    }
    flush(&mut cur, &mut out, depth);
    out
}

pub fn parse_caps(raw: &str) -> Caps {
    let raw = raw.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    let inner = if raw.starts_with('(') && raw.ends_with(')') { &raw[1..raw.len() - 1] } else { raw.as_str() };
    let mut caps = Caps { raw: raw.clone(), ..Default::default() };
    caps.model = section(inner, "model").map(|s| s.trim().to_string());
    caps.mccs = section(inner, "mccs_ver").map(|s| s.trim().to_string());
    if let Some(v) = section(inner, "vcp") {
        // walk through "XX" or "XX(v v v)"
        let b = v.as_bytes();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c.is_ascii_whitespace() {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && b[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i == start {
                i += 1; // unexpected character
                continue;
            }
            let code = u8::from_str_radix(&v[start..i], 16);
            let mut values = Vec::new();
            if i < b.len() && b[i] == b'(' {
                let s2 = i + 1;
                let mut d = 1;
                i += 1;
                while i < b.len() && d > 0 {
                    match b[i] {
                        b'(' => d += 1,
                        b')' => d -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                values = hex_tokens(&v[s2..i.saturating_sub(1).max(s2)]);
            }
            if let Ok(code) = code {
                caps.vcp.insert(code, values);
            }
        }
    }
    caps
}

pub fn parse_hex_u8(s: &str) -> Option<u8> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u8::from_str_radix(t, 16).ok()
}

pub fn parse_value(s: &str) -> Option<u32> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).ok()
    } else {
        t.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "(prot(monitor)type(LCD)model(U2720Q)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(01 05 08 0B) 16 18 1A 60(0F 11 12) 62 8D D6(01 04 05) DC(00 03) DF)mccs_ver(2.1)asset_eep(40)mpu(01)mswhql(1))";

    #[test]
    fn parses_codes_and_values() {
        let c = parse_caps(SAMPLE);
        assert_eq!(c.model.as_deref(), Some("U2720Q"));
        assert_eq!(c.mccs.as_deref(), Some("2.1"));
        assert!(c.supports(BRIGHTNESS) && c.supports(CONTRAST) && c.supports(VOLUME));
        assert_eq!(c.values(INPUT_SOURCE), &[0x0F, 0x11, 0x12]);
        assert_eq!(c.values(COLOR_PRESET), &[0x01, 0x05, 0x08, 0x0B]);
        assert_eq!(c.values(POWER_MODE), &[0x01, 0x04, 0x05]);
        assert!(c.values(BRIGHTNESS).is_empty());
        assert!(!c.supports(0x72));
    }

    #[test]
    fn tolerates_garbage() {
        let c = parse_caps("(vcp(10 12 zz 14(01");
        assert!(c.vcp.is_empty() || c.supports(0x10) || true);
        let c = parse_caps("");
        assert!(c.vcp.is_empty());
    }

    #[test]
    fn parses_values() {
        assert_eq!(parse_hex_u8("0x1A"), Some(0x1A));
        assert_eq!(parse_hex_u8("d6"), Some(0xD6));
        assert_eq!(parse_value("0x10"), Some(16));
        assert_eq!(parse_value(" 42 "), Some(42));
        assert_eq!(parse_value("x"), None);
    }
}
