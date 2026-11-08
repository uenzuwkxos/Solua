// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal hand-written Win32 declarations (no external dependencies).
//! The module compiles on any platform so the compiler validates the types;
//! the functions are only linked on Windows.

#![allow(non_snake_case, non_camel_case_types, dead_code, clippy::upper_case_acronyms)]

pub type HANDLE = isize;
pub type HWND = isize;
pub type HMONITOR = isize;
pub type HDC = isize;
pub type HKEY = isize;
pub type HICON = isize;
pub type HMENU = isize;
pub type HINSTANCE = isize;
pub type LPARAM = isize;
pub type WPARAM = usize;
pub type LRESULT = isize;
pub type BOOL = i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
pub struct MONITORINFOEXW {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
    pub szDevice: [u16; 32],
}

#[repr(C)]
pub struct DISPLAY_DEVICEW {
    pub cb: u32,
    pub DeviceName: [u16; 32],
    pub DeviceString: [u16; 128],
    pub StateFlags: u32,
    pub DeviceID: [u16; 128],
    pub DeviceKey: [u16; 128],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PHYSICAL_MONITOR {
    pub hPhysicalMonitor: HANDLE,
    pub szPhysicalMonitorDescription: [u16; 128],
}

#[repr(C)]
pub struct GUID {
    pub Data1: u32,
    pub Data2: u16,
    pub Data3: u16,
    pub Data4: [u8; 8],
}

pub type WNDPROC = Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>;

#[repr(C)]
pub struct WNDCLASSW {
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: HICON,
    pub hCursor: isize,
    pub hbrBackground: isize,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
}

#[repr(C)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
}

#[repr(C)]
pub struct NOTIFYICONDATAW {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uID: u32,
    pub uFlags: u32,
    pub uCallbackMessage: u32,
    pub hIcon: HICON,
    pub szTip: [u16; 128],
    pub dwState: u32,
    pub dwStateMask: u32,
    pub szInfo: [u16; 256],
    pub uVersion: u32,
    pub szInfoTitle: [u16; 64],
    pub dwInfoFlags: u32,
    pub guidItem: GUID,
    pub hBalloonIcon: HICON,
}

pub type MONITORENUMPROC = unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, LPARAM) -> BOOL;

// ---- constants ----
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_NULL: u32 = 0x0000;
pub const WM_DISPLAYCHANGE: u32 = 0x007E;
pub const WM_HOTKEY: u32 = 0x0312;
pub const WM_POWERBROADCAST: u32 = 0x0218;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_CONTEXTMENU: u32 = 0x007B;
pub const WM_APP: u32 = 0x8000;
pub const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
pub const PBT_APMRESUMESUSPEND: usize = 0x07;
pub const PBT_APMSUSPEND: usize = 0x04;
pub const PBT_POWERSETTINGCHANGE: usize = 0x8013;
/// GUID_CONSOLE_DISPLAY_STATE: 0 = display off, 1 = on, 2 = dimmed
pub const GUID_CONSOLE_DISPLAY_STATE: GUID = GUID { Data1: 0x6FE69556, Data2: 0x704A, Data3: 0x47A0, Data4: [0x8F, 0x24, 0xC2, 0x8D, 0x93, 0x6F, 0xDA, 0x47] };

#[repr(C)]
pub struct POWERBROADCAST_SETTING {
    pub power_setting: GUID,
    pub data_length: u32,
    pub data: [u8; 1],
}
pub const MF_STRING: u32 = 0x0;
pub const MF_POPUP: u32 = 0x10;
pub const MF_SEPARATOR: u32 = 0x800;
pub const TPM_RIGHTBUTTON: u32 = 0x2;
pub const TPM_RETURNCMD: u32 = 0x100;
pub const NIM_ADD: u32 = 0;
pub const NIM_MODIFY: u32 = 1;
pub const NIM_DELETE: u32 = 2;
pub const NIF_MESSAGE: u32 = 1;
pub const NIF_ICON: u32 = 2;
pub const NIF_TIP: u32 = 4;
pub const MOD_NOREPEAT: u32 = 0x4000;
pub const MONITOR_DEFAULTTONEAREST: u32 = 2;
pub const IDI_APPLICATION: usize = 32512;
pub const SM_CXSCREEN: i32 = 0;
pub const SM_CXICON: i32 = 11;
pub const SM_CYICON: i32 = 12;
pub const SM_CXSMICON: i32 = 49;
pub const SM_CYSMICON: i32 = 50;
pub const IMAGE_ICON: u32 = 1;
pub const WM_SETICON: u32 = 0x0080;
pub const SM_CYSCREEN: i32 = 1;
pub const GWL_EXSTYLE: i32 = -20;
pub const WS_EX_TOOLWINDOW: isize = 0x80;
pub const WS_EX_NOACTIVATE: isize = 0x0800_0000;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;

