//! Simulated monitors for the demo and for tests.

use super::{Backend, Kind, MonitorInfo, Vcp};
use crate::vcp::*;
use std::collections::HashMap;
use std::time::Duration;

const CAPS_FULL: &str = "(prot(monitor)type(LCD)model(SoluaView 27 4K)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(01 04 05 06 08 0B) 16 18 1A 52 60(0F 11 12 1B) 62 8D AC AE B6 C0 D6(01 04 05) DC(00 03 05) DF)mccs_ver(2.2))";
const CAPS_BASIC: &str = "(prot(monitor)type(LCD)model(OfficeLine 24)vcp(10 12 60(01 03 11) D6(01 04 05))mccs_ver(2.1))";

pub struct MockBackend {
    values: HashMap<(usize, u8), Vcp>,
    delay: Duration,
}

impl MockBackend {
    pub fn new() -> Self {
        let mut values = HashMap::new();
        let full = [
            (BRIGHTNESS, 62, 100), (CONTRAST, 50, 100), (VOLUME, 30, 100), (MUTE, 2, 2),
            (GAIN_RED, 50, 100), (GAIN_GREEN, 50, 100), (GAIN_BLUE, 50, 100),
            (BLACK_RED, 50, 100), (BLACK_GREEN, 50, 100), (BLACK_BLUE, 50, 100),
            (H_FREQ, 135_000, 0xFFFF), (V_FREQ, 6000, 0xFFFF), (TECH_TYPE, 4, 10), (CONTROLLER, 0x0123, 0xFFFF), (FIRMWARE, 0x0102, 0xFFFF), (VCP_VERSION, 0x0202, 0xFFFF),
            (COLOR_PRESET, 0x05, 0x0B), (INPUT_SOURCE, 0x0F, 0x1B), (POWER_MODE, 1, 5), (USAGE_HOURS, 4210, 65535),
        ];
        for (c, cur, max) in full {
            values.insert((0, c), Vcp { cur, max });
        }
        for (c, cur, max) in [(BRIGHTNESS, 40, 100), (CONTRAST, 75, 100), (INPUT_SOURCE, 0x11, 0x11), (POWER_MODE, 1, 5)] {
            values.insert((1, c), Vcp { cur, max });
        }
        values.insert((2, BRIGHTNESS), Vcp { cur: 80, max: 100 });
        MockBackend { values, delay: Duration::from_millis(25) }
    }
}

impl Backend for MockBackend {
    fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String> {
        Ok(vec![
            MonitorInfo { key: "DEMO0001-A".into(), name: "SoluaView 27\" 4K".into(), detail: "\\\\.\\DISPLAY1 · 3840×2160 · DisplayPort".into(), kind: Kind::External, inches: Some(27.0) },
            MonitorInfo { key: "DEMO0002-B".into(), name: "OfficeLine 24\"".into(), detail: "\\\\.\\DISPLAY2 · 1920×1080 · HDMI".into(), kind: Kind::External, inches: Some(24.0) },
            MonitorInfo { key: "DEMO0003-C".into(), name: "Laptop panel".into(), detail: "\\\\.\\DISPLAY3 · 1920×1200 · internal".into(), kind: Kind::Internal, inches: None },
        ])
    }

    fn get(&mut self, mon: usize, code: u8) -> Result<Vcp, String> {
        std::thread::sleep(self.delay);
        self.values.get(&(mon, code)).copied().ok_or_else(|| format!("VCP 0x{code:02X} not supported"))
    }

    fn set(&mut self, mon: usize, code: u8, value: u32) -> Result<(), String> {
        std::thread::sleep(self.delay);
        match self.values.get_mut(&(mon, code)) {
            Some(v) => {
                v.cur = value.min(v.max.max(value));
                Ok(())
            }
            None => Err(format!("VCP 0x{code:02X} not supported")),
        }
    }

    fn capabilities(&mut self, mon: usize) -> Result<String, String> {
        std::thread::sleep(self.delay * 8);
        match mon {
            0 => Ok(CAPS_FULL.into()),
            1 => Ok(CAPS_BASIC.into()),
            _ => Err("no capabilities string".into()),
        }
    }
}
