// SPDX-License-Identifier: GPL-3.0-or-later
//! UI controller: wires the Slint windows, the DDC worker, the tray/shortcuts and the schedule.

use crate::colorfx::{self, ColorFx, Mode};
use crate::config::{default_hotkeys, Config, FxValues, Profile, HOTKEY_ACTIONS};
use crate::core::{Core, MonInfo, MonView, VcpRow};
use crate::shell::{MenuModel, Shell, ShellEvent};
use crate::worker::{self, Cmd, Event, OsdKind, Status};
use crate::{hotkey, lang, schedule, sysutil, util, vcp};
use crate::{
    AppWindow, ChipItem, ChoiceItem, HotkeyItem, InfoItem, MiniProfileItem, MonitorItem, OsdWindow, ProfileItem, ScheduleItem,
    SliderCfg, Strings, Theme, TrayPopup, VcpRowItem,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;
#[cfg_attr(not(windows), allow(unused_imports))]
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Internal name of the profile that holds the "Free" mode state (never shown in the list).
const FREE_NAME: &str = "\u{0}free";

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    /// Kind of the next native window to be created: 0 main, 1 mini panel, 2 indicator (OSD).
    static KIND: Cell<u8> = const { Cell::new(0) };
}

/// Hidden window that "owns" the OSD and the mini panel (owned windows do not show in the taskbar).
static OWNER: AtomicIsize = AtomicIsize::new(0);

fn with_app(f: impl FnOnce(&Rc<App>) + 'static) {
    APP.with(|a| {
        if let Some(app) = a.borrow().as_ref() {
            f(app);
        }
    });
}

/// The winit attributes hook runs **when the Slint component is created**. That is why the OSD and the mini
/// panel are created with `KIND` set and destroyed on close (never hidden and shown again:
/// `hide()` drops the native window and the next `show()` would recreate it without the hook, as an
/// ordinary window — and ordinary windows without an owner show up in the taskbar).
#[cfg(windows)]
fn install_backend() {
    use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
    OWNER.store(sysutil::create_owner(), Ordering::SeqCst);
    let r = slint::BackendSelector::new()
        .with_winit_window_attributes_hook(|a| {
            let owner = OWNER.load(Ordering::SeqCst);
            match KIND.with(|k| k.get()) {
                1 => a.with_skip_taskbar(true).with_owner_window(owner),
                2 => a.with_skip_taskbar(true).with_active(false).with_owner_window(owner),
                _ => a,
            }
        })
        .select();
    if let Err(e) = r {
        eprintln!("solua: backend: {e}");
    }
}
/// On Linux this uses the software renderer: the UI is simple, and the OpenGL path loads
/// Mesa and the graphics driver (~100 MB more RAM). `SOLUA_GL=1` goes back to OpenGL.
#[cfg(not(windows))]
fn install_backend() {
    if std::env::var_os("SLINT_BACKEND").is_some() || std::env::var_os("SOLUA_GL").is_some() {
        return;
    }
    if let Err(e) = slint::BackendSelector::new().backend_name("winit".into()).renderer_name("software".into()).select() {
        eprintln!("solua: backend: {e}");
    }
}

fn show_as(kind: u8, f: impl FnOnce() -> Result<(), slint::PlatformError>) {
    KIND.with(|k| k.set(kind));
    let _ = f();
    KIND.with(|k| k.set(0));
}

/// Background alpha of the indicator and the mini panel: a fixed, mild transparency when enabled.
fn alpha_for(transparent: bool) -> f32 {
    if transparent { 0.88 } else { 1.0 }
}

