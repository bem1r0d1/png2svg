//! Minimal 2D geometry helpers.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

impl P {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: P) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn dist(self, o: P) -> f64 {
        (self - o).len()
    }
    /// Unit vector; zero vector stays zero.
    pub fn norm(self) -> P {
        let l = self.len();
        if l > 1e-12 {
            self * (1.0 / l)
        } else {
            P::default()
        }
    }
    pub fn lerp(self, o: P, t: f64) -> P {
        self + (o - self) * t
    }
}

impl Add for P {
    type Output = P;
    fn add(self, o: P) -> P {
        P::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for P {
    type Output = P;
    fn sub(self, o: P) -> P {
        P::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for P {
    type Output = P;
    fn mul(self, s: f64) -> P {
        P::new(self.x * s, self.y * s)
    }
}
impl Neg for P {
    type Output = P;
    fn neg(self) -> P {
        P::new(-self.x, -self.y)
    }
}

/// Signed area of a closed polygon (positive = counter-clockwise in y-up coordinates).
pub fn polygon_area(pts: &[P]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        a += pts[i].cross(pts[(i + 1) % n]);
    }
    a * 0.5
}

/// Least-squares line through points: returns (centroid, unit direction, rms residual).
pub fn fit_line(pts: &[P]) -> (P, P, f64) {
    let n = pts.len() as f64;
    let c = pts.iter().fold(P::default(), |s, &p| s + p) * (1.0 / n);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for &p in pts {
        let d = p - c;
        sxx += d.x * d.x;
        sxy += d.x * d.y;
        syy += d.y * d.y;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let dir = P::new(theta.cos(), theta.sin());
    let normal = P::new(-dir.y, dir.x);
    let ss: f64 = pts.iter().map(|&p| (p - c).dot(normal).powi(2)).sum();
    (c, dir, (ss / n).sqrt())
}

/// Intersection of two infinite lines given by point + direction.
pub fn intersect_lines(p1: P, d1: P, p2: P, d2: P) -> Option<P> {
    let den = d1.cross(d2);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = (p2 - p1).cross(d2) / den;
    Some(p1 + d1 * t)
}

/// Distance from point to segment.
pub fn dist_to_segment(p: P, a: P, b: P) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    if l2 < 1e-12 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}
