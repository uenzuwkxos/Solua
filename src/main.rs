// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]
#![cfg_attr(not(windows), allow(dead_code))]

mod app;
mod backend;
mod config;
mod colorfx;
#[cfg(target_os = "linux")]
mod access;
mod core;
#[cfg(target_os = "linux")]
mod dekeys;
mod edid;
mod hotkey;
mod lang;
#[cfg(target_os = "linux")]
mod portal;
mod schedule;
mod shell;
mod sysutil;
mod util;
mod vcp;
mod winapi;
mod worker;

slint::include_modules!();

fn main() {
    // The Wayland app_id ties the window to the solua.desktop file (icon and taskbar grouping).
    #[cfg(target_os = "linux")]
    let _ = slint::set_xdg_app_id("solua");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let demo = args.iter().any(|a| a == "--demo");
    let minimized = args.iter().any(|a| a == "--minimized");
    // `solua --action <id>`: run by the desktop's custom shortcuts (GNOME/XFCE); hands the id to the
    // running instance and exits
    #[cfg(target_os = "linux")]
    if let Some(i) = args.iter().position(|a| a == "--action") {
        if let Some(id) = args.get(i + 1) {
            sysutil::send_command(id);
        }
        return;
    }
    if !demo && !sysutil::single_instance() {
        // already running: ask that instance to show its window
        #[cfg(target_os = "linux")]
        if !minimized {
            sysutil::send_command("show");
        }
        return;
    }
    if let Err(e) = app::run(demo, minimized) {
        eprintln!("Solua: {e}");
        std::process::exit(1);
    }
}
