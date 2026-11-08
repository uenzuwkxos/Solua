// SPDX-License-Identifier: GPL-3.0-or-later
//! System utilities: single instance, start with Windows, system theme/language,
//! and the OSD window style.

pub fn exe_dir_open(path: &std::path::Path) {
    #[cfg(windows)]
    let prog = "explorer";
    #[cfg(not(windows))]
    let prog = "xdg-open";
    let _ = std::process::Command::new(prog).arg(path).spawn();
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Runs a Windows file dialog (via PowerShell, without a console window) and returns the chosen path.
#[cfg(windows)]
fn file_dialog(save: bool, default_name: &str) -> Option<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;
    let kind = if save { "SaveFileDialog" } else { "OpenFileDialog" };
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;\
         Add-Type -AssemblyName System.Windows.Forms;\
         $f=New-Object System.Windows.Forms.Form;$f.TopMost=$true;\
         $d=New-Object System.Windows.Forms.{kind};\
         $d.Filter='Solua backup (*.json)|*.json|All files (*.*)|*.*';\
         $d.FileName='{default_name}';\
         if($d.ShowDialog($f) -eq 'OK'){{[Console]::Out.Write($d.FileName)}}"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-STA", "-WindowStyle", "Hidden", "-Command", &script])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then(|| std::path::PathBuf::from(text))
}

/// Linux: uses zenity or kdialog if present; otherwise a fixed file in the config folder.
#[cfg(not(windows))]
fn file_dialog(save: bool, default_name: &str) -> Option<std::path::PathBuf> {
    let run = |prog: &str, args: &[&str]| -> Option<Option<String>> {
        let o = std::process::Command::new(prog).args(args).output().ok()?;
        let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
        Some((o.status.success() && !t.is_empty()).then_some(t))
    };
    let z: Vec<&str> = if save {
        vec!["--file-selection", "--save", "--confirm-overwrite", "--filename", default_name]
    } else {
        vec!["--file-selection"]
    };
    if let Some(r) = run("zenity", &z) {
        return r.map(std::path::PathBuf::from);
    }
    let k = if save { vec!["--getsavefilename", default_name] } else { vec!["--getopenfilename", "."] };
    if let Some(r) = run("kdialog", &k) {
        return r.map(std::path::PathBuf::from);
    }
    Some(crate::config::config_dir().join(default_name))
}

pub fn save_dialog(default_name: &str) -> Option<std::path::PathBuf> {
    file_dialog(true, default_name)
}

pub fn open_dialog() -> Option<std::path::PathBuf> {
    file_dialog(false, "solua-backup.json")
}

#[cfg(any(windows, feature = "wincheck"))]
mod imp {
    use crate::winapi::*;

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const APP_VALUE: &str = "Solua";

    pub fn single_instance() -> bool {
        let name = wide("Local\\Solua.SingleInstance");
        unsafe {
            let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
            if h == 0 {
                return true;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return false;
            }
        }
        true
    }

    pub fn set_autostart(on: bool) {
        let Some(k) = RegKey::create(HKEY_CURRENT_USER, RUN_KEY) else { return };
        if on {
            if let Ok(exe) = std::env::current_exe() {
                k.set_string(APP_VALUE, &format!("\"{}\" --minimized", exe.display()));
            }
        } else {
            k.delete_value(APP_VALUE);
        }
    }

    pub fn is_autostart() -> bool {
        RegKey::open(HKEY_CURRENT_USER, RUN_KEY, KEY_READ).and_then(|k| k.get_string(APP_VALUE)).is_some()
    }