struct App {
    ui: AppWindow,
    osd: RefCell<Option<OsdWindow>>,
    osd_timer: Timer,
    osd_drop: Timer,
    popup: RefCell<Option<TrayPopup>>,
    popup_shown: Cell<Option<Instant>>,
    popup_anchor: Cell<(i32, i32, bool)>,
    popup_closing: Cell<bool>,
    popup_had_focus: Cell<bool>,
    popup_mouse: Cell<bool>,
    last_tray: Cell<Option<Instant>>,
    last_close: Cell<Option<Instant>>,
    popup_hide: Timer,
    profile_model: Rc<VecModel<ProfileItem>>,
    mini_profiles: Rc<VecModel<MiniProfileItem>>,
    sched_model: Rc<VecModel<ScheduleItem>>,
    new_days: Cell<[bool; 7]>,
    dirty: Cell<bool>,
    cfg: RefCell<Config>,
    tx: Sender<Cmd>,
    shell: Shell,
    monitors: RefCell<Vec<crate::backend::MonitorInfo>>,
    selected: Cell<usize>,
    fx: Cell<ColorFx>,
    free_fx: RefCell<Option<FxValues>>,
    cap_idx: Cell<i32>,
    cap_timer: Timer,
    autosave: Timer,
    save_timer: Timer,
    poll_timer: Timer,
    /// restore attempts after the display wakes up
    wake_timers: [Timer; 2],
    last_user: Cell<Instant>,
    black_init: RefCell<std::collections::BTreeMap<String, (i32, i32, i32)>>,
    tick_timer: Timer,
    debounce: Timer,
    trim_timer: Timer,
    popup_timer: Timer,
    last_tick: Cell<i64>,
    sched_before: RefCell<Option<Option<String>>>,
    dark: Cell<bool>,
    startup_apply: Cell<bool>,
    has_tray: bool,
    /// Linux: keys the desktop assigned to the shortcuts (id -> description)
    hk_bound: RefCell<std::collections::BTreeMap<String, String>>,
    demo: bool,
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Updates a persistent model row by row (components are reused, so
/// animations — such as the on/off switches — keep working).
fn sync_vec<T: Clone + 'static>(m: &VecModel<T>, items: Vec<T>) {
    let n = items.len();
    for (i, it) in items.into_iter().enumerate() {
        if i < m.row_count() {
            m.set_row_data(i, it);
        } else {
            m.push(it);
        }
    }
    while m.row_count() > n {
        m.remove(m.row_count() - 1);
    }
}

fn fmt1(template: SharedString, arg: &str) -> SharedString {
    template.replace("%1", arg).into()
}

fn fx_from_values(f: &FxValues) -> ColorFx {
    ColorFx {
        saturation: f.saturation.min(100),
        mode: Mode::from_name(&f.mode),
        r: f.r.min(100),
        g: f.g.min(100),
        b: f.b.min(100),
        temp: f.temp.min(100),
    }
}

fn fx_to_values(f: &ColorFx) -> FxValues {
    FxValues { saturation: f.saturation, mode: f.mode.name().into(), r: f.r, g: f.g, b: f.b, temp: f.temp }
}

fn fx_from_cfg(c: &Config) -> ColorFx {
    ColorFx {
        saturation: c.fx_saturation.min(100),
        mode: Mode::from_name(&c.fx_mode),
        r: c.fx_r.min(100),
        g: c.fx_g.min(100),
        b: c.fx_b.min(100),
        temp: c.fx_temp.min(100),
    }
}

fn mods_text(m: u32) -> String {
    let mut parts = Vec::new();
    if m & hotkey::MOD_CONTROL != 0 {
        parts.push("Ctrl");
    }
    if m & hotkey::MOD_ALT != 0 {
        parts.push("Alt");
    }
    if m & hotkey::MOD_SHIFT != 0 {
        parts.push("Shift");
    }
    if m & hotkey::MOD_WIN != 0 {
        parts.push("Win");
    }
    parts.join("+")
}

pub fn run(demo_flag: bool, minimized: bool) -> Result<(), Box<dyn std::error::Error>> {
    install_backend();
    let cfg = Config::load();
    let ui = AppWindow::new()?;
    // the translation context only exists after the first window is created
    lang::apply(lang::resolve(&cfg.language));
    let (be, demo) = crate::backend::create(demo_flag);
    let (tx, rx) = worker::channel();
    let emit: worker::Emit = Arc::new(|ev| {
        let _ = slint::invoke_from_event_loop(move || with_app(move |app| app.on_event(ev)));
    });
    let handle = worker::spawn(Core::new(be), rx, emit);

    let shell_cb: Arc<dyn Fn(ShellEvent) + Send + Sync> = Arc::new(|ev| {
        let _ = slint::invoke_from_event_loop(move || with_app(move |app| app.on_shell(ev)));
    });
    let shell = Shell::start(shell_cb, Vec::new(), MenuModel::default());
    let has_tray = shell.has_tray();

    let fx = fx_from_cfg(&cfg);
    let app = Rc::new(App {
        ui,
        osd: RefCell::new(None),
        osd_timer: Timer::default(),
        osd_drop: Timer::default(),
        popup: RefCell::new(None),
        popup_shown: Cell::new(None),
        popup_anchor: Cell::new((0, 0, true)),
        popup_closing: Cell::new(false),
        popup_had_focus: Cell::new(false),
        popup_mouse: Cell::new(false),
        last_tray: Cell::new(None),
        last_close: Cell::new(None),
        popup_hide: Timer::default(),
        profile_model: Rc::new(VecModel::default()),
        mini_profiles: Rc::new(VecModel::default()),
        sched_model: Rc::new(VecModel::default()),
        new_days: Cell::new([true; 7]),
        dirty: Cell::new(false),
        cfg: RefCell::new(cfg),
        tx,
        shell,
        monitors: RefCell::new(Vec::new()),
        selected: Cell::new(0),
        fx: Cell::new(fx),
        free_fx: RefCell::new(None),
        cap_idx: Cell::new(-1),
        cap_timer: Timer::default(),
        autosave: Timer::default(),
        save_timer: Timer::default(),
        poll_timer: Timer::default(),
        wake_timers: [Timer::default(), Timer::default()],
        last_user: Cell::new(Instant::now()),
        black_init: RefCell::new(Default::default()),
        tick_timer: Timer::default(),
        debounce: Timer::default(),
        trim_timer: Timer::default(),
        popup_timer: Timer::default(),
        last_tick: Cell::new(0),
        sched_before: RefCell::new(None),
        dark: Cell::new(false),
        startup_apply: Cell::new(true),
        has_tray,
        hk_bound: RefCell::new(Default::default()),
        demo,
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    app.init();
    let start_hidden = (minimized && (app.has_tray || cfg!(target_os = "linux"))) || std::env::var_os("SOLUA_POPUP").is_some();
    if start_hidden {
        app.schedule_trim();
        // with no visible window the event loop must be kept alive
        slint::run_event_loop_until_quit()?;
    } else {
        show_as(0, || app.ui.show());
        if let Some(h) = hwnd_of(app.ui.window()) {
            sysutil::set_window_icons(h);
        }
        // winit applies the window icon (Slint's `icon` property) once the event loop is
        // running and overwrites ours: reapply shortly after so the first open looks sharp
        for ms in [150u64, 600, 1500] {
            let a = app.clone();
            Timer::single_shot(Duration::from_millis(ms), move || {
                if let Some(h) = hwnd_of(a.ui.window()) {
                    sysutil::set_window_icons(h);
                }
            });
        }
        slint::run_event_loop_until_quit()?;
    }

    app.cfg.borrow().save();
    app.shell.shutdown();
    sysutil::shutdown_color();
    let _ = app.tx.send(Cmd::Quit);
    let _ = handle.join();
    Ok(())
}

impl App {
    fn s(&self) -> Strings<'_> {
        self.ui.global::<Strings>()
    }

    fn locked(&self) -> bool {
        let c = self.cfg.borrow();
        c.active_profile.is_some() && !c.edit_profiles
    }

    // ------------------------------------------------------------ initialization
    fn init(self: &Rc<Self>) {
        let ui = &self.ui;
        ui.set_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
        ui.set_demo(self.demo);
        ui.set_profiles(ModelRc::from(self.profile_model.clone()));
        ui.set_schedule(ModelRc::from(self.sched_model.clone()));
        if let Some(t) = std::env::var("SOLUA_TAB").ok().and_then(|t| t.parse::<i32>().ok()) {
            ui.set_tab(t); // development shortcut for screenshots
        }
        // an active profile that no longer exists (config edited by hand) goes back to Free mode
        {
            let mut c = self.cfg.borrow_mut();
            if let Some(n) = c.active_profile.clone() {
                if c.profile(&n).is_none() {
                    c.active_profile = None;
                }
            }
        }
        self.apply_theme();
        self.apply_slider_cfg();
        self.refresh_all_texts();
        self.sync_settings_to_ui();
        self.sync_fx_to_ui();
        self.sync_new_days();
        self.sync_lock();
        if !colorfx::is_neutral(&self.fx.get()) {
            self.apply_fx(false);
        }
        self.push_shell_hotkeys();
        let _ = self.tx.send(Cmd::Sync(self.cfg.borrow().sync_all));
        let prefer = self.cfg.borrow().selected_monitor.clone();
        let _ = self.tx.send(Cmd::Refresh { prefer });
        if !self.demo {
            // put brightness/contrast back where they were when the app was last used
            let saved = self.cfg.borrow().levels.clone();
            let _ = self.tx.send(Cmd::Restore(Some(saved)));
        }
        self.wire_callbacks();
        self.start_polling();

        // closing the window: tray or quit
        let weak = Rc::downgrade(self);
        ui.window().on_close_requested(move || {
            if let Some(app) = weak.upgrade() {
                // Windows: tray option. Linux: the "run in the background" option (no tray icon).
                let keep = if cfg!(windows) { app.cfg.borrow().close_to_tray && app.has_tray } else { app.cfg.borrow().background };
                if keep {
                    app.schedule_trim();
                    return slint::CloseRequestResponse::HideWindow;
                }
                app.quit();
            }
            slint::CloseRequestResponse::HideWindow
        });

        // schedule + system theme (every 15 s)
        let weak = Rc::downgrade(self);
        self.tick_timer.start(TimerMode::Repeated, Duration::from_secs(15), move || {
            if let Some(app) = weak.upgrade() {
                app.tick();
            }
        });

        if std::env::var_os("SOLUA_ABOUT").is_some() {
            ui.set_about_open(true); // development shortcut for screenshots
        }
        if let Some(m) = std::env::var("SOLUA_POPUP").ok().and_then(|t| t.parse::<i32>().ok()) {
            self.open_popup(m, 600, 300, false); // development shortcut for screenshots
        }
        if std::env::var_os("SOLUA_OSD").is_some() {
            self.show_osd(OsdKind::Brightness, 64, "DELL U2723QE", true);
        }
    }

    /// Saves the config shortly after a change (with a small delay so it is not written at every pixel
    /// of a slider). Without it, killing the process lost the last filter/brightness.
    fn request_save(&self) {
        self.last_user.set(Instant::now());
        self.save_timer.start(TimerMode::SingleShot, Duration::from_millis(300), || {
            with_app(|app| app.cfg.borrow().save());
        });
    }

    /// Follows changes made on the monitor itself (monitor menu, restore defaults, etc.):
    /// while the window or the mini panel is visible, re-reads the values every few seconds.
    fn start_polling(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.poll_timer.start(TimerMode::Repeated, Duration::from_secs(4), move || {
            let Some(app) = weak.upgrade() else { return };
            let visible = app.ui.window().is_visible() || app.popup.borrow().is_some();
            if visible && app.last_user.get().elapsed() > Duration::from_secs(5) {
                let _ = app.tx.send(Cmd::Poll);
            }
        });
    }

    /// Extra re-reads after a command the monitor takes a while to apply (e.g. factory defaults).
    fn poll_soon(&self) {
        for ms in [1500u64, 4000] {
            let tx = self.tx.clone();
            Timer::single_shot(Duration::from_millis(ms), move || {
                let _ = tx.send(Cmd::Poll);
            });
        }
    }

    fn quit(&self) {
        self.cfg.borrow().save();
        let _ = slint::quit_event_loop();
    }

    /// With the window closed the app only needs a few MB: gives idle pages back to the system.
    fn schedule_trim(&self) {
        self.trim_timer.start(TimerMode::SingleShot, Duration::from_millis(900), || {
            sysutil::trim_memory();
            #[cfg(not(windows))]
            Timer::single_shot(Duration::from_secs(5), sysutil::trim_memory); // the renderer frees things gradually
        });
    }

    fn show_main(self: &Rc<Self>) {
        self.close_popup();
        let _ = self.tx.send(Cmd::Poll);
        show_as(0, || self.ui.show());
        update_screen(self.ui.window());
        if let Some(h) = hwnd_of(self.ui.window()) {
            sysutil::set_window_icons(h); // sharp icons (taskbar and title bar)
        }
    }

    /// (dark, black, violet, neutral) according to the settings.
    fn theme_flags(&self) -> (bool, bool, bool, bool) {
        let c = self.cfg.borrow();
        let violet = c.theme_color == "violet";
        let neutral = c.theme_color == "default";
        match c.theme.as_str() {
            "dark" => (true, false, violet, neutral),
            "light" => (false, false, false, false),
            "black" => (true, true, false, false),
            "violet" => (true, false, true, false), // old value
            _ => {
                let d = sysutil::system_dark();
                (d, false, d && violet, d && neutral)
            }
        }
    }

    fn style_theme(&self, t: Theme<'_>) {
        let (dark, black, violet, neutral) = self.theme_flags();
        t.set_black(black);
        t.set_violet(violet);
        t.set_neutral(neutral);
        t.invoke_apply(dark);
    }

    fn apply_theme(&self) {
        let (dark, _, _, _) = self.theme_flags();
        self.dark.set(dark);
        self.style_theme(self.ui.global::<Theme>());
        if let Some(p) = self.popup.borrow().as_ref() {
            self.style_theme(p.global::<Theme>());
        }
        if let Some(o) = self.osd.borrow().as_ref() {
            self.style_theme(o.global::<Theme>());
        }
    }

    /// Slider steps (window and mini panel).
    fn apply_slider_cfg(&self) {
        let (step, discrete) = {
            let c = self.cfg.borrow();
            (c.step as i32, c.discrete)
        };
        let g = self.ui.global::<SliderCfg>();
        g.set_step(step);
        g.set_discrete(discrete);
        if let Some(p) = self.popup.borrow().as_ref() {
            let g = p.global::<SliderCfg>();
            g.set_step(step);
            g.set_discrete(discrete);
        }
    }

    fn sync_settings_to_ui(&self) {
        let c = self.cfg.borrow();
        let ui = &self.ui;
        ui.set_opt_autostart(sysutil::is_autostart());
        ui.set_opt_background(c.background);
        #[cfg(target_os = "linux")]
        {
            ui.set_is_appimage(sysutil::is_appimage());
            ui.set_opt_menu_entry(sysutil::menu_entry_exists());
        }
        ui.set_opt_minimized(c.start_minimized);
        ui.set_opt_tray(c.close_to_tray);
        ui.set_opt_osd(c.show_osd);
        ui.set_opt_sync(c.sync_all);
        ui.set_opt_apply_on_start(c.apply_on_start);
        ui.set_opt_discrete(c.discrete);
        ui.set_opt_edit_profiles(c.edit_profiles);
        ui.set_opt_mini_audio(c.mini_audio);
        ui.set_soft_filter(cfg!(windows));
        ui.set_is_windows(cfg!(windows));
        ui.set_has_hotkeys(cfg!(windows));
        #[cfg(target_os = "linux")]
        {
            let b = self.shell.shortcut_backend();
            ui.set_has_hotkeys(b != crate::dekeys::Backend::None);
            ui.set_hk_editable(b.editable());
        }
        #[cfg(windows)]
        ui.set_hk_editable(true);
        ui.set_opt_transparent(c.transparent);
        ui.set_clock_12h(c.clock_12h);
        ui.set_opt_link_fx(c.link_fx);
        ui.set_opt_step(c.step as i32);
        ui.set_osd_pos(c.osd_pos as i32);
        ui.set_osd_scale(c.osd_scale as i32);
        ui.set_theme_index(match c.theme.as_str() {
            "light" => 1,
            "dark" | "violet" => 2,
            "black" => 3,
            _ => 0,
        });
        ui.set_theme_color_index(if c.theme_color == "default" { 2 } else if c.theme_color == "violet" || c.theme == "violet" { 1 } else { 0 });
        ui.set_language_index(if c.language == "auto" { 0 } else { lang::resolve(&c.language) as i32 + 1 });
    }

    /// Locks the settings while a saved profile is active.
    fn sync_lock(&self) {
        let locked = self.locked();
        let free = self.cfg.borrow().active_profile.is_none();
        self.ui.set_locked(locked);
        self.ui.set_free_active(free);
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_locked(locked);
            p.set_free_active(free);
        }
    }

    /// Rebuilds everything that holds translated text generated in Rust.
    fn refresh_all_texts(&self) {
        let s = self.s();
        let mut names = vec![s.get_lang_system()];
        names.extend(lang::LANGS.iter().map(|l| SharedString::from(l.2)));
        self.ui.set_language_names(model(names));
        self.ui.set_theme_names(model(vec![s.get_theme_system(), s.get_theme_light(), s.get_theme_dark(), s.get_theme_black()]));
        self.ui.set_pos_names(model(vec![
            s.get_pos_top_left(),
            s.get_pos_above(),
            s.get_pos_top_right(),
            s.get_pos_bottom_right(),
            s.get_pos_below(),
            s.get_pos_bottom_left(),
            s.get_pos_center(),
        ]));
        self.refresh_monitor_list();
        self.refresh_profiles();
        self.refresh_schedule();
        self.refresh_hotkeys();
        self.update_tray_menu();
        self.ui.set_status(s.get_st_ready());
    }

    fn monitor_display_name(&self, m: &crate::backend::MonitorInfo) -> SharedString {
        if m.kind == crate::backend::Kind::Internal {
            self.s().get_kind_internal()
        } else {
            m.name.clone().into()
        }
    }

    fn refresh_monitor_list(&self) {
        let items: Vec<MonitorItem> = self
            .monitors
            .borrow()
            .iter()
            .map(|m| MonitorItem {
                name: self.monitor_display_name(m),
                detail: m.detail.clone().into(),
                internal: m.kind == crate::backend::Kind::Internal,
            })
            .collect();
        self.ui.set_monitors(model(items));
        self.ui.set_selected(self.selected.get() as i32);
    }

    fn mode_label(&self, mode: Mode) -> SharedString {
        let s = self.s();
        match mode {
            Mode::Normal => s.get_fx_normal(),
            Mode::Gray => s.get_fx_gray(),
            Mode::Red => s.get_fx_red(),
            Mode::Green => s.get_fx_green(),
            Mode::Blue => s.get_fx_blue(),
            Mode::Yellow => s.get_fx_yellow(),
            Mode::Sepia => s.get_fx_sepia(),
            Mode::Invert => s.get_fx_invert(),
        }
    }

    /// Profile card labels: everything the profile stores, on two lines
    /// (monitor hardware / software filter).
    /// Profile chips, in rows: 1) brightness, contrast, saturation, temperature; 2) color filter and
    /// preset filter; 3) RGB (whites) and RGB (blacks); 4) monitor color temperature.
    fn chips(&self, p: &Profile) -> Vec<Vec<ChipItem>> {
        let chip = |icon: i32, text: String| ChipItem { icon, text: text.into() };
        let (mut r1, mut r2, mut r3, mut r4) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        if let Some(v) = p.monitors.values().next() {
            if let Some(b) = v.brightness {
                r1.push(chip(0, format!("{b}%")));
            }
            if let Some(c) = v.contrast {
                r1.push(chip(1, format!("{c}%")));
            }
            if let (Some(r), Some(g), Some(b)) = (v.red, v.green, v.blue) {
                r3.push(chip(0, format!("RGB {r}·{g}·{b}")));
            }
            if let (Some(r), Some(g), Some(b)) = (v.black_red, v.black_green, v.black_blue) {
                r3.push(chip(9, format!("RGB {r}·{g}·{b}")));
            }
            if let Some(x) = v.color_preset {
                r4.push(chip(8, vcp::preset_name(x)));
            }
        }
        // software color filter: only exists on Windows
        if let Some(f) = p.fx.as_ref().filter(|_| cfg!(windows)) {
            let m = Mode::from_name(&f.mode);
            r1.push(chip(2, format!("{}%", f.saturation)));
            r1.push(chip(4, format!("{}%", 100 - f.temp.min(100) as i32)));
            r2.push(chip(7, format!("{} {}·{}·{}", self.s().get_color_filter_chip(), f.r, f.g, f.b)));
            r2.push(ChipItem { icon: 5, text: self.mode_label(m) });
        }
        vec![r1, r2, r3, r4].into_iter().filter(|r| !r.is_empty()).collect()
    }

    fn refresh_profiles(&self) {
        let (items, minis, names, free) = {
            let c = self.cfg.borrow();
            let active = c.active_profile.clone();
            let is_active = |n: &str| active.as_deref().map(|a| a.eq_ignore_ascii_case(n)).unwrap_or(false);
            let items: Vec<ProfileItem> = c
                .profiles
                .iter()
                .map(|p| {
                    let rows: Vec<ModelRc<ChipItem>> = self.chips(p).into_iter().map(model).collect();
                    ProfileItem { name: p.name.clone().into(), active: is_active(&p.name), rows: model(rows) }
                })
                .collect();
            let minis: Vec<MiniProfileItem> = c
                .profiles
                .iter()
                .map(|p| MiniProfileItem { name: p.name.clone().into(), active: is_active(&p.name) })
                .collect();
            let names: Vec<SharedString> = c.profiles.iter().map(|p| SharedString::from(p.name.as_str())).collect();
            (items, minis, names, active.is_none())
        };
        sync_vec(&self.profile_model, items);
        sync_vec(&self.mini_profiles, minis);
        self.ui.set_profile_names(model(names));
        self.ui.set_free_active(free);
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_free_active(free);
        }
    }

