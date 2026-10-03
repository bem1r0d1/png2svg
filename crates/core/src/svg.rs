//! Figma-safe SVG writer: a flat list of `<path>` elements, absolute viewBox,
//! no transforms, `<use>`, styles, masks or filters. Path data uses relative
//! commands computed from *rounded* absolute coordinates, so rounding never
//! accumulates drift.

use std::fmt::Write;

use crate::fit::{FittedLoop, Seg};
use crate::geom::P;

pub(crate) struct SvgLayer<'a> {
    pub id: String,
    pub rgba: [u8; 4],
    pub loops: &'a [FittedLoop],
}

pub(crate) fn hex(c: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

struct Num {
    scale: f64,
    prec: usize,
}

impl Num {
    fn q(&self, v: f64) -> i64 {
        (v * self.scale).round() as i64
    }
    fn fmt(&self, v: i64, out: &mut String) {
        if self.prec == 0 {
            let _ = write!(out, "{v}");
            return;
        }
        let neg = v < 0;
        let a = v.unsigned_abs();
        let s = self.scale as u64;
        let (int, mut frac) = (a / s, a % s);
        if neg {
            out.push('-');
        }
        if frac == 0 {
            let _ = write!(out, "{int}");
            return;
        }
        let mut digits = self.prec;
        while frac % 10 == 0 {
            frac /= 10;
            digits -= 1;
        }
        if int == 0 {
            let _ = write!(out, ".{frac:0digits$}");
        } else {
            let _ = write!(out, "{int}.{frac:0digits$}");
        }
    }
}

/// Appends numbers, inserting a separator only when needed.
fn push_nums(out: &mut String, num: &Num, vals: &[i64]) {
    for &v in vals {
        let mut s = String::new();
        num.fmt(v, &mut s);
        let need_sep = match out.chars().last() {
            Some(c) if c.is_ascii_digit() => true,
            Some('.') => true,
            _ => false,
        };
        // A leading '-' already separates; a leading '.' only does after a number containing '.'.
        if need_sep && !s.starts_with('-') {
            out.push(' ');
        }
        out.push_str(&s);
    }
}

pub(crate) fn path_data(loops: &[FittedLoop], precision: u8) -> String {
    let num = Num {
        scale: 10f64.powi(precision as i32),
        prec: precision as usize,
    };
    let mut d = String::new();
    for lp in loops {
        let q = |p: P| (num.q(p.x), num.q(p.y));
        let (sx, sy) = q(lp.start);
        d.push('M');
        push_nums(&mut d, &num, &[sx, sy]);
        let (mut cx, mut cy) = (sx, sy);
        let nseg = lp.segs.len();
        for (k, seg) in lp.segs.iter().enumerate() {
            match *seg {
                Seg::Line(p) => {
                    let (x, y) = q(p);
                    if k + 1 == nseg && (x, y) == (sx, sy) {
                        break; // closing line is implied by 'z'
                    }
                    if (x, y) == (cx, cy) {
                        continue;
                    }
                    if y == cy {
                        d.push('h');
                        push_nums(&mut d, &num, &[x - cx]);
                    } else if x == cx {
                        d.push('v');
                        push_nums(&mut d, &num, &[y - cy]);
                    } else {
                        d.push('l');
                        push_nums(&mut d, &num, &[x - cx, y - cy]);
                    }
                    (cx, cy) = (x, y);
                }
                Seg::Cubic(c1, c2, p) => {
                    let (x1, y1) = q(c1);
                    let (x2, y2) = q(c2);
                    let (x, y) = q(p);
                    d.push('c');
                    push_nums(
                        &mut d,
                        &num,
                        &[x1 - cx, y1 - cy, x2 - cx, y2 - cy, x - cx, y - cy],
                    );
                    (cx, cy) = (x, y);
                }
            }
        }
        d.push('z');
    }
    d
}

pub(crate) fn write_svg(w: u32, h: u32, layers: &[SvgLayer], precision: u8) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" fill="none">"#
    );
    s.push('\n');
    for l in layers {
        let d = path_data(l.loops, precision);
        if d.is_empty() {
            continue;
        }
        let _ = write!(s, r#"<path id="{}" fill="{}""#, l.id, hex(l.rgba));
        if l.rgba[3] < 255 {
            let _ = write!(s, r#" fill-opacity="{:.3}""#, l.rgba[3] as f32 / 255.0);
        }
        let _ = write!(s, r#" fill-rule="evenodd" d="{d}"/>"#);
        s.push('\n');
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_numbers() {
        let lp = FittedLoop {
            start: P::new(1.0, 2.5),
            segs: vec![
                Seg::Line(P::new(1.0, -0.25)),
                Seg::Line(P::new(3.0, -0.25)),
                Seg::Line(P::new(1.0, 2.5)),
            ],
        };
        assert_eq!(path_data(&[lp], 2), "M1 2.5v-2.75h2z");
    }
}
