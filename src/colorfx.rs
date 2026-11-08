// SPDX-License-Identifier: GPL-3.0-or-later
//! Software color filter (a 5×5 matrix applied to the whole desktop).
//! Works on any graphics card, with no specific driver, and really controls the colors
//! (unlike the monitor's "RGB gain", which only moves the white point).

pub type M5 = [[f32; 5]; 5];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Gray,
    Red,
    Green,
    Blue,
    Yellow,
    Sepia,
    Invert,
}

impl Mode {
    pub const ALL: [Mode; 8] =
        [Mode::Normal, Mode::Gray, Mode::Red, Mode::Green, Mode::Blue, Mode::Yellow, Mode::Sepia, Mode::Invert];

    pub fn index(self) -> i32 {
        Mode::ALL.iter().position(|m| *m == self).unwrap_or(0) as i32
    }
    pub fn from_index(i: i32) -> Mode {
        Mode::ALL.get(i.max(0) as usize).copied().unwrap_or(Mode::Normal)
    }
    pub fn name(self) -> &'static str {
        ["normal", "gray", "red", "green", "blue", "yellow", "sepia", "invert"][self.index() as usize]
    }
    pub fn from_name(s: &str) -> Mode {
        Mode::ALL.iter().copied().find(|m| m.name() == s).unwrap_or(Mode::Normal)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorFx {
    /// 0..=100, 50 = normal (0 = black and white, 100 = double saturation)
    pub saturation: u8,
    pub mode: Mode,
    /// intensity of each channel, 0..=100 (100 = normal)
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0..=100, 50 = neutral (lower = warmer, higher = cooler)
    pub temp: u8,
}

impl Default for ColorFx {
    fn default() -> Self {
        ColorFx { saturation: 50, mode: Mode::Normal, r: 100, g: 100, b: 100, temp: 50 }
    }
}

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn identity() -> M5 {
    let mut m = [[0.0; 5]; 5];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// Product in application order (row vector: v · a · b).
fn mul(a: &M5, b: &M5) -> M5 {
    let mut o = [[0.0; 5]; 5];
    for i in 0..5 {
        for j in 0..5 {
            o[i][j] = (0..5).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn saturation_matrix(pct: u8) -> M5 {
    let s = pct.min(100) as f32 / 50.0; // 0..2
    let mut m = identity();
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = LUMA[i] * (1.0 - s) + if i == j { s } else { 0.0 };
        }
    }
    m
}

fn mode_matrix(mode: Mode) -> M5 {
    let mut m = identity();
    match mode {
        Mode::Normal => {}
        Mode::Gray => {
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] = LUMA[i];
                }
            }
        }
        Mode::Red | Mode::Green | Mode::Blue | Mode::Yellow => {
            // keep only the given channels, all with the image's luminance
            let keep: &[usize] = match mode {
                Mode::Red => &[0],
                Mode::Green => &[1],
                Mode::Blue => &[2],
                _ => &[0, 1],
            };
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] = if keep.contains(&j) { LUMA[i] } else { 0.0 };
                }
            }
        }
        Mode::Sepia => {
            let s = [[0.393, 0.349, 0.272], [0.769, 0.686, 0.534], [0.189, 0.168, 0.131]];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] = s[i][j];
                }
            }
        }
        Mode::Invert => {
            for i in 0..3 {
                m[i][i] = -1.0;
                m[4][i] = 1.0;
            }
        }
    }
    m
}

fn gain_matrix(fx: &ColorFx) -> M5 {
    let t = (fx.temp.min(100) as f32 - 50.0) / 50.0; // -1 warm .. +1 cool
    let (tr, tg, tb) = if t < 0.0 { (1.0, 1.0 + 0.30 * t, 1.0 + 0.65 * t) } else { (1.0 - 0.30 * t, 1.0 - 0.10 * t, 1.0) };
    let mut m = identity();
    m[0][0] = fx.r.min(100) as f32 / 100.0 * tr;
    m[1][1] = fx.g.min(100) as f32 / 100.0 * tg;
    m[2][2] = fx.b.min(100) as f32 / 100.0 * tb;
    m
}

pub fn matrix(fx: &ColorFx) -> M5 {
    let a = mul(&saturation_matrix(fx.saturation), &mode_matrix(fx.mode));
    mul(&a, &gain_matrix(fx))
}

pub fn is_neutral(fx: &ColorFx) -> bool {
    *fx == ColorFx::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &M5, c: [f32; 3]) -> [f32; 3] {
        let v = [c[0], c[1], c[2], 1.0, 1.0];
        let mut o = [0.0f32; 3];
        for j in 0..3 {
            o[j] = (0..5).map(|i| v[i] * m[i][j]).sum();
        }
        o
    }
    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn default_is_identity() {
        let m = matrix(&ColorFx::default());
        assert!(near(apply(&m, [0.2, 0.5, 0.9]), [0.2, 0.5, 0.9]));
        assert!(is_neutral(&ColorFx::default()));
    }

    #[test]
    fn saturation_extremes() {
        let gray = matrix(&ColorFx { saturation: 0, ..Default::default() });
        let o = apply(&gray, [1.0, 0.0, 0.0]);
        assert!((o[0] - o[1]).abs() < 1e-3 && (o[1] - o[2]).abs() < 1e-3);
        let boost = matrix(&ColorFx { saturation: 100, ..Default::default() });
        assert!(apply(&boost, [0.8, 0.4, 0.4])[0] > 0.8); // red becomes redder
        // pure gray does not change with saturation
        assert!(near(apply(&boost, [0.5, 0.5, 0.5]), [0.5, 0.5, 0.5]));
    }

    #[test]
    fn redscale_removes_other_channels() {
        let m = matrix(&ColorFx { mode: Mode::Red, ..Default::default() });
        for c in [[1.0, 1.0, 1.0], [0.0, 1.0, 0.0], [0.2, 0.3, 0.9]] {
            let o = apply(&m, c);
            assert!(o[1].abs() < 1e-4 && o[2].abs() < 1e-4, "{o:?}");
        }
        assert!(near(apply(&m, [1.0, 1.0, 1.0]), [1.0, 0.0, 0.0]));
    }

    #[test]
    fn channel_gain_zeroes_channels() {
        let m = matrix(&ColorFx { g: 0, b: 0, ..Default::default() });
        assert!(near(apply(&m, [0.7, 0.7, 0.7]), [0.7, 0.0, 0.0]));
    }

    #[test]
    fn invert_and_warm() {
        let m = matrix(&ColorFx { mode: Mode::Invert, ..Default::default() });
        assert!(near(apply(&m, [0.0, 0.25, 1.0]), [1.0, 0.75, 0.0]));
        let warm = matrix(&ColorFx { temp: 0, ..Default::default() });
        let o = apply(&warm, [1.0, 1.0, 1.0]);
        assert!(o[2] < o[1] && o[1] < o[0]);
    }

    #[test]
    fn mode_names_roundtrip() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_name(m.name()), m);
            assert_eq!(Mode::from_index(m.index()), m);
        }
    }

    #[test]
    fn yellowscale_keeps_red_and_green_only() {
        let fx = ColorFx { mode: Mode::Yellow, ..Default::default() };
        let out = apply(&matrix(&fx), [0.2, 0.4, 0.9]);
        assert!(out[2] < 0.01 && (out[0] - out[1]).abs() < 0.01 && out[0] > 0.1);
    }
}