    fn refresh_schedule(&self) {
        let h12 = self.cfg.borrow().clock_12h;
        let items: Vec<ScheduleItem> = self
            .cfg
            .borrow()
            .schedule
            .iter()
            .map(|e| ScheduleItem {
                time: schedule::label_str(&e.time, h12).into(),
                profile: e.profile.clone().into(),
                enabled: e.enabled,
                days: model((0..7).map(|d| e.on_day(d)).collect()),
                end: e.end.as_deref().map(|t| schedule::label_str(t, h12)).unwrap_or_default().into(),
            })
            .collect();
        sync_vec(&self.sched_model, items);
    }

    fn hotkey_ids(&self) -> Vec<String> {
        // on Linux there is no software saturation, no color temperature and no mini panel
        let mut ids: Vec<String> = HOTKEY_ACTIONS
            .iter()
            .filter(|id| cfg!(windows) || !matches!(**id, "saturation_up" | "saturation_down" | "temperature_up" | "temperature_down" | "mini"))
            .map(|s| s.to_string())
            .collect();
        ids.extend(self.cfg.borrow().profiles.iter().map(|p| format!("profile:{}", p.name)));
        ids
    }

    fn sync_new_days(&self) {
        self.ui.set_new_days(model(self.new_days.get().to_vec()));
    }

    fn hotkey_label(&self, id: &str) -> SharedString {
        let s = self.s();
        match id {
            "brightness_up" => s.get_hk_brightness_up(),
            "brightness_down" => s.get_hk_brightness_down(),
            "contrast_up" => s.get_hk_contrast_up(),
            "contrast_down" => s.get_hk_contrast_down(),
            "saturation_up" => s.get_hk_saturation_up(),
            "saturation_down" => s.get_hk_saturation_down(),
            "temperature_up" => s.get_hk_temperature_up(),
            "temperature_down" => s.get_hk_temperature_down(),
            "volume_up" => s.get_hk_volume_up(),
            "volume_down" => s.get_hk_volume_down(),
            "volume_mute" => s.get_hk_volume_mute(),
            "mini" => s.get_hk_mini(),
            "show" => s.get_hk_show(),
            other => fmt1(s.get_hk_profile(), other.strip_prefix("profile:").unwrap_or(other)),
        }
    }

    fn refresh_hotkeys(&self) {
        let c = self.cfg.borrow();
        let items: Vec<HotkeyItem> = self
            .hotkey_ids()
            .into_iter()
            .map(|id| HotkeyItem {
                label: self.hotkey_label(&id),
                combo: if self.ui.get_hk_editable() {
                    c.hotkeys.get(&id).cloned().unwrap_or_default().into()
                } else {
                    // Linux: the key the desktop assigned applies
                    self.hk_bound.borrow().get(&id).cloned().unwrap_or_default().into()
                },
                id: id.into(),
            })
            .collect();
        self.ui.set_hotkeys(model(items));
    }

    fn push_shell_hotkeys(&self) {
        let valid = self.hotkey_ids();
        let list: Vec<(String, hotkey::Combo)> = self
            .cfg
            .borrow()
            .hotkeys
            .iter()
            .filter(|(id, _)| valid.contains(id))
            .filter_map(|(id, c)| hotkey::parse(c).map(|combo| (id.clone(), combo)))
            .collect();
        self.shell.set_hotkeys(list);
        #[cfg(target_os = "linux")]
        {
            let c = self.cfg.borrow();
            let shortcuts: Vec<crate::portal::Shortcut> = self
                .hotkey_ids()
                .into_iter()
                .map(|id| crate::portal::Shortcut {
                    description: self.hotkey_label(&id).to_string(),
                    trigger: c.hotkeys.get(&id).and_then(|t| hotkey::portal_trigger(t)).unwrap_or_default(),
                    id,
                })
                .collect();
            self.shell.set_portal_shortcuts(shortcuts);
        }
    }

    fn update_tray_menu(&self) {
        let s = self.s();
        self.shell.set_menu(MenuModel { tip: s.get_tray_tip().into(), open: s.get_tray_open().into(), quit: s.get_tray_quit().into() });
    }

