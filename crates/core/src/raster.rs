//! Input raster and anti-aliasing (mixed pixel) detection.

/// Pixels with alpha below this are treated as fully transparent.
pub const TRANSPARENT_ALPHA: u8 = 8;

pub(crate) struct Raster {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<[u8; 4]>,
    /// Premultiplied, gamma-encoded (sRGB) RGBA in 0..1.
    /// Anti-aliasing in renderers is (almost always) a linear blend in this space,
    /// so mixed pixels are modelled here.
    pub pm: Vec<[f32; 4]>,
}

pub(crate) fn premultiply(c: [u8; 4]) -> [f32; 4] {
    let a = c[3] as f32 / 255.0;
    [
        c[0] as f32 / 255.0 * a,
        c[1] as f32 / 255.0 * a,
        c[2] as f32 / 255.0 * a,
        a,
    ]
}

impl Raster {
    pub fn new(rgba: &[u8], w: usize, h: usize) -> Self {
        let rgba: Vec<[u8; 4]> = rgba
            .chunks_exact(4)
            .map(|c| {
                if c[3] < TRANSPARENT_ALPHA {
                    [0, 0, 0, 0]
                } else {
                    [c[0], c[1], c[2], c[3]]
                }
            })
            .collect();
        let pm = rgba.iter().map(|&c| premultiply(c)).collect();
        Self { w, h, rgba, pm }
    }

    /// Marks pixels that look like a blend of two different colours on opposite
    /// sides (anti-aliased edges). These must not seed the palette, otherwise
    /// every edge produces an extra "fringe" colour.
    pub fn detect_mixed(&self) -> Vec<bool> {
        const DIRS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
        let (w, h) = (self.w as isize, self.h as isize);
        let mut out = vec![false; self.w * self.h];
        for y in 0..h {
            for x in 0..w {
                let p = self.pm[(y * w + x) as usize];
                'dirs: for &(dx, dy) in &DIRS {
                    for r in 1..=3isize {
                        let (x1, y1, x2, y2) = (x - dx * r, y - dy * r, x + dx * r, y + dy * r);
                        if x1 < 0
                            || y1 < 0
                            || x2 < 0
                            || y2 < 0
                            || x1 >= w
                            || x2 >= w
                            || y1 >= h
                            || y2 >= h
                        {
                            continue;
                        }
                        let n1 = self.pm[(y1 * w + x1) as usize];
                        let n2 = self.pm[(y2 * w + x2) as usize];
                        if is_blend(p, n1, n2) {
                            out[(y * w + x) as usize] = true;
                            break 'dirs;
                        }
                    }
                }
            }
        }
        out
    }
}

/// Is `p` ≈ n1 + t·(n2 − n1) for some t strictly inside (0, 1)?
pub(crate) fn is_blend(p: [f32; 4], n1: [f32; 4], n2: [f32; 4]) -> bool {
    let d = sub(n2, n1);
    let dd = dot(d, d);
    if dd < 0.12 * 0.12 {
        return false;
    }
    let t = dot(sub(p, n1), d) / dd;
    if !(0.06..=0.94).contains(&t) {
        return false;
    }
    let r = sub(p, add(n1, scale(d, t)));
    dot(r, r).sqrt() < 0.025 + 0.08 * dd.sqrt()
}

#[inline]
pub(crate) fn sub(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}
#[inline]
pub(crate) fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}
#[inline]
pub(crate) fn scale(a: [f32; 4], s: f32) -> [f32; 4] {
    [a[0] * s, a[1] * s, a[2] * s, a[3] * s]
}
#[inline]
pub(crate) fn dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}
