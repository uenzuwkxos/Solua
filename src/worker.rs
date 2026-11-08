// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker thread: all (slow) DDC/CI calls happen here, never on the UI thread.
//! Consecutive adjustment commands for the same control are merged (only the last value is sent).

use std::collections::BTreeMap;
use crate::backend::MonitorInfo;
use crate::config::Profile;
use crate::core::{Core, MonView, VcpRow};
use crate::vcp;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

#[allow(dead_code)]
#[derive(Debug)]
pub enum Cmd {
    /// `prefer`: key of the monitor to select after re-enumerating.
    Refresh { prefer: Option<String> },
    Select(usize),
    Sync(bool),
    Level { code: u8, pct: u8 },
    SetMute(bool),
    SetRaw { code: u8, value: u32 },
    Reset(u8),
    CaptureProfile { name: String, include_input: bool },
    ApplyProfile(Profile),
    ReadVcpTable,
    WriteVcp { code: u8, value: u32 },
    /// Keyboard shortcut: relative adjustment.
    Adjust { code: u8, delta: i32, osd: OsdKind },
    ToggleMute,
    /// Re-reads the selected monitor's values (follows changes made outside the app).
    Poll,
    ReadInfo,
    AllBrightness(u8),
    /// Puts brightness and contrast back to the last known values (per monitor key). `Some` seeds the
    /// remembered values (startup); `None` reuses the ones tracked so far (after a display change).
    Restore(Option<BTreeMap<String, BTreeMap<u8, u8>>>),
    /// The display is going to sleep: stop remembering levels until the next `Restore`.
    Pause,
    Quit,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdKind {
    Brightness = 0,
    Contrast = 1,
    Volume = 2,
    Muted = 3,
    Saturation = 4,
    Temperature = 5,
}

#[derive(Debug, Clone)]
pub enum Status {
    Ready,
    Reading,
    NoMonitors,
    Found(usize),
    SetFailed(String),
    ReadFailed(String),
    CapsMissing,
    ResetDone,
    PowerSent,
    Wrote,
    /// the monitor accepted the command but nothing changed (e.g. color temperature / input)
    Ignored(String),
}

#[derive(Debug)]
pub enum Event {
    Monitors { infos: Vec<MonitorInfo>, selected: usize },
    View(MonView),
    Status(Status),
    Busy(bool),
    ProfileCaptured(Profile),
    Applied(String),
    Info(crate::core::MonInfo),
    VcpRows { rows: Vec<VcpRow>, caps_raw: String },
    Osd { kind: OsdKind, pct: u8, monitor: String },
    /// Last known (brightness, contrast) of every monitor, by key; saved by the app.
    Levels(BTreeMap<String, BTreeMap<u8, u8>>),
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

fn is_write(c: &Cmd) -> bool {
    matches!(c, Cmd::Level { .. } | Cmd::SetRaw { .. } | Cmd::SetMute(_) | Cmd::Reset(_) | Cmd::ApplyProfile(_) | Cmd::Restore(_))
}

/// Merges queued commands: only the last value of each level matters (`Level`s of different codes
/// commute, so a run of them keeps just the newest per code), and a `Poll` that is followed by a
/// write is dropped because what it reads would already be stale.
fn coalesce(batch: Vec<Cmd>) -> Vec<Cmd> {
    let mut out: Vec<Cmd> = Vec::with_capacity(batch.len());
    for c in batch {
        if let Cmd::Level { code, .. } = &c {
            // look back through the current run of Level commands
            let pos = out.iter().rposition(|o| !matches!(o, Cmd::Level { .. })).map_or(0, |p| p + 1);
            if let Some(k) = out[pos..].iter().position(|o| matches!(o, Cmd::Level { code: pc, .. } if pc == code)) {
                out.remove(pos + k);
            }
        }
        out.push(c);
    }
    let last_write = out.iter().rposition(is_write);
    if let Some(w) = last_write {
        let mut i = 0;
        out.retain(|c| {
            let keep = !(matches!(c, Cmd::Poll) && i < w);
            i += 1;
            keep
        });
    }
    out
}

struct Worker {
    core: Core,
    selected: usize,
    sync: bool,
    emit: Emit,
    /// last known value (percent) of each remembered control, per monitor key
    levels: BTreeMap<String, BTreeMap<u8, u8>>,
    /// false until the startup restore ran, so the values read at launch never overwrite the saved ones
    armed: bool,
    /// when the last write to a monitor finished (a read right after it can still report the old value)
    last_write: std::time::Instant,
}

/// Controls remembered between launches and restored after sleep (in this order).
const REMEMBERED: [u8; 8] = [
    vcp::BRIGHTNESS, vcp::CONTRAST, vcp::GAIN_RED, vcp::GAIN_GREEN, vcp::GAIN_BLUE,
    vcp::BLACK_RED, vcp::BLACK_GREEN, vcp::BLACK_BLUE,
];

pub fn spawn(core: Core, rx: Receiver<Cmd>, emit: Emit) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("solua-worker".into())
        .spawn(move || {
            let mut w = Worker { core, selected: 0, sync: false, emit, levels: BTreeMap::new(), armed: false, last_write: std::time::Instant::now() };
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                batch.extend(rx.try_iter());
                for cmd in coalesce(batch) {
                    let wrote = is_write(&cmd);
                    if !w.handle(cmd) {
                        return;
                    }
                    if wrote {
                        w.last_write = std::time::Instant::now();
                    }
                    w.track_levels();
                }
            }
        })
        .expect("could not start the worker thread")
}

