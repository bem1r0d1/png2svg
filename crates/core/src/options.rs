//! Public conversion options and presets.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum Preset {
    /// Pick a preset from image analysis.
    Auto,
    /// Flat logos: few colors, crisp corners, exact brand colors.
    Logo,
    /// Mono / duo-tone icons: maximum smoothness.
    Icon,
    /// Flat illustrations with many colors.
    Illustration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(rename_all = "lowercase")
)]
pub enum Layering {
    /// Lower layers extend under upper ones: no seams between shapes.
    Stacked,
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(default, rename_all = "camelCase")
)]
pub struct Options {
    pub preset: Preset,
    /// Fixed palette size (excluding transparency). `None` = automatic.
    pub colors: Option<usize>,
    /// 0..1 — higher keeps more similar colors apart and smaller details.
    pub detail: f32,
    /// 0..1 — higher allows larger curve-fitting tolerance (fewer nodes).
    pub smoothness: f32,
    /// Turning angle (degrees) above which a contour point becomes a corner.
    pub corner_threshold: f32,
    /// Regions smaller than this (px²) are merged into a neighbour. `None` = from `detail`.
    pub speckle_area: Option<usize>,
    pub layering: Layering,
    /// Snap near-horizontal / near-vertical lines to the axes.
    pub snap_axes: bool,
    /// Decimal places in path data.
    pub precision: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            preset: Preset::Auto,
            colors: None,
            detail: 0.5,
            smoothness: 0.5,
            corner_threshold: 60.0,
            speckle_area: None,
            layering: Layering::Stacked,
            snap_axes: true,
            precision: 2,
        }
    }
}

/// Concrete numeric parameters derived from [`Options`] + preset.
#[derive(Clone, Debug)]
pub(crate) struct Params {
    pub max_colors: usize,
    pub fixed_colors: Option<usize>,
    /// OKLab distance below which palette entries are merged.
    pub merge_dist: f32,
    pub speckle_area: usize,
    /// Max curve deviation in px.
    pub fit_tolerance: f64,
    /// Max deviation for a segment to become a straight line.
    pub line_tolerance: f64,
    pub corner_angle: f64,
    /// Arc length (px) used to measure turning angles.
    pub corner_scale: f64,
    pub snap_axes: bool,
    pub precision: u8,
}

impl Params {
    pub fn resolve(o: &Options, preset: Preset, w: u32, h: u32) -> Self {
        let detail = o.detail.clamp(0.0, 1.0);
        let smooth = o.smoothness.clamp(0.0, 1.0) as f64;
        let (max_colors, merge_base, corner_bias) = match preset {
            Preset::Icon => (8, 0.03, 0.0),
            Preset::Logo | Preset::Auto => (24, 0.02, 0.0),
            Preset::Illustration => (64, 0.016, 10.0),
        };
        let merge_dist = merge_base * (1.5 - detail);
        let mp = (w as f64 * h as f64) / 1.0e6;
        let auto_speckle =
            ((2.0 + 10.0 * (1.0 - detail as f64)) * mp.sqrt().max(0.25)).round() as usize;
        Self {
            max_colors,
            fixed_colors: o.colors.filter(|&c| c > 0),
            merge_dist,
            speckle_area: o.speckle_area.unwrap_or(auto_speckle.max(2)),
            fit_tolerance: 0.15 + 0.6 * smooth,
            line_tolerance: 0.12 + 0.35 * smooth,
            corner_angle: (o.corner_threshold as f64 + corner_bias).to_radians(),
            corner_scale: 1.5 + 1.5 * smooth,
            snap_axes: o.snap_axes,
            precision: o.precision.min(4),
        }
    }
}
