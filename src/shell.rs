// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows shell integration: tray icon, context menu, global shortcuts
//! and system notices (monitor changes / resume from sleep).
//! Runs on its own thread with a message loop and a hidden window.

use crate::hotkey::Combo;
use std::sync::atomic::{AtomicIsize, AtomicU32};
#[cfg(any(windows, feature = "wincheck"))]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone)]
pub enum ShellEvent {
    Hotkey(String),
    HotkeyFailed(String),
    /// left click on the icon (cursor position in physical pixels)
    TrayClick(i32, i32),
    /// right click on the icon
    TrayMenu(i32, i32),
    /// double click: open the main window
    TrayOpen,
    /// "Quit" in the tray menu
    Quit,
    DisplayChange,
    Resume,
    /// The display was turned off (`false`) or back on (`true`) by Windows' power settings, or the
    /// computer went to sleep (`false`).
    DisplayState(bool),
    /// Linux: the portal registered the shortcuts (id, key assigned by the desktop)
    HotkeysBound(Vec<(String, String)>),
}

#[derive(Debug, Clone, Default)]
pub struct MenuModel {
    pub tip: String,
    pub open: String,
    pub quit: String,
}

type Callback = Arc<dyn Fn(ShellEvent) + Send + Sync>;

struct Shared {
    cb: Callback,
    hotkeys: Vec<(String, Combo)>,
    registered: Vec<(i32, String)>,
    menu: MenuModel,
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
static HWND_CELL: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

pub struct Shell {
    tray: bool,
}

#[cfg(any(windows, feature = "wincheck"))]
mod imp {
    use super::*;
    use crate::winapi::*;

    const WM_TRAY: u32 = WM_APP + 1;
    const WM_REREGISTER: u32 = WM_APP + 2;
    const WM_REFRESH_TIP: u32 = WM_APP + 3;

    fn shared() -> &'static Mutex<Shared> {
        SHARED.get().expect("shell not started")
    }

    fn emit(ev: ShellEvent) {
        let cb = shared().lock().map(|s| s.cb.clone()).ok();
        if let Some(cb) = cb {
            cb(ev);
        }
    }

    fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = unsafe {
            // resource 1 = app icon, loaded at the exact tray size (16 px at 100 %) to avoid jagged edges
            let icon = crate::sysutil::tray_icon();
            if icon == 0 { LoadIconW(0, IDI_APPLICATION as *const u16) } else { icon }
        };
        let tip = shared().lock().map(|s| s.menu.tip.clone()).unwrap_or_default();
        for (i, c) in tip.encode_utf16().take(127).enumerate() {
            nid.szTip[i] = c;
        }
        nid
    }

    fn add_tray(hwnd: HWND) {
        let mut nid = tray_data(hwnd);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &mut nid);
        }
    }

    fn update_tip(hwnd: HWND) {
        let mut nid = tray_data(hwnd);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &mut nid);
        }
    }

    fn remove_tray(hwnd: HWND) {
        let mut nid = tray_data(hwnd);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &mut nid);
        }
    }

    fn reregister_hotkeys(hwnd: HWND) {
        let (old, wanted, cb) = {
            let mut s = shared().lock().unwrap();
            let old = std::mem::take(&mut s.registered);
            (old, s.hotkeys.clone(), s.cb.clone())
        };
        for (id, _) in old {
            unsafe {
                UnregisterHotKey(hwnd, id);
            }
        }
        let mut registered = Vec::new();
        for (i, (action, combo)) in wanted.iter().enumerate() {
            let id = i as i32 + 1;
            let ok = unsafe { RegisterHotKey(hwnd, id, combo.mods, combo.vk) };
            if ok != 0 {
                registered.push((id, action.clone()));
            } else {
                cb(ShellEvent::HotkeyFailed(action.clone()));
            }
        }
        shared().lock().unwrap().registered = registered;
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_HOTKEY => {
                let action = shared().lock().ok().and_then(|s| s.registered.iter().find(|(id, _)| *id as usize == wp).map(|(_, a)| a.clone()));
                if let Some(a) = action {
                    emit(ShellEvent::Hotkey(a));
                }
                0
            }
            WM_TRAY => {
                let mut pt = POINT::default();
                GetCursorPos(&mut pt);
                match (lp as u32) & 0xFFFF {
                    WM_LBUTTONUP => emit(ShellEvent::TrayClick(pt.x, pt.y)),
                    WM_LBUTTONDBLCLK => emit(ShellEvent::TrayOpen),
                    WM_RBUTTONUP | WM_CONTEXTMENU => emit(ShellEvent::TrayMenu(pt.x, pt.y)),
                    _ => {}
                }
                0
            }
            WM_REREGISTER => {
                reregister_hotkeys(hwnd);
                0
            }
            WM_REFRESH_TIP => {
                update_tip(hwnd);
                0
            }
            WM_DISPLAYCHANGE => {
                emit(ShellEvent::DisplayChange);
                0
            }
            WM_POWERBROADCAST => {
                if wp == PBT_APMRESUMEAUTOMATIC || wp == PBT_APMRESUMESUSPEND {
                    emit(ShellEvent::Resume);
                } else if wp == PBT_APMSUSPEND {
                    emit(ShellEvent::DisplayState(false));
                } else if wp == PBT_POWERSETTINGCHANGE && lp != 0 {
                    let st = &*(lp as *const POWERBROADCAST_SETTING);
                    if st.power_setting.Data1 == GUID_CONSOLE_DISPLAY_STATE.Data1 && st.data_length >= 1 {
                        match st.data[0] {
                            0 => emit(ShellEvent::DisplayState(false)),
                            1 => emit(ShellEvent::DisplayState(true)),
                            _ => {} // dimmed: nothing to do
                        }
                    }
                }
                1
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                remove_tray(hwnd);
                PostQuitMessage(0);
                0
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                add_tray(hwnd); // Explorer restarted
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub fn start_tray() -> bool {
        true
    }

    pub fn run_thread() {
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide("SoluaShellWindow");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: 0,
                hCursor: 0,
                hbrBackground: 0,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);
            // hidden top-level window (receives broadcast messages, unlike message-only)
            let hwnd = CreateWindowExW(0, class.as_ptr(), wide("Solua").as_ptr(), 0, 0, 0, 0, 0, 0, 0, inst, std::ptr::null());
            if hwnd == 0 {
                return;
            }
            HWND_CELL.store(hwnd, Ordering::SeqCst);
            // be told when Windows turns the display off/on (monitors often wake up with factory levels)
            RegisterPowerSettingNotification(hwnd, &GUID_CONSOLE_DISPLAY_STATE, 0);
            add_tray(hwnd);
            reregister_hotkeys(hwnd);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn post(msg: u32) {
        let h = HWND_CELL.load(Ordering::SeqCst);
        if h != 0 {
            unsafe {
                PostMessageW(h, msg, 0, 0);
            }
        }
    }

    pub fn reregister() {
        post(WM_REREGISTER)
    }
    #[cfg(target_os = "linux")]
    pub fn portal_set(_: Vec<crate::portal::Shortcut>) {}
    pub fn refresh_tip() {
        post(WM_REFRESH_TIP)
    }
    pub fn close() {
        post(WM_CLOSE)
    }
}

#[cfg(all(not(any(windows, feature = "wincheck")), target_os = "linux"))]
mod imp {
    //! Linux has no tray icon (it would only open and close the window). Global shortcuts come from
    //! the XDG portal (KDE, GNOME 48+) or, on GNOME/XFCE without the portal, from custom keybindings
    //! that run `solua --action <id>` (see `dekeys.rs`).
    use super::*;
    use crate::dekeys::Backend;
    use std::sync::OnceLock;

    static BACKEND: OnceLock<Backend> = OnceLock::new();

    fn emit(ev: ShellEvent) {
        let cb = SHARED.get().and_then(|s| s.lock().ok().map(|s| s.cb.clone()));
        if let Some(cb) = cb {
            cb(ev);
        }
    }

    static PORTAL: Mutex<Option<std::sync::mpsc::Sender<Vec<crate::portal::Shortcut>>>> = Mutex::new(None);

    pub fn backend() -> Backend {
        *BACKEND.get_or_init(crate::dekeys::detect)
    }

    pub fn run_thread() {
        // commands from `solua --action <id>` (and a plain second launch, which asks to show the window)
        crate::sysutil::serve_commands(|id| emit(ShellEvent::Hotkey(id)));
        if backend() == Backend::Portal {
            let tx = crate::portal::start(Arc::new(|ev| match ev {
                crate::portal::Event::Activated(id) => emit(ShellEvent::Hotkey(id)),
                crate::portal::Event::Bound(list) => emit(ShellEvent::HotkeysBound(list)),
            }));
            *PORTAL.lock().unwrap() = Some(tx);
        }
    }

    pub fn portal_set(list: Vec<crate::portal::Shortcut>) {
        match backend() {
            Backend::Portal => {
                if let Some(tx) = PORTAL.lock().unwrap().as_ref() {
                    let _ = tx.send(list);
                }
            }
            b @ (Backend::Gnome | Backend::Xfce) => {
                // the desktop stores the keys, so applying them is slow: do it off the UI thread
                let _ = std::thread::Builder::new().name("solua-dekeys".into()).spawn(move || {
                    crate::dekeys::apply(b, &list);
                    let bound = list
                        .iter()
                        .map(|s| (s.id.clone(), crate::dekeys::accel(&s.trigger, "Ctrl").map(|a| pretty(&a)).unwrap_or_default()))
                        .collect();
                    emit(ShellEvent::HotkeysBound(bound));
                });
            }
            Backend::None => {}
        }
    }

    /// "<Ctrl><Alt>Up" -> "Ctrl+Alt+Up"
    fn pretty(accel: &str) -> String {
        accel.replace("><", "+").replace(['<', '>'], "+").trim_matches('+').to_string()
    }

    pub fn reregister() {}
    pub fn refresh_tip() {}
    pub fn close() {}
}

#[cfg(all(not(any(windows, feature = "wincheck")), not(target_os = "linux")))]
mod imp {
    pub fn run_thread() {}
    pub fn start_tray() -> bool { false }
    pub fn reregister() {}
    pub fn refresh_tip() {}
    pub fn close() {}
}

impl Shell {
    pub fn start(cb: Callback, hotkeys: Vec<(String, Combo)>, menu: MenuModel) -> Shell {
        let _ = SHARED.set(Mutex::new(Shared { cb, hotkeys, registered: Vec::new(), menu }));
        std::thread::Builder::new().name("solua-shell".into()).spawn(imp::run_thread).ok();
        Shell { tray: cfg!(windows) }
    }

    /// Is there a tray icon? (Windows only.)
    pub fn has_tray(&self) -> bool {
        self.tray
    }

    /// Linux: how global shortcuts are registered on this desktop.
    #[cfg(target_os = "linux")]
    pub fn shortcut_backend(&self) -> crate::dekeys::Backend {
        imp::backend()
    }

    pub fn set_hotkeys(&self, hotkeys: Vec<(String, Combo)>) {
        if let Some(s) = SHARED.get() {
            s.lock().unwrap().hotkeys = hotkeys;
        }
        imp::reregister();
    }

    /// Linux/Wayland: offers the commands to the global shortcuts portal.
    #[cfg(target_os = "linux")]
    pub fn set_portal_shortcuts(&self, list: Vec<crate::portal::Shortcut>) {
        imp::portal_set(list);
    }

    pub fn set_menu(&self, menu: MenuModel) {
        if let Some(s) = SHARED.get() {
            s.lock().unwrap().menu = menu;
        }
        imp::refresh_tip();
    }

    pub fn shutdown(&self) {
        imp::close();
    }
}
