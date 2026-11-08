// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal EDID parser: manufacturer, product code, serial and monitor name.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Edid {
    pub manufacturer: String,
    pub product: u16,
    pub serial: u32,
    pub name: Option<String>,
    pub serial_text: Option<String>,
    /// diagonal in inches (from the physical size reported in the EDID)
    pub inches: Option<f32>,
}

fn descriptor_text(d: &[u8]) -> String {
    let t = &d[5..18];
    let end = t.iter().position(|&b| b == 0x0A).unwrap_or(t.len());
    String::from_utf8_lossy(&t[..end]).trim().to_string()
}

pub fn parse(data: &[u8]) -> Option<Edid> {
    if data.len() < 128 || data[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }
    let w = ((data[8] as u16) << 8) | data[9] as u16;
    let c = |shift: u16| (((w >> shift) & 0x1F) as u8 + 64) as char;
    let manufacturer: String = [c(10), c(5), c(0)].iter().collect();
    let product = u16::from_le_bytes([data[10], data[11]]);
    let serial = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let mut edid = Edid { manufacturer, product, serial, ..Default::default() };
    let (wc, hc) = (data[21] as f32, data[22] as f32);
    if wc > 0.0 && hc > 0.0 {
        edid.inches = Some(((wc * wc + hc * hc).sqrt() / 2.54 * 10.0).round() / 10.0);
    }
    for off in [54usize, 72, 90, 108] {
        let d = &data[off..off + 18];
        if d[0] == 0 && d[1] == 0 && d[2] == 0 {
            match d[3] {
                0xFC => edid.name = Some(descriptor_text(d)).filter(|s| !s.is_empty()),
                0xFF => edid.serial_text = Some(descriptor_text(d)).filter(|s| !s.is_empty()),
                _ => {}
            }
        }
    }
    Some(edid)
}

impl Edid {
    /// Friendly name: "Manufacturer Model" when possible.
    pub fn display_name(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => format!("{} {:04X}", self.manufacturer, self.product),
        }
    }
    /// Stable key to identify the monitor across runs.
    pub fn key(&self) -> String {
        let ser = self.serial_text.clone().unwrap_or_else(|| format!("{:08X}", self.serial));
        format!("{}{:04X}-{}", self.manufacturer, self.product, ser)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // "DEL" = D(4) E(5) L(12) -> 00100 00101 01100
        let w: u16 = (4 << 10) | (5 << 5) | 12;
        d[8] = (w >> 8) as u8;
        d[9] = (w & 0xFF) as u8;
        d[10] = 0xB1;
        d[11] = 0xA0;
        d[12..16].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let name = b"DELL U2720Q\n  ";
        d[72..75].copy_from_slice(&[0, 0, 0]);
        d[75] = 0xFC;
        d[77..77 + 13].copy_from_slice(&name[..13]);
        d
    }

    #[test]
    fn parses_sample() {
        let e = parse(&sample()).unwrap();
        assert_eq!(e.manufacturer, "DEL");
        assert_eq!(e.product, 0xA0B1);
        assert_eq!(e.display_name(), "DELL U2720Q");
        assert_eq!(e.key(), "DELA0B1-12345678");
    }

    #[test]
    fn rejects_bad_header() {
        assert!(parse(&[0u8; 128]).is_none());
        assert!(parse(&[1, 2, 3]).is_none());
    }
}
