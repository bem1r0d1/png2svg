//! Input quality analysis and clean-up: noise estimation, edge-preserving
//! denoising, pixel-grid (pixelation) detection and the amount of edge
//! smoothing the contours need.
//!
//! The goal is a result that stays clean at any zoom: noise, JPEG artefacts,
//! aliasing staircases and pixelation must be removed, not traced.

use crate::raster::{dot, sub, Raster};
use crate::segment::Labels;

/// Noise level: 90th percentile of the local Laplacian in non-edge areas
/// (premultiplied sRGB, 0..1). ≈0 for clean graphics and smooth gradients,
/// ~0.01–0.03 for JPEG artefacts, ~0.04+ for sensor-like noise.
pub(crate) fn estimate_noise(r: &Raster) -> f32 {
    const BINS: usize = 256;
    const MAX: f32 = 0.12;
    let mut hist = [0usize; BINS];
    let mut total = 0usize;
    for y in 1..r.h.saturating_sub(1) {
        for x in 1..r.w.saturating_sub(1) {
            let i = y * r.w + x;
            let p = r.pm[i];
            let mut max = 0f32;
            let mut avg = [0f32; 4];
            for j in [i - 1, i + 1, i - r.w, i + r.w] {
                let d = sub(p, r.pm[j]);
                max = max.max(dot(d, d).sqrt());
                for (a, v) in avg.iter_mut().zip(r.pm[j]) {
                    *a += v * 0.25;
                }
            }
            if max >= MAX {
                continue; // an edge, not noise
            }
            // Laplacian magnitude: ~0 on flat areas and smooth ramps, high
            // for noise and JPEG ringing.
            let lap = sub(p, avg);
            let mean = dot(lap, lap).sqrt();
            hist[((mean / MAX) * BINS as f32) as usize % BINS] += 1;
            total += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    let pct = |q: f32| -> f32 {
        let mut acc = 0;
        for (b, &n) in hist.iter().enumerate() {
            acc += n;
            if acc as f32 >= q * total as f32 {
                return (b as f32 + 0.5) / BINS as f32 * MAX;
            }
        }
        MAX
    };
    pct(0.9)
}

/// Edge-preserving bilateral filter (5×5) in premultiplied sRGB; updates the
/// raster in place. Flat areas are smoothed, edges between colours are kept.
pub(crate) fn denoise(r: &mut Raster, noise: f32, passes: usize) {
    let (w, h) = (r.w as isize, r.h as isize);
    let range = (2.5 * noise).max(0.02);
    let inv_r = 1.0 / (2.0 * range * range);
    let inv_s = 1.0 / (2.0 * 1.5f32 * 1.5);
    for _ in 0..passes {
        let src = r.pm.clone();
        for y in 0..h {
            for x in 0..w {
                let p = src[(y * w + x) as usize];
                let mut acc = [0f32; 4];
                let mut wsum = 0f32;
                for dy in -2..=2isize {
                    for dx in -2..=2isize {
                        let (nx, ny) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                        let q = src[(ny * w + nx) as usize];
                        let d = sub(p, q);
                        let wt = (-(dot(d, d) * inv_r) - (dx * dx + dy * dy) as f32 * inv_s).exp();
                        for c in 0..4 {
                            acc[c] += q[c] * wt;
                        }
                        wsum += wt;
                    }
                }
                r.pm[(y * w + x) as usize] = acc.map(|v| v / wsum);
            }
        }
    }
    // Keep the straight 8-bit copy in sync.
    for (c, pm) in r.rgba.iter_mut().zip(&r.pm) {
        let a = pm[3].clamp(0.0, 1.0);
        if a * 255.0 < crate::raster::TRANSPARENT_ALPHA as f32 {
            *c = [0, 0, 0, 0];
        } else {
            *c = [
                (pm[0] / a * 255.0).round().clamp(0.0, 255.0) as u8,
                (pm[1] / a * 255.0).round().clamp(0.0, 255.0) as u8,
                (pm[2] / a * 255.0).round().clamp(0.0, 255.0) as u8,
                (a * 255.0).round() as u8,
            ];
        }
    }
    for (pm, &c) in r.pm.iter_mut().zip(&r.rgba) {
        *pm = crate::raster::premultiply(c);
    }
}

/// Pixel grid of an upscaled (pixelated) image: block size and grid offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Grid {
    pub k: usize,
    pub ox: usize,
    pub oy: usize,
}

/// Detects the pixel grid of an image upscaled k× with nearest neighbour:
/// all colour transitions fall on one residue modulo k in both directions.
pub(crate) fn pixel_grid(r: &Raster) -> Option<Grid> {
    let changed = |a: [f32; 4], b: [f32; 4]| {
        let d = sub(a, b);
        dot(d, d) > 0.03 * 0.03
    };
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for y in 0..r.h {
        for x in 1..r.w {
            if changed(r.pm[y * r.w + x], r.pm[y * r.w + x - 1]) {
                xs.push(x);
            }
        }
    }
    for y in 1..r.h {
        for x in 0..r.w {
            if changed(r.pm[y * r.w + x], r.pm[(y - 1) * r.w + x]) {
                ys.push(y);
            }
        }
    }
    if xs.len() < 40 || ys.len() < 40 {
        return None;
    }
    // (share of transitions on the best residue, that residue)
    let score = |pos: &[usize], k: usize| -> (f32, usize) {
        let mut hist = vec![0usize; k];
        for &p in pos {
            hist[p % k] += 1;
        }
        let (best, n) = hist.iter().enumerate().max_by_key(|&(_, n)| *n).unwrap();
        (*n as f32 / pos.len() as f32, best)
    };
    (2..=16).rev().find_map(|k| {
        if r.w < 4 * k || r.h < 4 * k {
            return None;
        }
        let (sx, ox) = score(&xs, k);
        let (sy, oy) = score(&ys, k);
        (sx >= 0.97 && sy >= 0.97).then_some(Grid { k, ox, oy })
    })
}

/// One pixel per grid block (sampled at the block centre). Returns the logical
/// image and the original-pixel position of its origin (≤ 0 if the first
/// block is cut by the image border).
pub(crate) fn downsample(r: &Raster, g: Grid) -> (Vec<u8>, usize, usize, f64, f64) {
    let x0 = if g.ox > 0 {
        g.ox as isize - g.k as isize
    } else {
        0
    };
    let y0 = if g.oy > 0 {
        g.oy as isize - g.k as isize
    } else {
        0
    };
    let lw = (r.w as isize - x0).div_euclid(g.k as isize) as usize
        + usize::from((r.w as isize - x0) % g.k as isize != 0);
    let lh = (r.h as isize - y0).div_euclid(g.k as isize) as usize
        + usize::from((r.h as isize - y0) % g.k as isize != 0);
    let mut out = Vec::with_capacity(lw * lh * 4);
    for j in 0..lh {
        for i in 0..lw {
            let cx = (x0 + (i * g.k + g.k / 2) as isize).clamp(0, r.w as isize - 1) as usize;
            let cy = (y0 + (j * g.k + g.k / 2) as isize).clamp(0, r.h as isize - 1) as usize;
            out.extend_from_slice(&r.rgba[cy * r.w + cx]);
        }
    }
    (out, lw, lh, x0 as f64, y0 as f64)
}

/// Scale2x / EPX: doubles a hard-edged pixel-art image, continuing diagonals
/// without breaking 8-connected lines (a classic depixelisation step).
pub(crate) fn scale2x(px: &[u8], w: usize, h: usize) -> Vec<u8> {
    let at = |x: isize, y: isize| -> [u8; 4] {
        let (x, y) = (
            x.clamp(0, w as isize - 1) as usize,
            y.clamp(0, h as isize - 1) as usize,
        );
        let i = (y * w + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    let (w2, h2) = (w * 2, h * 2);
    let mut out = vec![0u8; w2 * h2 * 4];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let (e, b, d, f, hh) = (
                at(x, y),
                at(x, y - 1),
                at(x - 1, y),
                at(x + 1, y),
                at(x, y + 1),
            );
            let q = [
                if d == b && b != f && d != hh { d } else { e },
                if b == f && b != d && f != hh { f } else { e },
                if d == hh && d != b && hh != f { d } else { e },
                if hh == f && d != hh && b != f { f } else { e },
            ];
            for (n, c) in q.iter().enumerate() {
                let (ox, oy) = (x as usize * 2 + n % 2, y as usize * 2 + n / 2);
                out[(oy * w2 + ox) * 4..(oy * w2 + ox) * 4 + 4].copy_from_slice(c);
            }
        }
    }
    out
}

/// Whether a logical (one pixel per block) image has hard edges only: no
/// anti-aliasing blends between colours, i.e. genuine pixel art.
pub(crate) fn is_hard_edged(r: &Raster) -> bool {
    let mixed = r.detect_mixed();
    let mut edge = 0usize;
    let mut blended = 0usize;
    for y in 0..r.h {
        for x in 0..r.w {
            let i = y * r.w + x;
            let e = (x + 1 < r.w && r.rgba[i + 1] != r.rgba[i])
                || (y + 1 < r.h && r.rgba[i + r.w] != r.rgba[i]);
            if e {
                edge += 1;
                blended += usize::from(mixed[i]);
            }
        }
    }
    edge > 0 && (blended as f32) < 0.1 * edge as f32
}

/// Average width (px) of colour transitions: ≈1 for crisp anti-aliasing,
/// 3–5 for blurry / upscaled edges.
pub(crate) fn edge_width(mixed: &[bool], l: &Labels) -> f32 {
    let mut boundary = 0usize;
    for y in 0..l.h {
        for x in 0..l.w {
            let i = y * l.w + x;
            if (x + 1 < l.w && l.a[i + 1] != l.a[i]) || (y + 1 < l.h && l.a[i + l.w] != l.a[i]) {
                boundary += 1;
            }
        }
    }
    if boundary == 0 {
        return 0.0;
    }
    mixed.iter().filter(|&&m| m).count() as f32 / boundary as f32
}

/// Share of colour-boundary pixels carrying anti-aliasing (fractional coverage).
/// Low values mean hard, aliased (staircase) edges.
pub(crate) fn edge_aa_ratio(l: &Labels) -> f32 {
    let (mut edge, mut aa) = (0usize, 0usize);
    for y in 0..l.h {
        for x in 0..l.w {
            let i = y * l.w + x;
            let boundary =
                (x + 1 < l.w && l.a[i + 1] != l.a[i]) || (y + 1 < l.h && l.a[i + l.w] != l.a[i]);
            if boundary {
                edge += 1;
                if l.t[i] < 0.97 {
                    aa += 1;
                }
            }
        }
    }
    if edge == 0 {
        1.0
    } else {
        aa as f32 / edge as f32
    }
}

/// Gaussian blur σ (px) for the coverage fields, from the input analysis.
pub(crate) fn edge_smoothing(aa_ratio: f32, noise: f32, edge_width: f32) -> f32 {
    // Blurry edges: the sub-pixel position is less certain, smooth accordingly.
    // (JPEG artefacts also widen edges; noise is handled by its own term.)
    let mut s = if noise < 0.006 {
        ((edge_width - 1.8) * 0.5).clamp(0.0, 1.5)
    } else {
        0.0
    };
    if aa_ratio < 0.25 {
        s = s.max(0.8); // aliased: 1 px staircases
    }
    s.max(((noise - 0.006) * 60.0).clamp(0.0, 1.5))
}

/// Separable Gaussian blur with clamp-to-edge borders (shapes touching the
/// image border stay attached to it).
pub(crate) fn gaussian_blur(f: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma < 0.05 {
        return;
    }
    let rad = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-rad..=rad)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let norm: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / norm).collect();
    let mut tmp = vec![0f32; f.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let xx = (x as isize + j as isize - rad).clamp(0, w as isize - 1) as usize;
                s += f[y * w + xx] * kv;
            }
            tmp[y * w + x] = s;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let yy = (y as isize + j as isize - rad).clamp(0, h as isize - 1) as usize;
                s += tmp[yy * w + x] * kv;
            }
            f[y * w + x] = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster_from(mut f: impl FnMut(usize, usize) -> [u8; 4], w: usize, h: usize) -> Raster {
        let px: Vec<u8> = (0..w * h).flat_map(|i| f(i % w, i / w)).collect();
        Raster::new(&px, w, h)
    }

    #[test]
    fn detects_pixel_grid() {
        // A 4× nearest-neighbour upscale of a checker-ish pattern.
        let r = raster_from(
            |x, y| {
                let (bx, by) = (x / 4, y / 4);
                if (bx * 7 + by * 3) % 5 < 2 {
                    [200, 40, 40, 255]
                } else {
                    [250, 250, 250, 255]
                }
            },
            96,
            96,
        );
        assert_eq!(pixel_grid(&r), Some(Grid { k: 4, ox: 0, oy: 0 }));
        let clean = raster_from(
            |x, y| {
                if (x * x + y * y) % 7 < 3 {
                    [0, 0, 0, 255]
                } else {
                    [255; 4]
                }
            },
            96,
            96,
        );
        assert_eq!(pixel_grid(&clean), None);
    }

    #[test]
    fn scale2x_rounds_diagonals() {
        // 2×2 checker of black on white: diagonal neighbours get joined.
        let b = [0u8, 0, 0, 255];
        let wh = [255u8, 255, 255, 255];
        let px: Vec<u8> = [b, wh, wh, b].concat();
        let out = scale2x(&px, 2, 2);
        assert_eq!(out.len(), 4 * 4 * 4);
        // Top-left block keeps its corner pixel black.
        assert_eq!(&out[0..4], &b);
    }

    #[test]
    fn noise_estimate_and_denoise() {
        let mut seed = 12345u32;
        let mut r = raster_from(
            |_, _| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let n = ((seed >> 16) % 21) as i32 - 10;
                let v = (128 + n) as u8;
                [v, v, v, 255]
            },
            64,
            64,
        );
        let n0 = estimate_noise(&r);
        assert!(n0 > 0.01, "{n0}");
        denoise(&mut r, n0, 2);
        assert!(estimate_noise(&r) < n0 * 0.5);
        let flat = raster_from(|_, _| [10, 20, 30, 255], 32, 32);
        assert_eq!(estimate_noise(&flat), 0.5 / 256.0 * 0.12);
    }

    #[test]
    fn blur_keeps_border_attached() {
        let (w, h) = (16, 16);
        let mut f = vec![1.0f32; w * h];
        gaussian_blur(&mut f, w, h, 1.5);
        assert!(f.iter().all(|&v| (v - 1.0).abs() < 1e-4));
    }
}
