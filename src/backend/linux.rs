// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux backend: DDC/CI straight over the I²C bus (`/dev/i2c-N`, address 0x37), as ddcutil does,
//! and internal panels through `/sys/class/backlight`.
//!
//! System requirements: the `i2c-dev` module loaded and permission to access `/dev/i2c-*`
//! (the Solua packages and the in-app "Enable monitor access" button set both up).

use super::{Backend, Kind, MonitorInfo, Vcp};
use crate::edid;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

extern "C" {
    fn ioctl(fd: std::os::raw::c_int, request: std::os::raw::c_ulong, ...) -> std::os::raw::c_int;
}
const I2C_SLAVE_FORCE: std::os::raw::c_ulong = 0x0706;
const DDC_ADDR: std::os::raw::c_ulong = 0x37;

// ------------------------------------------------------------------ protocol (pure functions)

/// Builds a DDC/CI write packet: 0x51, 0x80|length, data..., checksum (seed 0x6E).
fn packet(data: &[u8]) -> Vec<u8> {
    let mut p = vec![0x51, 0x80 | data.len() as u8];
    p.extend_from_slice(data);
    let chk = p.iter().fold(0x6Eu8, |a, b| a ^ b);
    p.push(chk);
    p
}

fn build_get(code: u8) -> Vec<u8> {
    packet(&[0x01, code])
}

fn build_set(code: u8, value: u32) -> Vec<u8> {
    packet(&[0x03, code, (value >> 8) as u8, value as u8])
}

fn build_caps(offset: u16) -> Vec<u8> {
    packet(&[0xF3, (offset >> 8) as u8, offset as u8])
}

/// Validates the framing of a reply (source 0x6E, length, checksum with seed 0x50)
/// and returns the payload.
fn payload(buf: &[u8]) -> Result<&[u8], String> {
    if buf.len() < 3 || buf[0] != 0x6E {
        return Err("invalid DDC/CI reply".into());
    }
    let len = (buf[1] & 0x7F) as usize;
    if len == 0 {
        return Err("empty DDC/CI reply".into());
    }
    if buf.len() < len + 3 {
        return Err("truncated DDC/CI reply".into());
    }
    let chk = buf[..len + 2].iter().fold(0x50u8, |a, b| a ^ b);
    if chk != buf[len + 2] {
        return Err("DDC/CI checksum mismatch".into());
    }
    Ok(&buf[2..len + 2])
}

fn parse_get(buf: &[u8], code: u8) -> Result<Vcp, String> {
    let p = payload(buf)?;
    if p.len() < 8 || p[0] != 0x02 {
        return Err("unexpected DDC/CI reply".into());
    }
    match p[1] {
        0 => {}
        1 => return Err(format!("VCP 0x{code:02X} not supported")),
        r => return Err(format!("DDC/CI error {r}")),
    }
    if p[2] != code {
        return Err("reply for a different VCP code".into());
    }
    Ok(Vcp { max: u16::from_be_bytes([p[4], p[5]]) as u32, cur: u16::from_be_bytes([p[6], p[7]]) as u32 })
}

/// Fragment of the capabilities string: (offset, bytes).
fn parse_caps(buf: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let p = payload(buf)?;
    if p.len() < 3 || p[0] != 0xE3 {
        return Err("unexpected capabilities reply".into());
    }
    Ok((u16::from_be_bytes([p[1], p[2]]), p[3..].to_vec()))
}

/// sysfs root (tests only, with a fake tree in `SOLUA_SYSROOT`).
fn sys(path: &str) -> PathBuf {
    match std::env::var_os("SOLUA_SYSROOT") {
        Some(r) => PathBuf::from(r).join(path.trim_start_matches('/')),
        None => PathBuf::from(path),
    }
}

// ------------------------------------------------------------------ monitors

struct Item {
    /// I²C bus of the connector (external monitor)
    i2c: Option<PathBuf>,
    file: Option<File>,
    /// internal panel: folder under /sys/class/backlight
    backlight: Option<PathBuf>,
}

