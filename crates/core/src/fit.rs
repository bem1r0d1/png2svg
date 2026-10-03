//! Contour → Bézier path: corner detection, corner sharpening, axis snapping
//! and least-squares cubic fitting (Schneider, "An Algorithm for Automatically
//! Fitting Digitized Curves", Graphics Gems 1990).

use crate::geom::{dist_to_segment, fit_line, intersect_lines, P};
use crate::options::Params;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Seg {
    Line(P),
    Cubic(P, P, P),
}

#[derive(Clone, Debug)]
pub(crate) struct FittedLoop {
    pub start: P,
    pub segs: Vec<Seg>,
}

/// Cyclic polyline with cumulative arc length.
struct Poly<'a> {
    p: &'a [P],
    cum: Vec<f64>,
    len: f64,
}

impl<'a> Poly<'a> {
    fn new(p: &'a [P]) -> Self {
        let n = p.len();
        let mut cum = Vec::with_capacity(n + 1);
        cum.push(0.0);
        for i in 0..n {
            let d = p[i].dist(p[(i + 1) % n]);
            cum.push(cum[i] + d);
        }
        let len = cum[n];
        Self { p, cum, len }
    }
    fn n(&self) -> usize {
        self.p.len()
    }
    /// Point at arc length `s` (wrapped).
    fn at(&self, s: f64) -> P {
        let s = s.rem_euclid(self.len);
        let i = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
        .min(self.n() - 1);
        let seg = self.cum[i + 1] - self.cum[i];
        let t = if seg > 1e-12 {
            (s - self.cum[i]) / seg
        } else {
            0.0
        };
        self.p[i].lerp(self.p[(i + 1) % self.n()], t)
    }
    /// Forward arc distance from index i to index j.
    fn arc(&self, i: usize, j: usize) -> f64 {
        let d = self.cum[j] - self.cum[i];
        if d < 0.0 {
            d + self.len
        } else {
            d
        }
    }
}

fn dedup(pts: &[P]) -> Vec<P> {
    let mut out: Vec<P> = Vec::with_capacity(pts.len());
    for &p in pts {
        if out.last().is_none_or(|&q: &P| q.dist(p) > 1e-3) {
            out.push(p);
        }
    }
    while out.len() > 1 && out[0].dist(*out.last().unwrap()) <= 1e-3 {
        out.pop();
    }
    out
}

