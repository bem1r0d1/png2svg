//! png2svg core: raster → clean, Figma-friendly SVG.
//!
//! Pipeline: anti-aliasing detection → OKLab palette → two-colour blend
//! labelling → speckle removal → stacked layers → sub-pixel iso-contours →
//! corner-aware Bézier fitting → compact SVG.

pub mod color;
mod contour;
mod fit;
pub mod geom;
mod layers;
mod options;
mod quantize;
mod raster;
mod segment;
mod svg;

use std::collections::HashMap;

pub use options::{Layering, Options, Preset};

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
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize), serde(rename_all = "camelCase"))]
pub struct Stats {
    pub preset: String,
    pub colors: usize,
    pub layers: usize,
    pub subpaths: usize,
    pub segments: usize,
    pub bytes: usize,
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
    let r = raster::Raster::new(rgba, w, h);
    let mixed = r.detect_mixed();
    let preset = match opts.preset {
        Preset::Auto => classify(&r, &mixed),
        p => p,
    };
    let prm = options::Params::resolve(opts, preset, width, height);

    let pal = quantize::build_palette(&r, &mixed, &prm);
    let mut labels = segment::assign(&r, &mixed, &pal);
    segment::remove_speckles(&mut labels, prm.speckle_area);
    let layers = layers::build_layers(&labels, &pal);

    let fitted: Vec<Vec<fit::FittedLoop>> = layers
        .iter()
        .map(|l| l.loops.iter().map(|lp| fit::fit_loop(lp, &prm)).collect())
        .collect();

    let mut used_ids: HashMap<String, usize> = HashMap::new();
    let mut infos = Vec::with_capacity(layers.len());
    let mut svg_layers = Vec::with_capacity(layers.len());
    for (layer, loops) in layers.iter().zip(&fitted) {
        let rgba = pal.colors[layer.label as usize].rgba;
        let hx = svg::hex(rgba);
        let base = format!("color-{}", &hx[1..]);
        let k = used_ids.entry(base.clone()).or_insert(0);
        *k += 1;
        let id = if *k == 1 { base } else { format!("{base}-{k}") };
        infos.push(LayerInfo {
            id: id.clone(),
            color: rgba,
            hex: hx,
            area: layer.area,
            subpaths: loops.len(),
            segments: loops.iter().map(|l| l.segs.len()).sum(),
        });
        svg_layers.push(svg::SvgLayer { id, rgba, loops });
    }
    let svg = svg::write_svg(width, height, &svg_layers, prm.precision);
    let stats = Stats {
        preset: format!("{preset:?}").to_lowercase(),
        colors: pal.colors.iter().filter(|c| !c.transparent).count(),
        layers: infos.len(),
        subpaths: infos.iter().map(|l| l.subpaths).sum(),
        segments: infos.iter().map(|l| l.segments).sum(),
        bytes: svg.len(),
    };
    Ok(Output {
        svg,
        width,
        height,
        layers: infos,
        stats,
    })
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
        let labels = segment::assign(&r, &mixed, &pal);
        let layers = layers::build_layers(&labels, &pal);
        let lp = &layers[1].loops[0];
        let mut maxe: f64 = 0.0;
        for p in lp {
            let e = ((p.x - 32.0).powi(2) + (p.y - 32.0).powi(2)).sqrt() - 20.0;
            maxe = maxe.max(e.abs());
        }
        assert!(maxe < 0.15, "max radial error {maxe}");
    }
}