    pub fn system_dark() -> bool {
        RegKey::open(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", KEY_READ)
            .and_then(|k| k.get_dword("AppsUseLightTheme"))
            .map(|v| v == 0)
            .unwrap_or(false)
    }

    pub fn system_locale() -> String {
        let mut buf = [0u16; 85];
        let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
        if n > 0 { from_wide(&buf) } else { String::new() }
    }

    pub fn screen_size() -> (i32, i32) {
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
    }


    static MAG_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Applies a 5×5 color matrix to the whole desktop (Windows Magnification API).
    pub fn apply_color_matrix(m: &[[f32; 5]; 5]) -> bool {
        use std::sync::atomic::Ordering;
        unsafe {
            if !MAG_READY.load(Ordering::SeqCst) {
                if MagInitialize() == 0 {
                    return false;
                }
                MAG_READY.store(true, Ordering::SeqCst);
            }
            let eff = MAGCOLOREFFECT { transform: *m };
            MagSetFullscreenColorEffect(&eff) != 0
        }
    }

    pub fn shutdown_color() {
        use std::sync::atomic::Ordering;
        if MAG_READY.swap(false, Ordering::SeqCst) {
            unsafe {
                MagUninitialize(); // restore the colors
            }
        }
    }

    pub fn foreground_window() -> isize {
        unsafe { GetForegroundWindow() }
    }

    /// Work area (without the taskbar) of the monitor containing the point: (left, top, right, bottom).
    pub fn work_area_at(x: i32, y: i32) -> (i32, i32, i32, i32) {
        unsafe {
            let hm = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFOEXW = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if hm != 0 && GetMonitorInfoW(hm, &mut mi) != 0 {
                let r = mi.rcWork;
                return (r.left, r.top, r.right, r.bottom);
            }
            (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        }
    }

    /// Brings the window to the foreground. Windows denies this to whoever did not receive the last user
    /// input (e.g. with another app window open); attaching to the foreground window's input queue
    /// during the call works around the restriction.
    pub fn set_foreground(hwnd: isize) {
        unsafe {
            let fg = GetForegroundWindow();
            let me = GetCurrentThreadId();
            let other = if fg != 0 { GetWindowThreadProcessId(fg, std::ptr::null_mut()) } else { 0 };
            let attached = other != 0 && other != me && AttachThreadInput(me, other, 1) != 0;
            BringWindowToTop(hwnd);
            SetForegroundWindow(hwnd);
            if attached {
                AttachThreadInput(me, other, 0);
            }
        }
    }

    /// Is any mouse button pressed right now?
    pub fn mouse_down() -> bool {
        unsafe { [0x01, 0x02, 0x04].iter().any(|vk| (GetAsyncKeyState(*vk) as u16) & 0x8000 != 0) }
    }

    #[allow(dead_code)]
    pub fn cursor_pos() -> (i32, i32) {
        let mut p = POINT::default();
        unsafe {
            GetCursorPos(&mut p);
        }
        (p.x, p.y)
    }

    /// Gives unused memory pages back to the system (reduces the RAM shown in Task Manager).
    pub fn trim_memory() {
        unsafe {
            K32EmptyWorkingSet(GetCurrentProcess());
        }
    }

    /// App icon (resource 1 of the .exe) at the requested native size, without Windows resampling.
    pub fn app_icon(small: bool) -> isize {
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let (cx, cy) = if small {
                (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON))
            } else {
                (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON))
            };
            LoadImageW(inst, 1usize as *const u16, IMAGE_ICON, cx, cy, 0)
        }
    }

    /// Tray icon (resource 2, no background); falls back to the app icon if missing.
    pub fn tray_icon() -> isize {
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            let h = LoadImageW(inst, 2usize as *const u16, IMAGE_ICON, cx, cy, 0);
            if h != 0 { h } else { app_icon(true) }
        }
    }

    /// Replaces the window icons (title bar/taskbar) with the resource ones, sharp at any DPI.
    pub fn set_window_icons(hwnd: isize) {
        let (small, big) = (app_icon(true), app_icon(false));
        unsafe {
            if small != 0 {
                SendMessageW(hwnd, WM_SETICON, 0, small);
            }
            if big != 0 {
                SendMessageW(hwnd, WM_SETICON, 1, big);
            }
        }
    }

    /// Current keyboard state: (modifiers, ordinary keys pressed), straight from Windows.
    pub fn keys_snapshot() -> (u32, Vec<u32>) {
        use crate::hotkey::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
        let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16) & 0x8000 != 0 };
        let mut mods = 0;
        if down(0x11) { mods |= MOD_CONTROL; }
        if down(0x12) { mods |= MOD_ALT; }
        if down(0x10) { mods |= MOD_SHIFT; }
        if down(0x5B) || down(0x5C) { mods |= MOD_WIN; }
        let keys = crate::hotkey::candidate_vks().into_iter().filter(|vk| down(*vk as i32)).collect();
        (mods, keys)
    }

    /// Hidden window (tool window, never shown) used as the "owner" of the OSD and the mini panel: Windows
    /// does not list owned windows in the taskbar.
    pub fn create_owner() -> isize {
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide("SoluaOwner");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
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
            CreateWindowExW(
                WS_EX_TOOLWINDOW as u32,
                class.as_ptr(),
                wide("Solua").as_ptr(),
                0x8000_0000, // WS_POPUP
                0, 0, 0, 0, 0, 0, inst, std::ptr::null(),
            )
        }
    }

    /// Window that does not appear in the taskbar or in Alt+Tab.
    #[allow(dead_code)]
    pub fn make_toolwindow(hwnd: isize) {
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW);
        }
    }

    /// On Windows the screen comes straight from the system API.
    pub fn set_screen(_x: i32, _y: i32, _w: i32, _h: i32) {}

    /// Makes the window not steal focus (OSD).
    pub fn make_noactivate(hwnd: isize) {
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
        }
    }
}