pub(crate) fn fit_loop(raw: &[P], prm: &Params) -> FittedLoop {
    let pts = dedup(raw);
    if pts.len() < 5 {
        return polygon(&pts);
    }
    let poly = Poly::new(&pts);
    let n = poly.n();
    let scale = prm.corner_scale.min(poly.len / 8.0).max(0.5);

    // Corners are found at two scales: the normal one (robust to noise) and a
    // fine one, so that small features (gear teeth, serifs) keep their corners.
    // Corners are found at two scales. The fine scale resolves small features
    // (gear teeth, serifs) whose corners the normal scale smears into one; it
    // needs a stricter angle and wins where both find a corner.
    let mut corners = find_corners(&poly, scale, prm.corner_angle);
    let fine = 1.0f64.min(scale);
    if fine < scale {
        let mut merged = find_corners(&poly, fine, prm.corner_angle + 15f64.to_radians());
        for &c in &corners {
            if !merged
                .iter()
                .any(|&k| poly.arc(k, c).min(poly.arc(c, k)) < 1.5)
            {
                merged.push(c);
            }
        }
        merged.sort_unstable();
        corners = merged;
    }
    let angles = turning_angles(&poly, scale);

    if corners.is_empty() {
        return fit_smooth_closed(&pts, &poly, &angles, prm, scale);
    }

    // Sharpen corners: anti-aliasing rounds polygon corners slightly; where both
    // sides are straight, move the corner to the intersection of the side lines.
    let mut drop = vec![false; n];
    let mut cpos: Vec<P> = corners.iter().map(|&c| pts[c]).collect();
    let nc = corners.len();
    for ci in 0..nc {
        let c = corners[ci];
        let prev = corners[(ci + nc - 1) % nc];
        let next = corners[(ci + 1) % nc];
        let max_back = if nc == 1 {
            poly.len / 2.0
        } else {
            poly.arc(prev, c) / 2.0
        };
        let max_fwd = if nc == 1 {
            poly.len / 2.0
        } else {
            poly.arc(c, next) / 2.0
        };
        let (cut, reach): (f64, f64) = (0.9, 4.5);
        let mut back = Vec::new();
        let mut fwd = Vec::new();
        let mut k = 1;
        while k < n {
            let j = (c + n - k) % n;
            let d = poly.arc(j, c);
            if d > reach.min(max_back) {
                break;
            }
            if d >= cut {
                back.push(pts[j]);
            }
            k += 1;
        }
        k = 1;
        while k < n {
            let j = (c + k) % n;
            let d = poly.arc(c, j);
            if d > reach.min(max_fwd) {
                break;
            }
            if d >= cut {
                fwd.push(pts[j]);
            }
            k += 1;
        }
        if back.len() < 2 || fwd.len() < 2 {
            continue;
        }
        let (c1, d1, r1) = fit_line(&back);
        let (c2, d2, r2) = fit_line(&fwd);
        if r1 > 0.12 || r2 > 0.12 || d1.cross(d2).abs() < 0.2 {
            continue;
        }
        if let Some(ip) = intersect_lines(c1, d1, c2, d2) {
            let limit = (0.3 * max_back.min(max_fwd)).min(1.6);
            if ip.dist(pts[c]) < limit {
                cpos[ci] = ip;
                let mut k = 1;
                while k < n && poly.arc((c + n - k) % n, c) < cut {
                    drop[(c + n - k) % n] = true;
                    k += 1;
                }
                k = 1;
                while k < n && poly.arc(c, (c + k) % n) < cut {
                    drop[(c + k) % n] = true;
                    k += 1;
                }
            }
        }
    }
    for &c in &corners {
        drop[c] = false;
    }

    // Split into segments between consecutive corners.
    let mut segs_pts: Vec<Vec<P>> = Vec::with_capacity(nc);
    for ci in 0..nc {
        let (c0, c1) = (corners[ci], corners[(ci + 1) % nc]);
        let mut s = vec![cpos[ci]];
        let mut j = (c0 + 1) % n;
        while j != c1 {
            if !drop[j] {
                s.push(pts[j]);
            }
            j = (j + 1) % n;
        }
        s.push(cpos[(ci + 1) % nc]);
        segs_pts.push(s);
    }

    // Classify straight segments. Points within ~1 px of a corner are ignored:
    // anti-aliasing rounds corners, which would otherwise turn straight sides
    // into curves. Corners between two straight sides are then moved to the
    // intersection of the fitted side lines (polygon regularisation).
    let lines: Vec<Option<(P, P)>> = segs_pts
        .iter()
        .map(|s| straight_side(s, prm.line_tolerance))
        .collect();
    let is_line: Vec<bool> = lines.iter().map(Option::is_some).collect();
    for ci in 0..nc {
        let prev = (ci + nc - 1) % nc;
        if let (Some((c1, d1)), Some((c2, d2))) = (lines[prev], lines[ci]) {
            if d1.cross(d2).abs() > 0.15 {
                if let Some(ip) = intersect_lines(c1, d1, c2, d2) {
                    // Never extrapolate far beyond short sides (tiny teeth become spikes).
                    let side = |k: usize| segs_pts[k][0].dist(*segs_pts[k].last().unwrap());
                    let limit = (0.15 * side(prev).min(side(ci))).min(prm.corner_reach);
                    if ip.dist(cpos[ci]) < limit {
                        cpos[ci] = ip;
                    }
                }
            }
        }
    }
    for ci in 0..nc {
        let last = segs_pts[ci].len() - 1;
        segs_pts[ci][0] = cpos[ci];
        segs_pts[ci][last] = cpos[(ci + 1) % nc];
    }
    if prm.snap_axes {
        let tol = 2.5f64.to_radians().tan();
        for ci in 0..nc {
            if !is_line[ci] {
                continue;
            }
            let (a, b) = (cpos[ci], cpos[(ci + 1) % nc]);
            let d = b - a;
            if d.len() < 3.0 {
                continue;
            }
            if d.y.abs() <= d.x.abs() * tol {
                let y = (a.y + b.y) * 0.5;
                cpos[ci].y = y;
                cpos[(ci + 1) % nc].y = y;
            } else if d.x.abs() <= d.y.abs() * tol {
                let x = (a.x + b.x) * 0.5;
                cpos[ci].x = x;
                cpos[(ci + 1) % nc].x = x;
            }
        }
        for ci in 0..nc {
            let last = segs_pts[ci].len() - 1;
            segs_pts[ci][0] = cpos[ci];
            segs_pts[ci][last] = cpos[(ci + 1) % nc];
        }
    }

    let err2 = prm.fit_tolerance * prm.fit_tolerance;
    let mut out = FittedLoop {
        start: cpos[0],
        segs: Vec::new(),
    };
    for (ci, s) in segs_pts.iter().enumerate() {
        if is_line[ci] || s.len() < 3 {
            out.segs.push(Seg::Line(*s.last().unwrap()));
            continue;
        }
        let t1 = end_tangent(s, false, scale);
        let t2 = end_tangent(s, true, scale);
        push_cubics(s, t1, t2, err2, prm.line_tolerance, &mut out.segs);
    }
    out
}

