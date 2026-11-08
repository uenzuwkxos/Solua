// SPDX-License-Identifier: GPL-3.0-or-later
//! Interface de linha de comando (sem janela): `soluna list|get|set|profile`.

use crate::backend;
use crate::config::Config;
use crate::core::{to_pct, Core};
use crate::vcp;

fn pt() -> bool {
    crate::sysutil::system_locale().to_ascii_lowercase().starts_with("pt")
}

fn t(en: &'static str, pt_br: &'static str) -> &'static str {
    if pt() { pt_br } else { en }
}

pub fn is_cli(args: &[String]) -> bool {
    matches!(
        args.iter().find(|a| !a.starts_with("--demo")).map(String::as_str),
        Some("list" | "get" | "set" | "profile" | "help" | "--help" | "-h" | "--version" | "-V")
    )
}

fn help() {
    println!("Soluna {}", env!("CARGO_PKG_VERSION"));
    println!("{}", t("Usage:", "Uso:"));
    println!("  soluna list");
    println!("  soluna get <brightness|contrast|volume|red|green|blue|input|preset|0xNN> [-m N]");
    println!("  soluna set <brightness|contrast|volume|red|green|blue> <0-100|+N|-N> [-m N|all]");
    println!("  soluna set <input|preset|power|0xNN> <value|name> [-m N|all]");
    println!("  soluna profile <name>");
    println!("  {}", t("Options: --demo (simulated monitors), -m N (monitor number from `list`)", "Opções: --demo (monitores simulados), -m N (número do monitor em `list`)"));
    println!("  {}", t("Inputs: dp1 dp2 hdmi1 hdmi2 vga dvi usbc · Power: on standby off", "Entradas: dp1 dp2 hdmi1 hdmi2 vga dvi usbc · Energia: on standby off"));
}

fn code_of(what: &str) -> Option<u8> {
    Some(match what.to_ascii_lowercase().as_str() {
        "brightness" | "brilho" => vcp::BRIGHTNESS,
        "contrast" | "contraste" => vcp::CONTRAST,
        "volume" => vcp::VOLUME,
        "red" | "vermelho" => vcp::GAIN_RED,
        "green" | "verde" => vcp::GAIN_GREEN,
        "blue" | "azul" => vcp::GAIN_BLUE,
        "input" | "entrada" => vcp::INPUT_SOURCE,
        "preset" => vcp::COLOR_PRESET,
        "power" | "energia" => vcp::POWER_MODE,
        other => return vcp::parse_hex_u8(other),
    })
}

fn is_percent(code: u8) -> bool {
    matches!(code, vcp::BRIGHTNESS | vcp::CONTRAST | vcp::VOLUME | vcp::GAIN_RED | vcp::GAIN_GREEN | vcp::GAIN_BLUE)
}

fn alias_value(code: u8, s: &str) -> Option<u32> {
    let l = s.to_ascii_lowercase();
    let named = match (code, l.as_str()) {
        (vcp::INPUT_SOURCE, "vga") => Some(0x01),
        (vcp::INPUT_SOURCE, "dvi") => Some(0x03),
        (vcp::INPUT_SOURCE, "dp1") => Some(0x0F),
        (vcp::INPUT_SOURCE, "dp2") => Some(0x10),
        (vcp::INPUT_SOURCE, "hdmi1") => Some(0x11),
        (vcp::INPUT_SOURCE, "hdmi2") => Some(0x12),
        (vcp::INPUT_SOURCE, "usbc") => Some(0x1B),
        (vcp::POWER_MODE, "on") => Some(1),
        (vcp::POWER_MODE, "standby") => Some(4),
        (vcp::POWER_MODE, "off") => Some(5),
        _ => None,
    };
    named.or_else(|| vcp::parse_value(s))
}

fn targets(core: &Core, args: &[String]) -> Result<Vec<usize>, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-m" || args[i] == "--monitor" {
            let v = args.get(i + 1).ok_or("missing monitor number")?;
            if v == "all" {
                return Ok((0..core.mons.len()).collect());
            }
            let n: usize = v.parse().map_err(|_| "invalid monitor number")?;
            if n == 0 || n > core.mons.len() {
                return Err(format!("monitor {n} not found"));
            }
            return Ok(vec![n - 1]);
        }
        i += 1;
    }
    Ok(if core.mons.is_empty() { vec![] } else { vec![0] })
}

