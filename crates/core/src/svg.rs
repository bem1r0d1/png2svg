//! Figma-safe SVG writer: a flat list of `<path>` elements, absolute viewBox,
//! no transforms, `<use>`, styles, masks or filters. Path data uses relative
//! commands computed from *rounded* absolute coordinates, so rounding never
//! accumulates drift.

use std::fmt::Write;

use crate::fit::{FittedLoop, Seg};
use crate::geom::P;
use crate::shapes::Shape;

/// One SVG element: a path (contour + holes) or a native primitive.
pub(crate) struct SvgItem<'a> {
    pub loops: Vec<&'a FittedLoop>,
    /// Emit as `<circle>/<ellipse>/<rect>` (axis-aligned, no holes).
    pub native: Option<Shape>,
}

pub(crate) struct SvgLayer<'a> {
    pub id: String,
    pub rgba: [u8; 4],
    pub items: Vec<SvgItem<'a>>,
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

pub(crate) fn path_data<'a>(
    loops: impl IntoIterator<Item = &'a FittedLoop>,
    precision: u8,
) -> String {
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

fn num(v: f64, precision: u8) -> String {
    let n = Num {
        scale: 10f64.powi(precision as i32),
        prec: precision as usize,
    };
    let mut s = String::new();
    n.fmt(n.q(v), &mut s);
    if s.starts_with('.') {
        s.insert(0, '0');
    } else if s.starts_with("-.") {
        s.insert(1, '0');
    }
    s
}

fn write_item(s: &mut String, id: &str, rgba: [u8; 4], item: &SvgItem, precision: u8) {
    let f = |v: f64| num(v, precision);
    match item.native {
        Some(Shape::Ellipse { c, rx, ry, .. }) if (rx - ry).abs() < 1e-9 => {
            let _ = write!(
                s,
                r#"<circle id="{id}" cx="{}" cy="{}" r="{}""#,
                f(c.x),
                f(c.y),
                f(rx)
            );
        }
        Some(Shape::Ellipse { c, rx, ry, .. }) => {
            let _ = write!(
                s,
                r#"<ellipse id="{id}" cx="{}" cy="{}" rx="{}" ry="{}""#,
                f(c.x),
                f(c.y),
                f(rx),
                f(ry)
            );
        }
        Some(Shape::RoundRect { c, hw, hh, r, .. }) => {
            let _ = write!(
                s,
                r#"<rect id="{id}" x="{}" y="{}" width="{}" height="{}""#,
                f(c.x - hw),
                f(c.y - hh),
                f(hw * 2.0),
                f(hh * 2.0)
            );
            if r > 0.0 {
                let _ = write!(s, r#" rx="{}""#, f(r));
            }
        }
        None => {
            let _ = write!(s, r#"<path id="{id}""#);
        }
    }
    let _ = write!(s, r#" fill="{}""#, hex(rgba));
    if rgba[3] < 255 {
        let _ = write!(s, r#" fill-opacity="{:.3}""#, rgba[3] as f32 / 255.0);
    }
    if item.native.is_none() {
        let d = path_data(item.loops.iter().copied(), precision);
        let _ = write!(s, r#" fill-rule="evenodd" d="{d}""#);
    }
    s.push_str("/>\n");
}

pub(crate) fn write_svg(w: u32, h: u32, layers: &[SvgLayer], precision: u8) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" fill="none">"#
    );
    s.push('\n');
    for l in layers {
        match l.items.len() {
            0 => {}
            1 => write_item(&mut s, &l.id, l.rgba, &l.items[0], precision),
            _ => {
                let _ = writeln!(s, r#"<g id="{}">"#, l.id);
                for (k, item) in l.items.iter().enumerate() {
                    write_item(
                        &mut s,
                        &format!("{}-{}", l.id, k + 1),
                        l.rgba,
                        item,
                        precision,
                    );
                }
                s.push_str("</g>\n");
            }
        }
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