pub struct LinuxBackend {
    items: Vec<Item>,
}

impl LinuxBackend {
    pub fn new() -> Self {
        LinuxBackend { items: Vec::new() }
    }

    fn open(it: &mut Item) -> Result<&mut File, String> {
        if it.file.is_none() {
            let path = it.i2c.clone().ok_or("no DDC/CI bus found for this connector (is the i2c-dev module loaded?)")?;
            let f = OpenOptions::new().read(true).write(true).open(&path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    format!("no permission for {} (use \"Enable monitor access\" in Solua)", path.display())
                } else {
                    format!("cannot open {}: {e} (is the i2c-dev module loaded?)", path.display())
                }
            })?;
            let r = unsafe { ioctl(f.as_raw_fd(), I2C_SLAVE_FORCE, DDC_ADDR) };
            if r < 0 {
                return Err("cannot select the DDC/CI address".into());
            }
            it.file = Some(f);
        }
        Ok(it.file.as_mut().unwrap())
    }

    fn transact(f: &mut File, req: &[u8], reply_len: usize) -> Result<Vec<u8>, String> {
        f.write_all(req).map_err(|e| format!("I2C write failed: {e}"))?;
        sleep(Duration::from_millis(50));
        let mut buf = vec![0u8; reply_len];
        let n = f.read(&mut buf).map_err(|e| format!("I2C read failed: {e}"))?;
        buf.truncate(n);
        Ok(buf)
    }
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Looks for the I²C bus of a DRM connector (`ddc` or an `i2c-N` folder).
fn i2c_for_connector(dir: &Path) -> Option<PathBuf> {
    let name_of = |p: &Path| -> Option<PathBuf> {
        let real = std::fs::canonicalize(p).ok()?;
        let n = real.file_name()?.to_string_lossy().to_string();
        n.strip_prefix("i2c-")?.parse::<u32>().ok()?;
        Some(PathBuf::from("/dev").join(n))
    };
    if let Some(p) = name_of(&dir.join("ddc")) {
        return Some(p);
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with("i2c-") {
            if let Some(p) = name_of(&e.path()) {
                return Some(p);
            }
        }
    }
    None
}

/// Numbers of the existing `/dev/i2c-N` buses, in order, ignoring SMBus adapters
/// (RAM SPD memories live there; they are never monitors).
fn i2c_buses() -> Vec<PathBuf> {
    let mut nums: Vec<u32> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sys("/dev")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(num) = n.strip_prefix("i2c-").and_then(|x| x.parse::<u32>().ok()) {
                nums.push(num);
            }
        }
    }
    nums.sort();
    nums.into_iter()
        .filter(|n| {
            let name = read_trim(&sys(&format!("/sys/class/i2c-dev/i2c-{n}/name"))).unwrap_or_default().to_ascii_lowercase();
            !(name.contains("smbus") || name.contains("i801") || name.contains("piix4") || name.contains("nvidia i2c"))
        })
        .map(|n| sys("/dev").join(format!("i2c-{n}")))
        .collect()
}

