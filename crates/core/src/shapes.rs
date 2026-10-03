//! Parameterized shape detection (idea from Vectorizer.AI): whole contours that
//! are circles, ellipses, rectangles or rounded rectangles are replaced by the
//! exact primitive. In Figma, axis-aligned ones become native Ellipse /
//! Rectangle layers with editable size and corner radius.

use std::f64::consts::{FRAC_PI_2, PI};

use crate::fit::{FittedLoop, Seg};
use crate::geom::{polygon_area, P};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    /// Centre, semi-axes, rotation (radians). `rx == ry` for circles.
    Ellipse { c: P, rx: f64, ry: f64, angle: f64 },
    /// Centre, half extents, corner radius, rotation (radians).
    RoundRect {
        c: P,
        hw: f64,
        hh: f64,
        r: f64,
        angle: f64,
    },
}

impl Shape {
    pub fn is_axis_aligned(&self) -> bool {
        match *self {
            Shape::Ellipse { rx, ry, angle, .. } => angle == 0.0 || (rx - ry).abs() < 1e-9,
            Shape::RoundRect { angle, .. } => angle == 0.0,
        }
    }
}

/// Polygon area moments: (area, centroid, central second moments xx, yy, xy).
fn moments(pts: &[P]) -> (f64, P, f64, f64, f64) {
    let o = pts[0];
    let n = pts.len();
    let (mut a, mut cx, mut cy, mut ixx, mut iyy, mut ixy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let p = pts[i] - o;
        let q = pts[(i + 1) % n] - o;
        let c = p.cross(q);
        a += c;
        cx += (p.x + q.x) * c;
        cy += (p.y + q.y) * c;
        ixx += (p.x * p.x + p.x * q.x + q.x * q.x) * c;
        iyy += (p.y * p.y + p.y * q.y + q.y * q.y) * c;
        ixy += (p.x * q.y + 2.0 * p.x * p.y + 2.0 * q.x * q.y + q.x * p.y) * c;
    }
    a *= 0.5;
    if a.abs() < 1e-9 {
        return (0.0, o, 0.0, 0.0, 0.0);
    }
    cx /= 6.0 * a;
    cy /= 6.0 * a;
    let mxx = ixx / 12.0 / a - cx * cx;
    let myy = iyy / 12.0 / a - cy * cy;
    let mxy = ixy / 24.0 / a - cx * cy;
    (a.abs(), o + P::new(cx, cy), mxx, myy, mxy)
}

fn rotate(p: P, a: f64) -> P {
    let (s, c) = a.sin_cos();
    P::new(p.x * c - p.y * s, p.x * s + p.y * c)
}

/// Snaps angles within ~1° of an axis to exactly 0, normalising to [0, period).
fn snap_angle(a: f64, period: f64) -> f64 {
    let a = a.rem_euclid(period);
    let tol = 1.2f64.to_radians();
    if a < tol || period - a < tol {
        0.0
    } else {
        a
    }
}

struct Err {
    max: f64,
    rms: f64,
}

fn errors(pts: &[P], dist: impl Fn(P) -> f64) -> Err {
    let (mut max, mut ss) = (0f64, 0.0);
    for &p in pts {
        let d = dist(p).abs();
        max = max.max(d);
        ss += d * d;
    }
    Err {
        max,
        rms: (ss / pts.len() as f64).sqrt(),
    }
}

