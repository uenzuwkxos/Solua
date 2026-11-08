//! Real backend: DDC/CI through dxva2 (High-Level + Low-Level Monitor Configuration API),
//! friendly name from the EDID in the registry, and internal panel through WMI.

use super::{Backend, Kind, MonitorInfo, Vcp};
use crate::edid;
use crate::winapi::*;

struct Item {
    handle: HANDLE,
    internal: bool,
}

pub struct WinBackend {
    groups: Vec<Vec<PHYSICAL_MONITOR>>,
    items: Vec<Item>,
    wmi: Option<bool>,
}

unsafe extern "system" fn collect(h: HMONITOR, _dc: HDC, _rc: *mut RECT, data: LPARAM) -> BOOL {
    let v = &mut *(data as *mut Vec<HMONITOR>);
    v.push(h);
    1
}

impl WinBackend {
    pub fn new() -> Self {
        WinBackend { groups: Vec::new(), items: Vec::new(), wmi: None }
    }

    fn destroy_all(&mut self) {
        for g in self.groups.iter_mut() {
            unsafe {
                DestroyPhysicalMonitors(g.len() as u32, g.as_mut_ptr());
            }
        }
        self.groups.clear();
        self.items.clear();
    }

    fn wmi_available(&mut self) -> bool {
        if let Some(v) = self.wmi {
            return v;
        }
        let ok = powershell("(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness -ErrorAction SilentlyContinue | Measure-Object).Count")
            .map(|s| s.trim().parse::<u32>().unwrap_or(0) > 0)
            .unwrap_or(false);
        self.wmi = Some(ok);
        ok
    }
}

impl Drop for WinBackend {
    fn drop(&mut self) {
        self.destroy_all();
    }
}