pub fn channel() -> (Sender<Cmd>, Receiver<Cmd>) {
    std::sync::mpsc::channel()
}

impl Worker {
    fn send(&self, e: Event) {
        (self.emit)(e);
    }

    fn targets(&self) -> Vec<usize> {
        if self.sync {
            (0..self.core.mons.len()).collect()
        } else if self.selected < self.core.mons.len() {
            vec![self.selected]
        } else {
            vec![]
        }
    }

    /// Remembers the brightness/contrast the monitors are at now (so the next launch can restore them).
    fn track_levels(&mut self) {
        if !self.armed {
            return;
        }
        let mut changed = false;
        for m in &self.core.mons {
            // brightness and contrast must both be known; the colour gains/black levels follow when read
            if !(m.values.contains_key(&vcp::BRIGHTNESS) && m.values.contains_key(&vcp::CONTRAST)) {
                continue;
            }
            let entry = self.levels.entry(m.info.key.clone()).or_default();
            for code in REMEMBERED {
                if let Some(v) = m.values.get(&code) {
                    let pct = crate::core::to_pct(*v);
                    if entry.insert(code, pct) != Some(pct) {
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.send(Event::Levels(self.levels.clone()));
        }
    }

    fn publish_view(&self) {
        if let Some(v) = self.core.view(self.selected) {
            self.send(Event::View(v));
        }
    }

    fn load_selected(&mut self) {
        let i = self.selected;
        if i >= self.core.mons.len() {
            return;
        }
        self.send(Event::Busy(true));
        self.send(Event::Status(Status::Reading));
        self.core.probe_basic(i);
        self.publish_view();
        self.core.ensure_caps(i);
        self.core.read_extended(i);
        self.publish_view();
        self.send(Event::Busy(false));
        self.send(Event::Status(Status::Ready));
    }

    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Quit => return false,
            Cmd::Sync(v) => self.sync = v,
            Cmd::Refresh { prefer } => {
                self.send(Event::Busy(true));
                self.send(Event::Status(Status::Reading));
                match self.core.refresh() {
                    Err(e) => {
                        self.send(Event::Status(Status::ReadFailed(e)));
                        self.send(Event::Busy(false));
                    }
                    Ok(()) => {
                        let infos: Vec<MonitorInfo> = self.core.mons.iter().map(|m| m.info.clone()).collect();
                        self.selected = prefer
                            .and_then(|k| infos.iter().position(|m| m.key == k))
                            .unwrap_or(0);
                        let n = infos.len();
                        self.send(Event::Monitors { infos, selected: self.selected });
                        if n == 0 {
                            self.send(Event::Status(Status::NoMonitors));
                            self.send(Event::Busy(false));
                        } else {
                            self.load_selected();
                            self.send(Event::Status(Status::Found(n)));
                        }
                    }
                }
            }
            Cmd::Pause => self.armed = false,
            Cmd::Restore(seed) => {
                if let Some(map) = seed {
                    // a seed (startup, imported backup) replaces what was remembered for those monitors
                    for (k, v) in map {
                        self.levels.insert(k, v);
                    }
                }
                for i in 0..self.core.mons.len() {
                    let Some(saved) = self.levels.get(&self.core.mons[i].info.key).cloned() else { continue };
                    for code in REMEMBERED {
                        if let Some(&pct) = saved.get(&code) {
                            let _ = self.core.set_pct(i, code, pct);
                        }
                    }
                }
                self.armed = true;
                self.publish_view();
            }
            Cmd::Select(i) => {
                if i < self.core.mons.len() {
                    self.selected = i;
                    self.load_selected();
                }
            }
            Cmd::Level { code, pct } => {
                let mut failed = None;
                for i in self.targets() {
                    if i != self.selected && !self.core.supports(i, code) && !self.core.mons[i].values.contains_key(&code) {
                        // secondary monitors: try anyway (set_pct reads the maximum first)
                    }
                    if let Err(e) = self.core.set_pct(i, code, pct) {
                        if i == self.selected {
                            failed = Some(e);
                        }
                    }
                }
                if let Some(_e) = failed {
                    self.send(Event::Status(Status::SetFailed(vcp::code_name(code).to_string())));
                    self.publish_view();
                }
            }
            Cmd::SetMute(on) => {
                let mut ok = false;
                for i in self.targets() {
                    ok |= self.core.set_raw(i, vcp::MUTE, if on { 1 } else { 2 }).is_ok();
                }
                if !ok {
                    self.send(Event::Status(Status::SetFailed(vcp::code_name(vcp::MUTE).to_string())));
                }
                self.publish_view();
            }
            Cmd::SetRaw { code, value } => {
                let i = self.selected;
                // the color preset follows "control all together"; input and power do not
                if code == vcp::COLOR_PRESET && self.sync {
                    for j in self.targets().into_iter().filter(|j| *j != i) {
                        let _ = self.core.set_raw(j, code, value);
                    }
                }
                match self.core.set_raw(i, code, value) {
                    Ok(()) => {
                        if code == vcp::POWER_MODE {
                            self.send(Event::Status(Status::PowerSent));
                        } else if matches!(code, vcp::INPUT_SOURCE | vcp::COLOR_PRESET) {
                            if self.core.verify_raw(i, code, value) == Some(false) {
                                self.send(Event::Status(Status::Ignored(vcp::code_name(code).to_string())));
                            }
                        }
                        // update the cached value even for codes not read yet
                        if let Some(m) = self.core.mons.get_mut(i) {
                            m.values.entry(code).and_modify(|v| v.cur = value);
                        }
                    }
                    Err(_) => self.send(Event::Status(Status::SetFailed(vcp::code_name(code).to_string()))),
                }
                self.publish_view();
            }
            Cmd::Reset(which) => {
                let i = self.selected;
                self.send(Event::Busy(true));
                if which != 0 && self.sync {
                    for j in self.targets().into_iter().filter(|j| *j != i) {
                        let _ = self.core.reset(j, which);
                    }
                }
                match self.core.reset(i, which) {
                    Ok(()) => self.send(Event::Status(Status::ResetDone)),
                    Err(_) => self.send(Event::Status(Status::SetFailed(vcp::code_name(vcp::RESET_ALL).to_string()))),
                }
                self.publish_view();
                self.send(Event::Busy(false));
            }
            Cmd::CaptureProfile { name, include_input } => {
                self.send(Event::Busy(true));
                let p = self.core.capture_profile(&name, include_input);
                self.send(Event::ProfileCaptured(p));
                self.publish_view();
                self.send(Event::Busy(false));
            }
            Cmd::ApplyProfile(p) => {
                self.send(Event::Busy(true));
                let n = self.core.apply_profile(&p);
                if n > 0 {
                    self.send(Event::Applied(p.name.clone()));
                }
                self.publish_view();
                self.send(Event::Busy(false));
            }
            Cmd::ReadVcpTable => {
                self.send(Event::Busy(true));
                let i = self.selected;
                self.core.ensure_caps(i);
                let rows = self.core.vcp_rows(i);
                let caps_raw = self.core.mons.get(i).and_then(|m| m.caps.as_ref()).map(|c| c.raw.clone()).unwrap_or_default();
                if caps_raw.is_empty() {
                    self.send(Event::Status(Status::CapsMissing));
                } else {
                    self.send(Event::Status(Status::Ready));
                }
                self.send(Event::VcpRows { rows, caps_raw });
                self.send(Event::Busy(false));
            }
            Cmd::WriteVcp { code, value } => {
                match self.core.set_raw(self.selected, code, value) {
                    Ok(()) => self.send(Event::Status(Status::Wrote)),
                    Err(_) => self.send(Event::Status(Status::SetFailed(format!("0x{code:02X}")))),
                }
                self.publish_view();
            }
            Cmd::Adjust { code, delta, osd } => {
                let mut shown: Option<(u8, String)> = None;
                for i in self.targets() {
                    if let Ok(p) = self.core.adjust_pct(i, code, delta) {
                        if shown.is_none() || i == self.selected {
                            shown = Some((p, self.core.mons[i].info.name.clone()));
                        }
                    }
                }
                match shown {
                    Some((pct, monitor)) => {
                        self.send(Event::Osd { kind: osd, pct, monitor });
                        self.publish_view();
                    }
                    None => self.send(Event::Status(Status::SetFailed(vcp::code_name(code).to_string()))),
                }
            }
            Cmd::Poll => {
                let i = self.selected;
                // monitors can report the old value for a moment after a write: wait before re-reading
                if self.last_write.elapsed() < std::time::Duration::from_millis(2500) {
                    return true;
                }
                if i < self.core.mons.len() {
                    self.core.poll(i);
                    self.publish_view();
                }
            }
            Cmd::ReadInfo => {
                let i = self.selected;
                if i < self.core.mons.len() {
                    let info = self.core.read_info(i);
                    self.send(Event::Info(info));
                }
            }
            Cmd::ToggleMute => {
                let mut muted = None;
                let mut name = String::new();
                for i in self.targets() {
                    if let Ok(m) = self.core.toggle_mute(i) {
                        if muted.is_none() || i == self.selected {
                            muted = Some(m);
                            name = self.core.mons[i].info.name.clone();
                        }
                    }
                }
                if let Some(m) = muted {
                    self.send(Event::Osd { kind: OsdKind::Muted, pct: if m { 0 } else { 100 }, monitor: name });
                    self.publish_view();
                }
            }
            Cmd::AllBrightness(pct) => {
                let mut name = String::new();
                for i in 0..self.core.mons.len() {
                    if self.core.set_pct(i, vcp::BRIGHTNESS, pct).is_ok() && name.is_empty() {
                        name = self.core.mons[i].info.name.clone();
                    }
                }
                self.send(Event::Osd { kind: OsdKind::Brightness, pct, monitor: name });
                self.publish_view();
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn coalesces_levels() {
        let b = vec![
            Cmd::Level { code: 0x10, pct: 1 },
            Cmd::Level { code: 0x10, pct: 2 },
            Cmd::Level { code: 0x12, pct: 3 },
            Cmd::Level { code: 0x10, pct: 4 },
        ];
        let out = coalesce(b);
        // only the newest value of each code survives
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Cmd::Level { code: 0x12, pct: 3 }));
        assert!(matches!(out[1], Cmd::Level { code: 0x10, pct: 4 }));
    }

    #[test]
    fn coalesces_interleaved_levels_and_drops_stale_polls() {
        let b = vec![
            Cmd::Poll,
            Cmd::Level { code: 0x10, pct: 1 },
            Cmd::Level { code: 0x12, pct: 3 },
            Cmd::Level { code: 0x10, pct: 4 },
            Cmd::Poll,
        ];
        let out = coalesce(b);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], Cmd::Level { code: 0x12, pct: 3 }));
        assert!(matches!(out[1], Cmd::Level { code: 0x10, pct: 4 }));
        assert!(matches!(out[2], Cmd::Poll));
    }

    #[test]
    fn worker_end_to_end() {
        let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let emit: Emit = Arc::new(move |e| sink.lock().unwrap().push(e));
        let (tx, rx) = channel();
        let h = spawn(Core::new(Box::new(MockBackend::new())), rx, emit);
        tx.send(Cmd::Refresh { prefer: Some("DEMO0002-B".into()) }).unwrap();
        tx.send(Cmd::Level { code: vcp::BRIGHTNESS, pct: 10 }).unwrap();
        tx.send(Cmd::Adjust { code: vcp::BRIGHTNESS, delta: 5, osd: OsdKind::Brightness }).unwrap();
        tx.send(Cmd::Quit).unwrap();
        h.join().unwrap();
        let ev = events.lock().unwrap();
        assert!(ev.iter().any(|e| matches!(e, Event::Monitors { selected: 1, infos } if infos.len() == 3)));
        assert!(ev.iter().any(|e| matches!(e, Event::Osd { pct: 15, .. })));
        assert!(ev.iter().any(|e| matches!(e, Event::View(v) if v.has_brightness)));
        let _ = Duration::from_millis(1);
    }
}