    // ------------------------------------------------------------ color filter (saturation etc.)
    fn sync_fx_to_ui(&self) {
        let f = self.fx.get();
        let ui = &self.ui;
        ui.set_saturation(f.saturation as i32);
        ui.set_fx_mode(f.mode.index());
        ui.set_fx_r(f.r as i32);
        ui.set_fx_g(f.g as i32);
        ui.set_fx_b(f.b as i32);
        ui.set_fx_temp(100 - f.temp as i32); // left = cool, right = warm
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_saturation(f.saturation as i32);
            p.set_fx_mode(f.mode.index());
            p.set_fx_temp(100 - f.temp as i32);
        }
    }

    fn apply_fx(&self, report: bool) {
        let f = self.fx.get();
        {
            let mut c = self.cfg.borrow_mut();
            c.fx_saturation = f.saturation;
            c.fx_mode = f.mode.name().into();
            c.fx_r = f.r;
            c.fx_g = f.g;
            c.fx_b = f.b;
            c.fx_temp = f.temp;
        }
        self.request_save();
        let ok = sysutil::apply_color_matrix(&colorfx::matrix(&f));
        if !ok && report {
            self.say(self.s().get_st_fx_unavailable());
        }
    }

    fn set_fx_value(&self, what: &str, v: i32) {
        if self.locked() {
            self.sync_fx_to_ui();
            return;
        }
        let v = v.clamp(0, 100) as u8;
        let linked = self.cfg.borrow().link_fx;
        let mut f = self.fx.get();
        match what {
            "saturation" => f.saturation = v,
            "r" | "g" | "b" if linked => {
                f.r = v;
                f.g = v;
                f.b = v;
            }
            "r" => f.r = v,
            "g" => f.g = v,
            "b" => f.b = v,
            "temp" => f.temp = 100 - v,
            _ => return,
        }
        self.fx.set(f);
        self.apply_fx(true);
        self.sync_fx_to_ui();
        self.touch_profile();
    }

    fn set_fx_mode(&self, i: i32) {
        if self.locked() {
            self.sync_fx_to_ui();
            return;
        }
        let mut f = self.fx.get();
        f.mode = Mode::from_index(i);
        self.fx.set(f);
        self.apply_fx(true);
        self.sync_fx_to_ui();
        self.touch_profile();
    }

    /// Restore from the "Color filter" card: only the filter (saturation stays as it is).
    fn restore_filter(&self) {
        if self.locked() {
            return;
        }
        let cur = self.fx.get();
        self.fx.set(ColorFx { saturation: cur.saturation, temp: cur.temp, ..ColorFx::default() });
        self.apply_fx(true);
        self.sync_fx_to_ui();
        self.touch_profile();
    }

    /// Restore from "Configure RGB": only the three monitor color gains.
    fn restore_rgb(&self) {
        if self.locked() {
            return;
        }
        for code in [vcp::GAIN_RED, vcp::GAIN_GREEN, vcp::GAIN_BLUE] {
            let _ = self.tx.send(Cmd::Level { code, pct: 100 });
        }
        self.ui.set_red(100);
        self.ui.set_green(100);
        self.ui.set_blue(100);
        self.touch_profile();
    }

    /// Restore from "Color temperature": only the preset (6500 K, or sRGB if there is none).
    fn restore_preset(&self) {
        if self.locked() {
            return;
        }
        let list = self.ui.get_presets();
        let has = |v: i32| list.iter().any(|c| c.value == v);
        let target = if has(0x05) {
            0x05
        } else if has(0x01) {
            0x01
        } else {
            match list.row_data(0) {
                Some(c) => c.value,
                None => return,
            }
        };
        self.ui.set_preset_value(target);
        self.last_user.set(Instant::now());
        let _ = self.tx.send(Cmd::SetRaw { code: vcp::COLOR_PRESET, value: target as u32 });
        self.touch_profile();
    }

    /// Restore from the black levels: goes back to the value read when the monitor first appeared.
    fn restore_black(&self) {
        if self.locked() {
            return;
        }
        self.last_user.set(Instant::now());
        let key = self.monitors.borrow().get(self.selected.get()).map(|m| m.key.clone());
        let init = key.and_then(|k| self.black_init.borrow().get(&k).copied()).unwrap_or((50, 50, 50));
        for (code, v) in [(vcp::BLACK_RED, init.0), (vcp::BLACK_GREEN, init.1), (vcp::BLACK_BLUE, init.2)] {
            let _ = self.tx.send(Cmd::Level { code, pct: v as u8 });
        }
        self.ui.set_black_r(init.0);
        self.ui.set_black_g(init.1);
        self.ui.set_black_b(init.2);
        self.touch_profile();
    }

    /// Monitor factory defaults (VCP 0x04).
    fn factory_reset(&self) {
        if self.locked() {
            return;
        }
        let _ = self.tx.send(Cmd::Reset(0));
        self.poll_soon();
    }

    /// Restore from "Controls": monitor brightness/contrast and saturation.
    fn restore_controls(&self) {
        if self.locked() {
            return;
        }
        let _ = self.tx.send(Cmd::Reset(1));
        self.poll_soon();
        let mut f = self.fx.get();
        f.saturation = 50;
        f.temp = 50;
        self.fx.set(f);
        self.apply_fx(true);
        self.sync_fx_to_ui();
        self.touch_profile();
    }

    // ------------------------------------------------------------ status
    fn set_status(&self, st: &Status) {
        let s = self.s();
        let text: SharedString = match st {
            Status::Ready => s.get_st_ready(),
            Status::Reading => s.get_st_reading(),
            Status::NoMonitors => s.get_st_no_monitors(),
            Status::Found(n) => fmt1(s.get_st_found(), &n.to_string()),
            Status::SetFailed(what) => fmt1(s.get_st_set_failed(), what),
            Status::ReadFailed(what) => fmt1(s.get_st_read_failed(), what),
            Status::CapsMissing => s.get_st_caps_missing(),
            Status::ResetDone => s.get_st_reset_done(),
            Status::PowerSent => s.get_st_power_sent(),
            Status::Wrote => s.get_st_wrote(),
            Status::Ignored(what) => fmt1(s.get_st_ignored(), what),
        };
        let text = if self.demo && matches!(st, Status::Ready) { s.get_st_demo() } else { text };
        self.ui.set_status(text);
    }

    fn say(&self, text: SharedString) {
        self.ui.set_status(text);
    }

    // ------------------------------------------------------------ worker events
    fn on_event(self: &Rc<Self>, ev: Event) {
        match ev {
            Event::Monitors { infos, selected } => {
                *self.monitors.borrow_mut() = infos;
                self.selected.set(selected);
                self.refresh_monitor_list();
                if let Some(m) = self.monitors.borrow().get(selected) {
                    self.cfg.borrow_mut().selected_monitor = Some(m.key.clone());
                }
            }
            Event::View(v) => {
                // a read that arrives right after the user touched a control is stale: ignore it,
                // so the sliders are not pulled back
                if self.last_user.get().elapsed() < Duration::from_secs(3) && !self.startup_apply.get() {
                    return;
                }
                self.apply_view(&v);
                if self.startup_apply.replace(false) && self.cfg.borrow().apply_on_start {
                    self.apply_current_schedule();
                }
            }
            Event::Status(st) => {
                #[cfg(target_os = "linux")]
                match st {
                    worker::Status::NoMonitors => self.ui.set_access_problem(crate::access::check() != crate::access::Problem::None),
                    worker::Status::Found(_) => self.ui.set_access_problem(crate::access::check() != crate::access::Problem::None),
                    _ => {}
                }
                self.set_status(&st)
            }
            Event::Busy(b) => self.ui.set_busy(b),
            Event::ProfileCaptured(p) => self.on_captured(p),
            Event::Applied(name) => {
                if !name.starts_with('\u{0}') {
                    self.say(fmt1(self.s().get_st_profile_applied(), &name));
                }
            }
            Event::VcpRows { rows, caps_raw } => self.apply_rows(rows, caps_raw),
            Event::Info(i) => self.apply_info(i),
            Event::Levels(map) => {
                self.cfg.borrow_mut().levels = map;
                self.dirty.set(true);
            }
            Event::Osd { kind, pct, monitor } => {
                if matches!(kind, OsdKind::Brightness | OsdKind::Contrast) {
                    self.touch_profile();
                }
                self.show_osd(kind, pct, &monitor, false);
            }
        }
    }

    fn apply_view(&self, v: &MonView) {
        let ui = &self.ui;
        ui.set_has_brightness(v.has_brightness);
        ui.set_has_contrast(v.has_contrast);
        ui.set_has_rgb(v.has_rgb);
        ui.set_has_volume(v.has_volume);
        ui.set_has_black(v.has_black);
        ui.set_volume(v.volume as i32);
        ui.set_muted(v.muted);
        ui.set_black_r(v.black_r as i32);
        ui.set_black_g(v.black_g as i32);
        ui.set_black_b(v.black_b as i32);
        if v.has_black {
            if let Some(m) = self.monitors.borrow().get(self.selected.get()) {
                self.black_init.borrow_mut().entry(m.key.clone()).or_insert((v.black_r as i32, v.black_g as i32, v.black_b as i32));
            }
        }
        ui.set_has_presets(v.has_presets);
        ui.set_has_inputs(v.has_inputs);
        ui.set_has_power(v.has_power);
        ui.set_is_internal(v.internal);
        ui.set_brightness(v.brightness as i32);
        ui.set_contrast(v.contrast as i32);
        ui.set_red(v.red as i32);
        ui.set_green(v.green as i32);
        ui.set_blue(v.blue as i32);
        let choice = |list: &[(u16, String, bool)]| -> Vec<ChoiceItem> {
            list.iter()
                .map(|(val, label, dim)| ChoiceItem { label: label.clone().into(), value: *val as i32, dim: *dim })
                .collect()
        };
        ui.set_inputs(model(choice(&v.inputs)));
        ui.set_input_value(v.input_value);
        ui.set_presets(model(choice(&v.presets)));
        ui.set_preset_value(v.preset_value);
        let usage = match v.usage_hours {
            Some(h) => fmt1(self.s().get_usage_hours(), &self.format_usage(h as u64)),
            None => SharedString::new(),
        };
        ui.set_usage_text(usage);
        ui.set_caps_text(v.caps_raw.clone().into());
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_has_brightness(v.has_brightness);
            p.set_has_contrast(v.has_contrast);
            p.set_has_power(v.has_power);
            p.set_brightness(v.brightness as i32);
            p.set_contrast(v.contrast as i32);
            p.set_volume(v.volume as i32);
            p.set_muted(v.muted);
            p.set_show_audio(self.cfg.borrow().mini_audio && v.has_volume);
        }
    }

    fn apply_info(&self, i: MonInfo) {
        let s = self.s();
        let na = s.get_info_na();
        let ver = |v: u32| format!("{}.{}", (v >> 8) & 0xFF, v & 0xFF);
        let tech = |v: u32| -> String {
            match v {
                1 => "CRT (shadow mask)".into(),
                2 => "CRT (aperture grille)".into(),
                3 => "TFT".into(),
                4 => "LCD".into(),
                5 => "LCoS".into(),
                6 => "Plasma".into(),
                7 => "OLED".into(),
                8 => "EL".into(),
                9 => "MEMS".into(),
                10 => "MEMS (static)".into(),
                x => format!("0x{x:02X}"),
            }
        };
        let rows = vec![
            (s.get_info_hfreq(), i.hfreq.map(|x| format!("{:.1} kHz", x as f64 / 1000.0)).map(SharedString::from).unwrap_or(na.clone())),
            (s.get_info_vfreq(), i.vfreq.map(|x| format!("{:.1} Hz", x as f64 / 100.0)).map(SharedString::from).unwrap_or(na.clone())),
            (s.get_info_tech(), i.tech.map(|x| SharedString::from(tech(x))).unwrap_or(na.clone())),
            (s.get_info_controller(), i.controller.map(|x| SharedString::from(format!("0x{x:04X}"))).unwrap_or(na.clone())),
            (s.get_info_firmware(), i.firmware.map(|x| SharedString::from(ver(x))).unwrap_or(na.clone())),
            (s.get_info_vcp(), i.vcp_version.map(|x| SharedString::from(ver(x))).unwrap_or(na.clone())),
        ];
        self.ui.set_info_rows(model(rows.into_iter().map(|(label, value)| InfoItem { label, value }).collect()));
    }

    fn format_usage(&self, hours: u64) -> String {
        let s = self.s();
        util::format_duration(
            hours,
            [
                s.get_u_year_one().as_str(),
                s.get_u_day_one().as_str(),
                s.get_u_hour_one().as_str(),
            ],
            [
                s.get_u_year_many().as_str(),
                s.get_u_day_many().as_str(),
                s.get_u_hour_many().as_str(),
            ],
        )
    }

    fn apply_rows(&self, rows: Vec<VcpRow>, caps_raw: String) {
        let items: Vec<VcpRowItem> = rows
            .into_iter()
            .map(|r| VcpRowItem {
                code: format!("0x{:02X}", r.code).into(),
                name: vcp::code_name(r.code).into(),
                current: r.current.map(|v| v.to_string()).unwrap_or_else(|| "—".into()).into(),
                max: r.max.map(|v| v.to_string()).unwrap_or_else(|| "—".into()).into(),
                values: if r.values.is_empty() {
                    SharedString::new()
                } else {
                    r.values.iter().map(|v| format!("{v:02X}")).collect::<Vec<_>>().join(" ").into()
                },
            })
            .collect();
        self.ui.set_vcp_rows(model(items));
        self.ui.set_caps_text(caps_raw.into());
    }

    // ------------------------------------------------------------ OSD (shortcut indicator)
    /// The indicator is created on each use and destroyed after the exit animation: that way the window
    /// is born already marked as "not in the taskbar" and takes no memory at rest.
    fn show_osd(self: &Rc<Self>, kind: OsdKind, pct: u8, monitor: &str, force: bool) {
        if !force && !self.cfg.borrow().show_osd {
            return;
        }
        let heading = {
            let s = self.s();
            match kind {
                OsdKind::Brightness => s.get_osd_brightness(),
                OsdKind::Contrast => s.get_osd_contrast(),
                OsdKind::Volume => s.get_osd_volume(),
                OsdKind::Muted => s.get_osd_muted(),
                OsdKind::Saturation => s.get_osd_saturation(),
                OsdKind::Temperature => s.get_mini_temp(),
            }
        };
        // Linux/Wayland: frameless windows cannot be positioned, so there is no on-screen indicator
        if cfg!(target_os = "linux") {
            let _ = (monitor, heading, pct);
            return;
        }
        self.osd_drop.stop();
        let fresh = self.osd.borrow().is_none();
        if fresh {
            KIND.with(|k| k.set(2));
            let r = OsdWindow::new();
            KIND.with(|k| k.set(0));
            let Ok(o) = r else { return };
            self.style_theme(o.global::<Theme>());
            *self.osd.borrow_mut() = Some(o);
        }
        {
            let guard = self.osd.borrow();
            let Some(o) = guard.as_ref() else { return };
            let c = self.cfg.borrow();
            o.set_heading(heading);
            o.set_subtitle(monitor.into());
            o.set_value(pct as i32);
            o.set_kind(kind as i32);
            o.set_k(c.osd_scale as f32 / 100.0);
            o.set_alpha(alpha_for(c.transparent));
            drop(c);
            self.place_osd(o);
            if fresh {
                o.set_phase(0);
                let _ = o.show();
                update_screen(o.window());
                if let Some(h) = hwnd_of(o.window()) {
                    sysutil::make_noactivate(h);
                }
                self.place_osd(o); // now with the screen's real scale factor
                let w = o.as_weak();
                Timer::single_shot(Duration::from_millis(30), move || {
                    if let Some(o) = w.upgrade() {
                        o.set_phase(1);
                    }
                });
            } else {
                o.set_phase(1);
            }
        }
        self.osd_timer.start(TimerMode::SingleShot, Duration::from_millis(1400), || {
            with_app(|app| app.hide_osd());
        });
    }

    fn hide_osd(&self) {
        if let Some(o) = self.osd.borrow().as_ref() {
            o.set_phase(2); // exit animation
        }
        self.osd_drop.start(TimerMode::SingleShot, Duration::from_millis(230), || {
            with_app(|app| {
                if let Some(o) = app.osd.borrow_mut().take() {
                    let _ = o.hide();
                }
                app.schedule_trim();
            });
        });
    }

    fn place_osd(&self, o: &OsdWindow) {
        let c = self.cfg.borrow();
        let scale = o.window().scale_factor();
        let k = c.osd_scale as f32 / 100.0;
        let w = (270.0 * k * scale) as i32;
        let h = (86.0 * k * scale) as i32;
        let (cx, cy) = sysutil::cursor_pos();
        let (l, t, r, b) = sysutil::work_area_at(cx, cy);
        let m = (24.0 * scale) as i32;
        let low = (70.0 * scale) as i32;
        let mid_x = l + (r - l - w) / 2;
        let (x, y) = match c.osd_pos {
            0 => (l + m, t + m),
            1 => (mid_x, t + m),
            2 => (r - w - m, t + m),
            3 => (r - w - m, b - h - m),
            4 => (mid_x, b - h - low),
            5 => (l + m, b - h - m),
            _ => (mid_x, t + (b - t - h) / 2),
        };
        o.window().set_position(slint::PhysicalPosition::new(x, y));
    }

    // ------------------------------------------------------------ tray: mini panel
    fn ensure_popup(self: &Rc<Self>) -> bool {
        if self.popup.borrow().is_some() {
            return true;
        }
        KIND.with(|k| k.set(1));
        let r = TrayPopup::new();
        KIND.with(|k| k.set(0));
        let Ok(p) = r else { return false };
        self.style_theme(p.global::<Theme>());
        p.set_profiles(ModelRc::from(self.mini_profiles.clone()));
        p.set_schedule(ModelRc::from(self.sched_model.clone()));
        p.set_clock_12h(self.cfg.borrow().clock_12h);
        let weak = Rc::downgrade(self);
        macro_rules! app {
            () => {{
                let w = weak.clone();
                move || w.upgrade()
            }};
        }
        let a = app!();
        p.on_dismissed(move || {
            if let Some(app) = a() {
                app.close_popup();
            }
        });
        let a = app!();
        p.on_open(move || {
            if let Some(app) = a() {
                app.close_popup();
                app.ui.set_tab(7);
                app.show_main();
            }
        });
        let a = app!();
        p.on_quit(move || {
            if let Some(app) = a() {
                app.quit();
            }
        });
        let a = app!();
        p.on_power(move || {
            if let Some(app) = a() {
                let _ = app.tx.send(Cmd::SetRaw { code: vcp::POWER_MODE, value: 4 });
            }
        });
        let a = app!();
        p.on_page_changed(move || {
            if let Some(app) = a() {
                app.place_popup();
            }
        });
        let a = app!();
        p.on_activate_profile(move |i, on| {
            if let Some(app) = a() {
                app.activate_profile(i, on);
            }
        });
        let a = app!();
        p.on_toggle_schedule(move |i, on| {
            if let Some(app) = a() {
                app.toggle_schedule(i, on);
            }
        });
        let a = app!();
        p.on_toggle_mute(move || {
            if let Some(app) = a() {
                let on = !app.ui.get_muted();
                app.set_mute(on);
            }
        });
        let a = app!();
        p.on_toggle_sync(move |on| {
            if let Some(app) = a() {
                app.set_sync(on);
            }
        });
        let a = app!();
        p.on_select_monitor(move |i| {
            if let Some(app) = a() {
                app.select_monitor(i);
            }
        });
        let a = app!();
        p.on_set_level(move |what, v| {
            if let Some(app) = a() {
                app.set_level(&what, v);
            }
        });
        let a = app!();
        p.on_set_fx(move |what, v| {
            if let Some(app) = a() {
                app.set_fx_value(&what, v);
            }
        });
        let a = app!();
        p.on_set_fx_mode(move |i| {
            if let Some(app) = a() {
                app.set_fx_mode(i);
            }
        });
        *self.popup.borrow_mut() = Some(p);
        self.apply_slider_cfg();
        true
    }

    /// `corner`: opens anchored to the bottom-right corner of the work area (keyboard shortcut);
    /// otherwise, next to the point (cx, cy) of the tray icon.
    fn open_popup(self: &Rc<Self>, page: i32, cx: i32, cy: i32, corner: bool) {
        // a panel that is still leaving is discarded: each opening creates a new window
        self.popup_hide.stop();
        self.popup_closing.set(false);
        if let Some(old) = self.popup.borrow_mut().take() {
            let _ = old.hide();
        }
        if !self.ensure_popup() {
            return;
        }
        {
            let guard = self.popup.borrow();
            let Some(p) = guard.as_ref() else { return };
            let names: Vec<SharedString> = self.monitors.borrow().iter().map(|m| self.monitor_display_name(m)).collect();
            let c = self.cfg.borrow();
            p.set_page(page);
            p.set_monitor_names(model(names));
            p.set_selected(self.selected.get() as i32);
            p.set_has_brightness(self.ui.get_has_brightness());
            p.set_has_contrast(self.ui.get_has_contrast());
            p.set_has_power(self.ui.get_has_power());
            p.set_brightness(self.ui.get_brightness());
            p.set_contrast(self.ui.get_contrast());
            p.set_sync(c.sync_all);
            p.set_alpha(alpha_for(c.transparent));
            p.set_show_audio(c.mini_audio && self.ui.get_has_volume());
            p.set_volume(self.ui.get_volume());
            p.set_muted(self.ui.get_muted());
            p.set_step(c.step as i32);
            p.set_locked(c.active_profile.is_some() && !c.edit_profiles);
            p.set_free_active(c.active_profile.is_none());
            drop(c);
            let f = self.fx.get();
            p.set_saturation(f.saturation as i32);
            p.set_fx_mode(f.mode.index());
            p.set_fx_temp(100 - f.temp as i32);
            p.set_phase(0);
            show_as(1, || p.show());
            // anchor: final horizontal position and the edge (top or bottom) that stays fixed
            let scale = p.window().scale_factor();
            let w = (300.0 * scale) as i32;
            let (l, t, r, b) = sysutil::work_area_at(cx, cy);
            let gap = (8.0 * scale) as i32;
            let (x, y, bottom) = if corner {
                (r - w - gap, b - gap, true)
            } else {
                let x = (cx - w / 2).clamp(l + gap, (r - w - gap).max(l + gap));
                if cy > (t + b) / 2 { (x, b - gap, true) } else { (x, t + gap, false) }
            };
            self.popup_anchor.set((x, y, bottom));
        }
        self.place_popup();
        if let Some(hw) = self.popup.borrow().as_ref().and_then(|p| hwnd_of(p.window())) {
            sysutil::set_foreground(hw);
        }
        // entry animation: the window is already positioned, only then does it appear
        let pw = self.popup.borrow().as_ref().map(|p| p.as_weak());
        if let Some(pw) = pw {
            Timer::single_shot(Duration::from_millis(30), move || {
                if let Some(p) = pw.upgrade() {
                    p.set_phase(1);
                    p.invoke_focus_keys();
                    if let Some(hw) = hwnd_of(p.window()) {
                        sysutil::set_foreground(hw); // second attempt: the first can race with the window being shown
                    }
                }
            });
        }
        self.popup_shown.set(Some(Instant::now()));
        // close on click outside or on losing focus. Focus only counts once it has been ours: Windows
        // sometimes does not hand the foreground to the panel (e.g. with the main window open), and closing
        // because of that made the panel appear and vanish in a loop.
        let _ = self.tx.send(Cmd::Poll);
        self.popup_had_focus.set(false);
        self.popup_mouse.set(sysutil::mouse_down());
        let weak = Rc::downgrade(self);
        self.popup_timer.start(TimerMode::Repeated, Duration::from_millis(60), move || {
            let Some(app) = weak.upgrade() else { return };
            if app.popup_closing.get() {
                return;
            }
            let (own, rect) = {
                let g = app.popup.borrow();
                let Some(p) = g.as_ref() else { return };
                let pos = p.window().position();
                let size = p.window().size();
                (hwnd_of(p.window()), (pos.x, pos.y, pos.x + size.width as i32, pos.y + size.height as i32))
            };
            let down = sysutil::mouse_down();
            let pressed = down && !app.popup_mouse.replace(down);
            if !down {
                app.popup_mouse.set(false);
            }
            let grace = app.popup_shown.get().map(|t| t.elapsed() < Duration::from_millis(400)).unwrap_or(false);
            if grace {
                return;
            }
            if let Some(h) = own {
                let fg = sysutil::foreground_window();
                if fg == h {
                    app.popup_had_focus.set(true);
                } else if app.popup_had_focus.get() {
                    app.close_popup();
                    return;
                }
            }
            if pressed {
                let (cx, cy) = sysutil::cursor_pos();
                if cx < rect.0 || cx >= rect.2 || cy < rect.1 || cy >= rect.3 {
                    app.close_popup();
                }
            }
        });
    }

    /// Positions the panel according to the anchor and the current height (which changes between pages).
    fn place_popup(&self) {
        let guard = self.popup.borrow();
        let Some(p) = guard.as_ref() else { return };
        let scale = p.window().scale_factor();
        let h = (p.get_panel_h() * scale).round() as i32;
        let (x, y, bottom) = self.popup_anchor.get();
        let y = if bottom { y - h } else { y };
        p.window().set_position(slint::PhysicalPosition::new(x, y.max(0)));
    }

    fn popup_visible(&self) -> bool {
        self.popup.borrow().as_ref().map(|p| p.window().is_visible()).unwrap_or(false) && !self.popup_closing.get()
    }

    fn toggle_popup(self: &Rc<Self>, x: i32, y: i32, corner: bool) {
        if self.popup_visible() {
            self.close_popup();
        } else if self.last_close.get().map(|t| t.elapsed() < Duration::from_millis(350)).unwrap_or(false) {
            // the click that took focus away from the panel must not reopen it
        } else {
            self.open_popup(0, x, y, corner);
        }
    }

    fn close_popup(self: &Rc<Self>) {
        if self.popup_closing.get() || !self.popup_visible() {
            return;
        }
        self.popup_timer.stop();
        self.popup_closing.set(true);
        self.last_close.set(Some(Instant::now()));
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_phase(2); // exit animation
        }
        let weak = Rc::downgrade(self);
        self.popup_hide.start(TimerMode::SingleShot, Duration::from_millis(230), move || {
            if let Some(app) = weak.upgrade() {
                // the window is destroyed (not just hidden): the next opening creates a new one
                if let Some(p) = app.popup.borrow_mut().take() {
                    let _ = p.hide();
                }
                app.popup_closing.set(false);
                app.schedule_trim();
            }
        });
    }

    // ------------------------------------------------------------ shared actions
    fn select_monitor(&self, i: i32) {
        self.selected.set(i.max(0) as usize);
        self.ui.set_selected(i);
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_selected(i);
        }
        if let Some(m) = self.monitors.borrow().get(i.max(0) as usize) {
            self.cfg.borrow_mut().selected_monitor = Some(m.key.clone());
        }
        self.last_user.set(Instant::now().checked_sub(Duration::from_secs(60)).unwrap_or_else(Instant::now));
        let _ = self.tx.send(Cmd::Select(i.max(0) as usize));
        if self.ui.get_tab() == 5 {
            let _ = self.tx.send(Cmd::ReadInfo);
        }
    }

    fn set_level(&self, what: &str, v: i32) {
        if what == "volume" {
            // the monitor audio is not part of profiles: it is not locked
            let v = v.clamp(0, 100);
            self.ui.set_volume(v);
            self.last_user.set(Instant::now());
            let _ = self.tx.send(Cmd::Level { code: vcp::VOLUME, pct: v as u8 });
            return;
        }
        if self.locked() {
            return;
        }
        let v = v.clamp(0, 100);
        // hardware sliders must also pause stale re-reads, or they jump back while being dragged
        self.last_user.set(Instant::now());
        let code = match what {
            "brightness" => vcp::BRIGHTNESS,
            "contrast" => vcp::CONTRAST,
            "red" => vcp::GAIN_RED,
            "green" => vcp::GAIN_GREEN,
            "volume" => vcp::VOLUME,
            "black_red" => vcp::BLACK_RED,
            "black_green" => vcp::BLACK_GREEN,
            "black_blue" => vcp::BLACK_BLUE,
            _ => vcp::GAIN_BLUE,
        };
        match what {
            "brightness" => {
                self.ui.set_brightness(v);
            }
            "contrast" => self.ui.set_contrast(v),
            _ => {}
        }
        let _ = self.tx.send(Cmd::Level { code, pct: v as u8 });
        self.touch_profile();
    }

    fn set_mute(&self, on: bool) {
        self.ui.set_muted(on);
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_muted(on);
        }
        let _ = self.tx.send(Cmd::SetMute(on));
    }

    fn set_sync(&self, on: bool) {
        {
            let mut c = self.cfg.borrow_mut();
            c.sync_all = on;
            c.save();
        }
        self.ui.set_opt_sync(on);
        if let Some(p) = self.popup.borrow().as_ref() {
            p.set_sync(on);
        }
        let _ = self.tx.send(Cmd::Sync(on));
    }

    // ------------------------------------------------------------ profiles and schedule
    /// With "allow editing profiles" on, every change is written to the active profile.
    fn touch_profile(&self) {
        let name = {
            let c = self.cfg.borrow();
            if !c.edit_profiles {
                return;
            }
            match &c.active_profile {
                Some(n) => n.clone(),
                None => return,
            }
        };
        let tx = self.tx.clone();
        self.autosave.start(TimerMode::SingleShot, Duration::from_millis(700), move || {
            let _ = tx.send(Cmd::CaptureProfile { name: name.clone(), include_input: false });
        });
    }

    fn on_captured(&self, p: Profile) {
        if p.name == FREE_NAME {
            let mut free = p;
            free.name = "Free".into();
            free.fx = self.free_fx.borrow_mut().take();
            let mut c = self.cfg.borrow_mut();
            c.free = Some(free);
            c.save();
            return;
        }
        self.store_profile(p);
    }

    fn store_profile(&self, mut p: Profile) {
        p.fx = cfg!(windows).then(|| fx_to_values(&self.fx.get()));
        let name = p.name.clone();
        let existed;
        {
            let mut c = self.cfg.borrow_mut();
            if let Some(slot) = c.profiles.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)) {
                // keep the original capitalization of the name
                p.name = slot.name.clone();
                *slot = p;
                existed = true;
            } else {
                c.profiles.push(p);
                existed = false;
            }
            c.save();
        }
        self.refresh_profiles();
        self.refresh_hotkeys();
        self.update_tray_menu();
        self.push_shell_hotkeys();
        let s = self.s();
        self.say(fmt1(if existed { s.get_st_profile_exists() } else { s.get_st_profile_saved() }, &name));
    }

    /// Applies a profile's values (monitor + software filter).
    fn apply_profile_obj(&self, p: Profile) {
        if let Some(f) = &p.fx {
            self.fx.set(fx_from_values(f));
            self.apply_fx(true);
            self.sync_fx_to_ui();
        }
        let _ = self.tx.send(Cmd::ApplyProfile(p));
    }

    fn apply_profile_by_name(&self, name: &str) -> bool {
        let p = self.cfg.borrow().profile(name).cloned();
        match p {
            Some(p) => {
                self.apply_profile_obj(p);
                true
            }
            None => false,
        }
    }

    /// Activates a saved profile (settings stay locked until going back to "Free").
    fn enter_profile(&self, name: &str) {
        let Some(real) = self.cfg.borrow().profile(name).map(|p| p.name.clone()) else { return };
        if self.cfg.borrow().active_profile.is_none() {
            // save the Free mode state so we can go back to it
            *self.free_fx.borrow_mut() = Some(fx_to_values(&self.fx.get()));
            let _ = self.tx.send(Cmd::CaptureProfile { name: FREE_NAME.into(), include_input: false });
        }
        {
            let mut c = self.cfg.borrow_mut();
            c.active_profile = Some(real.clone());
            c.save();
        }
        self.apply_profile_by_name(&real);
        self.refresh_profiles();
        self.sync_lock();
    }

    /// Goes back to Free mode and restores the state from before the first profile.
    fn go_free(&self) {
        let free = {
            let mut c = self.cfg.borrow_mut();
            if c.active_profile.is_none() {
                return;
            }
            c.active_profile = None;
            c.save();
            c.free.clone()
        };
        if let Some(f) = free {
            self.apply_profile_obj(f);
        }
        self.refresh_profiles();
        self.sync_lock();
    }

    /// `idx < 0` is the Free profile.
    fn activate_profile(&self, idx: i32, on: bool) {
        if idx < 0 {
            if on {
                self.go_free();
            }
        } else {
            let name = self.cfg.borrow().profiles.get(idx as usize).map(|p| p.name.clone());
            if let Some(n) = name {
                if on {
                    self.enter_profile(&n);
                } else {
                    self.go_free();
                }
            }
        }
        self.refresh_profiles(); // re-assert the switches' state (the click does not change them by itself)
    }

    fn apply_current_schedule(&self) {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        let minute = now.hour() * 60 + now.minute();
        let wd = now.weekday().num_days_from_monday();
        let name = schedule::current(&self.cfg.borrow().schedule, minute, wd).map(|e| e.profile.clone());
        if let Some(n) = name {
            self.enter_profile(&n);
        }
    }

    fn tick(&self) {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        let abs = schedule::abs_minutes(now.num_days_from_ce() as i64, now.hour() * 60 + now.minute());
        let last = self.last_tick.replace(abs);
        if last != 0 {
            let ended = schedule::ended(&self.cfg.borrow().schedule, last, abs).map(|e| e.profile.clone());
            let fired = schedule::fired(&self.cfg.borrow().schedule, last, abs).map(|e| (e.profile.clone(), e.end.is_some()));
            if let Some((n, has_end)) = fired {
                if self.cfg.borrow().profile(&n).is_some() {
                    if has_end {
                        let before = self.cfg.borrow().active_profile.clone();
                        *self.sched_before.borrow_mut() = Some(before);
                    }
                    self.enter_profile(&n);
                    self.say(fmt1(self.s().get_st_schedule(), &n));
                }
            } else if let Some(n) = ended {
                let still = self.cfg.borrow().active_profile.as_deref().map(|a| a.eq_ignore_ascii_case(&n)).unwrap_or(false);
                let before = self.sched_before.borrow_mut().take();
                if still {
                    match before {
                        Some(Some(p)) if self.cfg.borrow().profile(&p).is_some() => self.enter_profile(&p),
                        _ => self.go_free(),
                    }
                }
            }
        }
        if self.cfg.borrow().theme == "system" && sysutil::system_dark() != self.dark.get() {
            self.apply_theme();
        }
        if self.dirty.replace(false) {
            self.cfg.borrow().save();
        }
    }

    fn toggle_schedule(&self, i: i32, on: bool) {
        {
            let mut c = self.cfg.borrow_mut();
            if let Some(e) = c.schedule.get_mut(i as usize) {
                e.enabled = on;
            }
            c.save();
        }
        self.refresh_schedule();
    }

    // ------------------------------------------------------------ shortcut capture (Windows)
    /// Capture reads the keyboard directly from Windows (GetAsyncKeyState), because Slint's
    /// keyboard events do not carry letters with Ctrl/Alt, Tab, AltGr or the Win key.
    fn start_capture(self: &Rc<Self>, idx: i32) {
        if self.cap_idx.get() == idx {
            return;
        }
        self.cap_idx.set(idx);
        self.ui.set_capture_idx(idx);
        self.ui.set_capture_live(SharedString::new());
        self.shell.set_hotkeys(Vec::new()); // current shortcuts must not fire during capture
        if !cfg!(windows) {
            return; // outside Windows the Slint keyboard event is used
        }
        let weak = Rc::downgrade(self);
        self.cap_timer.start(TimerMode::Repeated, Duration::from_millis(25), move || {
            let Some(app) = weak.upgrade() else { return };
            let (mods, down) = sysutil::keys_snapshot();
            match hotkey::decide(mods, &down) {
                hotkey::Capture::Pending(m) => app.ui.set_capture_live(mods_text(m).into()),
                hotkey::Capture::Done(c) => app.finish_capture(Some(hotkey::format(c))),
                hotkey::Capture::Clear => app.finish_capture(Some(String::new())),
                hotkey::Capture::Cancel => app.finish_capture(None),
            }
        });
    }

    fn finish_capture(&self, combo: Option<String>) {
        let idx = self.cap_idx.replace(-1);
        self.cap_timer.stop();
        self.ui.set_capture_idx(-1);
        self.ui.set_capture_live(SharedString::new());
        if let Some(combo) = combo {
            let ids = self.hotkey_ids();
            if let Some(id) = ids.get(idx.max(0) as usize).cloned() {
                let mut c = self.cfg.borrow_mut();
                if !combo.is_empty() {
                    for (k, v) in c.hotkeys.iter_mut() {
                        if *k != id && *v == combo {
                            v.clear(); // shortcuts are unique: remove it from the other command
                        }
                    }
                }
                c.hotkeys.insert(id, combo);
                c.save();
            }
        }
        self.refresh_hotkeys();
        self.push_shell_hotkeys();
    }

    // ------------------------------------------------------------ shell events
    fn on_shell(self: &Rc<Self>, ev: ShellEvent) {
        let step = self.cfg.borrow().step as i32;
        let adjust = |code: u8, delta: i32, osd: OsdKind| {
            if self.locked() {
                self.say(self.s().get_st_locked());
                return;
            }
            let _ = self.tx.send(Cmd::Adjust { code, delta, osd });
        };
        match ev {
            ShellEvent::Hotkey(id) => match id.as_str() {
                "brightness_up" => adjust(vcp::BRIGHTNESS, step, OsdKind::Brightness),
                "brightness_down" => adjust(vcp::BRIGHTNESS, -step, OsdKind::Brightness),
                "contrast_up" => adjust(vcp::CONTRAST, step, OsdKind::Contrast),
                "contrast_down" => adjust(vcp::CONTRAST, -step, OsdKind::Contrast),
                "volume_up" | "volume_down" => {
                    // the monitor audio is not locked by profiles
                    let d = if id == "volume_up" { step } else { -step };
                    let _ = self.tx.send(Cmd::Adjust { code: vcp::VOLUME, delta: d, osd: OsdKind::Volume });
                }
                "volume_mute" => {
                    let _ = self.tx.send(Cmd::ToggleMute);
                }
                "saturation_up" | "saturation_down" => {
                    if self.locked() {
                        self.say(self.s().get_st_locked());
                        return;
                    }
                    let d = if id == "saturation_up" { step } else { -step };
                    let v = (self.fx.get().saturation as i32 + d).clamp(0, 100);
                    self.set_fx_value("saturation", v);
                    self.show_osd(OsdKind::Saturation, v as u8, "", false);
                }
                "temperature_up" | "temperature_down" => {
                    if self.locked() {
                        self.say(self.s().get_st_locked());
                        return;
                    }
                    let d = if id == "temperature_up" { step } else { -step };
                    let v = (100 - self.fx.get().temp as i32 + d).clamp(0, 100);
                    self.set_fx_value("temp", v);
                    self.show_osd(OsdKind::Temperature, v as u8, "", false);
                }
                "mini" => {
                    let (x, y) = sysutil::cursor_pos();
                    self.toggle_popup(x, y, true);
                }
                "show" => {
                    if self.ui.window().is_visible() {
                        let _ = self.ui.hide();
                        self.schedule_trim();
                    } else {
                        self.show_main();
                    }
                }
                other => {
                    if let Some(name) = other.strip_prefix("profile:") {
                        self.enter_profile(name);
                    }
                }
            },
            ShellEvent::HotkeyFailed(id) => {
                let combo = self.cfg.borrow().hotkeys.get(&id).cloned().unwrap_or_default();
                self.say(fmt1(self.s().get_st_hotkey_fail(), &combo));
            }
            ShellEvent::TrayClick(x, y) | ShellEvent::TrayMenu(x, y) => {
                // Windows sends two notices for a right click (BUTTONUP and CONTEXTMENU): only the first counts
                let dup = self.last_tray.get().map(|t| t.elapsed() < Duration::from_millis(300)).unwrap_or(false);
                self.last_tray.set(Some(Instant::now()));
                if !dup {
                    self.toggle_popup(x, y, false);
                }
            }
            ShellEvent::HotkeysBound(list) => {
                *self.hk_bound.borrow_mut() = list.into_iter().collect();
                self.ui.set_has_hotkeys(true);
                self.refresh_hotkeys();
            }
            ShellEvent::TrayOpen => self.show_main(),
            ShellEvent::Quit => self.quit(),
            ShellEvent::DisplayState(false) => {
                // asleep: whatever the monitor reports on waking must not replace the remembered levels
                for t in &self.wake_timers {
                    t.stop();
                }
                let _ = self.tx.send(Cmd::Pause);
            }
            ShellEvent::DisplayState(true) | ShellEvent::Resume => {
                if !self.demo {
                    let _ = self.tx.send(Cmd::Pause);
                    // monitors need a few seconds before they answer DDC/CI after waking; try twice
                    for (n, (t, ms)) in self.wake_timers.iter().zip([3000u64, 9000]).enumerate() {
                        let tx = self.tx.clone();
                        let prefer = self.cfg.borrow().selected_monitor.clone();
                        t.start(TimerMode::SingleShot, Duration::from_millis(ms), move || {
                            if n == 0 {
                                let _ = tx.send(Cmd::Refresh { prefer: prefer.clone() });
                            }
                            let _ = tx.send(Cmd::Restore(None));
                        });
                    }
                }
            }
            ShellEvent::DisplayChange => {
                let delay = 1500;
                let tx = self.tx.clone();
                let prefer = self.cfg.borrow().selected_monitor.clone();
                let restore = !self.demo;
                self.debounce.start(TimerMode::SingleShot, Duration::from_millis(delay), move || {
                    let _ = tx.send(Cmd::Refresh { prefer: prefer.clone() });
                    if restore {
                        // monitors often come back from sleep/power-off with factory levels
                        let _ = tx.send(Cmd::Restore(None));
                    }
                });
            }
        }
    }

    // ------------------------------------------------------------ backup
    fn export_backup(&self) {
        let json = match self.cfg.borrow().export_json() {
            Ok(j) => j,
            Err(e) => {
                self.say(fmt1(self.s().get_st_export_failed(), &e));
                return;
            }
        };
        std::thread::spawn(move || {
            let Some(path) = sysutil::save_dialog("solua-backup.json") else { return };
            let r = std::fs::write(&path, json).map_err(|e| e.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                with_app(move |app| match r {
                    Ok(()) => app.say(app.s().get_st_exported()),
                    Err(e) => app.say(fmt1(app.s().get_st_export_failed(), &e)),
                })
            });
        });
    }

    fn import_backup_dialog(&self) {
        std::thread::spawn(|| {
            let Some(path) = sysutil::open_dialog() else { return };
            let r = std::fs::read(&path).map_err(|e| e.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                with_app(move |app| match r {
                    Ok(bytes) => app.import_backup(&bytes),
                    Err(e) => app.say(fmt1(app.s().get_st_import_failed(), &e)),
                })
            });
        });
    }

    fn import_backup(&self, bytes: &[u8]) {
        let new = match Config::from_backup(bytes) {
            Ok(c) => c,
            Err(e) => {
                self.say(fmt1(self.s().get_st_import_failed(), &e));
                return;
            }
        };
        let (np, ns) = (new.profiles.len(), new.schedule.len());
        sysutil::set_autostart(new.start_with_windows);
        let code = new.language.clone();
        *self.cfg.borrow_mut() = new;
        self.cfg.borrow().save();
        lang::apply(lang::resolve(&code));
        self.fx.set(fx_from_cfg(&self.cfg.borrow()));
        self.apply_fx(false);
        self.apply_theme();
        self.apply_slider_cfg();
        self.refresh_all_texts();
        self.sync_settings_to_ui();
        self.sync_fx_to_ui();
        self.sync_lock();
        self.push_shell_hotkeys();
        let _ = self.tx.send(Cmd::Sync(self.cfg.borrow().sync_all));
        if !self.demo {
            // put the monitors at the levels stored in the backup
            let saved = self.cfg.borrow().levels.clone();
            let _ = self.tx.send(Cmd::Restore(Some(saved)));
            self.poll_soon();
        }
        let text = self.s().get_st_imported().replace("%1", &np.to_string()).replace("%2", &ns.to_string());
        self.say(text.into());
    }

    // ------------------------------------------------------------ numeric settings
    fn setting_changed(self: &Rc<Self>, name: &str, v: i32) {
        let mut preview = false;
        {
            let mut c = self.cfg.borrow_mut();
            match name {
                "step" => {
                    c.step = v.clamp(1, 25) as u8;
                    self.ui.set_opt_step(c.step as i32);
                }
                "osd_pos" => {
                    c.osd_pos = v.clamp(0, 6) as u8;
                    self.ui.set_osd_pos(c.osd_pos as i32);
                    preview = true;
                }
                "osd_scale" | "osd_scale_live" => {
                    c.osd_scale = v.clamp(60, 150) as u8;
                    self.ui.set_osd_scale(c.osd_scale as i32);
                    preview = name == "osd_scale";
                }
                _ => {}
            }
            c.save();
        }
        if name == "step" {
            self.apply_slider_cfg();
            if let Some(p) = self.popup.borrow().as_ref() {
                p.set_step(self.cfg.borrow().step as i32);
            }
        }
        if preview {
            // show the indicator with the current options (even if it is disabled for shortcuts)
            self.hide_osd_now();
            self.show_osd(OsdKind::Brightness, self.ui.get_brightness().clamp(0, 100) as u8, "", true);
        }
    }

    fn hide_osd_now(&self) {
        self.osd_drop.stop();
        self.osd_timer.stop();
        if let Some(o) = self.osd.borrow_mut().take() {
            let _ = o.hide();
        }
    }

    // ------------------------------------------------------------ UI callbacks
    fn wire_callbacks(self: &Rc<Self>) {
        let ui = &self.ui;
        macro_rules! app {
            () => {{
                let w = Rc::downgrade(self);
                move || w.upgrade()
            }};
        }

        let a = app!();
        #[cfg(target_os = "linux")]
        ui.on_enable_access(|| {
            std::thread::spawn(|| {
                let ok = crate::access::enable().is_ok();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(move |app| {
                        let s = app.s();
                        app.say(if ok { s.get_st_access_ok() } else { s.get_st_access_failed() });
                        if ok {
                            let prefer = app.cfg.borrow().selected_monitor.clone();
                            let _ = app.tx.send(Cmd::Refresh { prefer });
                        }
                    });
                });
            });
        });
        ui.on_open_link(|url| sysutil::open_url(url.as_str()));
        ui.on_refresh(move || {
            if let Some(app) = a() {
                let prefer = app.cfg.borrow().selected_monitor.clone();
                let _ = app.tx.send(Cmd::Refresh { prefer });
            }
        });

        let a = app!();
        ui.on_select_monitor(move |i| {
            if let Some(app) = a() {
                app.select_monitor(i);
            }
        });

        let a = app!();
        ui.on_set_level(move |what, v| {
            if let Some(app) = a() {
                app.set_level(&what, v);
            }
        });

        let a = app!();
        ui.on_set_fx(move |what, v| {
            if let Some(app) = a() {
                app.set_fx_value(&what, v);
            }
        });

        let a = app!();
        ui.on_set_fx_mode(move |i| {
            if let Some(app) = a() {
                app.set_fx_mode(i);
            }
        });

        let a = app!();
        ui.on_restore_filter(move || {
            if let Some(app) = a() {
                app.restore_filter();
            }
        });
        let a = app!();
        ui.on_restore_controls(move || {
            if let Some(app) = a() {
                app.restore_controls();
            }
        });
        let a = app!();
        ui.on_restore_rgb(move || {
            if let Some(app) = a() {
                app.restore_rgb();
            }
        });
        let a = app!();
        ui.on_restore_black(move || {
            if let Some(app) = a() {
                app.restore_black();
            }
        });
        let a = app!();
        ui.on_set_mute(move |on| {
            if let Some(app) = a() {
                app.set_mute(on);
            }
        });
        let a = app!();
        ui.on_factory_reset(move || {
            if let Some(app) = a() {
                app.factory_reset();
            }
        });
        let a = app!();
        ui.on_request_info(move || {
            if let Some(app) = a() {
                let _ = app.tx.send(Cmd::ReadInfo);
            }
        });
        let a = app!();
        ui.on_restore_preset(move || {
            if let Some(app) = a() {
                app.restore_preset();
            }
        });

        let a = app!();
        ui.on_toggle_new_day(move |d| {
            if let Some(app) = a() {
                let mut v = app.new_days.get();
                if let Some(x) = v.get_mut(d as usize) {
                    *x = !*x;
                }
                app.new_days.set(v);
                app.sync_new_days();
            }
        });

        let a = app!();
        ui.on_toggle_sched_day(move |i, d| {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    if let Some(e) = c.schedule.get_mut(i as usize) {
                        let mask = e.days ^ (1 << d.clamp(0, 6));
                        if mask & schedule::ALL_DAYS != 0 {
                            e.days = mask; // at least one day must stay selected
                        }
                    }
                    c.save();
                }
                app.refresh_schedule();
            }
        });

        let a = app!();
        ui.on_set_input(move |v| {
            if let Some(app) = a() {
                app.ui.set_input_value(v);
                let _ = app.tx.send(Cmd::SetRaw { code: vcp::INPUT_SOURCE, value: v as u32 });
            }
        });

        let a = app!();
        ui.on_set_preset(move |v| {
            if let Some(app) = a() {
                if app.locked() {
                    return;
                }
                app.ui.set_preset_value(v);
                let _ = app.tx.send(Cmd::SetRaw { code: vcp::COLOR_PRESET, value: v as u32 });
                app.touch_profile();
            }
        });

        let a = app!();
        ui.on_set_power(move |v| {
            if let Some(app) = a() {
                let _ = app.tx.send(Cmd::SetRaw { code: vcp::POWER_MODE, value: v as u32 });
            }
        });

        let a = app!();
        ui.on_save_profile(move |name, include_input| {
            if let Some(app) = a() {
                let name = name.trim().to_string();
                if name.is_empty() {
                    app.say(app.s().get_st_name_empty());
                    return;
                }
                app.ui.set_new_profile_name(SharedString::new());
                let _ = app.tx.send(Cmd::CaptureProfile { name, include_input });
            }
        });

        let a = app!();
        ui.on_activate_profile(move |i, on| {
            if let Some(app) = a() {
                app.activate_profile(i, on);
            }
        });

        let a = app!();
        ui.on_delete_profile(move |i| {
            if let Some(app) = a() {
                let was_active;
                {
                    let mut c = app.cfg.borrow_mut();
                    was_active = c.profiles.get(i as usize).map(|p| c.active_profile.as_deref().map(|a| a.eq_ignore_ascii_case(&p.name)).unwrap_or(false)).unwrap_or(false);
                    if (i as usize) < c.profiles.len() {
                        let p = c.profiles.remove(i as usize);
                        c.hotkeys.remove(&format!("profile:{}", p.name));
                        c.schedule.retain(|e| !e.profile.eq_ignore_ascii_case(&p.name));
                        c.save();
                    }
                }
                if was_active {
                    app.go_free();
                }
                app.refresh_profiles();
                app.refresh_schedule();
                app.refresh_hotkeys();
                app.update_tray_menu();
                app.push_shell_hotkeys();
            }
        });

        let a = app!();
        ui.on_reorder_profile(move |from, delta| {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    let n = c.profiles.len() as i32;
                    let to = (from + delta).clamp(0, n - 1);
                    if from < 0 || from >= n || to == from {
                        return;
                    }
                    let p = c.profiles.remove(from as usize);
                    c.profiles.insert(to as usize, p);
                    c.save();
                }
                app.refresh_profiles();
                app.refresh_hotkeys();
            }
        });

        let a = app!();
        ui.on_add_schedule(move |time, idx, end| {
            if let Some(app) = a() {
                let Some(t) = schedule::normalize(&time) else {
                    app.say(app.s().get_st_invalid_time());
                    return;
                };
                let end = if end.trim().is_empty() {
                    None
                } else {
                    match schedule::normalize(&end) {
                        Some(e) => Some(e),
                        None => {
                            app.say(app.s().get_st_invalid_time());
                            return;
                        }
                    }
                };
                let name = app.cfg.borrow().profiles.get(idx.max(0) as usize).map(|p| p.name.clone());
                let Some(name) = name else {
                    app.say(app.s().get_st_need_profile());
                    return;
                };
                {
                    let mut c = app.cfg.borrow_mut();
                    let days = app.new_days.get().iter().enumerate().fold(0u8, |m, (i, on)| if *on { m | (1 << i) } else { m });
                    let days = if days == 0 { schedule::ALL_DAYS } else { days };
                    c.schedule.push(schedule::Entry { time: t, profile: name, enabled: true, days, end });
                    c.save();
                }
                app.refresh_schedule();
            }
        });

        let a = app!();
        ui.on_edit_schedule_end(move |i, text| {
            if let Some(app) = a() {
                let new = if text.trim().is_empty() { Some(None) } else { schedule::normalize(&text).map(Some) };
                match new {
                    Some(end) => {
                        let mut c = app.cfg.borrow_mut();
                        if let Some(e) = c.schedule.get_mut(i as usize) {
                            e.end = end;
                        }
                        c.save();
                    }
                    None => app.say(app.s().get_st_invalid_time()),
                }
                app.refresh_schedule();
            }
        });

        let a = app!();
        ui.on_toggle_schedule(move |i, on| {
            if let Some(app) = a() {
                app.toggle_schedule(i, on);
            }
        });

        let a = app!();
        ui.on_edit_schedule_time(move |i, text| {
            if let Some(app) = a() {
                match schedule::normalize(&text) {
                    Some(t) => {
                        let mut c = app.cfg.borrow_mut();
                        if let Some(e) = c.schedule.get_mut(i as usize) {
                            e.time = t;
                        }
                        c.save();
                    }
                    None => app.say(app.s().get_st_invalid_time()),
                }
                app.refresh_schedule();
            }
        });

        let a = app!();
        ui.on_delete_schedule(move |i| {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    if (i as usize) < c.schedule.len() {
                        c.schedule.remove(i as usize);
                    }
                    c.save();
                }
                app.refresh_schedule();
            }
        });

        let a = app!();
        ui.on_reorder_schedule(move |from, delta| {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    let n = c.schedule.len() as i32;
                    let to = (from + delta).clamp(0, n - 1);
                    if from < 0 || from >= n || to == from {
                        return;
                    }
                    let e = c.schedule.remove(from as usize);
                    c.schedule.insert(to as usize, e);
                    c.save();
                }
                app.refresh_schedule();
            }
        });

        let a = app!();
        ui.on_capture_start(move |i| {
            if let Some(app) = a() {
                app.start_capture(i);
            }
        });
        let a = app!();
        ui.on_capture_end(move |i| {
            if let Some(app) = a() {
                if app.cap_idx.get() == i {
                    app.finish_capture(None);
                }
            }
        });

        let a = app!();
        ui.on_restore_hotkeys(move || {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    c.hotkeys = default_hotkeys();
                    c.save();
                }
                app.refresh_hotkeys();
                app.push_shell_hotkeys();
                app.say(app.s().get_st_restored_hk());
            }
        });

        // Outside Windows (without direct keyboard reading) the Slint keyboard event is used.
        let a = app!();
        ui.on_hotkey_captured(move |idx, text, ctrl, alt, shift, meta| -> bool {
            let Some(app) = a() else { return true };
            if cfg!(windows) {
                return false; // on Windows the capture is done by `start_capture`
            }
            let combo = match text.as_str() {
                "\u{1b}" => {
                    app.finish_capture(None);
                    return true;
                }
                "\u{8}" => String::new(), // Backspace clears
                t => match hotkey::from_slint_event(t, ctrl, alt, shift, meta) {
                    Some(c) => c,
                    None => return false, // only modifiers or an invalid key: keep capturing
                },
            };
            app.cap_idx.set(idx);
            app.finish_capture(Some(combo));
            true
        });

        let a = app!();
        ui.on_read_vcp_table(move || {
            if let Some(app) = a() {
                let _ = app.tx.send(Cmd::ReadVcpTable);
            }
        });

        let a = app!();
        ui.on_write_vcp(move |code, val| {
            if let Some(app) = a() {
                match (vcp::parse_hex_u8(&code), vcp::parse_value(&val)) {
                    (Some(c), Some(v)) => {
                        let _ = app.tx.send(Cmd::WriteVcp { code: c, value: v });
                    }
                    _ => app.say(app.s().get_st_bad_value()),
                }
            }
        });

        let a = app!();
        ui.on_option_changed(move |name, on| {
            let Some(app) = a() else { return };
            {
                let mut c = app.cfg.borrow_mut();
                match name.as_str() {
                    "autostart" => {
                        c.start_with_windows = on;
                        sysutil::set_autostart(on);
                    }
                    "background" => c.background = on,
                    "menu_entry" => {
                        #[cfg(target_os = "linux")]
                        sysutil::set_menu_entry(on);
                    }
                    "minimized" => c.start_minimized = on,
                    "tray" => c.close_to_tray = on,
                    "osd" => c.show_osd = on,
                    "sync" => c.sync_all = on,
                    "apply_on_start" => c.apply_on_start = on,
                    "discrete" => c.discrete = on,
                    "edit_profiles" => c.edit_profiles = on,
                    "mini_audio" => c.mini_audio = on,
                    "link_fx" => c.link_fx = on,
                    "transparent" => c.transparent = on,
                    "clock_12h" => c.clock_12h = on,
                    _ => {}
                }
                c.save();
            }
            match name.as_str() {
                "sync" => app.set_sync(on),
                "discrete" => app.apply_slider_cfg(),
                "edit_profiles" => app.sync_lock(),
                "clock_12h" => {
                    app.ui.set_clock_12h(on);
                    if let Some(p) = app.popup.borrow().as_ref() {
                        p.set_clock_12h(on);
                    }
                    app.refresh_schedule();
                }
                "transparent" => {
                    if let Some(p) = app.popup.borrow().as_ref() {
                        p.set_alpha(alpha_for(on));
                    }
                    // show the indicator with the new look
                    app.hide_osd_now();
                    app.show_osd(OsdKind::Brightness, app.ui.get_brightness().clamp(0, 100) as u8, "", true);
                }
                "mini_audio" => {
                    if let Some(p) = app.popup.borrow().as_ref() {
                        p.set_show_audio(on && app.ui.get_has_volume());
                    }
                }
                _ => {}
            }
        });

        let a = app!();
        ui.on_setting_changed(move |name, v| {
            if let Some(app) = a() {
                app.setting_changed(&name, v);
            }
        });

        let a = app!();
        ui.on_language_changed(move |idx| {
            if let Some(app) = a() {
                let code = if idx <= 0 { "auto".to_string() } else { lang::LANGS[(idx - 1) as usize].0.to_string() };
                app.cfg.borrow_mut().language = code.clone();
                app.cfg.borrow().save();
                lang::apply(lang::resolve(&code));
                app.ui.set_language_index(idx);
                app.refresh_all_texts();
            }
        });

        let a = app!();
        ui.on_theme_changed(move |idx| {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    c.theme = match idx {
                        1 => "light",
                        2 => "dark",
                        3 => "black",
                        _ => "system",
                    }
                    .into();
                    c.save();
                }
                app.ui.set_theme_index(idx);
                app.apply_theme();
            }
        });

        let a = app!();
        ui.on_theme_color_changed(move |idx| {
            if let Some(app) = a() {
                {
                    let mut c = app.cfg.borrow_mut();
                    c.theme_color = match idx { 1 => "violet", 2 => "default", _ => "blue" }.into();
                    if c.theme == "violet" {
                        c.theme = "dark".into();
                    }
                    c.save();
                }
                app.ui.set_theme_color_index(idx);
                app.apply_theme();
            }
        });

        let a = app!();
        ui.on_export_backup(move || {
            if let Some(app) = a() {
                app.export_backup();
            }
        });
        let a = app!();
        ui.on_import_backup(move || {
            if let Some(app) = a() {
                app.import_backup_dialog();
            }
        });
    }
}

/// HWND of the native window (only exists on Windows and while the window is visible).
/// Records the screen size of the monitor the window is on (used to position the indicator on Linux).
fn update_screen(w: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    let info = w.with_winit_window(|win| win.current_monitor().map(|m| (m.position(), m.size())));
    if let Some(Some((p, sz))) = info {
        sysutil::set_screen(p.x, p.y, sz.width as i32, sz.height as i32);
    }
}

fn hwnd_of(w: &slint::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = w.window_handle().window_handle() {
        if let RawWindowHandle::Win32(w) = h.as_raw() {
            return Some(w.hwnd.get());
        }
    }
    None
}
