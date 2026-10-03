//! png2svg core: raster → clean, Figma-friendly SVG.
//!
//! Pipeline: anti-aliasing detection → OKLab palette → two-colour blend
//! labelling → speckle removal → stacked layers → sub-pixel iso-contours →
//! corner-aware Bézier fitting → compact SVG.

mod analyze;
pub mod color;
mod contour;
mod fit;
pub mod geom;
mod layers;
mod options;
mod quantize;
mod raster;
mod segment;
mod shapes;
mod svg;

use std::collections::HashMap;

pub use options::{GroupBy, Layering, Options, Preset};

#[cfg(feature = "serde")]
use serde::Serialize;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize), serde(rename_all = "camelCase"))]
pub struct LayerInfo {
    /// SVG element id (becomes the layer name in Figma).
    pub id: String,
    pub color: [u8; 4],
    pub hex: String,
    /// Pixel area of the colour in the source image.
    pub area: usize,
    pub subpaths: usize,
    pub segments: usize,
    /// SVG elements (shapes) in this layer.
    pub elements: usize,
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize), serde(rename_all = "camelCase"))]
pub struct Stats {
    pub preset: String,
    pub colors: usize,
    pub layers: usize,
    pub subpaths: usize,
    pub segments: usize,
    /// Native circles / ellipses / rectangles in the output.
    pub primitives: usize,
    pub bytes: usize,
    /// Estimated input noise (0 = clean).
    pub noise: f32,
    /// Detected pixel grid of a pixelated input (1 = none).
    pub pixel_grid: usize,
    /// Edge smoothing σ (px) that was applied.
    pub smoothing: f32,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize), serde(rename_all = "camelCase"))]
pub struct Output {
    pub svg: String,
    pub width: u32,
    pub height: u32,
    /// Layers bottom → top.
    pub layers: Vec<LayerInfo>,
    pub stats: Stats,
}