#[cfg(not(any(windows, feature = "wincheck")))]
mod imp {
    use std::sync::Mutex;

    /// Main screen (x, y, width, height) in physical pixels; filled in by the app from winit.
    static SCREEN: Mutex<(i32, i32, i32, i32)> = Mutex::new((0, 0, 1920, 1080));

    pub fn set_screen(x: i32, y: i32, w: i32, h: i32) {
        if w > 0 && h > 0 {
            *SCREEN.lock().unwrap() = (x, y, w, h);
        }
    }

    #[cfg(target_os = "linux")]
    static LOCK: std::sync::OnceLock<std::os::unix::net::UnixListener> = std::sync::OnceLock::new();

    #[cfg(target_os = "linux")]
    fn instance_addr() -> Option<std::os::unix::net::SocketAddr> {
        use std::os::linux::net::SocketAddrExt;
        std::os::unix::net::SocketAddr::from_abstract_name(b"solua.single-instance").ok()
    }

    /// Claims the single-instance socket. Returns `false` when another instance already owns it.
    #[cfg(target_os = "linux")]
    pub fn single_instance() -> bool {
        use std::os::unix::net::UnixListener;
        let Some(addr) = instance_addr() else { return true };
        match UnixListener::bind_addr(&addr) {
            Ok(l) => {
                let _ = LOCK.set(l);
                true
            }
            Err(_) => false,
        }
    }

    /// Sends one command line (a shortcut id such as `brightness_up`, or `show`) to the running
    /// instance. Returns `false` when no instance is listening.
    #[cfg(target_os = "linux")]
    pub fn send_command(cmd: &str) -> bool {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        let Some(addr) = instance_addr() else { return false };
        match UnixStream::connect_addr(&addr) {
            Ok(mut s) => s.write_all(format!("{}\n", cmd.replace('\n', " ")).as_bytes()).is_ok(),
            Err(_) => false,
        }
    }

    /// Serves commands sent by `send_command` on a background thread; `cb` gets each line.
    #[cfg(target_os = "linux")]
    pub fn serve_commands(cb: impl Fn(String) + Send + 'static) {
        use std::io::{BufRead, BufReader, Read};
        let Some(listener) = LOCK.get().and_then(|l| l.try_clone().ok()) else { return };
        let _ = std::thread::Builder::new().name("solua-ipc".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut line = String::new();
                if BufReader::new(conn).take(4096).read_line(&mut line).is_ok() {
                    let line = line.trim();
                    if !line.is_empty() {
                        cb(line.to_string());
                    }
                }
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    pub fn single_instance() -> bool { true }

    /// Path of the program as the user launched it: the `.AppImage` file itself when running from an
    /// AppImage (the executable inside it lives in a temporary mount that changes on every run).
    #[cfg(target_os = "linux")]
    pub fn exe_path() -> Option<std::path::PathBuf> {
        if let Some(p) = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from) {
            if p.exists() {
                return Some(p);
            }
        }
        std::env::current_exe().ok()
    }

    #[cfg(target_os = "linux")]
    pub fn is_appimage() -> bool {
        std::env::var_os("APPIMAGE").is_some()
    }

    #[cfg(target_os = "linux")]
    fn data_home() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
    }

    #[cfg(target_os = "linux")]
    fn menu_file() -> Option<std::path::PathBuf> {
        Some(data_home()?.join("applications").join("solua.desktop"))
    }

    #[cfg(target_os = "linux")]
    pub fn menu_entry_exists() -> bool {
        menu_file().map(|f| f.exists()).unwrap_or(false)
    }