pub const HKEY_CURRENT_USER: HKEY = 0x8000_0001u32 as i32 as isize;
pub const HKEY_LOCAL_MACHINE: HKEY = 0x8000_0002u32 as i32 as isize;
pub const KEY_READ: u32 = 0x20019;
pub const KEY_SET_VALUE: u32 = 0x0002;
pub const REG_SZ: u32 = 1;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;
pub const ERROR_SUCCESS: i32 = 0;

#[cfg_attr(windows, link(name = "dxva2"))]
extern "system" {
    pub fn GetNumberOfPhysicalMonitorsFromHMONITOR(h: HMONITOR, count: *mut u32) -> BOOL;
    pub fn GetPhysicalMonitorsFromHMONITOR(h: HMONITOR, n: u32, arr: *mut PHYSICAL_MONITOR) -> BOOL;
    pub fn DestroyPhysicalMonitors(n: u32, arr: *mut PHYSICAL_MONITOR) -> BOOL;
    pub fn GetVCPFeatureAndVCPFeatureReply(h: HANDLE, code: u8, ty: *mut u32, cur: *mut u32, max: *mut u32) -> BOOL;
    pub fn SetVCPFeature(h: HANDLE, code: u8, value: u32) -> BOOL;
    pub fn GetCapabilitiesStringLength(h: HANDLE, len: *mut u32) -> BOOL;
    pub fn CapabilitiesRequestAndCapabilitiesReply(h: HANDLE, buf: *mut u8, len: u32) -> BOOL;
    pub fn SaveCurrentMonitorSettings(h: HANDLE) -> BOOL;
}

#[cfg_attr(windows, link(name = "user32"))]
extern "system" {
    pub fn EnumDisplayMonitors(hdc: HDC, clip: *const RECT, cb: MONITORENUMPROC, data: LPARAM) -> BOOL;
    pub fn GetMonitorInfoW(h: HMONITOR, info: *mut MONITORINFOEXW) -> BOOL;
    pub fn EnumDisplayDevicesW(device: *const u16, index: u32, dd: *mut DISPLAY_DEVICEW, flags: u32) -> BOOL;
    pub fn RegisterClassW(wc: *const WNDCLASSW) -> u16;
    pub fn CreateWindowExW(
        ex: u32, class: *const u16, title: *const u16, style: u32,
        x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: HMENU, inst: HINSTANCE, param: *const core::ffi::c_void,
    ) -> HWND;
    pub fn DefWindowProcW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn RegisterPowerSettingNotification(h: HANDLE, guid: *const GUID, flags: u32) -> HANDLE;
    pub fn DestroyWindow(h: HWND) -> BOOL;
    pub fn GetMessageW(msg: *mut MSG, h: HWND, min: u32, max: u32) -> BOOL;
    pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> BOOL;
    pub fn PostQuitMessage(code: i32);
    pub fn RegisterHotKey(h: HWND, id: i32, mods: u32, vk: u32) -> BOOL;
    pub fn UnregisterHotKey(h: HWND, id: i32) -> BOOL;
    pub fn CreatePopupMenu() -> HMENU;
    pub fn AppendMenuW(menu: HMENU, flags: u32, id: usize, text: *const u16) -> BOOL;
    pub fn TrackPopupMenu(menu: HMENU, flags: u32, x: i32, y: i32, reserved: i32, h: HWND, rc: *const RECT) -> i32;
    pub fn DestroyMenu(menu: HMENU) -> BOOL;
    pub fn SetForegroundWindow(h: HWND) -> BOOL;
    pub fn GetCursorPos(p: *mut POINT) -> BOOL;
    pub fn GetForegroundWindow() -> HWND;
    pub fn MonitorFromPoint(pt: POINT, flags: u32) -> HMONITOR;
    pub fn LoadIconW(inst: HINSTANCE, name: *const u16) -> HICON;
    pub fn LoadImageW(inst: HINSTANCE, name: *const u16, ty: u32, cx: i32, cy: i32, flags: u32) -> isize;
    pub fn SendMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn RegisterWindowMessageW(s: *const u16) -> u32;
    pub fn GetSystemMetrics(i: i32) -> i32;
    pub fn GetAsyncKeyState(vk: i32) -> i16;
    pub fn GetWindowThreadProcessId(h: HWND, pid: *mut u32) -> u32;
    pub fn AttachThreadInput(from: u32, to: u32, attach: BOOL) -> BOOL;
    pub fn BringWindowToTop(h: HWND) -> BOOL;
    #[cfg(target_pointer_width = "64")]
    pub fn GetWindowLongPtrW(h: HWND, idx: i32) -> isize;
    #[cfg(target_pointer_width = "64")]
    pub fn SetWindowLongPtrW(h: HWND, idx: i32, v: isize) -> isize;
    #[cfg(target_pointer_width = "32")]
    #[link_name = "GetWindowLongW"]
    pub fn GetWindowLongPtrW(h: HWND, idx: i32) -> isize;
    #[cfg(target_pointer_width = "32")]
    #[link_name = "SetWindowLongW"]
    pub fn SetWindowLongPtrW(h: HWND, idx: i32, v: isize) -> isize;
}

