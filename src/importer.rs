// SPDX-License-Identifier: GPL-3.0-or-later
//! Importa configurações do ClickMonitorDDC (arquivo ClickMonitorDDC.xml, linhas `<set chave="valor">`).

use crate::config::{Config, Profile, ProfileValues};
use crate::schedule::{self, Entry};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn parse_settings(xml: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in xml.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("<set ") else { continue };
        let Some((key, after)) = rest.split_once("=\"") else { continue };
        let Some((val, _)) = after.split_once('"') else { continue };
        map.insert(
            key.trim().to_string(),
            val.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\""),
        );
    }
    map
}

fn hex(map: &BTreeMap<String, String>, key: &str) -> Option<u32> {
    map.get(key).and_then(|v| u32::from_str_radix(v.trim(), 16).ok())
}

fn flag(map: &BTreeMap<String, String>, key: &str) -> Option<bool> {
    map.get(key).map(|v| v.trim().eq_ignore_ascii_case("y"))
}

/// Traduz uma linha de comando do original ("b 30 c 40 r 200") para valores de perfil.
/// Comandos relativos (+/-) e os demais argumentos são ignorados.
pub fn parse_legacy_command(cmd: &str) -> Option<ProfileValues> {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    let mut v = ProfileValues::default();
    let mut any = false;
    let mut i = 0;
    while i + 1 < toks.len() {
        let arg = toks[i].to_ascii_lowercase();
        let val = toks[i + 1];
        if let Ok(n) = val.parse::<u32>() {
            let pct255 = |x: u32| ((x.min(255) * 100 + 127) / 255) as u8;
            let set = match arg.as_str() {
                "b" => {
                    v.brightness = Some(n.min(100) as u8);
                    true
                }
                "c" => {
                    v.contrast = Some(n.min(100) as u8);
                    true
                }
                "o" | "v" => {
                    v.volume = Some(n.min(100) as u8);
                    true
                }
                "r" => {
                    v.red = Some(pct255(n));
                    true
                }
                "g" => {
                    v.green = Some(pct255(n));
                    true
                }
                "l" | "bl" | "i" => {
                    v.blue = Some(pct255(n));
                    true
                }
                _ => false,
            };
            if set {
                any = true;
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    any.then_some(v)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub settings: usize,
    pub schedule: usize,
}

pub fn apply(map: &BTreeMap<String, String>, cfg: &mut Config) -> Report {
    let mut r = Report::default();
    if let Some(v) = flag(map, "autostart") {
        cfg.start_with_windows = v;
        r.settings += 1;
    }
    if let Some(v) = flag(map, "notifications") {
        cfg.show_osd = v;
        r.settings += 1;
    }
    if let Some(v) = flag(map, "samevalues") {
        cfg.sync_all = v;
        r.settings += 1;
    }
    if let Some(step) = hex(map, "wheel_B") {
        cfg.step = step.clamp(1, 25) as u8;
        r.settings += 1;
    }
    for n in 0..8 {
        let enabled = flag(map, &format!("ptimer{n}timeflag")).unwrap_or(false);
        let buf = map.get(&format!("ptimer{n}buffer")).cloned().unwrap_or_default();
        let (Some(h), Some(m)) = (hex(map, &format!("ptimer{n}stimeHour")), hex(map, &format!("ptimer{n}stimeMinute"))) else { continue };
        if !enabled || h >= 24 || m >= 60 {
            continue;
        }
        let Some(values) = parse_legacy_command(&buf) else { continue };
        let name = format!("ClickMonitorDDC {}", n + 1);
        let profile = Profile { name: name.clone(), monitors: [("*".to_string(), values)].into(), ..Default::default() };
        match cfg.profiles.iter_mut().find(|p| p.name == name) {
            Some(slot) => *slot = profile,
            None => cfg.profiles.push(profile),
        }
        cfg.schedule.retain(|e| e.profile != name);
        cfg.schedule.push(Entry { time: schedule::format_time(h * 60 + m), profile: name, enabled: true, days: schedule::ALL_DAYS });
        r.schedule += 1;
    }
    cfg.schedule.sort_by_key(|e| schedule::parse_time(&e.time).unwrap_or(0));
    r
}

/// Procura o arquivo do original nos locais habituais.
pub fn find_default() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in ["USERPROFILE", "APPDATA", "HOME"] {
        if let Some(v) = std::env::var_os(var) {
            dirs.push(PathBuf::from(v));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    dirs.into_iter().map(|d| d.join("ClickMonitorDDC.xml")).find(|p| p.is_file())
}

pub fn import_file(path: &str, cfg: &mut Config) -> Result<Report, String> {
    let bytes = std::fs::read(path.trim().trim_matches('"')).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let map = parse_settings(&text);
    if !map.keys().any(|k| k.starts_with("Color") || k == "autostart" || k.starts_with("ptimer")) {
        return Err("not a ClickMonitorDDC settings file".into());
    }
    Ok(apply(&map, cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<settings ClickMonitorDDC="7.2">
  <set mUID0="TXD0000#5&amp;b0d08cf&amp;0&amp;UID4353"></set>
  <set wheel_B="00000012"></set>
  <set ptimer0stimeHour="0000000E"></set>
  <set ptimer0stimeMinute="0000002F"></set>
  <set ptimer0timeflag="Y"></set>
  <set ptimer0buffer="b 30 c 45 r 255"></set>
  <set ptimer1stimeHour="00000014"></set>
  <set ptimer1stimeMinute="00000000"></set>
  <set ptimer1timeflag="Y"></set>
  <set ptimer1buffer=""></set>
  <set autostart="Y"></set>
  <set notifications="N"></set>
</settings>"#;

    #[test]
    fn parses_xml_lines() {
        let m = parse_settings(SAMPLE);
        assert_eq!(m["mUID0"], "TXD0000#5&b0d08cf&0&UID4353");
        assert_eq!(m["autostart"], "Y");
    }

    #[test]
    fn legacy_commands() {
        let v = parse_legacy_command("b 30 c 45 r 255 g 0").unwrap();
        assert_eq!((v.brightness, v.contrast, v.red, v.green), (Some(30), Some(45), Some(100), Some(0)));
        assert!(parse_legacy_command("b + 10").is_none());
        assert!(parse_legacy_command("").is_none());
        assert!(parse_legacy_command("x f").is_none());
    }

    #[test]
    fn applies_to_config() {
        let mut cfg = Config::default();
        let rep = apply(&parse_settings(SAMPLE), &mut cfg);
        assert_eq!(rep, Report { settings: 3, schedule: 1 }); // autostart, notifications, wheel_B; só o timer com comando
        assert!(cfg.start_with_windows && !cfg.show_osd);
        assert_eq!(cfg.step, 18);
        assert_eq!(cfg.schedule[0].time, "14:47");
        assert_eq!(cfg.profiles[0].monitors["*"].brightness, Some(30));
    }

    #[test]
    fn rejects_other_files() {
        let dir = std::env::temp_dir().join("soluna_imp_test.xml");
        std::fs::write(&dir, "<hello/>").unwrap();
        assert!(import_file(dir.to_str().unwrap(), &mut Config::default()).is_err());
        assert!(import_file("/nonexistent/x.xml", &mut Config::default()).is_err());
    }
}