fn powershell(script: &str) -> Result<String, String> {
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Looks up the EDID in the registry from the DeviceID `MONITOR\XXXNNNN\{guid}\NNNN`.
fn edid_for_device_id(device_id: &str) -> Option<edid::Edid> {
    let parts: Vec<&str> = device_id.split('\\').collect();
    if parts.len() < 4 || !parts[0].eq_ignore_ascii_case("MONITOR") {
        return None;
    }
    let model = parts[1];
    let driver = format!("{}\\{}", parts[2], parts[3]);
    let base = format!("SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{model}");
    let root = RegKey::open(HKEY_LOCAL_MACHINE, &base, KEY_READ)?;
    let mut fallback = None;
    for sub in root.subkeys() {
        let Some(k) = RegKey::open(HKEY_LOCAL_MACHINE, &format!("{base}\\{sub}"), KEY_READ) else { continue };
        let drv = k.get_string("Driver").unwrap_or_default();
        let Some(dp) = RegKey::open(HKEY_LOCAL_MACHINE, &format!("{base}\\{sub}\\Device Parameters"), KEY_READ) else { continue };
        let Some(bytes) = dp.get_binary("EDID") else { continue };
        let Some(e) = edid::parse(&bytes) else { continue };
        if drv.eq_ignore_ascii_case(&driver) {
            return Some(e);
        }
        fallback.get_or_insert(e);
    }
    fallback
}

fn display_device(adapter: &[u16], index: u32) -> Option<DISPLAY_DEVICEW> {
    let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
    dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
    let ok = unsafe { EnumDisplayDevicesW(adapter.as_ptr(), index, &mut dd, 0) };
    (ok != 0).then_some(dd)
}

impl Backend for WinBackend {
    fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String> {
        self.destroy_all();
        let mut hmons: Vec<HMONITOR> = Vec::new();
        unsafe {
            EnumDisplayMonitors(0, std::ptr::null(), collect, &mut hmons as *mut _ as LPARAM);
        }
        let mut infos = Vec::new();
        let mut internal_taken = false;
        for hm in hmons {
            let mut mi: MONITORINFOEXW = unsafe { std::mem::zeroed() };
            mi.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if unsafe { GetMonitorInfoW(hm, &mut mi) } == 0 {
                continue;
            }
            let device = from_wide(&mi.szDevice);
            let (w, h) = (mi.rcMonitor.right - mi.rcMonitor.left, mi.rcMonitor.bottom - mi.rcMonitor.top);
            let primary = mi.dwFlags & 1 != 0;

            let mut count = 0u32;
            let have = unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hm, &mut count) } != 0 && count > 0;
            let mut group: Vec<PHYSICAL_MONITOR> = Vec::new();
            if have {
                group = vec![unsafe { std::mem::zeroed() }; count as usize];
                if unsafe { GetPhysicalMonitorsFromHMONITOR(hm, count, group.as_mut_ptr()) } == 0 {
                    group.clear();
                }
            }

            if group.is_empty() {
                // no physical DDC monitor: possibly a laptop's internal panel
                if !internal_taken && self.wmi_available() {
                    internal_taken = true;
                    self.items.push(Item { handle: 0, internal: true });
                    infos.push(MonitorInfo {
                        key: format!("INTERNAL-{device}"),
                        name: "Built-in display".into(),
                        detail: format!("{device} · {w}×{h}"),
                        kind: Kind::Internal,
                        inches: None,
                    });
                }
                continue;
            }

            for (j, pm) in group.iter().enumerate() {
                let dd = display_device(&mi.szDevice, j as u32);
                let edid = dd.as_ref().and_then(|d| edid_for_device_id(&from_wide(&d.DeviceID)));
                let handle = pm.hPhysicalMonitor;
                // probe: if the monitor does not answer DDC and WMI is available, treat it as the internal panel
                let (mut ty, mut cur, mut max) = (0u32, 0u32, 0u32);
                let responds = unsafe { GetVCPFeatureAndVCPFeatureReply(handle, 0x10, &mut ty, &mut cur, &mut max) } != 0;
                if !responds && !internal_taken && self.wmi_available() {
                    internal_taken = true;
                    self.items.push(Item { handle: 0, internal: true });
                    infos.push(MonitorInfo {
                        key: format!("INTERNAL-{device}"),
                        name: "Built-in display".into(),
                        detail: format!("{device} · {w}×{h}"),
                        kind: Kind::Internal,
                        inches: None,
                    });
                    continue;
                }
                let desc = from_wide(&pm.szPhysicalMonitorDescription);
                let name = edid
                    .as_ref()
                    .map(|e| e.display_name())
                    .or_else(|| dd.as_ref().map(|d| from_wide(&d.DeviceString)).filter(|s| !s.is_empty()))
                    .unwrap_or(desc);
                let key = edid.as_ref().map(|e| e.key()).unwrap_or_else(|| format!("{device}-{j}"));
                self.items.push(Item { handle, internal: false });
                infos.push(MonitorInfo {
                    key,
                    name,
                    detail: format!("{device} · {w}×{h}{}", if primary { " · primary" } else { "" }),
                    kind: Kind::External,
                    inches: edid.as_ref().and_then(|e| e.inches),
                });
            }
            self.groups.push(group);
        }
        Ok(infos)
    }

    fn get(&mut self, mon: usize, code: u8) -> Result<Vcp, String> {
        let it = self.items.get(mon).ok_or("monitor index out of range")?;
        if it.internal {
            if code != 0x10 {
                return Err("unsupported on built-in display".into());
            }
            let out = powershell("(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness | Select-Object -First 1).CurrentBrightness")?;
            let cur: u32 = out.trim().parse().map_err(|_| "unexpected WMI output".to_string())?;
            return Ok(Vcp { cur, max: 100 });
        }
        let (mut ty, mut cur, mut max) = (0u32, 0u32, 0u32);
        let ok = unsafe { GetVCPFeatureAndVCPFeatureReply(it.handle, code, &mut ty, &mut cur, &mut max) };
        if ok == 0 {
            return Err(format!("DDC/CI read of 0x{code:02X} failed (error {})", unsafe { GetLastError() }));
        }
        Ok(Vcp { cur, max })
    }

    fn set(&mut self, mon: usize, code: u8, value: u32) -> Result<(), String> {
        let it = self.items.get(mon).ok_or("monitor index out of range")?;
        if it.internal {
            if code != 0x10 {
                return Err("unsupported on built-in display".into());
            }
            powershell(&format!(
                "Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods | Invoke-CimMethod -MethodName WmiSetBrightness -Arguments @{{Timeout=0;Brightness={}}} | Out-Null",
                value.min(100)
            ))?;
            return Ok(());
        }
        let ok = unsafe { SetVCPFeature(it.handle, code, value) };
        if ok == 0 {
            return Err(format!("DDC/CI write of 0x{code:02X} failed (error {})", unsafe { GetLastError() }));
        }
        Ok(())
    }

    fn capabilities(&mut self, mon: usize) -> Result<String, String> {
        let it = self.items.get(mon).ok_or("monitor index out of range")?;
        if it.internal {
            return Err("no capabilities".into());
        }
        let mut len = 0u32;
        if unsafe { GetCapabilitiesStringLength(it.handle, &mut len) } == 0 || len == 0 {
            return Err("capabilities string unavailable".into());
        }
        let mut buf = vec![0u8; len as usize + 1];
        if unsafe { CapabilitiesRequestAndCapabilitiesReply(it.handle, buf.as_mut_ptr(), len) } == 0 {
            return Err("capabilities request failed".into());
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).to_string())
    }

    fn save_settings(&mut self, mon: usize) -> Result<(), String> {
        let it = self.items.get(mon).ok_or("monitor index out of range")?;
        if it.internal {
            return Ok(());
        }
        unsafe {
            SaveCurrentMonitorSettings(it.handle);
        }
        Ok(())
    }
}