/// Reads the first 128 bytes of the EDID of a bus (address 0x50), as ddcutil does.
fn read_edid_from_bus(path: &Path) -> Option<Vec<u8>> {
    let mut f = OpenOptions::new().read(true).write(true).open(path).ok()?;
    if unsafe { ioctl(f.as_raw_fd(), I2C_SLAVE_FORCE, 0x50 as std::os::raw::c_ulong) } < 0 {
        return None;
    }
    f.write_all(&[0]).ok()?;
    sleep(Duration::from_millis(10));
    let mut buf = vec![0u8; 128];
    let mut got = 0;
    while got < 128 {
        match f.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    (got == 128 && buf[..8] == [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]).then_some(buf)
}

/// Buses that answer with a valid EDID (read once per scan).
fn scan_bus_edids() -> Vec<(PathBuf, Vec<u8>)> {
    i2c_buses().into_iter().filter_map(|b| read_edid_from_bus(&b).map(|e| (b, e))).collect()
}

fn pick_backlight() -> Option<PathBuf> {
    let mut found: Vec<(u8, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(sys("/sys/class/backlight")).ok()?.flatten() {
        let p = e.path();
        let max = read_trim(&p.join("max_brightness")).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        if max == 0 {
            continue;
        }
        // "raw" (GPU driver) is preferred over "platform" and "firmware"
        let rank = match read_trim(&p.join("type")).as_deref() {
            Some("raw") => 0,
            Some("platform") => 1,
            _ => 2,
        };
        found.push((rank, p));
    }
    found.sort();
    found.into_iter().next().map(|(_, p)| p)
}

impl Backend for LinuxBackend {
    fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String> {
        self.items.clear();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(sys("/sys/class/drm"))
            .map_err(|e| format!("cannot read /sys/class/drm: {e}"))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                n.starts_with("card") && n.contains('-')
            })
            .collect();
        dirs.sort();
        let mut infos = Vec::new();
        let mut internal_taken = false;
        // Drivers without the `ddc` link in sysfs (nouveau, proprietary NVIDIA...): the bus is found
        // by comparing the connector's EDID with the one of each /dev/i2c-N, as ddcutil does.
        let mut bus_edids: Option<Vec<(PathBuf, Vec<u8>)>> = None;
        let mut claimed: Vec<PathBuf> = Vec::new();
        // external connectors with no bus and no readable EDID: (position in items/infos)
        let mut orphans: Vec<usize> = Vec::new();
        for dir in dirs {
            if read_trim(&dir.join("status")).as_deref() != Some("connected") {
                continue;
            }
            let conn = dir.file_name().unwrap().to_string_lossy().to_string();
            // "card0-eDP-1" -> "eDP-1"
            let short = conn.split_once('-').map(|(_, r)| r.to_string()).unwrap_or(conn.clone());
            let internal = ["eDP", "LVDS", "DSI"].iter().any(|k| short.starts_with(k));
            let mode = read_trim(&dir.join("modes")).and_then(|s| s.lines().next().map(|l| l.to_string()));
            let edid_raw = std::fs::read(dir.join("edid")).ok().unwrap_or_default();
            let edid = edid::parse(&edid_raw);
            let detail = match &mode {
                Some(m) => format!("{short} · {}", m.replace('x', "×")),
                None => short.clone(),
            };
            if internal && !internal_taken {
                if let Some(bl) = pick_backlight() {
                    internal_taken = true;
                    self.items.push(Item { i2c: None, file: None, backlight: Some(bl) });
                    infos.push(MonitorInfo {
                        key: edid.as_ref().map(|e| e.key()).unwrap_or_else(|| format!("INTERNAL-{short}")),
                        name: edid.as_ref().and_then(|e| e.name.clone()).unwrap_or_else(|| "Built-in display".into()),
                        detail,
                        kind: Kind::Internal,
                        inches: edid.as_ref().and_then(|e| e.inches),
                    });
                    continue;
                }
            }
            let mut i2c = i2c_for_connector(&dir);
            if i2c.is_none() && edid_raw.len() >= 128 {
                let list = bus_edids.get_or_insert_with(scan_bus_edids);
                i2c = list.iter().find(|(b, e)| !claimed.contains(b) && e[..128] == edid_raw[..128]).map(|(b, _)| b.clone());
            }
            // empty connector EDID (nouveau): use the one read straight from the bus, to get the name and size
            let edid = edid.or_else(|| i2c.as_ref().and_then(|b| read_edid_from_bus(b)).and_then(|raw| edid::parse(&raw)));
            if let Some(b) = &i2c {
                claimed.push(b.clone());
            } else if edid_raw.len() < 128 && !internal {
                orphans.push(self.items.len());
            }
            self.items.push(Item { i2c, file: None, backlight: None });
            infos.push(MonitorInfo {
                key: edid.as_ref().map(|e| e.key()).unwrap_or_else(|| format!("DRM-{short}")),
                name: edid.as_ref().map(|e| e.display_name()).unwrap_or_else(|| short.clone()),
                detail,
                kind: if internal { Kind::Internal } else { Kind::External },
                inches: edid.as_ref().and_then(|e| e.inches),
            });
        }
        // unreadable connector EDID (common on nouveau): if exactly as many buses with a monitor are left
        // as connectors without a bus, pair them in order.
        if !orphans.is_empty() {
            let list = bus_edids.get_or_insert_with(scan_bus_edids);
            let free: Vec<&(PathBuf, Vec<u8>)> = list.iter().filter(|(b, _)| !claimed.contains(b)).collect();
            if free.len() == orphans.len() {
                for (idx, (bus, raw)) in orphans.iter().zip(free) {
                    self.items[*idx].i2c = Some(bus.clone());
                    if let Some(e) = edid::parse(raw) {
                        infos[*idx].name = e.display_name();
                        infos[*idx].key = e.key();
                        infos[*idx].inches = e.inches;
                    }
                }
            }
        }
        Ok(infos)
    }

    fn get(&mut self, mon: usize, code: u8) -> Result<Vcp, String> {
        let it = self.items.get_mut(mon).ok_or("monitor index out of range")?;
        if let Some(bl) = &it.backlight {
            if code != crate::vcp::BRIGHTNESS {
                return Err(format!("VCP 0x{code:02X} not supported"));
            }
            let cur = read_trim(&bl.join("brightness")).and_then(|s| s.parse().ok()).ok_or("cannot read the backlight")?;
            let max = read_trim(&bl.join("max_brightness")).and_then(|s| s.parse().ok()).ok_or("cannot read the backlight")?;
            return Ok(Vcp { cur, max });
        }
        let f = Self::open(it)?;
        let reply = Self::transact(f, &build_get(code), 11)?;
        parse_get(&reply, code)
    }

    fn set(&mut self, mon: usize, code: u8, value: u32) -> Result<(), String> {
        let it = self.items.get_mut(mon).ok_or("monitor index out of range")?;
        if let Some(bl) = &it.backlight {
            if code != crate::vcp::BRIGHTNESS {
                return Err(format!("VCP 0x{code:02X} not supported"));
            }
            if std::fs::write(bl.join("brightness"), value.to_string()).is_ok() {
                return Ok(());
            }
            // no write permission on sysfs: try brightnessctl, if present
            let dev = bl.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ok = std::process::Command::new("brightnessctl")
                .args(["-d", &dev, "set", &value.to_string()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            return if ok {
                Ok(())
            } else {
                Err("no permission to change the backlight (install brightnessctl or a udev rule)".into())
            };
        }
        let f = Self::open(it)?;
        f.write_all(&build_set(code, value)).map_err(|e| format!("I2C write failed: {e}"))?;
        sleep(Duration::from_millis(50));
        Ok(())
    }

    fn capabilities(&mut self, mon: usize) -> Result<String, String> {
        let it = self.items.get_mut(mon).ok_or("monitor index out of range")?;
        if it.backlight.is_some() {
            return Err("no capabilities".into());
        }
        let f = Self::open(it)?;
        let mut out: Vec<u8> = Vec::new();
        let mut offset = 0u16;
        for _ in 0..64 {
            let reply = Self::transact(f, &build_caps(offset), 38)?;
            let (off, data) = parse_caps(&reply)?;
            if off != offset {
                return Err("capabilities fragment out of order".into());
            }
            if data.is_empty() {
                break;
            }
            offset += data.len() as u16;
            out.extend_from_slice(&data);
        }
        if out.is_empty() {
            return Err("capabilities string unavailable".into());
        }
        Ok(String::from_utf8_lossy(&out).trim_end_matches('\0').to_string())
    }

    fn save_settings(&mut self, mon: usize) -> Result<(), String> {
        // VCP 0x0C is "save current settings" on some monitors; only try it on external ones
        let Some(it) = self.items.get_mut(mon) else { return Ok(()) };
        if it.backlight.is_some() {
            return Ok(());
        }
        if let Ok(f) = Self::open(it) {
            let _ = f.write_all(&packet(&[0x0C]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Example of the pattern: read request for brightness (0x10) = 51 82 01 10 AC.
    #[test]
    fn get_request_matches_the_spec_example() {
        assert_eq!(build_get(0x10), vec![0x51, 0x82, 0x01, 0x10, 0xAC]);
    }

    #[test]
    fn set_request_has_value_and_checksum() {
        let p = build_set(0x10, 0x0032);
        assert_eq!(&p[..6], &[0x51, 0x84, 0x03, 0x10, 0x00, 0x32]);
        assert_eq!(p[6], p[..6].iter().fold(0x6E, |a, b| a ^ b));
    }

    fn reply(payload: &[u8]) -> Vec<u8> {
        let mut r = vec![0x6E, 0x80 | payload.len() as u8];
        r.extend_from_slice(payload);
        let c = r.iter().fold(0x50u8, |a, b| a ^ b);
        r.push(c);
        r
    }

    #[test]
    fn parses_a_get_reply() {
        let r = reply(&[0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x2A]);
        assert_eq!(parse_get(&r, 0x10).unwrap(), Vcp { cur: 42, max: 100 });
    }

    #[test]
    fn unsupported_and_bad_checksum_are_errors() {
        let r = reply(&[0x02, 0x01, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert!(parse_get(&r, 0x10).unwrap_err().contains("not supported"));
        let mut r = reply(&[0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x2A]);
        *r.last_mut().unwrap() ^= 0xFF;
        assert!(parse_get(&r, 0x10).is_err());
    }

    #[test]
    fn parses_capabilities_fragments() {
        let mut p = vec![0xE3, 0x00, 0x20];
        p.extend_from_slice(b"(prot(monitor)");
        let (off, data) = parse_caps(&reply(&p)).unwrap();
        assert_eq!(off, 0x20);
        assert_eq!(data, b"(prot(monitor)");
        assert_eq!(build_caps(0x0140)[..5], [0x51, 0x83, 0xF3, 0x01, 0x40]);
    }

    /// Fake sysfs tree: an external monitor with no I²C bus and an internal panel with backlight.
    #[test]
    fn enumerates_a_fake_sysfs_and_drives_the_backlight() {
        let root = std::env::temp_dir().join(format!("solua-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let drm = root.join("sys/class/drm");
        for c in ["card0-DP-1", "card0-eDP-1"] {
            std::fs::create_dir_all(drm.join(c)).unwrap();
            std::fs::write(drm.join(c).join("status"), "connected\n").unwrap();
            std::fs::write(drm.join(c).join("modes"), "1920x1080\n").unwrap();
        }
        std::fs::create_dir_all(drm.join("card0-HDMI-A-1")).unwrap();
        std::fs::write(drm.join("card0-HDMI-A-1/status"), "disconnected\n").unwrap();
        let bl = root.join("sys/class/backlight/intel_backlight");
        std::fs::create_dir_all(&bl).unwrap();
        std::fs::write(bl.join("brightness"), "300\n").unwrap();
        std::fs::write(bl.join("max_brightness"), "1000\n").unwrap();
        std::fs::write(bl.join("type"), "raw\n").unwrap();
        std::env::set_var("SOLUA_SYSROOT", &root);

        let mut b = LinuxBackend::new();
        let infos = b.enumerate().unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].kind, Kind::External);
        assert_eq!(infos[1].kind, Kind::Internal);
        assert_eq!(b.get(1, crate::vcp::BRIGHTNESS).unwrap(), Vcp { cur: 300, max: 1000 });
        assert!(b.get(1, crate::vcp::CONTRAST).is_err());
        b.set(1, crate::vcp::BRIGHTNESS, 640).unwrap();
        assert_eq!(b.get(1, crate::vcp::BRIGHTNESS).unwrap().cur, 640);
        // the external one has no I²C bus in this tree: a clear error instead of a panic
        assert!(b.get(0, crate::vcp::BRIGHTNESS).is_err());
        std::env::remove_var("SOLUA_SYSROOT");
        let _ = std::fs::remove_dir_all(&root);
    }
}
