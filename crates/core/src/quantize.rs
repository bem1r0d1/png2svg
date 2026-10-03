//! Palette extraction in OKLab.
//!
//! Only "flat" pixels (not anti-aliasing blends) seed the palette, so edges do
//! not create fringe colours. Clusters are found with weighted k-means++ and then
//! merged agglomeratively while they are perceptually indistinguishable.
//! The final colour of each cluster is its most frequent exact colour (mode),
//! which keeps brand colours byte-exact.

use std::collections::HashMap;

use crate::color::{oklab_to_srgb8, srgb8_to_oklab};
use crate::options::Params;
use crate::raster::{premultiply, Raster};

#[derive(Clone, Debug)]
pub(crate) struct PalColor {
    /// Colour as measured in the image: used for labelling.
    pub rgba: [u8; 4],
    /// Colour written to the SVG (snapped to the target palette / pure white
    /// or black). Snapping only the output keeps labelling accurate.
    pub out: [u8; 4],
    pub feat: [f32; 4],
    pub pm: [f32; 4],
    pub transparent: bool,
}

pub(crate) struct Palette {
    pub colors: Vec<PalColor>,
}

impl Palette {
    pub fn transparent_index(&self) -> Option<usize> {
        self.colors.iter().position(|c| c.transparent)
    }

    /// Nearest opaque palette entry in feature space.
    pub fn nearest(&self, feat: [f32; 4]) -> usize {
        let mut best = (f32::MAX, 0);
        for (i, c) in self.colors.iter().enumerate() {
            if c.transparent {
                continue;
            }
            let d = fdist2(c.feat, feat);
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }
}

pub(crate) fn feature(c: [u8; 4]) -> [f32; 4] {
    let lab = srgb8_to_oklab([c[0], c[1], c[2]]);
    [lab[0], lab[1], lab[2], c[3] as f32 / 255.0]
}

#[inline]
pub(crate) fn fdist2(a: [f32; 4], b: [f32; 4]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) + (a[3] - b[3]).powi(2)
}

struct Entry {
    color: [u8; 4],
    feat: [f32; 4],
    w: f64,
}

#[derive(Clone)]
struct Cluster {
    w: f64,
    sum: [f64; 4],
    /// Weighted sum of squared feature norms (for the spread).
    sumsq: f64,
    mode: [u8; 4],
    mode_feat: [f32; 4],
    mode_w: f64,
}

impl Cluster {
    fn empty() -> Self {
        Self {
            w: 0.0,
            sum: [0.0; 4],
            sumsq: 0.0,
            mode: [0; 4],
            mode_feat: [0.0; 4],
            mode_w: -1.0,
        }
    }
    fn add(&mut self, e: &Entry) {
        self.w += e.w;
        for i in 0..4 {
            self.sum[i] += e.feat[i] as f64 * e.w;
            self.sumsq += (e.feat[i] as f64).powi(2) * e.w;
        }
        if e.w > self.mode_w {
            self.mode_w = e.w;
            self.mode = e.color;
            self.mode_feat = e.feat;
        }
    }
    fn merge(&mut self, o: &Cluster) {
        self.w += o.w;
        self.sumsq += o.sumsq;
        for i in 0..4 {
            self.sum[i] += o.sum[i];
        }
        if o.mode_w > self.mode_w {
            self.mode_w = o.mode_w;
            self.mode = o.mode;
            self.mode_feat = o.mode_feat;
        }
    }
    /// RMS distance of members to the centroid.
    fn spread(&self) -> f32 {
        let w = self.w.max(1e-12);
        let m2: f64 = self.sum.iter().map(|s| (s / w).powi(2)).sum();
        ((self.sumsq / w - m2).max(0.0)).sqrt() as f32
    }
    fn centroid(&self) -> [f32; 4] {
        let w = self.w.max(1e-12);
        self.sum.map(|s| (s / w) as f32)
    }
}

/// Simple deterministic xorshift RNG (reproducible output, no deps).
struct Rng(u64);
impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

const MAX_ENTRIES: usize = 30_000;

fn histogram(r: &Raster, mixed: &[bool]) -> Vec<Entry> {
    let mut exact: HashMap<[u8; 4], f64> = HashMap::new();
    for (i, &c) in r.rgba.iter().enumerate() {
        if c[3] == 0 {
            continue;
        }
        let w = if mixed[i] { 0.02 } else { 1.0 };
        *exact.entry(c).or_insert(0.0) += w;
    }
    let mut shift = 0u32;
    loop {
        // Bucket colours (no-op for shift 0) and keep the most frequent exact colour per bucket.
        let mut buckets: HashMap<[u8; 4], (f64, [u8; 4], f64)> = HashMap::new();
        for (&c, &w) in &exact {
            let key = c.map(|v| v >> shift);
            let b = buckets.entry(key).or_insert((0.0, c, -1.0));
            b.0 += w;
            if w > b.2 {
                b.1 = c;
                b.2 = w;
            }
        }
        if buckets.len() <= MAX_ENTRIES || shift >= 5 {
            let mut v: Vec<Entry> = buckets
                .into_values()
                .map(|(w, c, _)| Entry {
                    color: c,
                    feat: feature(c),
                    w,
                })
                .collect();
            // Deterministic order regardless of HashMap iteration.
            v.sort_by(|a, b| b.w.total_cmp(&a.w).then(a.color.cmp(&b.color)));
            return v;
        }
        shift += 1;
    }
}

