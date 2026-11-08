// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux: checks whether Solua can reach the monitors' I²C buses and, if not, sets it up.
//!
//! Monitors are controlled through `/dev/i2c-*`. That needs the `i2c-dev` kernel module and a udev
//! rule that gives the logged-in user access to those files. The `.deb` package installs both;
//! for an AppImage (or a build from source) the in-app "Enable monitor access" button does it, asking
//! for the administrator password through polkit (`pkexec`).

use std::fs::OpenOptions;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// everything needed seems to be in place
    None,
    /// no `/dev/i2c-*` at all: the `i2c-dev` module is not loaded
    ModuleMissing,
    /// the buses exist but this user cannot open them
    NoPermission,
}

pub fn check() -> Problem {
    let mut any = false;
    if let Ok(rd) = std::fs::read_dir("/dev") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.strip_prefix("i2c-").and_then(|n| n.parse::<u32>().ok()).is_none() {
                continue;
            }
            any = true;
            if OpenOptions::new().read(true).write(true).open(e.path()).is_ok() {
                return Problem::None;
            }
        }
    }
    if any {
        Problem::NoPermission
    } else {
        Problem::ModuleMissing
    }
}

const RULE: &str = "KERNEL==\"i2c-[0-9]*\", TAG+=\"uaccess\"";

/// Installs the udev rule and the module-loading file, then loads the module (needs administrator
/// rights, requested with `pkexec`). Blocking.
pub fn enable() -> Result<(), String> {
    let script = format!(
        "set -e\n\
         printf '%s\\n' '{RULE}' > /etc/udev/rules.d/60-solua-i2c.rules\n\
         printf '%s\\n' i2c-dev > /etc/modules-load.d/solua-i2c.conf\n\
         modprobe i2c-dev || true\n\
         udevadm control --reload-rules\n\
         udevadm trigger --subsystem-match=i2c-dev --action=add || true\n\
         udevadm settle || true\n"
    );
    let out = Command::new("pkexec").args(["sh", "-c", &script]).output().map_err(|e| format!("pkexec: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