pub fn run(args: &[String]) -> i32 {
    let demo = args.iter().any(|a| a == "--demo");
    let args: Vec<String> = args.iter().filter(|a| *a != "--demo").cloned().collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    if matches!(cmd, "help" | "--help" | "-h") {
        help();
        return 0;
    }
    if matches!(cmd, "--version" | "-V") {
        println!("Soluna {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let (be, _) = backend::create(demo);
    let mut core = Core::new(be);
    if let Err(e) = core.refresh() {
        eprintln!("error: {e}");
        return 1;
    }
    if core.mons.is_empty() {
        eprintln!("{}", t("No DDC/CI monitors found", "Nenhum monitor DDC/CI encontrado"));
        return 1;
    }
    match cmd {
        "list" => {
            for (i, m) in core.mons.iter().enumerate() {
                println!("{}: {}  [{}]", i + 1, m.info.name, m.info.detail);
            }
            0
        }
        "get" => {
            let Some(code) = args.get(1).and_then(|w| code_of(w)) else {
                help();
                return 2;
            };
            let idx = match targets(&core, &args[2..]) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            let mut status = 0;
            for i in idx {
                core.probe_basic(i);
                let r = core.backend.get(i, code);
                match r {
                    Ok(v) if is_percent(code) => println!("{}: {}% ({} / {})", i + 1, to_pct(v), v.cur, v.max),
                    Ok(v) => println!("{}: {} (0x{:02X}) max {}", i + 1, v.cur, v.cur, v.max),
                    Err(e) => {
                        eprintln!("{}: {e}", i + 1);
                        status = 1;
                    }
                }
            }
            status
        }
        "set" => {
            let (Some(code), Some(val)) = (args.get(1).and_then(|w| code_of(w)), args.get(2)) else {
                help();
                return 2;
            };
            let idx = match targets(&core, &args[3..]) {
                Ok(v) => v,
                Err(e) => return fail(&e),
            };
            let mut status = 0;
            for i in idx {
                core.probe_basic(i);
                let res = if is_percent(code) {
                    if val.starts_with('+') || val.starts_with('-') {
                        match val.parse::<i32>() {
                            Ok(d) => core.adjust_pct(i, code, d).map(|p| p as u32),
                            Err(_) => return fail("invalid value"),
                        }
                    } else {
                        match val.parse::<u8>() {
                            Ok(p) if p <= 100 => core.set_pct(i, code, p).map(|_| p as u32),
                            _ => return fail("value must be 0-100"),
                        }
                    }
                } else {
                    match alias_value(code, val) {
                        Some(v) => core.set_raw(i, code, v).map(|_| v),
                        None => return fail("invalid value"),
                    }
                };
                match res {
                    Ok(v) => println!("{}: OK ({v})", i + 1),
                    Err(e) => {
                        eprintln!("{}: {e}", i + 1);
                        status = 1;
                    }
                }
            }
            status
        }
        "profile" => {
            let Some(name) = args.get(1) else {
                help();
                return 2;
            };
            let cfg = Config::load();
            let Some(p) = cfg.profile(name).cloned() else {
                return fail(&format!("{}: {name}", t("profile not found", "perfil não encontrado")));
            };
            for i in 0..core.mons.len() {
                core.probe_basic(i);
            }
            let n = core.apply_profile(&p);
            println!("{}", format!("{} · {n}", p.name));
            if n == 0 { 1 } else { 0 }
        }
        _ => {
            help();
            2
        }
    }
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn detects_cli() {
        assert!(is_cli(&s(&["list"])));
        assert!(is_cli(&s(&["--demo", "get", "brightness"])));
        assert!(!is_cli(&s(&["--minimized"])));
        assert!(!is_cli(&s(&[])));
    }

    #[test]
    fn codes_and_aliases() {
        assert_eq!(code_of("Brightness"), Some(0x10));
        assert_eq!(code_of("0xD6"), Some(0xD6));
        assert_eq!(alias_value(vcp::INPUT_SOURCE, "hdmi1"), Some(0x11));
        assert_eq!(alias_value(vcp::POWER_MODE, "off"), Some(5));
        assert_eq!(alias_value(vcp::INPUT_SOURCE, "0x0F"), Some(15));
    }

    #[test]
    fn demo_set_get_flow() {
        assert_eq!(run(&s(&["--demo", "set", "brightness", "33", "-m", "1"])), 0);
        assert_eq!(run(&s(&["--demo", "set", "brightness", "+10", "-m", "all"])), 0);
        assert_eq!(run(&s(&["--demo", "get", "contrast"])), 0);
        assert_eq!(run(&s(&["--demo", "set", "volume", "50", "-m", "2"])), 1); // monitor 2 não tem volume
        assert_eq!(run(&s(&["--demo", "set", "brightness", "300"])), 2);
        assert_eq!(run(&s(&["--demo", "list"])), 0);
    }
}