fn kmeans(entries: &[Entry], k: usize) -> Vec<Cluster> {
    let k = k.min(entries.len()).max(1);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // k-means++ seeding, weighted by histogram weight.
    let mut centers: Vec<[f32; 4]> = vec![entries[0].feat];
    let mut d2: Vec<f64> = entries
        .iter()
        .map(|e| fdist2(e.feat, centers[0]) as f64)
        .collect();
    while centers.len() < k {
        let total: f64 = entries.iter().zip(&d2).map(|(e, d)| e.w * d).sum();
        if total <= 0.0 {
            break;
        }
        let mut pick = rng.next_f64() * total;
        let mut chosen = entries.len() - 1;
        for (i, (e, d)) in entries.iter().zip(&d2).enumerate() {
            pick -= e.w * d;
            if pick <= 0.0 {
                chosen = i;
                break;
            }
        }
        let c = entries[chosen].feat;
        centers.push(c);
        for (i, e) in entries.iter().enumerate() {
            d2[i] = d2[i].min(fdist2(e.feat, c) as f64);
        }
    }
    let mut assign = vec![0usize; entries.len()];
    for _ in 0..16 {
        let mut changed = false;
        for (i, e) in entries.iter().enumerate() {
            let mut best = (f32::MAX, 0);
            for (j, c) in centers.iter().enumerate() {
                let d = fdist2(e.feat, *c);
                if d < best.0 {
                    best = (d, j);
                }
            }
            if assign[i] != best.1 {
                assign[i] = best.1;
                changed = true;
            }
        }
        let mut cl = vec![Cluster::empty(); centers.len()];
        for (i, e) in entries.iter().enumerate() {
            cl[assign[i]].add(e);
        }
        for (j, c) in cl.iter().enumerate() {
            if c.w > 0.0 {
                centers[j] = c.centroid();
            }
        }
        if !changed {
            break;
        }
    }
    let mut cl = vec![Cluster::empty(); centers.len()];
    for (i, e) in entries.iter().enumerate() {
        cl[assign[i]].add(e);
    }
    cl.retain(|c| c.w > 0.0);
    cl
}

/// Repeatedly merges the closest pair for which `mergeable(dist, a, b)` holds;
/// when none is mergeable but there are more than `max` clusters, merges the
/// closest pair regardless.
fn agglomerate(
    cl: &mut Vec<Cluster>,
    max: usize,
    mergeable: impl Fn(f32, &Cluster, &Cluster) -> bool,
) {
    while cl.len() > 1 {
        let cents: Vec<[f32; 4]> = cl.iter().map(Cluster::centroid).collect();
        let mut best_ok = (f32::MAX, 0, 0);
        let mut best_any = (f32::MAX, 0, 0);
        for i in 0..cl.len() {
            for j in i + 1..cl.len() {
                let d = fdist2(cents[i], cents[j]).sqrt();
                if d < best_any.0 {
                    best_any = (d, i, j);
                }
                if d < best_ok.0 && mergeable(d, &cl[i], &cl[j]) {
                    best_ok = (d, i, j);
                }
            }
        }
        let (_, i, j) = if best_ok.0 < f32::MAX {
            best_ok
        } else if cl.len() > max {
            best_any
        } else {
            break;
        };
        let o = cl.remove(j);
        cl[i].merge(&o);
    }
}

/// Removes clusters that are just anti-aliasing blends of two much heavier
/// clusters (or of a cluster and transparency).
fn prune_blends(cl: &mut Vec<Cluster>, has_transparent: bool) {
    loop {
        let pms: Vec<[f32; 4]> = cl.iter().map(|c| premultiply(c.mode)).collect();
        let mut victim = None;
        'outer: for k in 0..cl.len() {
            let wk = cl[k].w;
            let mut parents: Vec<([f32; 4], f64)> = cl
                .iter()
                .enumerate()
                .filter(|&(i, c)| i != k && c.w > wk * 4.0)
                .map(|(i, c)| (pms[i], c.w))
                .collect();
            if has_transparent {
                parents.push(([0.0; 4], f64::MAX));
            }
            for a in 0..parents.len() {
                for b in a + 1..parents.len() {
                    if blend_residual(pms[k], parents[a].0, parents[b].0) < 0.03 {
                        victim = Some(k);
                        break 'outer;
                    }
                }
            }
        }
        match victim {
            Some(k) => {
                cl.remove(k);
            }
            None => break,
        }
    }
}