fn ellipse_candidate(pts: &[P]) -> Option<(Shape, Err)> {
    let (area, c, mxx, myy, mxy) = moments(pts);
    if area < 3.0 {
        return None;
    }
    let tr = mxx + myy;
    let disc = (((mxx - myy) * 0.5).powi(2) + mxy * mxy).sqrt();
    let (l1, l2) = (tr * 0.5 + disc, tr * 0.5 - disc);
    if l2 <= 0.0 {
        return None;
    }
    // Filled ellipse: eigenvalues of the covariance are a²/4 and b²/4.
    let (mut rx, mut ry) = (2.0 * l1.sqrt(), 2.0 * l2.sqrt());
    let mut angle = 0.5 * (2.0 * mxy).atan2(mxx - myy);
    if (rx - ry).abs() < 0.02 * rx.max(1.0) {
        let r = (area / PI).sqrt();
        rx = r;
        ry = r;
        angle = 0.0;
    } else {
        angle = angle.rem_euclid(PI);
        // Prefer the representation closest to axis-aligned (swap axes if needed).
        if angle > FRAC_PI_2 * 0.5 && angle < FRAC_PI_2 * 1.5 {
            angle -= FRAC_PI_2;
            std::mem::swap(&mut rx, &mut ry);
        }
        angle = snap_angle(angle, PI);
    }
    let e = errors(pts, |p| {
        let q = rotate(p - c, -angle);
        let rho = ((q.x / rx).powi(2) + (q.y / ry).powi(2)).sqrt();
        if rho < 1e-9 {
            rx.min(ry)
        } else {
            q.len() * (1.0 - 1.0 / rho)
        }
    });
    Some((Shape::Ellipse { c, rx, ry, angle }, e))
}

/// Dominant edge direction modulo 90° (length-weighted histogram).
fn dominant_axis(pts: &[P]) -> f64 {
    const BINS: usize = 180; // 0.5° bins over 90°
    let mut h = [0f64; BINS];
    let n = pts.len();
    for i in 0..n {
        let d = pts[(i + 1) % n] - pts[i];
        let len = d.len();
        if len < 1e-6 {
            continue;
        }
        let a = d.y.atan2(d.x).rem_euclid(FRAC_PI_2);
        let b = ((a / FRAC_PI_2) * BINS as f64) as usize % BINS;
        h[b] += len;
    }
    let mut best = (0.0, 0);
    for b in 0..BINS {
        let s: f64 = (-2i32..=2)
            .map(|k| h[(b as i32 + k).rem_euclid(BINS as i32) as usize])
            .sum();
        if s > best.0 {
            best = (s, b);
        }
    }
    snap_angle((best.1 as f64 + 0.5) / BINS as f64 * FRAC_PI_2, FRAC_PI_2)
}

fn round_rect_sdf(q: P, hw: f64, hh: f64, r: f64) -> f64 {
    let dx = q.x.abs() - (hw - r);
    let dy = q.y.abs() - (hh - r);
    let outside = P::new(dx.max(0.0), dy.max(0.0)).len();
    let inside = dx.max(dy).min(0.0);
    outside + inside - r
}