/// Turning angle at each point: the angle between the directions of the
/// incoming and outgoing sides, each measured over `scale` px of arc and
/// skipping a small gap around the point. The gap makes the measure insensitive
/// to anti-aliased rounding and to the true vertex lying between two samples.
fn turning_angles(poly: &Poly, scale: f64) -> Vec<f64> {
    let gap = 0.5f64.min(scale * 0.25);
    let margin = if poly.len < 120.0 {
        10f64.to_radians()
    } else {
        0.0
    };
    (0..poly.n())
        .map(|i| {
            let s = poly.cum[i];
            let turn = |v1: P, v2: P| v1.cross(v2).atan2(v1.dot(v2)).abs();
            let p = poly.p[i];
            // Max of the gap measure (finds vertices lying between samples)
            // and the plain point-centred one.
            // On small contours the gap measure overshoots at tiny rounded
            // features, so there it must beat the threshold by a margin.
            (turn(
                poly.at(s - gap) - poly.at(s - gap - scale),
                poly.at(s + gap + scale) - poly.at(s + gap),
            ) - margin)
                .max(turn(p - poly.at(s - scale), poly.at(s + scale) - p))
        })
        .collect()
}

/// Points whose turning angle exceeds `min_angle` and is maximal within ±scale.
fn find_corners(poly: &Poly, scale: f64, min_angle: f64) -> Vec<usize> {
    let n = poly.n();
    let angles = turning_angles(poly, scale);
    let mut corners = Vec::new();
    for i in 0..n {
        if angles[i] < min_angle {
            continue;
        }
        let mut is_max = true;
        let mut k = 1;
        while k < n && poly.arc(i, (i + k) % n) <= scale {
            if angles[(i + k) % n] > angles[i] {
                is_max = false;
                break;
            }
            k += 1;
        }
        k = 1;
        while is_max && k < n && poly.arc((i + n - k) % n, i) <= scale {
            if angles[(i + n - k) % n] >= angles[i] {
                is_max = false;
            }
            k += 1;
        }
        if is_max {
            corners.push(i);
        }
    }
    corners
}