fn blend_residual(p: [f32; 4], a: [f32; 4], b: [f32; 4]) -> f32 {
    use crate::raster::{add, dot, scale, sub};
    let d = sub(b, a);
    let dd = dot(d, d);
    if dd < 1e-6 {
        return f32::MAX;
    }
    let t = dot(sub(p, a), d) / dd;
    if !(0.04..=0.96).contains(&t) {
        return f32::MAX;
    }
    let r = sub(p, add(a, scale(d, t)));
    dot(r, r).sqrt()
}

/// Replaces a colour by the nearest user palette colour within tolerance.
fn snap_to_palette(c: [u8; 4], p: &Params) -> [u8; 4] {
    let f = feature(c);
    let mut best = (p.palette_tolerance, None);
    for t in &p.palette {
        let ft = feature([t[0], t[1], t[2], c[3]]);
        let d = fdist2(f, ft).sqrt();
        if d <= best.0 {
            best = (d, Some(*t));
        }
    }
    if best.1.is_none() && p.pure_snap > 0.0 {
        for t in [[255u8, 255, 255], [0, 0, 0]] {
            if fdist2(f, feature([t[0], t[1], t[2], c[3]])).sqrt() < p.pure_snap {
                best.1 = Some(t);
            }
        }
    }
    match best.1 {
        Some(t) => [t[0], t[1], t[2], c[3]],
        None => c,
    }
}

pub(crate) fn build_palette(r: &Raster, mixed: &[bool], p: &Params) -> Palette {
    let entries = histogram(r, mixed);
    let mut colors = Vec::new();
    if r.rgba.iter().any(|c| c[3] == 0) {
        colors.push(PalColor {
            rgba: [0; 4],
            out: [0; 4],
            feat: [0.0; 4],
            pm: [0.0; 4],
            transparent: true,
        });
    }
    if entries.is_empty() {
        return Palette { colors };
    }

    let mut cl = match p.fixed_colors {
        Some(k) => {
            let mut cl = kmeans(&entries, k * 2);
            agglomerate(&mut cl, k, |_, _, _| false);
            cl
        }
        None => {
            let mut cl = if entries.len() <= p.max_colors * 3 {
                entries
                    .iter()
                    .map(|e| {
                        let mut c = Cluster::empty();
                        c.add(e);
                        c
                    })
                    .collect()
            } else {
                kmeans(&entries, p.max_colors * 3)
            };
            let md = p.merge_dist;
            let min_w = (p.speckle_area as f64).max(1.0);
            // Merge imperceptible differences always. Up to `3·md`, merge only
            // clusters that overlap (distance small vs. their spread — noise,
            // JPEG artefacts) or weak satellites of a dominant colour. Two
            // flat colours (dominated by one exact value) stay apart.
            let flat = |c: &Cluster| c.w >= min_w && c.mode_w > c.w * 0.5;
            agglomerate(&mut cl, usize::MAX, |d, a, b| {
                let (lo, hi) = if a.w < b.w { (a.w, b.w) } else { (b.w, a.w) };
                d < md * 0.6
                    || (d < md * 3.0
                        && !(flat(a) && flat(b))
                        && (d < 2.5 * a.spread().max(b.spread()) || lo < hi * 0.05))
            });
            // Drop colours that only exist as tiny specks or as edge blends.
            if cl.iter().any(|c| c.w >= min_w) {
                cl.retain(|c| c.w >= min_w);
            }
            prune_blends(&mut cl, r.rgba.iter().any(|c| c[3] == 0));
            agglomerate(&mut cl, p.max_colors, |_, _, _| false);
            cl
        }
    };
    cl.sort_by(|a, b| b.w.total_cmp(&a.w));

    for c in &cl {
        let cent = c.centroid();
        let rgba = if fdist2(cent, c.mode_feat).sqrt() < p.merge_dist.max(0.02) {
            c.mode
        } else {
            let rgb = oklab_to_srgb8([cent[0], cent[1], cent[2]]);
            [
                rgb[0],
                rgb[1],
                rgb[2],
                (cent[3] * 255.0).round().clamp(1.0, 255.0) as u8,
            ]
        };
        let out = snap_to_palette(rgba, p);
        // A forced palette (huge tolerance) also drives labelling, so colours
        // mapped onto the same target collapse into one layer.
        let rgba = if p.palette_tolerance >= 0.1 {
            out
        } else {
            rgba
        };
        if colors.iter().any(|c: &PalColor| c.rgba == rgba) {
            continue;
        }
        colors.push(PalColor {
            rgba,
            out,
            feat: feature(rgba),
            pm: premultiply(rgba),
            transparent: false,
        });
    }
    Palette { colors }
}
