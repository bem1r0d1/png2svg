//! Color space conversions: sRGB ⇄ linear ⇄ OKLab.

use std::sync::OnceLock;

fn lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 255.0);
        }
        t
    })
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb8_to_linear(v: u8) -> f32 {
    lut()[v as usize]
}

/// OKLab from 8-bit sRGB.
pub fn srgb8_to_oklab(rgb: [u8; 3]) -> [f32; 3] {
    linear_to_oklab([
        srgb8_to_linear(rgb[0]),
        srgb8_to_linear(rgb[1]),
        srgb8_to_linear(rgb[2]),
    ])
}

pub fn linear_to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn oklab_to_linear([ll, a, b]: [f32; 3]) -> [f32; 3] {
    let l = ll + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = ll - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = ll - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

pub fn oklab_to_srgb8(lab: [f32; 3]) -> [u8; 3] {
    let lin = oklab_to_linear(lab);
    lin.map(|c| (linear_to_srgb(c) * 255.0).round() as u8)
}

/// Euclidean OKLab distance (≈ ΔE / 100).
pub fn oklab_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_roundtrip() {
        for rgb in [[0u8, 0, 0], [255, 255, 255], [18, 120, 240], [250, 30, 90]] {
            let back = oklab_to_srgb8(srgb8_to_oklab(rgb));
            for i in 0..3 {
                assert!(
                    (back[i] as i32 - rgb[i] as i32).abs() <= 1,
                    "{rgb:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn white_is_l1() {
        let w = srgb8_to_oklab([255, 255, 255]);
        assert!((w[0] - 1.0).abs() < 1e-3);
        assert!(w[1].abs() < 1e-3 && w[2].abs() < 1e-3);
    }
}