fn rect_candidate(pts: &[P]) -> Option<(Shape, Err)> {
    let angle = dominant_axis(pts);
    let local: Vec<P> = pts.iter().map(|&p| rotate(p, -angle)).collect();
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in &local {
        x0 = x0.min(p.x);
        x1 = x1.max(p.x);
        y0 = y0.min(p.y);
        y1 = y1.max(p.y);
    }
    let (hw, hh) = ((x1 - x0) * 0.5, (y1 - y0) * 0.5);
    if hw < 1.0 || hh < 1.0 {
        return None;
    }
    let lc = P::new((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let eval = |r: f64| errors(&local, |q| round_rect_sdf(q - lc, hw, hh, r));
    let rmax = hw.min(hh);
    // Coarse scan then refine the corner radius.
    let mut best = (f64::MAX, 0.0);
    let steps = (rmax / 0.25).ceil().max(1.0) as usize;
    for k in 0..=steps {
        let r = rmax * k as f64 / steps as f64;
        let e = eval(r).max;
        if e < best.0 {
            best = (e, r);
        }
    }
    let (lo, hi) = ((best.1 - 0.3).max(0.0), (best.1 + 0.3).min(rmax));
    for k in 0..=30 {
        let r = lo + (hi - lo) * k as f64 / 30.0;
        let e = eval(r).max;
        if e < best.0 {
            best = (e, r);
        }
    }
    let mut r = best.1;
    if r < 0.35 {
        r = 0.0;
    } else if rmax - r < 0.25 {
        r = rmax; // stadium / pill
    }
    let e = eval(r);
    Some((
        Shape::RoundRect {
            c: rotate(lc, angle),
            hw,
            hh,
            r,
            angle,
        },
        e,
    ))
}

/// Detects a primitive matching the whole contour within `tol` px.
pub(crate) fn detect(pts: &[P], tol: f64) -> Option<Shape> {
    if pts.len() < 8 || polygon_area(pts).abs() < 6.0 {
        return None;
    }
    let ok = |e: &Err| e.max <= tol && e.rms <= tol * 0.45;
    let ell = ellipse_candidate(pts).filter(|(_, e)| ok(e));
    let rect = rect_candidate(pts).filter(|(_, e)| ok(e)).map(|(s, e)| {
        // A fully rounded square is a circle.
        match s {
            Shape::RoundRect { c, hw, hh, r, .. }
                if (hw - hh).abs() < tol && r >= hw.min(hh) - 0.8 =>
            {
                let r = (hw + hh) * 0.5;
                (
                    Shape::Ellipse {
                        c,
                        rx: r,
                        ry: r,
                        angle: 0.0,
                    },
                    e,
                )
            }
            s => (s, e),
        }
    });
    match (ell, rect) {
        // A circle is also a fully rounded square; prefer the ellipse on ties.
        (Some((s1, e1)), Some((s2, e2))) => Some(if e2.max < e1.max * 0.8 { s2 } else { s1 }),
        (Some((s, _)), None) | (None, Some((s, _))) => Some(s),
        _ => None,
    }
}

/// Cubic Bézier outline of a primitive, following the winding of `like`.
pub(crate) fn to_loop(s: &Shape, ccw: bool) -> FittedLoop {
    const K: f64 = 0.552_284_749_830_793_6; // 4/3·(√2−1)
    let mut pts: Vec<(P, Seg)> = Vec::new(); // (start, segment) in local frame
    let (c, angle) = match *s {
        Shape::Ellipse { c, rx, ry, angle } => {
            let q = [
                P::new(rx, 0.0),
                P::new(0.0, ry),
                P::new(-rx, 0.0),
                P::new(0.0, -ry),
            ];
            for i in 0..4 {
                let (a, b) = (q[i], q[(i + 1) % 4]);
                // Tangent at a points towards b's direction around the ellipse.
                let ta = q[(i + 1) % 4] * K;
                let tb = q[i] * K;
                pts.push((a, Seg::Cubic(a + ta, b + tb, b)));
            }
            (c, angle)
        }
        Shape::RoundRect {
            c,
            hw,
            hh,
            r,
            angle,
        } => {
            // Corners: (+,+), (−,+), (−,−), (+,−) going counter-clockwise (y up).
            let corners = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
            for (i, &(sx, sy)) in corners.iter().enumerate() {
                // Arc around corner i from the edge before it to the edge after it.
                let (nx, ny) = corners[(i + 1) % 4];
                let start = if i % 2 == 0 {
                    P::new(sx * hw, sy * (hh - r))
                } else {
                    P::new(sx * (hw - r), sy * hh)
                };
                let end = if i % 2 == 0 {
                    P::new(sx * (hw - r), sy * hh)
                } else {
                    P::new(sx * hw, sy * (hh - r))
                };
                if r > 0.0 {
                    let h1 = if i % 2 == 0 {
                        P::new(0.0, sy * r * K)
                    } else {
                        P::new(sx * r * K, 0.0)
                    };
                    let h2 = if i % 2 == 0 {
                        P::new(sx * r * K, 0.0)
                    } else {
                        P::new(0.0, sy * r * K)
                    };
                    pts.push((start, Seg::Cubic(start + h1, end + h2, end)));
                }
                // Straight edge to the start of the next corner.
                let next_start = if (i + 1) % 2 == 0 {
                    P::new(nx * hw, ny * (hh - r))
                } else {
                    P::new(nx * (hw - r), ny * hh)
                };
                if end.dist(next_start) > 1e-9 {
                    pts.push((end, Seg::Line(next_start)));
                }
            }
            (c, angle)
        }
    };
    let tf = |p: P| c + rotate(p, angle);
    let mut segs: Vec<Seg> = pts
        .iter()
        .map(|&(_, s)| match s {
            Seg::Line(p) => Seg::Line(tf(p)),
            Seg::Cubic(a, b, p) => Seg::Cubic(tf(a), tf(b), tf(p)),
        })
        .collect();
    let mut start = tf(pts[0].0);
    if !ccw {
        // Reverse the path direction.
        let starts: Vec<P> = pts.iter().map(|&(p, _)| tf(p)).collect();
        let n = segs.len();
        let mut rev = Vec::with_capacity(n);
        for i in (0..n).rev() {
            let to = starts[i];
            rev.push(match segs[i] {
                Seg::Line(_) => Seg::Line(to),
                Seg::Cubic(a, b, _) => Seg::Cubic(b, a, to),
            });
        }
        start = match segs[n - 1] {
            Seg::Line(p) | Seg::Cubic(_, _, p) => p,
        };
        segs = rev;
    }
    FittedLoop { start, segs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn ring(c: P, rx: f64, ry: f64, angle: f64, n: usize) -> Vec<P> {
        (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * TAU;
                c + rotate(P::new(rx * t.cos(), ry * t.sin()), angle)
            })
            .collect()
    }

    #[test]
    fn detects_circle() {
        let s = detect(&ring(P::new(40.0, 30.0), 20.0, 20.0, 0.0, 160), 0.4).unwrap();
        let Shape::Ellipse { c, rx, ry, angle } = s else {
            panic!("{s:?}")
        };
        assert!((c.x - 40.0).abs() < 0.05 && (c.y - 30.0).abs() < 0.05);
        assert!((rx - 20.0).abs() < 0.05 && rx == ry && angle == 0.0);
    }

    #[test]
    fn detects_rotated_ellipse() {
        let s = detect(&ring(P::new(50.0, 50.0), 30.0, 12.0, 0.6, 200), 0.4).unwrap();
        let Shape::Ellipse { rx, ry, angle, .. } = s else {
            panic!("{s:?}")
        };
        assert!(
            (rx - 30.0).abs() < 0.1 && (ry - 12.0).abs() < 0.1,
            "{rx} {ry}"
        );
        assert!((angle - 0.6).abs() < 0.01);
    }

    #[test]
    fn detects_rounded_rect() {
        let (hw, hh, r) = (30.0, 18.0, 6.0);
        let mut pts = Vec::new();
        for i in 0..400 {
            let t = i as f64 / 400.0 * TAU;
            // Sample a rounded rect by marching along its SDF zero set radially.
            let dir = P::new(t.cos(), t.sin());
            let (mut lo, mut hi) = (0.0, 100.0);
            for _ in 0..60 {
                let m = (lo + hi) * 0.5;
                if round_rect_sdf(dir * m, hw, hh, r) < 0.0 {
                    lo = m
                } else {
                    hi = m
                }
            }
            pts.push(P::new(60.0, 40.0) + dir * lo);
        }
        let s = detect(&pts, 0.4).unwrap();
        let Shape::RoundRect {
            c,
            hw: w,
            hh: h,
            r: rr,
            angle,
        } = s
        else {
            panic!("{s:?}")
        };
        assert!((c.x - 60.0).abs() < 0.1 && (c.y - 40.0).abs() < 0.1);
        assert!(
            (w - hw).abs() < 0.1 && (h - hh).abs() < 0.1 && (rr - r).abs() < 0.2 && angle == 0.0
        );
    }

    #[test]
    fn rejects_star() {
        let pts: Vec<P> = (0..200)
            .map(|i| {
                let t = i as f64 / 200.0 * TAU;
                let r = 20.0 + 6.0 * (5.0 * t).cos();
                P::new(50.0 + r * t.cos(), 50.0 + r * t.sin())
            })
            .collect();
        assert!(detect(&pts, 0.4).is_none());
    }

    #[test]
    fn outline_matches_shape() {
        let s = Shape::RoundRect {
            c: P::new(10.0, 10.0),
            hw: 5.0,
            hh: 3.0,
            r: 1.0,
            angle: 0.0,
        };
        let l = to_loop(&s, true);
        assert_eq!(l.segs.len(), 8);
        let end = match l.segs.last().unwrap() {
            Seg::Line(p) | Seg::Cubic(_, _, p) => *p,
        };
        assert!(end.dist(l.start) < 1e-9);
    }
}