    /// Adds/removes an applications-menu launcher (used for AppImages, which do not install one).
    #[cfg(target_os = "linux")]
    pub fn set_menu_entry(on: bool) {
        let (Some(f), Some(base)) = (menu_file(), data_home()) else { return };
        let icon = base.join("icons/hicolor/256x256/apps/solua.png");
        if on {
            let Some(exe) = exe_path() else { return };
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            let _ = std::fs::create_dir_all(icon.parent().unwrap());
            let _ = std::fs::write(&icon, include_bytes!("../assets/icon.png"));
            let _ = std::fs::write(
                &f,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Solua\nComment=Monitor control\nExec=\"{}\"\nIcon={}\nTerminal=false\nCategories=Utility;Settings;\nStartupWMClass=solua\n",
                    exe.display(),
                    icon.display()
                ),
            );
        } else {
            let _ = std::fs::remove_file(f);
            let _ = std::fs::remove_file(icon);
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn is_appimage() -> bool { false }

    fn autostart_file() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
        Some(base.join("autostart").join("solua.desktop"))
    }

    pub fn set_autostart(on: bool) {
        let Some(f) = autostart_file() else { return };
        if on {
            let Some(exe) = exe_path() else { return };
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            let _ = std::fs::write(
                f,
                format!(
                    "[Desktop Entry]\nType=Application\nName=Solua\nComment=Monitor control\nExec=\"{}\" --minimized\nIcon=solua\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
                    exe.display()
                ),
            );
        } else {
            let _ = std::fs::remove_file(f);
        }
    }

    pub fn is_autostart() -> bool {
        autostart_file().map(|f| f.exists()).unwrap_or(false)
    }

    pub fn system_dark() -> bool {
        if let Ok(v) = std::env::var("SOLUA_DARK") {
            return v == "1";
        }
        if let Ok(o) = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "color-scheme"]).output() {
            let t = String::from_utf8_lossy(&o.stdout).to_lowercase();
            if t.contains("dark") {
                return true;
            }
            if t.contains("default") || t.contains("light") {
                return false;
            }
        }
        // KDE: ~/.config/kdeglobals -> ColorScheme=BreezeDark
        if let Some(h) = std::env::var_os("HOME") {
            if let Ok(t) = std::fs::read_to_string(std::path::PathBuf::from(h).join(".config/kdeglobals")) {
                if let Some(l) = t.lines().find(|l| l.starts_with("ColorScheme=")) {
                    return l.to_lowercase().contains("dark");
                }
            }
        }
        false
    }
    pub fn system_locale() -> String {
        std::env::var("LC_ALL").or_else(|_| std::env::var("LANG")).unwrap_or_default()
    }
    pub fn screen_size() -> (i32, i32) {
        let s = *SCREEN.lock().unwrap();
        (s.2, s.3)
    }
    pub fn make_noactivate(_hwnd: isize) {}
    pub fn apply_color_matrix(_m: &[[f32; 5]; 5]) -> bool { false }
    pub fn shutdown_color() {}
    pub fn foreground_window() -> isize { 0 }
    pub fn work_area_at(_x: i32, _y: i32) -> (i32, i32, i32, i32) {
        let s = *SCREEN.lock().unwrap();
        (s.0, s.1, s.0 + s.2, s.1 + s.3)
    }
    pub fn set_foreground(_hwnd: isize) {}
    pub fn mouse_down() -> bool { false }
    pub fn cursor_pos() -> (i32, i32) {
        let s = *SCREEN.lock().unwrap();
        (s.0 + s.2 / 2, s.1 + s.3 / 2)
    }
    /// Gives the allocator's free pages (glibc) back to the system after closing the window.
    pub fn trim_memory() {
        #[cfg(target_os = "linux")]
        {
            extern "C" {
                fn malloc_trim(pad: usize) -> i32;
            }
            unsafe {
                malloc_trim(0);
            }
        }
    }
    pub fn make_toolwindow(_hwnd: isize) {}
    pub fn create_owner() -> isize { 0 }
    pub fn keys_snapshot() -> (u32, Vec<u32>) { (0, Vec::new()) }
    pub fn set_window_icons(_hwnd: isize) {}
    pub fn app_icon(_small: bool) -> isize { 0 }
    pub fn tray_icon() -> isize { 0 }
}

pub use imp::*;
