//! Sub-pixel iso-contours (marching squares) of a coverage field.
//!
//! The field is sampled at pixel centres. Because anti-aliased pixels carry
//! fractional coverage, the 0.5 iso-line lands *between* pixel centres at the
//! true edge position — this is what removes "staircase" artefacts.

use std::collections::{BTreeMap, HashMap};

use crate::geom::P;

const ISO: f32 = 0.5;

/// Closed loops where `field >= 0.5`. The field is `w*h`, values outside the
/// image are treated as 0 so every loop is closed. Loops are oriented
/// consistently (outer boundaries and holes have opposite winding).
pub(crate) fn iso_contours(field: &[f32], w: usize, h: usize) -> Vec<Vec<P>> {
    let gw = w + 2;
    let gh = h + 2;
    let val = |gx: usize, gy: usize| -> f32 {
        if gx == 0 || gy == 0 || gx > w || gy > h {
            0.0
        } else {
            field[(gy - 1) * w + (gx - 1)]
        }
    };
    let pos = |gx: usize, gy: usize| P::new(gx as f64 - 0.5, gy as f64 - 0.5);
    let hkey = |gx: usize, gy: usize| ((gy * gw + gx) * 2) as u32;
    let vkey = |gx: usize, gy: usize| ((gy * gw + gx) * 2 + 1) as u32;

    let mut next: BTreeMap<u32, u32> = BTreeMap::new();
    let mut points: HashMap<u32, P> = HashMap::new();

    let crossing = |key: u32, q0: P, v0: f32, q1: P, v1: f32, points: &mut HashMap<u32, P>| {
        points.entry(key).or_insert_with(|| {
            let s = ((ISO - v0) / (v1 - v0)).clamp(1e-3, 1.0 - 1e-3) as f64;
            q0.lerp(q1, s)
        });
        key
    };

    for cy in 0..gh - 1 {
        for cx in 0..gw - 1 {
            let v = [
                val(cx, cy),
                val(cx + 1, cy),
                val(cx + 1, cy + 1),
                val(cx, cy + 1),
            ];
            let inside = v.map(|x| x >= ISO);
            let case = inside
                .iter()
                .enumerate()
                .fold(0u8, |m, (i, &b)| m | ((b as u8) << i));
            if case == 0 || case == 15 {
                continue;
            }
            let q = [
                pos(cx, cy),
                pos(cx + 1, cy),
                pos(cx + 1, cy + 1),
                pos(cx, cy + 1),
            ];
            // Edge index: 0 top (0-1), 1 right (1-2), 2 bottom (2-3), 3 left (3-0).
            let keys = [
                hkey(cx, cy),
                vkey(cx + 1, cy),
                hkey(cx, cy + 1),
                vkey(cx, cy),
            ];
            let mut edge_pt = |e: usize| -> (u32, P) {
                let (c0, c1) = (e, (e + 1) % 4);
                let k = crossing(keys[e], q[c0], v[c0], q[c1], v[c1], &mut points);
                (k, points[&k])
            };
            let mut segs: [(usize, usize, Option<usize>); 2] = [(0, 0, None); 2];
            let nseg;
            match case {
                5 | 10 => {
                    let center_in = (v.iter().sum::<f32>() * 0.25) >= ISO;
                    // Corner cut off by each segment: edges adjacent to corner c are (c-1, c).
                    let cut = if (case == 5) == center_in {
                        [1, 3]
                    } else {
                        [0, 2]
                    };
                    segs[0] = ((cut[0] + 3) % 4, cut[0], Some(cut[0]));
                    segs[1] = ((cut[1] + 3) % 4, cut[1], Some(cut[1]));
                    nseg = 2;
                }
                _ => {
                    let mut es = [0usize; 2];
                    let mut k = 0;
                    for e in 0..4 {
                        if inside[e] != inside[(e + 1) % 4] {
                            es[k] = e;
                            k += 1;
                        }
                    }
                    segs[0] = (es[0], es[1], None);
                    nseg = 1;
                }
            }
            for &(e0, e1, cut) in &segs[..nseg] {
                let (k0, p0) = edge_pt(e0);
                let (k1, p1) = edge_pt(e1);
                let dir = p1 - p0;
                // Orientation: inside must be on the positive-cross side.
                let s: f64 = match cut {
                    Some(c) => {
                        let sgn = if inside[c] { 1.0 } else { -1.0 };
                        sgn * dir.cross(q[c] - p0)
                    }
                    None => (0..4)
                        .map(|c| {
                            let sgn = if inside[c] { 1.0 } else { -1.0 };
                            sgn * dir.cross(q[c] - p0)
                        })
                        .sum(),
                };
                if s >= 0.0 {
                    next.insert(k0, k1);
                } else {
                    next.insert(k1, k0);
                }
            }
        }
    }

    let mut loops = Vec::new();
    let mut visited: HashMap<u32, bool> = HashMap::with_capacity(next.len());
    for &start in next.keys() {
        if visited.contains_key(&start) {
            continue;
        }
        let mut lp = Vec::new();
        let mut k = start;
        loop {
            if visited.insert(k, true).is_some() {
                break;
            }
            lp.push(points[&k]);
            match next.get(&k) {
                Some(&n) => k = n,
                None => break,
            }
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::polygon_area;

    #[test]
    fn square_area_and_subpixel() {
        // 10x10 field with a 4x4 solid block and half-covered right column.
        let (w, h) = (10, 10);
        let mut f = vec![0.0; w * h];
        for y in 3..7 {
            for x in 3..7 {
                f[y * w + x] = 1.0;
            }
            f[y * w + 7] = 0.5; // edge exactly at x = 7.5 .. treated as inside
        }
        let loops = iso_contours(&f, w, h);
        assert_eq!(loops.len(), 1);
        let a = polygon_area(&loops[0]).abs();
        assert!(a > 12.0 && a < 22.0, "area {a}");
    }

    #[test]
    fn hole_has_opposite_winding() {
        let (w, h) = (12, 12);
        let mut f = vec![0.0; w * h];
        for y in 1..11 {
            for x in 1..11 {
                f[y * w + x] = if (4..8).contains(&x) && (4..8).contains(&y) {
                    0.0
                } else {
                    1.0
                };
            }
        }
        let loops = iso_contours(&f, w, h);
        assert_eq!(loops.len(), 2);
        let s: Vec<f64> = loops.iter().map(|l| polygon_area(l)).collect();
        assert!(s[0].signum() != s[1].signum(), "{s:?}");
    }
}