/// If the segment is straight (ignoring ~1 px at each end), its fitted line.
fn straight_side(s: &[P], tol: f64) -> Option<(P, P)> {
    let (a, b) = (s[0], *s.last().unwrap());
    let len = a.dist(b);
    if len < 1e-6 {
        return None;
    }
    if s.len() <= 2 {
        return Some((a, (b - a).norm()));
    }
    let trim = if len > 4.0 { 1.0 } else { 0.0 };
    let inner: Vec<P> = s
        .iter()
        .copied()
        .filter(|&p| p.dist(a) >= trim && p.dist(b) >= trim)
        .collect();
    if inner.len() < 3 || len < 3.0 {
        // Short side: plain chord test.
        let ok = s[1..s.len() - 1]
            .iter()
            .all(|&p| dist_to_segment(p, a, b) <= tol);
        return ok.then(|| (a, (b - a).norm()));
    }
    let (c, d, _) = fit_line(&inner);
    let normal = P::new(-d.y, d.x);
    let ok = inner.iter().all(|&p| (p - c).dot(normal).abs() <= tol)
        && s.iter()
            .all(|&p| (p - c).dot(normal).abs() <= tol * 1.5 + 0.2);
    ok.then_some((c, d))
}

/// Smooths a closed contour along its arc length (Gaussian, σ in px) while
/// keeping real corners pinned. Each contour is smoothed on its own, so thin
/// outlines keep their width and neighbouring shapes never merge — unlike a
/// blur of the raster.
pub(crate) fn smooth_contour(raw: &[P], sigma: f64, corner_angle: f64) -> Vec<P> {
    let pts = dedup(raw);
    if pts.len() < 8 {
        return pts;
    }
    let poly = Poly::new(&pts);
    // Small loops would collapse: limit σ to a fraction of the perimeter.
    let sigma = sigma.min(poly.len / 14.0);
    if sigma < 0.2 {
        return pts;
    }
    // Uniform resampling (0.5 px) so the kernel is a fixed number of samples.
    let h = 0.5;
    let n = ((poly.len / h).round() as usize).max(8);
    let step = poly.len / n as f64;
    let sp: Vec<P> = (0..n).map(|i| poly.at(i as f64 * step)).collect();
    let spoly = Poly::new(&sp);
    let mut corners = find_corners(&spoly, (2.0 * sigma).max(2.0), corner_angle);
    // A "corner" whose two sides are closer than the smoothing scale is the
    // tip of a thin spur (texture / noise): smooth it away instead of pinning.
    corners.retain(|&c| {
        let s0 = spoly.cum[c];
        spoly.at(s0 - 2.0 * sigma).dist(spoly.at(s0 + 2.0 * sigma)) > 0.6 * sigma
    });
    corners.sort_unstable();
    let rad = ((3.0 * sigma) / step).ceil() as isize;
    let kernel: Vec<f64> = (-rad..=rad)
        .map(|k| {
            let d = k as f64 * step;
            (-d * d / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    // Segment id of every sample (between consecutive corners) so the kernel
    // never averages across a corner.
    let mut seg = vec![0usize; n];
    if !corners.is_empty() {
        let mut s = 0;
        for i in 0..n {
            let idx = (corners[0] + i) % n;
            if i > 0 && corners.binary_search(&idx).is_ok() {
                s += 1;
            }
            seg[idx] = s;
        }
    }
    let is_corner = |i: usize| corners.binary_search(&i).is_ok();
    (0..n)
        .map(|i| {
            if is_corner(i) {
                return sp[i];
            }
            let (mut acc, mut wsum) = (P::default(), 0.0);
            for (k, &wk) in kernel.iter().enumerate() {
                let j = (i as isize + k as isize - rad).rem_euclid(n as isize) as usize;
                if seg[j] != seg[i] && !is_corner(j) {
                    continue;
                }
                acc = acc + sp[j] * wk;
                wsum += wk;
            }
            acc * (1.0 / wsum)
        })
        .collect()
}

fn polygon(pts: &[P]) -> FittedLoop {
    FittedLoop {
        start: pts.first().copied().unwrap_or_default(),
        segs: pts.iter().skip(1).map(|&p| Seg::Line(p)).collect(),
    }
}

fn fit_smooth_closed(
    pts: &[P],
    poly: &Poly,
    angles: &[f64],
    prm: &Params,
    scale: f64,
) -> FittedLoop {
    let n = pts.len();
    // Start where the curve is straightest: the forced tangent there is most reliable.
    let start = (0..n)
        .min_by(|&a, &b| angles[a].total_cmp(&angles[b]))
        .unwrap_or(0);
    let mut s: Vec<P> = (0..=n).map(|k| pts[(start + k) % n]).collect();
    let s0 = poly.cum[start];
    let tan = (poly.at(s0 + scale * 0.5) - poly.at(s0 - scale * 0.5)).norm();
    let last = s.len() - 1;
    s[last] = s[0];
    let mut out = FittedLoop {
        start: s[0],
        segs: Vec::new(),
    };
    let err2 = prm.fit_tolerance * prm.fit_tolerance;
    push_cubics(&s, tan, -tan, err2, prm.line_tolerance, &mut out.segs);
    out
}

/// Unit tangent at the start (pointing into the curve) or at the end (pointing back).
fn end_tangent(s: &[P], at_end: bool, scale: f64) -> P {
    let n = s.len();
    let get = |i: usize| if at_end { s[n - 1 - i] } else { s[i] };
    let total: f64 = (1..n).map(|i| s[i].dist(s[i - 1])).sum();
    let reach = scale.min(total * 0.3).max(1e-6);
    let p0 = get(0);
    let mut acc = 0.0;
    for i in 1..n {
        acc += get(i).dist(get(i - 1));
        if acc >= reach {
            return (get(i) - p0).norm();
        }
    }
    (get(n - 1) - p0).norm()
}

fn push_cubics(d: &[P], t1: P, t2: P, err2: f64, line_tol: f64, out: &mut Vec<Seg>) {
    let mut bez = Vec::new();
    fit_cubic(d, t1, t2, err2, 0, &mut bez);
    for b in bez {
        // Degenerate cubics whose handles lie on the chord become lines.
        if dist_to_segment(b[1], b[0], b[3]) <= line_tol * 0.5
            && dist_to_segment(b[2], b[0], b[3]) <= line_tol * 0.5
        {
            out.push(Seg::Line(b[3]));
        } else {
            out.push(Seg::Cubic(b[1], b[2], b[3]));
        }
    }
}

fn bezier(b: &[P; 4], t: f64) -> P {
    let mt = 1.0 - t;
    b[0] * (mt * mt * mt)
        + b[1] * (3.0 * mt * mt * t)
        + b[2] * (3.0 * mt * t * t)
        + b[3] * (t * t * t)
}

fn fit_cubic(d: &[P], t1: P, t2: P, err2: f64, depth: u32, out: &mut Vec<[P; 4]>) {
    let n = d.len();
    let (p0, p3) = (d[0], d[n - 1]);
    if n == 2 {
        let dist = p0.dist(p3) / 3.0;
        out.push([p0, p0 + t1 * dist, p3 + t2 * dist, p3]);
        return;
    }
    let mut u = chord_params(d);
    let mut bez = generate(d, &u, t1, t2);
    let (mut maxe, mut split) = max_error(d, &bez, &u);
    if maxe < err2 {
        out.push(bez);
        return;
    }
    if maxe < err2 * 16.0 {
        for _ in 0..6 {
            u = reparameterize(d, &u, &bez);
            bez = generate(d, &u, t1, t2);
            (maxe, split) = max_error(d, &bez, &u);
            if maxe < err2 {
                out.push(bez);
                return;
            }
        }
    }
    if depth > 30 || n < 4 {
        out.push(bez);
        return;
    }
    let split = split.clamp(1, n - 2);
    let k = 2.min(split).min(n - 1 - split);
    let mut tc = (d[split - k] - d[split + k]).norm();
    if tc.len() < 0.5 {
        tc = (d[split - 1] - d[split + 1]).norm();
    }
    fit_cubic(&d[..=split], t1, tc, err2, depth + 1, out);
    fit_cubic(&d[split..], -tc, t2, err2, depth + 1, out);
}

fn chord_params(d: &[P]) -> Vec<f64> {
    let mut u = Vec::with_capacity(d.len());
    u.push(0.0);
    for i in 1..d.len() {
        u.push(u[i - 1] + d[i].dist(d[i - 1]));
    }
    let total = *u.last().unwrap();
    if total > 0.0 {
        u.iter_mut().for_each(|v| *v /= total);
    }
    u
}

fn generate(d: &[P], u: &[f64], t1: P, t2: P) -> [P; 4] {
    let n = d.len();
    let (p0, p3) = (d[0], d[n - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let t = u[i];
        let mt = 1.0 - t;
        let b0 = mt * mt * mt;
        let b1 = 3.0 * mt * mt * t;
        let b2 = 3.0 * mt * t * t;
        let b3 = t * t * t;
        let a0 = t1 * b1;
        let a1 = t2 * b2;
        c00 += a0.dot(a0);
        c01 += a0.dot(a1);
        c11 += a1.dot(a1);
        let tmp = d[i] - (p0 * (b0 + b1) + p3 * (b2 + b3));
        x0 += a0.dot(tmp);
        x1 += a1.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg = p0.dist(p3);
    let (mut al, mut ar) = if det.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det)
    } else {
        (0.0, 0.0)
    };
    let eps = 1e-6 * seg;
    if al < eps || ar < eps || al > seg * 2.0 || ar > seg * 2.0 {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    [p0, p0 + t1 * al, p3 + t2 * ar, p3]
}

fn max_error(d: &[P], b: &[P; 4], u: &[f64]) -> (f64, usize) {
    let n = d.len();
    let mut best = (0.0, n / 2);
    for i in 1..n - 1 {
        let e = bezier(b, u[i]) - d[i];
        let e2 = e.dot(e);
        if e2 >= best.0 {
            best = (e2, i);
        }
    }
    best
}

fn reparameterize(d: &[P], u: &[f64], b: &[P; 4]) -> Vec<f64> {
    let q1 = [
        (b[1] - b[0]) * 3.0,
        (b[2] - b[1]) * 3.0,
        (b[3] - b[2]) * 3.0,
    ];
    let q2 = [(q1[1] - q1[0]) * 2.0, (q1[2] - q1[1]) * 2.0];
    d.iter()
        .zip(u)
        .map(|(&p, &t)| {
            let mt = 1.0 - t;
            let qt = bezier(b, t);
            let q1t = q1[0] * (mt * mt) + q1[1] * (2.0 * mt * t) + q1[2] * (t * t);
            let q2t = q2[0] * mt + q2[1] * t;
            let diff = qt - p;
            let num = diff.dot(q1t);
            let den = q1t.dot(q1t) + diff.dot(q2t);
            if den.abs() < 1e-12 {
                t
            } else {
                (t - num / den).clamp(0.0, 1.0)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Options, Params, Preset};

    fn params() -> Params {
        Params::resolve(&Options::default(), Preset::Logo, 100, 100)
    }

    #[test]
    fn square_becomes_four_lines() {
        let mut pts = Vec::new();
        for i in 0..20 {
            pts.push(P::new(10.0 + i as f64, 10.0));
        }
        for i in 0..20 {
            pts.push(P::new(30.0, 10.0 + i as f64));
        }
        for i in 0..20 {
            pts.push(P::new(30.0 - i as f64, 30.0));
        }
        for i in 0..20 {
            pts.push(P::new(10.0, 30.0 - i as f64));
        }
        let f = fit_loop(&pts, &params());
        assert_eq!(f.segs.len(), 4, "{:?}", f.segs);
        assert!(f.segs.iter().all(|s| matches!(s, Seg::Line(_))));
    }

    #[test]
    fn circle_is_smooth_and_compact() {
        let pts: Vec<P> = (0..120)
            .map(|i| {
                let a = i as f64 / 120.0 * std::f64::consts::TAU;
                P::new(50.0 + 20.0 * a.cos(), 50.0 + 20.0 * a.sin())
            })
            .collect();
        let f = fit_loop(&pts, &params());
        assert!(f.segs.len() <= 6, "{} segs", f.segs.len());
        assert!(f.segs.iter().all(|s| matches!(s, Seg::Cubic(..))));
    }
}
