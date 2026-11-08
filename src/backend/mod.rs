//! Layer that talks to the monitors. On Windows it uses real DDC/CI (dxva2);
//! the simulated backend lets the interface be tested on any system (`--demo`).

pub mod mock;
#[cfg(target_os = "linux")]
pub mod linux;
// Compiled on every platform (FFI declared by hand) so that it is checked
// by the compiler even outside Windows; it is only used on Windows.
#[allow(dead_code)]
pub mod win;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    External,
    Internal,
}

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    /// stable key (EDID) used in profiles
    pub key: String,
    pub name: String,
    /// e.g. "\\.\DISPLAY1 · 2560×1440"
    pub detail: String,
    pub kind: Kind,
    /// screen size (diagonal) in inches, when the EDID reports it
    pub inches: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vcp {
    pub cur: u32,
    pub max: u32,
}

pub trait Backend: Send {
    fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String>;
    /// A single read attempt (retries happen in `Core`).
    fn get(&mut self, mon: usize, code: u8) -> Result<Vcp, String>;
    fn set(&mut self, mon: usize, code: u8, value: u32) -> Result<(), String>;
    fn capabilities(&mut self, mon: usize) -> Result<String, String>;
    #[allow(dead_code)]
    fn save_settings(&mut self, _mon: usize) -> Result<(), String> {
        Ok(())
    }
}

pub fn create(demo: bool) -> (Box<dyn Backend>, bool) {
    #[cfg(windows)]
    {
        if !demo {
            return (Box::new(win::WinBackend::new()), false);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if !demo {
            return (Box::new(linux::LinuxBackend::new()), false);
        }
    }
    let _ = demo;
    (Box::new(mock::MockBackend::new()), true)
}