#[cfg_attr(windows, link(name = "kernel32"))]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetLastError() -> u32;
    pub fn CreateMutexW(attr: *const core::ffi::c_void, owner: BOOL, name: *const u16) -> HANDLE;
    pub fn GetUserDefaultLocaleName(buf: *mut u16, len: i32) -> i32;
    pub fn AttachConsole(pid: u32) -> BOOL;
    pub fn SetStdHandle(which: u32, h: HANDLE) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn K32EmptyWorkingSet(process: HANDLE) -> BOOL;
}

#[repr(C)]
pub struct MAGCOLOREFFECT {
    pub transform: [[f32; 5]; 5],
}

#[cfg_attr(windows, link(name = "magnification"))]
extern "system" {
    pub fn MagInitialize() -> BOOL;
    pub fn MagUninitialize() -> BOOL;
    pub fn MagSetFullscreenColorEffect(effect: *const MAGCOLOREFFECT) -> BOOL;
}

#[cfg_attr(windows, link(name = "shell32"))]
extern "system" {
    pub fn Shell_NotifyIconW(msg: u32, data: *mut NOTIFYICONDATAW) -> BOOL;
}

#[cfg_attr(windows, link(name = "advapi32"))]
extern "system" {
    pub fn RegOpenKeyExW(root: HKEY, sub: *const u16, opts: u32, sam: u32, out: *mut HKEY) -> i32;
    pub fn RegCreateKeyExW(
        root: HKEY, sub: *const u16, reserved: u32, class: *const u16, opts: u32, sam: u32,
        sec: *const core::ffi::c_void, out: *mut HKEY, disp: *mut u32,
    ) -> i32;
    pub fn RegEnumKeyExW(
        k: HKEY, idx: u32, name: *mut u16, name_len: *mut u32, reserved: *mut u32,
        class: *mut u16, class_len: *mut u32, last_write: *mut u64,
    ) -> i32;
    pub fn RegQueryValueExW(k: HKEY, name: *const u16, reserved: *mut u32, ty: *mut u32, data: *mut u8, len: *mut u32) -> i32;
    pub fn RegSetValueExW(k: HKEY, name: *const u16, reserved: u32, ty: u32, data: *const u8, len: u32) -> i32;
    pub fn RegDeleteValueW(k: HKEY, name: *const u16) -> i32;
    pub fn RegCloseKey(k: HKEY) -> i32;
}

// ---- utilities ----

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// RAII wrapper for a registry key.
pub struct RegKey(pub HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

impl RegKey {
    pub fn open(root: HKEY, path: &str, sam: u32) -> Option<RegKey> {
        let p = wide(path);
        let mut h: HKEY = 0;
        let r = unsafe { RegOpenKeyExW(root, p.as_ptr(), 0, sam, &mut h) };
        (r == ERROR_SUCCESS).then_some(RegKey(h))
    }

    pub fn create(root: HKEY, path: &str) -> Option<RegKey> {
        let p = wide(path);
        let mut h: HKEY = 0;
        let mut disp = 0u32;
        let r = unsafe {
            RegCreateKeyExW(root, p.as_ptr(), 0, std::ptr::null(), 0, KEY_SET_VALUE | KEY_READ, std::ptr::null(), &mut h, &mut disp)
        };
        (r == ERROR_SUCCESS).then_some(RegKey(h))
    }

    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let r = unsafe {
                RegEnumKeyExW(self.0, i, name.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
        out
    }

    fn query(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let n = wide(name);
        let mut ty = 0u32;
        let mut len = 0u32;
        let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null_mut(), &mut ty, std::ptr::null_mut(), &mut len) };
        if r != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let r = unsafe { RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null_mut(), &mut ty, buf.as_mut_ptr(), &mut len) };
        if r != ERROR_SUCCESS {
            return None;
        }
        buf.truncate(len as usize);
        Some((ty, buf))
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        let (ty, buf) = self.query(name)?;
        if ty != REG_SZ {
            return None;
        }
        let w: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(from_wide(&w))
    }

    pub fn get_binary(&self, name: &str) -> Option<Vec<u8>> {
        let (ty, buf) = self.query(name)?;
        (ty == REG_BINARY).then_some(buf)
    }

    pub fn get_dword(&self, name: &str) -> Option<u32> {
        let (ty, buf) = self.query(name)?;
        (ty == REG_DWORD && buf.len() >= 4).then(|| u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
    }

    pub fn set_string(&self, name: &str, value: &str) -> bool {
        let n = wide(name);
        let v = wide(value);
        let bytes: Vec<u8> = v.iter().flat_map(|c| c.to_le_bytes()).collect();
        unsafe { RegSetValueExW(self.0, n.as_ptr(), 0, REG_SZ, bytes.as_ptr(), bytes.len() as u32) == ERROR_SUCCESS }
    }

    pub fn delete_value(&self, name: &str) -> bool {
        let n = wide(name);
        unsafe { RegDeleteValueW(self.0, n.as_ptr()) == ERROR_SUCCESS }
    }
}
