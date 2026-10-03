//! Stacked layering: one layer per palette colour, bottom → top.
//!
//! A layer's shape is its own region plus the parts of *later* (upper) layers
//! it touches (a small dilation band and fully enclosed holes). Upper layers
//! paint over those parts, so adjacent shapes overlap instead of abutting and
//! no hairline seams can appear at any zoom level.

use crate::analyze::gaussian_blur;
use crate::contour::iso_contours;
use crate::geom::{polygon_area, P};
use crate::quantize::Palette;
use crate::segment::Labels;

/// How far (px) a layer extends under the layers above it.
const UNDERLAP: f32 = 2.0;

pub(crate) struct Layer {
    pub label: u16,
    pub area: usize,
    pub loops: Vec<Vec<P>>,
}

/// Contours enclosing less than `min_area` px² (islands or holes) are dropped.
pub(crate) fn build_layers(l: &Labels, pal: &Palette, smoothing: f32, min_area: f64) -> Vec<Layer> {
    let n = l.w * l.h;
    let transparent = pal.transparent_index().map(|i| i as u16);
    let mut area = vec![0usize; pal.colors.len()];
    for &a in &l.a {
        area[a as usize] += 1;
    }
    let mut order: Vec<u16> = (0..pal.colors.len() as u16)
        .filter(|&c| Some(c) != transparent && area[c as usize] > 0)
        .collect();
    order.sort_by_key(|&c| (std::cmp::Reverse(area[c as usize]), c));
    let mut rank = vec![usize::MAX; pal.colors.len()];
    for (r, &c) in order.iter().enumerate() {
        rank[c as usize] = r;
    }

    let mut layers = Vec::with_capacity(order.len());
    // The underlap must survive the edge smoothing.
    let offsets = disc_offsets(UNDERLAP + smoothing);
    let mut field = vec![0f32; n];
    let mut own = vec![0f32; n];
    let mut hole = vec![false; n];
    for (r, &lab) in order.iter().enumerate() {
        let is_later = |c: u16| rank[c as usize] != usize::MAX && rank[c as usize] > r;
        for (i, o) in own.iter_mut().enumerate() {
            *o = l.cov(i, lab);
        }
        enclosed_later_holes(l, lab, &is_later, &mut hole);
        for i in 0..n {
            let (a, b, t) = (l.a[i], l.b[i], l.t[i]);
            let mut later = 0.0;
            if is_later(a) {
                later += t;
            }
            if b != a && is_later(b) {
                later += 1.0 - t;
            }
            let reach = if hole[i] || later == 0.0 {
                1.0
            } else {
                dilated(&own, l.w, l.h, i, &offsets)
            };
            field[i] = (own[i] + later * reach).min(1.0);
        }
        gaussian_blur(&mut field, l.w, l.h, smoothing);
        preserve_thin_lines(&mut field, l.w, l.h);
        let mut loops = iso_contours(&field, l.w, l.h);
        loops.retain(|lp| polygon_area(lp).abs() >= min_area.max(0.3));
        if !loops.is_empty() {
            layers.push(Layer {
                label: lab,
                area: area[lab as usize],
                loops,
            });
        }
    }
    layers
}

/// Lines thinner than a pixel never reach 0.5 coverage, so a plain 0.5
/// iso-line breaks them into dashes. Ridge pixels (local maxima across the
/// line) are raised just enough for the iso-line to enclose them with a width
/// close to the line's total coverage.
fn preserve_thin_lines(field: &mut [f32], w: usize, h: usize) {
    const DIRS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
    let src = field.to_vec();
    let get = |x: isize, y: isize| -> f32 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            0.0
        } else {
            src[y as usize * w + x as usize]
        }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let f = get(x, y);
            if !(0.12..0.5).contains(&f) {
                continue;
            }
            // Pixels touching a solid interior are ordinary anti-aliased edges.
            let touches_solid = (-1..=1).any(|dy| (-1..=1).any(|dx| get(x + dx, y + dy) >= 0.9));
            if touches_solid {
                continue;
            }
            let mut boost = f;
            for &(dx, dy) in &DIRS {
                let (n1, n2) = (get(x - dx, y - dy), get(x + dx, y + dy));
                let (lo, hi) = (n1.min(n2), n1.max(n2));
                if f < hi || hi >= 0.5 || f - lo < 0.25 {
                    continue;
                }
                let total = f + n1 + n2;
                if total < 0.35 {
                    continue;
                }
                // Peak value whose linear iso-crossings sit `hw` px from the centre.
                let hw = (total * 0.5).clamp(0.35, 0.49);
                let p = (0.5 - hi * hw) / (1.0 - hw);
                boost = boost.max(p);
            }
            field[y as usize * w + x as usize] = boost;
        }
    }
}

/// Integer offsets inside a disc of radius `r`.
fn disc_offsets(r: f32) -> Vec<(isize, isize)> {
    let ri = r.ceil() as isize;
    let mut v = Vec::new();
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            if (dx * dx + dy * dy) as f32 <= r * r + 1e-3 {
                v.push((dx, dy));
            }
        }
    }
    v
}

/// Max-filter of the layer's own coverage over a disc: the 0.5 iso-line of the
/// result is the layer outline offset outwards by the disc radius, keeping its
/// sub-pixel smoothness (a plain pixel dilation would produce a staircase).
fn dilated(own: &[f32], w: usize, h: usize, i: usize, offsets: &[(isize, isize)]) -> f32 {
    let (x, y) = ((i % w) as isize, (i / w) as isize);
    let mut m = 0f32;
    for &(dx, dy) in offsets {
        let (nx, ny) = (x + dx, y + dy);
        if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
            continue;
        }
        m = m.max(own[ny as usize * w + nx as usize]);
        if m >= 1.0 {
            break;
        }
    }
    m
}

/// Marks holes of this layer's region that consist only of later-layer pixels.
fn enclosed_later_holes(l: &Labels, lab: u16, is_later: &impl Fn(u16) -> bool, ext: &mut [bool]) {
    let (w, h) = (l.w, l.h);
    let n = w * h;
    ext.iter_mut().for_each(|e| *e = false);

    let mut seen = vec![false; n];
    let mut stack = Vec::new();
    let mut comp = Vec::new();
    for start in 0..n {
        if seen[start] || l.a[start] == lab {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        comp.clear();
        let (mut border, mut all_later) = (false, true);
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                border = true;
            }
            if !is_later(l.a[i]) {
                all_later = false;
            }
            let mut push = |j: usize| {
                if !seen[j] && l.a[j] != lab {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if !border && all_later {
            for &i in &comp {
                ext[i] = true;
            }
        }
    }
}
