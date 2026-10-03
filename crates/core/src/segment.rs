//! Pixel labelling with an explicit anti-aliasing model and speckle removal.
//!
//! Every pixel is described as a blend of at most two palette colours:
//! `pixel ≈ t·a + (1−t)·b`, where `a` is the majority label (t ≥ 0.5).
//! `t` is the coverage of `a` and later drives sub-pixel contour placement.

use std::collections::HashMap;

use crate::quantize::{feature, Palette};
use crate::raster::{add, dot, scale, sub, Raster};

pub(crate) struct Labels {
    pub w: usize,
    pub h: usize,
    /// Majority label.
    pub a: Vec<u16>,
    /// Minority label (== a for pure pixels).
    pub b: Vec<u16>,
    /// Coverage of `a` in [0.5, 1].
    pub t: Vec<f32>,
}

impl Labels {
    /// Coverage of label `l` in pixel `i`.
    #[inline]
    pub fn cov(&self, i: usize, l: u16) -> f32 {
        let mut c = 0.0;
        if self.a[i] == l {
            c += self.t[i];
        }
        if self.b[i] == l && self.b[i] != self.a[i] {
            c += 1.0 - self.t[i];
        }
        c
    }
}

const PAIR_PENALTY: f32 = 0.004;

pub(crate) fn assign(r: &Raster, mixed: &[bool], pal: &Palette) -> Labels {
    let n = r.w * r.h;
    let transparent = pal.transparent_index().map(|i| i as u16);
    let mut cache: HashMap<[u8; 4], u16> = HashMap::new();
    let mut a = vec![0u16; n];
    for (ai, &c) in a.iter_mut().zip(&r.rgba) {
        *ai = if c[3] == 0 {
            transparent.unwrap_or(0)
        } else {
            *cache
                .entry(c)
                .or_insert_with(|| pal.nearest(feature(c)) as u16)
        };
    }
    let mut b = a.clone();
    let mut t = vec![1.0f32; n];
    let pms: Vec<[f32; 4]> = pal.colors.iter().map(|c| c.pm).collect();

    let (w, h) = (r.w as isize, r.h as isize);
    let mut cand: Vec<u16> = Vec::with_capacity(16);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let p = r.pm[i];
            let near = a[i];
            let d0 = sub(p, pms[near as usize]);
            let res0 = dot(d0, d0).sqrt();
            if !mixed[i] && res0 < 0.03 {
                continue;
            }
            // Candidate labels: non-mixed neighbours, searching a growing window
            // until colours on both sides of a (possibly wide) ramp are found.
            cand.clear();
            cand.push(near);
            let mut flat_found = 0;
            for rad in [3isize, 5, 8] {
                for dy in -rad..=rad {
                    for dx in -rad..=rad {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let j = (ny * w + nx) as usize;
                        if !mixed[j] {
                            if !cand.contains(&a[j]) {
                                cand.push(a[j]);
                            }
                            if a[j] != near {
                                flat_found += 1;
                            }
                        }
                    }
                }
                if cand.len() >= 3 || (cand.len() >= 2 && flat_found > 0 && rad >= 5) {
                    break;
                }
            }
            let mut best = (res0, near, near, 1.0f32);
            for (ci, &la) in cand.iter().enumerate() {
                let pa = pms[la as usize];
                let da = sub(p, pa);
                let r1 = dot(da, da).sqrt();
                if r1 < best.0 {
                    best = (r1, la, la, 1.0);
                }
                for &lb in &cand[ci + 1..] {
                    let d = sub(pms[lb as usize], pa);
                    let dd = dot(d, d);
                    if dd < 1e-6 {
                        continue;
                    }
                    let s = (dot(da, d) / dd).clamp(0.0, 1.0);
                    let e = sub(p, add(pa, scale(d, s)));
                    let r2 = dot(e, e).sqrt() + PAIR_PENALTY;
                    if r2 < best.0 {
                        best = (r2, la, lb, 1.0 - s);
                    }
                }
            }
            let (_, la, lb, ta) = best;
            if ta >= 0.5 {
                a[i] = la;
                b[i] = lb;
                t[i] = ta;
            } else {
                a[i] = lb;
                b[i] = la;
                t[i] = 1.0 - ta;
            }
            if a[i] == b[i] {
                t[i] = 1.0;
            }
        }
    }
    Labels {
        w: r.w,
        h: r.h,
        a,
        b,
        t,
    }
}

/// Merge connected components (8-connectivity) smaller than `min_area` into
/// the neighbour label they share the longest border with.
pub(crate) fn remove_speckles(l: &mut Labels, min_area: usize) {
    if min_area <= 1 {
        return;
    }
    let (w, h) = (l.w as isize, l.h as isize);
    let n = l.w * l.h;
    for _pass in 0..4 {
        let mut comp = vec![u32::MAX; n];
        let mut changed = false;
        let mut stack = Vec::new();
        let mut pixels = Vec::new();
        for start in 0..n {
            if comp[start] != u32::MAX {
                continue;
            }
            let lab = l.a[start];
            comp[start] = start as u32;
            stack.push(start);
            pixels.clear();
            let mut big = false;
            while let Some(i) = stack.pop() {
                if !big {
                    pixels.push(i);
                    if pixels.len() >= min_area {
                        big = true;
                    }
                }
                let (x, y) = ((i % l.w) as isize, (i / l.w) as isize);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let j = (ny * w + nx) as usize;
                        if comp[j] == u32::MAX && l.a[j] == lab {
                            comp[j] = start as u32;
                            stack.push(j);
                        }
                    }
                }
            }
            if big {
                continue;
            }
            // Count contacts with other labels.
            let mut contacts: HashMap<u16, usize> = HashMap::new();
            for &i in &pixels {
                let (x, y) = ((i % l.w) as isize, (i / l.w) as isize);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let j = (ny * w + nx) as usize;
                        if l.a[j] != lab {
                            *contacts.entry(l.a[j]).or_insert(0) += 1;
                        }
                    }
                }
            }
            let Some((&new, _)) = contacts
                .iter()
                .max_by_key(|(&k, &v)| (v, std::cmp::Reverse(k)))
            else {
                continue;
            };
            changed = true;
            for &i in &pixels {
                l.a[i] = new;
                l.b[i] = new;
                l.t[i] = 1.0;
                let (x, y) = ((i % l.w) as isize, (i / l.w) as isize);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let j = (ny * w + nx) as usize;
                        if l.b[j] == lab {
                            l.b[j] = new;
                            if l.a[j] == new {
                                l.t[j] = 1.0;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}
