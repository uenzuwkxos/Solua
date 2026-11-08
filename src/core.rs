// SPDX-License-Identifier: GPL-3.0-or-later
//! Interface-independent control logic (used by the GUI).

use crate::backend::{Backend, Kind, MonitorInfo, Vcp};
use crate::config::{Profile, ProfileValues};
use crate::vcp::{self, Caps};
use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

pub struct MonState {
    pub info: MonitorInfo,
    pub caps: Option<Caps>,
    pub caps_tried: bool,
    pub values: BTreeMap<u8, Vcp>,
}

pub struct Core {
    pub backend: Box<dyn Backend>,
    pub mons: Vec<MonState>,
}

pub fn to_pct(v: Vcp) -> u8 {
    if v.max == 0 {
        0
    } else {
        ((v.cur as f64 * 100.0 / v.max as f64).round() as i64).clamp(0, 100) as u8
    }
}

pub fn from_pct(pct: u8, max: u32) -> u32 {
    ((pct.min(100) as f64 * max as f64 / 100.0).round()) as u32
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct MonView {
    pub name: String,
    pub detail: String,
    pub internal: bool,
    pub has_brightness: bool,
    pub has_contrast: bool,
    pub has_volume: bool,
    pub has_rgb: bool,
    pub has_presets: bool,
    pub has_inputs: bool,
    pub has_power: bool,
    pub brightness: u8,
    pub contrast: u8,
    pub volume: u8,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub muted: bool,
    pub has_black: bool,
    pub black_r: u8,
    pub black_g: u8,
    pub black_b: u8,
    /// (value, name, not-reported-by-the-monitor)
    pub inputs: Vec<(u16, String, bool)>,
    pub input_value: i32,
    pub presets: Vec<(u16, String, bool)>,
    pub preset_value: i32,
    pub usage_hours: Option<u32>,
    pub caps_raw: String,
}

#[derive(Clone, Debug, Default)]
pub struct MonInfo {
    pub inches: Option<f32>,
    /// Hz
    pub hfreq: Option<u32>,
    /// hundredths of a Hz
    pub vfreq: Option<u32>,
    pub tech: Option<u32>,
    pub controller: Option<u32>,
    pub firmware: Option<u32>,
    pub vcp_version: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct VcpRow {
    pub code: u8,
    pub current: Option<u32>,
    pub max: Option<u32>,
    pub values: Vec<u16>,
}

impl Core {
    pub fn new(backend: Box<dyn Backend>) -> Core {
        Core { backend, mons: Vec::new() }
    }

    /// Re-enumerates monitors keeping capabilities already read (by EDID key).
    pub fn refresh(&mut self) -> Result<(), String> {
        let infos = self.backend.enumerate()?;
        let mut old: BTreeMap<String, MonState> = self.mons.drain(..).map(|m| (m.info.key.clone(), m)).collect();
        for info in infos {
            let caps = old.remove(&info.key).map(|o| (o.caps, o.caps_tried));
            let (caps, tried) = caps.unwrap_or((None, false));
            self.mons.push(MonState { info, caps, caps_tried: tried, values: BTreeMap::new() });
        }
        Ok(())
    }

    fn get_retry(&mut self, i: usize, code: u8, tries: u32) -> Result<Vcp, String> {
        let mut last = String::new();
        for t in 0..tries.max(1) {
            match self.backend.get(i, code) {
                Ok(v) => {
                    if let Some(m) = self.mons.get_mut(i) {
                        m.values.insert(code, v);
                    }
                    return Ok(v);
                }
                Err(e) => {
                    last = e;
                    if t + 1 < tries {
                        sleep(Duration::from_millis(60));
                    }
                }
            }
        }
        Err(last)
    }

    fn set_retry(&mut self, i: usize, code: u8, value: u32) -> Result<(), String> {
        let mut last = String::new();
        for t in 0..3 {
            match self.backend.set(i, code, value) {
                Ok(()) => {
                    if let Some(m) = self.mons.get_mut(i) {
                        if let Some(v) = m.values.get_mut(&code) {
                            v.cur = value;
                        }
                    }
                    return Ok(());
                }
                Err(e) => {
                    last = e;
                    if t < 2 {
                        sleep(Duration::from_millis(60));
                    }
                }
            }
        }
        Err(last)
    }

    /// Quick read: only brightness and contrast (enough to show something on screen).
    pub fn probe_basic(&mut self, i: usize) {
        let _ = self.get_retry(i, vcp::BRIGHTNESS, 3);
        if self.mons.get(i).map(|m| m.info.kind) != Some(Kind::Internal) {
            let _ = self.get_retry(i, vcp::CONTRAST, 2);
        }
    }

    /// Fetches (once) and parses the capabilities string.
    pub fn ensure_caps(&mut self, i: usize) {
        let Some(m) = self.mons.get(i) else { return };
        if m.caps_tried || m.info.kind == Kind::Internal {
            return;
        }
        let res = self.backend.capabilities(i);
        if let Some(m) = self.mons.get_mut(i) {
            m.caps_tried = true;
            if let Ok(raw) = res {
                let c = vcp::parse_caps(&raw);
                if !c.vcp.is_empty() {
                    m.caps = Some(c);
                }
            }
        }
    }

    pub fn supports(&self, i: usize, code: u8) -> bool {
        let Some(m) = self.mons.get(i) else { return false };
        if m.info.kind == Kind::Internal {
            return code == vcp::BRIGHTNESS;
        }
        match &m.caps {
            Some(c) => c.supports(code),
            None => m.values.contains_key(&code),
        }
    }

    /// Reads the other controls the monitor declares (or, without caps, tries each once).
    pub fn read_extended(&mut self, i: usize) {
        let codes = [
            vcp::VOLUME, vcp::MUTE, vcp::GAIN_RED, vcp::GAIN_GREEN, vcp::GAIN_BLUE,
            vcp::BLACK_RED, vcp::BLACK_GREEN, vcp::BLACK_BLUE,
            vcp::COLOR_PRESET, vcp::INPUT_SOURCE, vcp::USAGE_HOURS,
        ];
        let has_caps = self.mons.get(i).map(|m| m.caps.is_some()).unwrap_or(false);
        if self.mons.get(i).map(|m| m.info.kind) == Some(Kind::Internal) {
            return;
        }
        for code in codes {
            // many monitors answer these codes even when they are not listed in the capabilities
            let always = matches!(code, vcp::USAGE_HOURS | vcp::COLOR_PRESET | vcp::INPUT_SOURCE);
            if has_caps && !self.supports(i, code) && !always {
                continue;
            }
            let _ = self.get_retry(i, code, if has_caps { 2 } else { 1 });
        }
        if has_caps && self.supports(i, vcp::BRIGHTNESS) && !self.mons[i].values.contains_key(&vcp::BRIGHTNESS) {
            let _ = self.get_retry(i, vcp::BRIGHTNESS, 2);
        }
    }

    /// Re-reads only the controls already known (to follow changes made in the monitor's own menu).
    pub fn poll(&mut self, i: usize) {
        let codes: Vec<u8> = match self.mons.get(i) {
            Some(m) => m.values.keys().copied().filter(|c| *c != vcp::USAGE_HOURS).collect(),
            None => return,
        };
        for code in codes {
            let _ = self.get_retry(i, code, 1);
        }
    }

    /// Monitor information (read-only MCCS codes + EDID size).
    pub fn read_info(&mut self, i: usize) -> MonInfo {
        let mut info = MonInfo { inches: self.mons.get(i).and_then(|m| m.info.inches), ..Default::default() };
        if self.mons.get(i).map(|m| m.info.kind) == Some(Kind::Internal) {
            return info;
        }
        let mut rd = |code: u8| -> Option<u32> {
            self.get_retry(i, code, 1).ok().map(|v| v.cur).filter(|v| *v != u32::MAX)
        };
        info.hfreq = rd(vcp::H_FREQ);
        info.vfreq = rd(vcp::V_FREQ);
        info.tech = rd(vcp::TECH_TYPE);
        info.controller = rd(vcp::CONTROLLER);
        info.firmware = rd(vcp::FIRMWARE);
        info.vcp_version = rd(vcp::VCP_VERSION);
        info
    }

    pub fn view(&self, i: usize) -> Option<MonView> {
        let m = self.mons.get(i)?;
        let get = |c: u8| m.values.get(&c).copied();
        let pct = |c: u8| get(c).map(to_pct).unwrap_or(0);
        let mut v = MonView {
            name: m.info.name.clone(),
            detail: m.info.detail.clone(),
            internal: m.info.kind == Kind::Internal,
            has_brightness: get(vcp::BRIGHTNESS).is_some(),
            has_contrast: get(vcp::CONTRAST).is_some(),
            has_volume: get(vcp::VOLUME).is_some(),
            has_rgb: [vcp::GAIN_RED, vcp::GAIN_GREEN, vcp::GAIN_BLUE].iter().all(|c| get(*c).is_some()),
            brightness: pct(vcp::BRIGHTNESS),
            contrast: pct(vcp::CONTRAST),
            volume: pct(vcp::VOLUME),
            red: pct(vcp::GAIN_RED),
            green: pct(vcp::GAIN_GREEN),
            blue: pct(vcp::GAIN_BLUE),
            has_black: [vcp::BLACK_RED, vcp::BLACK_GREEN, vcp::BLACK_BLUE].iter().all(|c| get(*c).is_some()),
            black_r: pct(vcp::BLACK_RED),
            black_g: pct(vcp::BLACK_GREEN),
            black_b: pct(vcp::BLACK_BLUE),
            muted: get(vcp::MUTE).map(|x| x.cur == 1).unwrap_or(false),
            input_value: get(vcp::INPUT_SOURCE).map(|x| x.cur as i32).unwrap_or(-1),
            preset_value: get(vcp::COLOR_PRESET).map(|x| x.cur as i32).unwrap_or(-1),
            usage_hours: get(vcp::USAGE_HOURS).map(|x| x.cur),
            caps_raw: m.caps.as_ref().map(|c| c.raw.clone()).unwrap_or_default(),
            ..Default::default()
        };
        // inputs: everything the monitor reports + the default ones (many monitors do not list HDMI/DP
        // in the capabilities string but accept the command); the unreported ones appear "dimmed"
        let ext = !internal_only(m);
        let have_caps = m.caps.is_some();
        let detected: Vec<u16> = m.caps.as_ref().map(|c| c.values(vcp::INPUT_SOURCE).to_vec()).unwrap_or_default();
        let cur_in = get(vcp::INPUT_SOURCE).map(|x| x.cur as u16);
        let mut all: std::collections::BTreeSet<u16> = detected.iter().copied().collect();
        all.extend(cur_in);
        if ext {
            all.extend(vcp::STANDARD_INPUTS.iter().copied());
        }
        v.has_inputs = ext;
        v.inputs = all
            .into_iter()
            .map(|x| (x, vcp::input_name(x), have_caps && !detected.is_empty() && !detected.contains(&x) && Some(x) != cur_in))
            .collect();
        // color presets
        let detected_p: Vec<u16> = m.caps.as_ref().map(|c| c.values(vcp::COLOR_PRESET).to_vec()).unwrap_or_default();
        let cur_p = get(vcp::COLOR_PRESET).map(|x| x.cur as u16);
        let mut allp: std::collections::BTreeSet<u16> = detected_p.iter().copied().collect();
        allp.extend(cur_p);
        if ext {
            allp.extend(vcp::COMMON_PRESETS.iter().copied());
        }
        v.has_presets = ext;
        v.presets = allp
            .into_iter()
            .map(|x| (x, vcp::preset_name(x), have_caps && !detected_p.is_empty() && !detected_p.contains(&x) && Some(x) != cur_p))
            .collect();
        v.has_power = !internal_only(m);
        Some(v)
    }

    pub fn set_pct(&mut self, i: usize, code: u8, pct: u8) -> Result<(), String> {
        if !self.mons.get(i).map(|m| m.values.contains_key(&code)).unwrap_or(false) {
            self.get_retry(i, code, 2)?;
        }
        let max = self.mons[i].values.get(&code).map(|v| v.max).unwrap_or(100);
        self.set_retry(i, code, from_pct(pct, max))
    }

    /// Relative adjustment (shortcuts). Reads the current value first so it does not depend on a stale cache.
    pub fn adjust_pct(&mut self, i: usize, code: u8, delta: i32) -> Result<u8, String> {
        let cur = self.get_retry(i, code, 2).map(to_pct)?;
        let new = (cur as i32 + delta).clamp(0, 100) as u8;
        self.set_pct(i, code, new)?;
        Ok(new)
    }

    pub fn set_raw(&mut self, i: usize, code: u8, value: u32) -> Result<(), String> {
        self.set_retry(i, code, value)?;
        if let Some(m) = self.mons.get_mut(i) {
            m.values.entry(code).and_modify(|v| v.cur = value);
        }
        Ok(())
    }

    /// Re-reads the value after writing: `Some(false)` = the monitor ignored the command.
    pub fn verify_raw(&mut self, i: usize, code: u8, value: u32) -> Option<bool> {
        sleep(Duration::from_millis(180));
        self.get_retry(i, code, 1).ok().map(|v| v.cur == value)
    }

    pub fn toggle_mute(&mut self, i: usize) -> Result<bool, String> {
        let cur = self.get_retry(i, vcp::MUTE, 2)?.cur;
        let muted_now = cur != 1;
        self.set_raw(i, vcp::MUTE, if muted_now { 1 } else { 2 })?;
        Ok(muted_now)
    }

    /// which: 0 = everything, 1 = luminance/contrast, 2 = color
    pub fn reset(&mut self, i: usize, which: u8) -> Result<(), String> {
        let code = match which {
            0 => vcp::RESET_ALL,
            1 => vcp::RESET_LUMINANCE,
            _ => vcp::RESET_COLOR,
        };
        self.backend.set(i, code, 1)?;
        sleep(Duration::from_millis(400));
        self.mons[i].values.clear();
        self.probe_basic(i);
        self.read_extended(i);
        Ok(())
    }

    pub fn capture_profile(&mut self, name: &str, include_input: bool) -> Profile {
        let mut p = Profile { name: name.to_string(), ..Default::default() };
        for i in 0..self.mons.len() {
            self.probe_basic(i);
            self.ensure_caps(i);
            self.read_extended(i);
            let m = &self.mons[i];
            let pct = |c: u8| m.values.get(&c).copied().map(to_pct);
            let vals = ProfileValues {
                brightness: pct(vcp::BRIGHTNESS),
                contrast: pct(vcp::CONTRAST),
                volume: None,
                red: pct(vcp::GAIN_RED),
                green: pct(vcp::GAIN_GREEN),
                blue: pct(vcp::GAIN_BLUE),
                black_red: pct(vcp::BLACK_RED),
                black_green: pct(vcp::BLACK_GREEN),
                black_blue: pct(vcp::BLACK_BLUE),
                color_preset: m.values.get(&vcp::COLOR_PRESET).map(|v| v.cur as u16),
                input: if include_input { m.values.get(&vcp::INPUT_SOURCE).map(|v| v.cur as u16) } else { None },
            };
            p.monitors.insert(m.info.key.clone(), vals);
        }
        p
    }

    /// Applies a profile to the matching monitors. Returns how many were touched.
    pub fn apply_profile(&mut self, p: &Profile) -> usize {
        let mut touched = 0;
        for i in 0..self.mons.len() {
            let Some(vals) = p.monitors.get(&self.mons[i].info.key).or_else(|| p.monitors.get("*")).cloned() else { continue };
            touched += 1;
            // 1) color preset first: it may rewrite the monitor's RGB gains
            if let Some(x) = vals.color_preset {
                let _ = self.set_raw(i, vcp::COLOR_PRESET, x as u32);
                sleep(Duration::from_millis(150));
            }
            // 2) the remaining values. The RGB gains (whites) are always applied, even with a
            // fixed preset: many monitors only accept the stored gain and switch to the
            // "User" mode on their own. Values that are already equal are not resent.
            let pairs = [
                (vcp::BRIGHTNESS, vals.brightness),
                (vcp::CONTRAST, vals.contrast),
                (vcp::VOLUME, vals.volume),
                (vcp::GAIN_RED, vals.red),
                (vcp::GAIN_GREEN, vals.green),
                (vcp::GAIN_BLUE, vals.blue),
                (vcp::BLACK_RED, vals.black_red),
                (vcp::BLACK_GREEN, vals.black_green),
                (vcp::BLACK_BLUE, vals.black_blue),
            ];
            for (code, v) in pairs {
                let Some(pct) = v else { continue };
                let is_gain = matches!(code, vcp::GAIN_RED | vcp::GAIN_GREEN | vcp::GAIN_BLUE | vcp::BLACK_RED | vcp::BLACK_GREEN | vcp::BLACK_BLUE);
                if is_gain {
                    if let Ok(cur) = self.get_retry(i, code, 2) {
                        if to_pct(cur) == pct {
                            continue;
                        }
                    }
                }
                let _ = self.set_pct(i, code, pct);
            }
            if let Some(x) = vals.input {
                let _ = self.set_raw(i, vcp::INPUT_SOURCE, x as u32);
            }
        }
        touched
    }

    pub fn vcp_rows(&mut self, i: usize) -> Vec<VcpRow> {
        let codes: Vec<(u8, Vec<u16>)> = match self.mons.get(i).and_then(|m| m.caps.as_ref()) {
            Some(c) => c.vcp.iter().map(|(k, v)| (*k, v.clone())).collect(),
            None => self.mons.get(i).map(|m| m.values.keys().map(|k| (*k, vec![])).collect()).unwrap_or_default(),
        };
        let mut rows = Vec::new();
        for (code, values) in codes {
            let r = self.get_retry(i, code, 1).ok();
            rows.push(VcpRow { code, current: r.map(|v| v.cur), max: r.map(|v| v.max), values });
        }
        rows
    }
}

fn internal_only(m: &MonState) -> bool {
    m.info.kind == Kind::Internal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;

    fn core() -> Core {
        let mut c = Core::new(Box::new(MockBackend::new()));
        c.refresh().unwrap();
        c
    }

    #[test]
    fn pct_roundtrip() {
        assert_eq!(to_pct(Vcp { cur: 31, max: 62 }), 50);
        assert_eq!(from_pct(50, 62), 31);
        assert_eq!(to_pct(Vcp { cur: 0, max: 0 }), 0);
        assert_eq!(from_pct(200, 100), 100);
    }

    #[test]
    fn full_monitor_flow() {
        let mut c = core();
        assert_eq!(c.mons.len(), 3);
        c.probe_basic(0);
        c.ensure_caps(0);
        c.read_extended(0);
        let v = c.view(0).unwrap();
        assert!(v.has_brightness && v.has_contrast && v.has_volume && v.has_rgb && v.has_presets);
        assert_eq!(v.brightness, 62);
        assert_eq!(v.input_value, 0x0F);
        assert!(v.inputs.iter().any(|(x, n, _)| *x == 0x11 && n == "HDMI 1"));
        // 0x10 (DP 2) is not in the capabilities, but it is offered (dimmed)
        assert!(v.inputs.iter().any(|(x, _, dim)| *x == 0x10 && *dim));
        assert!(v.inputs.iter().any(|(x, _, dim)| *x == 0x11 && !dim));
        assert_eq!(v.usage_hours, Some(4210));
    }

    #[test]
    fn set_and_adjust() {
        let mut c = core();
        c.probe_basic(0);
        c.set_pct(0, vcp::BRIGHTNESS, 80).unwrap();
        assert_eq!(c.adjust_pct(0, vcp::BRIGHTNESS, 30).unwrap(), 100);
        assert_eq!(c.adjust_pct(0, vcp::BRIGHTNESS, -150).unwrap(), 0);
        assert!(c.set_pct(1, vcp::VOLUME, 10).is_err()); // monitor 1 has no volume
    }

    #[test]
    fn internal_panel_only_brightness() {
        let mut c = core();
        c.probe_basic(2);
        c.ensure_caps(2);
        c.read_extended(2);
        let v = c.view(2).unwrap();
        assert!(v.internal && v.has_brightness && !v.has_contrast && !v.has_inputs && !v.has_power);
    }

    #[test]
    fn profile_capture_and_apply() {
        let mut c = core();
        let p = c.capture_profile("Teste", false);
        assert_eq!(p.monitors.len(), 3);
        c.set_pct(0, vcp::BRIGHTNESS, 5).unwrap();
        assert_eq!(c.apply_profile(&p), 3);
        assert_eq!(to_pct(c.mons[0].values[&vcp::BRIGHTNESS]), 62);
        assert!(p.monitors.values().all(|v| v.input.is_none()));
    }

    #[test]
    fn wildcard_profile_applies_everywhere() {
        let mut c = core();
        c.probe_basic(0);
        c.probe_basic(1);
        let p = Profile {
            name: "Todos".into(),
            monitors: [("*".to_string(), ProfileValues { brightness: Some(20), ..Default::default() })].into(),
            ..Default::default()
        };
        assert_eq!(c.apply_profile(&p), 3);
        assert_eq!(to_pct(c.mons[1].values[&vcp::BRIGHTNESS]), 20);
    }

    #[test]
    fn verify_detects_ignored_write() {
        let mut c = core();
        c.probe_basic(0);
        c.read_extended(0);
        c.set_raw(0, vcp::COLOR_PRESET, 4).unwrap();
        assert_eq!(c.verify_raw(0, vcp::COLOR_PRESET, 4), Some(true));
        assert_eq!(c.verify_raw(0, vcp::COLOR_PRESET, 9), Some(false));
    }

    #[test]
    fn mute_toggle() {
        let mut c = core();
        c.probe_basic(0);
        c.ensure_caps(0);
        c.read_extended(0);
        assert!(c.toggle_mute(0).unwrap()); // was 2 (unmuted) -> mutes
        assert!(!c.toggle_mute(0).unwrap());
    }
}
