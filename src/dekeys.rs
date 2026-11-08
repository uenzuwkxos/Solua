// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux: global shortcuts on desktops where the XDG GlobalShortcuts portal is missing.
//!
//! * GNOME (before 48): custom keybindings stored in GSettings (`gsettings`).
//! * XFCE: custom commands stored in xfconf (`xfconf-query`).
//!
//! Both backends register the command `solua --action <id>`; the desktop runs it when the key is
//! pressed and the new process hands the id to the running instance (see `sysutil::send_command`).
//! KDE Plasma and GNOME 48+ use the portal instead (see `portal.rs`).

use crate::portal::Shortcut;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// xdg-desktop-portal GlobalShortcuts (the desktop assigns the keys)
    Portal,
    /// GNOME custom keybindings through `gsettings`
    Gnome,
    /// XFCE custom commands through `xfconf-query`
    Xfce,
    /// no way to register global shortcuts
    None,
}

impl Backend {
    /// True when Solua itself chooses the keys (so the settings page lets the user edit them).
    pub fn editable(self) -> bool {
        matches!(self, Backend::Gnome | Backend::Xfce)
    }
}

fn desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_uppercase()
}

fn portal_available() -> bool {
    let Ok(conn) = zbus::blocking::Connection::session() else { return false };
    let Ok(proxy) = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.GlobalShortcuts",
    ) else {
        return false;
    };
    proxy.get_property::<u32>("version").is_ok()
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

pub fn detect() -> Backend {
    let de = desktop();
    if de.contains("XFCE") {
        return if have("xfconf-query") { Backend::Xfce } else { Backend::None };
    }
    if portal_available() {
        return Backend::Portal;
    }
    if (de.contains("GNOME") || de.contains("UNITY") || de.contains("PANTHEON")) && have("gsettings") {
        return Backend::Gnome;
    }
    Backend::None
}

// ---------------------------------------------------------------- helpers

/// GVariant text for a string: `'it\'s'`.
fn gv_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// `'text'` as a single shell word.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn command_for(id: &str) -> String {
    let exe = crate::sysutil::exe_path().map(|p| p.display().to_string()).unwrap_or_else(|| "solua".into());
    format!("{} --action {}", quote(&exe), quote(id))
}

/// Portal trigger ("CTRL+ALT+Up") to GTK accelerator ("<Control><Alt>Up"); `ctrl` is the name used
/// for the Ctrl modifier ("Control" for GNOME, "Primary" for XFCE).
pub fn accel(trigger: &str, ctrl: &str) -> Option<String> {
    let mut out = String::new();
    let mut key = None;
    for part in trigger.split('+') {
        match part {
            "CTRL" => out.push_str(&format!("<{ctrl}>")),
            "ALT" => out.push_str("<Alt>"),
            "SHIFT" => out.push_str("<Shift>"),
            "LOGO" => out.push_str("<Super>"),
            "" => {}
            k => key = Some(k.to_string()),
        }
    }
    let key = key?;
    if out.is_empty() {
        return None;
    }
    out.push_str(&key);
    Some(out)
}

fn run(cmd: &mut Command) -> Option<String> {
    let o = cmd.output().ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Quoted strings from a GVariant text array such as `['a', 'b']`.
fn parse_str_array(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut inq = false;
    for c in s.chars() {
        match (inq, c) {
            (false, '\'') => {
                inq = true;
                cur.clear();
            }
            (true, '\'') => {
                inq = false;
                out.push(cur.clone());
            }
            (true, c) => cur.push(c),
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------- GNOME

const GN_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const GN_ITEM: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const GN_BASE: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/";

fn apply_gnome(list: &[Shortcut]) {
    let current = run(Command::new("gsettings").args(["get", GN_SCHEMA, "custom-keybindings"])).unwrap_or_default();
    let mut paths: Vec<String> = parse_str_array(&current).into_iter().filter(|p| !p.contains("/solua-")).collect();
    for (i, s) in list.iter().enumerate() {
        let Some(binding) = accel(&s.trigger, "Control") else { continue };
        let path = format!("{GN_BASE}solua-{i}/");
        let schema = format!("{GN_ITEM}:{path}");
        let set = |key: &str, val: &str| {
            let _ = Command::new("gsettings").args(["set", &schema, key, &gv_quote(val)]).output();
        };
        set("name", &format!("Solua: {}", s.description));
        set("command", &command_for(&s.id));
        set("binding", &binding);
        paths.push(path);
    }
    let value = format!("[{}]", paths.iter().map(|p| gv_quote(p)).collect::<Vec<_>>().join(", "));
    let _ = Command::new("gsettings").args(["set", GN_SCHEMA, "custom-keybindings", &value]).output();
}

// ---------------------------------------------------------------- XFCE

const XF_CHANNEL: &str = "xfce4-keyboard-shortcuts";

fn apply_xfce(list: &[Shortcut]) {
    // drop the shortcuts we registered before
    let all = run(Command::new("xfconf-query").args(["-c", XF_CHANNEL, "-l", "-v"])).unwrap_or_default();
    for line in all.lines() {
        let Some((prop, value)) = line.split_once(char::is_whitespace) else { continue };
        if prop.starts_with("/commands/custom/") && value.contains("--action") && value.contains("solua") {
            let _ = Command::new("xfconf-query").args(["-c", XF_CHANNEL, "-r", "-p", prop]).output();
        }
    }
    for s in list {
        let Some(binding) = accel(&s.trigger, "Primary") else { continue };
        let prop = format!("/commands/custom/{binding}");
        let _ = Command::new("xfconf-query")
            .args(["-c", XF_CHANNEL, "-n", "-t", "string", "-p", &prop, "-s", &command_for(&s.id)])
            .output();
    }
}

/// Registers `list` with the backend (blocking: run it on a worker thread).
pub fn apply(backend: Backend, list: &[Shortcut]) {
    match backend {
        Backend::Gnome => apply_gnome(list),
        Backend::Xfce => apply_xfce(list),
        Backend::Portal | Backend::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators() {
        assert_eq!(accel("CTRL+ALT+Up", "Control").as_deref(), Some("<Control><Alt>Up"));
        assert_eq!(accel("CTRL+SHIFT+s", "Primary").as_deref(), Some("<Primary><Shift>s"));
        assert_eq!(accel("Up", "Control"), None);
    }

    #[test]
    fn gvariant_arrays() {
        assert_eq!(parse_str_array("@as []"), Vec::<String>::new());
        assert_eq!(parse_str_array("['/a/', '/b/']"), vec!["/a/".to_string(), "/b/".to_string()]);
    }

    #[test]
    fn gvariant_quoting() {
        assert_eq!(gv_quote("it's"), "'it\\'s'");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }
}