#[derive(Debug)]
pub enum Error {
    BadDimensions,
    BufferSize { expected: usize, got: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadDimensions => write!(f, "image dimensions must be non-zero"),
            Error::BufferSize { expected, got } => {
                write!(f, "RGBA buffer has {got} bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Convert straight (non-premultiplied) RGBA8 pixels to SVG.
pub fn convert(rgba: &[u8], width: u32, height: u32, opts: &Options) -> Result<Output, Error> {
    if width == 0 || height == 0 {
        return Err(Error::BadDimensions);
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(Error::BufferSize {
            expected,
            got: rgba.len(),
        });
    }
    let (w, h) = (width as usize, height as usize);
    let src = raster::Raster::new(rgba, w, h);

    // Pixelated input (nearest-neighbour upscale): trace the logical image,
    // one pixel per block, and scale the result back. Genuine pixel art (hard
    // edges) is first enlarged 4× with Scale2x so diagonals become smooth.
    let grid = if opts.depixelate {
        analyze::pixel_grid(&src)
    } else {
        None
    };
    let (mut r, xf, unit) = match grid {
        Some(g) => {
            let (px, lw, lh, x0, y0) = analyze::downsample(&src, g);
            let logical = raster::Raster::new(&px, lw, lh);
            if analyze::is_hard_edged(&logical) {
                let x2 = analyze::scale2x(&px, lw, lh);
                let x4 = analyze::scale2x(&x2, lw * 2, lh * 2);
                let xf = Xf {
                    s: g.k as f64 / 4.0,
                    ox: x0,
                    oy: y0,
                };
                (raster::Raster::new(&x4, lw * 4, lh * 4), xf, 4)
            } else {
                (
                    logical,
                    Xf {
                        s: g.k as f64,
                        ox: x0,
                        oy: y0,
                    },
                    1,
                )
            }
        }
        None => (src, Xf::IDENTITY, 1),
    };
    let block = grid.map_or(1, |g| g.k);
    let (tw, th) = (r.w as u32, r.h as u32);

    // Noise / JPEG artefacts are filtered out before anything is traced.
    // Pixelated sources are exact by construction, and on tiny images
    // anti-aliasing is indistinguishable from noise: skip those.
    let noise = if grid.is_none() && r.w.min(r.h) >= 48 {
        analyze::estimate_noise(&r)
    } else {
        0.0
    };
    if opts.denoise && noise > 0.01 {
        analyze::denoise(&mut r, noise, if noise > 0.02 { 2 } else { 1 });
    }
    let mixed = r.detect_mixed();
    let preset = match opts.preset {
        Preset::Auto => classify(&r, &mixed),
        p => p,
    };
    let mut prm = options::Params::resolve(opts, preset, tw, th);
    prm.apply_noise(noise);

    let pal = quantize::build_palette(&r, &mixed, &prm);
    let mut labels = segment::assign(&r, &mixed, &pal, 0.03 + 2.0 * noise);
    segment::resolve_blend_layers(&mut labels, &r, &pal);
    let aa_ratio = analyze::edge_aa_ratio(&labels);
    let sm = match opts.smoothing {
        Some(v) => {
            let v = v.max(0.0);
            analyze::Smoothing {
                field: v.min(1.0),
                contour: v * 1.5,
            }
        }
        None => analyze::edge_smoothing(
            aa_ratio,
            noise,
            analyze::edge_width(&mixed, &labels),
            (tw.max(th) as f64 * xf.s) as usize,
        ),
    };
    let smoothing = sm.field;
    prm.apply_smoothing(smoothing, unit);
    prm.apply_contour_smoothing(sm.contour);
    if noise > 0.01 {
        segment::absorb_ringing(&mut labels, &pal, 0.08);
    }
    segment::remove_speckles(&mut labels, prm.speckle_area);
    // Contour-level speckle filter (islands inside fields that pixel-level
    // speckle removal cannot see), only where smoothing is active.
    let min_loop = if sm.contour > 0.1 {
        prm.speckle_area as f64 * 0.6
    } else {
        0.3
    };
    let layers = layers::build_layers(&labels, &pal, smoothing, min_loop);

    let fitted: Vec<Vec<Fitted>> = layers
        .iter()
        .map(|l| {
            l.loops
                .iter()
                .map(|lp| xf.apply(fit_contour(lp, &prm)))
                .collect()
        })
        .collect();

    let mut used_ids: HashMap<String, usize> = HashMap::new();
    let mut infos = Vec::with_capacity(layers.len());
    let mut svg_layers = Vec::with_capacity(layers.len());
    let mut primitives = 0;
    for (layer, loops) in layers.iter().zip(&fitted) {
        let rgba = pal.colors[layer.label as usize].out;
        let hx = svg::hex(rgba);
        let base = format!("color-{}", &hx[1..]);
        let k = used_ids.entry(base.clone()).or_insert(0);
        *k += 1;
        let id = if *k == 1 { base } else { format!("{base}-{k}") };
        let items = group_items(&layer.loops, loops, opts);
        primitives += items.iter().filter(|i| i.native.is_some()).count();
        infos.push(LayerInfo {
            id: id.clone(),
            color: rgba,
            hex: hx,
            area: layer.area,
            subpaths: loops.len(),
            segments: loops.iter().map(|l| l.path.segs.len()).sum(),
            elements: items.len(),
        });
        svg_layers.push(svg::SvgLayer { id, rgba, items });
    }
    let svg = svg::write_svg(width, height, &svg_layers, prm.precision);
    let stats = Stats {
        preset: format!("{preset:?}").to_lowercase(),
        colors: pal.colors.iter().filter(|c| !c.transparent).count(),
        layers: infos.len(),
        subpaths: infos.iter().map(|l| l.subpaths).sum(),
        segments: infos.iter().map(|l| l.segments).sum(),
        primitives,
        bytes: svg.len(),
        noise,
        pixel_grid: block,
        smoothing: sm.field.max(sm.contour),
    };
    Ok(Output {
        svg,
        width,
        height,
        layers: infos,
        stats,
    })
}

/// Maps traced coordinates back to source pixels (depixelisation path).
#[derive(Clone, Copy)]
struct Xf {
    s: f64,
    ox: f64,
    oy: f64,
}

impl Xf {
    const IDENTITY: Xf = Xf {
        s: 1.0,
        ox: 0.0,
        oy: 0.0,
    };

    fn p(&self, p: geom::P) -> geom::P {
        geom::P::new(self.ox + p.x * self.s, self.oy + p.y * self.s)
    }

    fn apply(&self, mut f: Fitted) -> Fitted {
        if self.s == 1.0 && self.ox == 0.0 && self.oy == 0.0 {
            return f;
        }
        f.path.start = self.p(f.path.start);
        for seg in &mut f.path.segs {
            *seg = match *seg {
                fit::Seg::Line(p) => fit::Seg::Line(self.p(p)),
                fit::Seg::Cubic(a, b, p) => fit::Seg::Cubic(self.p(a), self.p(b), self.p(p)),
            };
        }
        f.shape = f.shape.map(|s| match s {
            shapes::Shape::Ellipse { c, rx, ry, angle } => shapes::Shape::Ellipse {
                c: self.p(c),
                rx: rx * self.s,
                ry: ry * self.s,
                angle,
            },
            shapes::Shape::RoundRect {
                c,
                hw,
                hh,
                r,
                angle,
            } => shapes::Shape::RoundRect {
                c: self.p(c),
                hw: hw * self.s,
                hh: hh * self.s,
                r: r * self.s,
                angle,
            },
        });
        f
    }
}

/// A fitted contour plus the primitive it was recognised as (if any).
struct Fitted {
    path: fit::FittedLoop,
    shape: Option<shapes::Shape>,
}

fn fit_contour(raw: &[geom::P], prm: &options::Params) -> Fitted {
    let smoothed;
    let raw = if prm.contour_sigma > 0.1 {
        smoothed = fit::smooth_contour(
            raw,
            prm.contour_sigma,
            prm.corner_angle + 10f64.to_radians(),
        );
        &smoothed[..]
    } else {
        raw
    };
    if prm.shape_tolerance > 0.0 {
        if let Some(shape) = shapes::detect(raw, prm.shape_tolerance) {
            let ccw = geom::polygon_area(raw) > 0.0;
            return Fitted {
                path: shapes::to_loop(&shape, ccw),
                shape: Some(shape),
            };
        }
    }
    Fitted {
        path: fit::fit_loop(raw, prm),
        shape: None,
    }
}

fn point_in_polygon(p: geom::P, poly: &[geom::P]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Splits a layer's contours into SVG elements: per shape (outer contour with
/// its holes) or one path per colour. Hole-free axis-aligned primitives become
/// native elements unless `flatten_shapes` is set.
fn group_items<'a>(
    raw: &[Vec<geom::P>],
    fitted: &'a [Fitted],
    opts: &Options,
) -> Vec<svg::SvgItem<'a>> {
    let n = raw.len();
    let bbox: Vec<(f64, f64, f64, f64)> = raw
        .iter()
        .map(|l| {
            l.iter()
                .fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| {
                    (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y))
                })
        })
        .collect();
    let area: Vec<f64> = raw.iter().map(|l| geom::polygon_area(l).abs()).collect();
    // containers[i] = loops that contain loop i.
    let containers: Vec<Vec<usize>> = (0..n)
        .map(|i| {
            let p = raw[i][0];
            (0..n)
                .filter(|&j| {
                    j != i
                        && area[j] > area[i]
                        && p.x >= bbox[j].0
                        && p.y >= bbox[j].1
                        && p.x <= bbox[j].2
                        && p.y <= bbox[j].3
                        && point_in_polygon(p, &raw[j])
                })
                .collect()
        })
        .collect();
    let native_ok = |i: usize| -> Option<shapes::Shape> {
        if opts.flatten_shapes {
            return None;
        }
        fitted[i].shape.filter(|s| s.is_axis_aligned())
    };

    if opts.group_by == options::GroupBy::Color || n <= 1 {
        let native = if n == 1 { native_ok(0) } else { None };
        return vec![svg::SvgItem {
            loops: fitted.iter().map(|f| &f.path).collect(),
            native,
        }];
    }
    let depth: Vec<usize> = containers.iter().map(Vec::len).collect();
    let mut items: Vec<(usize, svg::SvgItem)> = Vec::new();
    let mut index_of = vec![usize::MAX; n];
    for i in (0..n).filter(|&i| depth[i].is_multiple_of(2)) {
        index_of[i] = items.len();
        items.push((
            i,
            svg::SvgItem {
                loops: vec![&fitted[i].path],
                native: None,
            },
        ));
    }
    for i in (0..n).filter(|&i| depth[i] % 2 == 1) {
        // The hole belongs to its innermost container (depth − 1).
        if let Some(&outer) = containers[i]
            .iter()
            .filter(|&&j| depth[j] + 1 == depth[i])
            .min_by(|&&a, &&b| area[a].total_cmp(&area[b]))
        {
            items[index_of[outer]].1.loops.push(&fitted[i].path);
        }
    }
    items
        .into_iter()
        .map(|(i, mut item)| {
            if item.loops.len() == 1 {
                item.native = native_ok(i);
            }
            item
        })
        .collect()
}

/// Cheap image classification for [`Preset::Auto`].
fn classify(r: &raster::Raster, mixed: &[bool]) -> Preset {
    let mut buckets: HashMap<[u8; 4], usize> = HashMap::new();
    let mut flat = 0usize;
    for (i, c) in r.rgba.iter().enumerate() {
        if mixed[i] || c[3] == 0 {
            continue;
        }
        flat += 1;
        *buckets.entry(c.map(|v| v >> 4)).or_insert(0) += 1;
    }
    let min = (flat / 500).max(1);
    let significant = buckets.values().filter(|&&n| n >= min).count();
    match significant {
        0..=2 => Preset::Icon,
        3..=20 => Preset::Logo,
        _ => Preset::Illustration,
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// Anti-aliased disc rendered analytically (box-filter coverage via supersampling).
    pub fn disc(size: usize, cx: f64, cy: f64, r: f64, fg: [u8; 3], bg: [u8; 4]) -> Vec<u8> {
        let mut out = vec![0u8; size * size * 4];
        let ss = 8;
        for y in 0..size {
            for x in 0..size {
                let mut cov = 0;
                for sy in 0..ss {
                    for sx in 0..ss {
                        let px = x as f64 + (sx as f64 + 0.5) / ss as f64;
                        let py = y as f64 + (sy as f64 + 0.5) / ss as f64;
                        if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                            cov += 1;
                        }
                    }
                }
                let t = cov as f64 / (ss * ss) as f64;
                let i = (y * size + x) * 4;
                let ba = bg[3] as f64 / 255.0;
                let a = t + ba * (1.0 - t);
                for c in 0..3 {
                    let v = fg[c] as f64 * t + bg[c] as f64 * ba * (1.0 - t);
                    out[i + c] = if a > 0.0 { (v / a).round() as u8 } else { 0 };
                }
                out[i + 3] = (a * 255.0).round() as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::disc;
    use super::*;

    #[test]
    fn disc_on_white_two_layers_no_fringe() {
        let img = disc(64, 32.0, 32.0, 20.0, [220, 30, 60], [255, 255, 255, 255]);
        let out = convert(&img, 64, 64, &Options::default()).unwrap();
        assert_eq!(out.layers.len(), 2, "{:?}", out.layers);
        assert_eq!(out.layers[1].color, [220, 30, 60, 255]);
        assert_eq!(out.layers[1].subpaths, 1);
        assert!(out.layers[1].segments <= 6, "{:?}", out.layers[1]);
    }

    #[test]
    fn transparent_background_is_not_drawn() {
        let img = disc(48, 24.0, 24.0, 15.0, [10, 10, 10], [0, 0, 0, 0]);
        let out = convert(&img, 48, 48, &Options::default()).unwrap();
        assert_eq!(out.layers.len(), 1);
        assert_eq!(out.layers[0].color, [10, 10, 10, 255]);
    }

    /// Two separate red discs on white.
    fn two_discs() -> Vec<u8> {
        let a = disc(96, 24.0, 48.0, 14.0, [220, 30, 60], [255, 255, 255, 255]);
        let b = disc(96, 70.0, 48.0, 14.0, [220, 30, 60], [255, 255, 255, 255]);
        a.chunks(4)
            .zip(b.chunks(4))
            .flat_map(|(p, q)| {
                if p[1] < q[1] {
                    [p[0], p[1], p[2], p[3]]
                } else {
                    [q[0], q[1], q[2], q[3]]
                }
            })
            .collect()
    }

    #[test]
    fn native_shapes_grouped_per_colour() {
        let out = convert(&two_discs(), 96, 96, &Options::default()).unwrap();
        assert!(
            out.svg
                .contains(r#"<rect id="color-ffffff" x="0" y="0" width="96" height="96""#),
            "{}",
            out.svg
        );
        assert!(out.svg.contains(r#"<g id="color-dc1e3c">"#), "{}", out.svg);
        assert_eq!(out.svg.matches("<circle").count(), 2, "{}", out.svg);
        assert_eq!(out.stats.primitives, 3);
    }

    #[test]
    fn group_by_colour_and_flatten() {
        let opts = Options {
            group_by: GroupBy::Color,
            flatten_shapes: true,
            ..Options::default()
        };
        let out = convert(&two_discs(), 96, 96, &opts).unwrap();
        assert!(
            !out.svg.contains("<circle") && !out.svg.contains("<g"),
            "{}",
            out.svg
        );
        assert_eq!(out.svg.matches("<path").count(), 2);
    }

    #[test]
    fn palette_snapping() {
        let img = disc(64, 32.0, 32.0, 20.0, [220, 30, 60], [255, 255, 255, 255]);
        let opts = Options {
            palette: vec!["#dd2040".into(), "#fafafa".into()],
            ..Options::default()
        };
        let out = convert(&img, 64, 64, &opts).unwrap();
        let hex: Vec<&str> = out.layers.iter().map(|l| l.hex.as_str()).collect();
        assert_eq!(hex, ["#fafafa", "#dd2040"]);

        // Far-away palette colours are ignored unless the tolerance is huge.
        let opts = Options {
            palette: vec!["#00ff00".into()],
            ..Options::default()
        };
        let out = convert(&img, 64, 64, &opts).unwrap();
        assert!(out.layers.iter().all(|l| l.hex != "#00ff00"));
        let opts = Options {
            palette: vec!["#000000".into(), "#ffffff".into(), "#ff0000".into()],
            palette_tolerance: 1000.0,
            ..Options::default()
        };
        let out = convert(&img, 64, 64, &opts).unwrap();
        let hex: Vec<&str> = out.layers.iter().map(|l| l.hex.as_str()).collect();
        assert_eq!(hex, ["#ffffff", "#ff0000"]);
    }

    #[test]
    fn rejects_bad_buffer() {
        assert!(convert(&[0; 10], 2, 2, &Options::default()).is_err());
    }
}

#[cfg(test)]
mod accuracy_tests {
    use super::*;

    /// Sub-pixel contours of an anti-aliased disc lie on the true circle.
    #[test]
    fn contour_radial_error() {
        let img = tests_support::disc(64, 32.0, 32.0, 20.0, [27, 42, 74], [255, 255, 255, 255]);
        let r = raster::Raster::new(&img, 64, 64);
        let mixed = r.detect_mixed();
        let prm = options::Params::resolve(&Options::default(), Preset::Logo, 64, 64);
        let pal = quantize::build_palette(&r, &mixed, &prm);
        let labels = segment::assign(&r, &mixed, &pal, 0.03);
        let layers = layers::build_layers(&labels, &pal, 0.0, 0.3);
        let lp = &layers[1].loops[0];
        let mut maxe: f64 = 0.0;
        for p in lp {
            let e = ((p.x - 32.0).powi(2) + (p.y - 32.0).powi(2)).sqrt() - 20.0;
            maxe = maxe.max(e.abs());
        }
        assert!(maxe < 0.15, "max radial error {maxe}");
    }
}
