//! Rendering and quality metrics used by the CLI, the benchmark and CI.
//!
//! The vector result is rendered back with resvg at the source resolution and
//! compared with the original raster:
//! * **SSIM** on luma (structure — catches jaggies, wobble, lost details);
//! * **ΔE (OKLab × 100)** per pixel — mean and 99th percentile (colour accuracy,
//!   fringes, seams).
//!
//! Both are computed with the images composited over white *and* over black, so
//! errors in transparency count too.

use std::path::Path;

use png2svg_core::color::srgb8_to_oklab;
use resvg::{tiny_skia, usvg};

/// Straight-alpha RGBA8 image.
#[derive(Clone)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn load_image(path: &Path) -> Result<Rgba> {
    let img = image::open(path)?.to_rgba8();
    Ok(Rgba {
        w: img.width(),
        h: img.height(),
        data: img.into_raw(),
    })
}

pub fn save_png(path: &Path, img: &Rgba) -> Result<()> {
    image::save_buffer(path, &img.data, img.w, img.h, image::ColorType::Rgba8)?;
    Ok(())
}

/// Render SVG to straight RGBA at `w × h` (viewBox scaled to fit).
pub fn render_svg(svg: &str, w: u32, h: u32) -> Result<Rgba> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())?;
    let mut pm = tiny_skia::Pixmap::new(w, h).ok_or("bad size")?;
    let size = tree.size();
    let ts = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, ts, &mut pm.as_mut());
    let data = pm
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Ok(Rgba { w, h, data })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Quality {
    pub ssim: f64,
    pub mean_de: f64,
    pub p99_de: f64,
}

fn composite(img: &Rgba, bg: f64) -> Vec<[u8; 3]> {
    img.data
        .chunks_exact(4)
        .map(|c| {
            let a = c[3] as f64 / 255.0;
            [0, 1, 2].map(|i| (c[i] as f64 * a + bg * (1.0 - a)).round() as u8)
        })
        .collect()
}

pub fn compare(a: &Rgba, b: &Rgba) -> Quality {
    assert_eq!((a.w, a.h), (b.w, b.h), "size mismatch");
    let mut q = Quality::default();
    let mut des = Vec::with_capacity(a.data.len() / 2);
    for bg in [255.0, 0.0] {
        let ca = composite(a, bg);
        let cb = composite(b, bg);
        for (x, y) in ca.iter().zip(&cb) {
            if x == y {
                des.push(0.0);
                continue;
            }
            let (la, lb) = (srgb8_to_oklab(*x), srgb8_to_oklab(*y));
            let d = ((la[0] - lb[0]).powi(2) + (la[1] - lb[1]).powi(2) + (la[2] - lb[2]).powi(2))
                .sqrt();
            des.push(d as f64 * 100.0);
        }
        q.ssim += 0.5 * ssim(&luma(&ca), &luma(&cb), a.w as usize, a.h as usize);
    }
    q.mean_de = des.iter().sum::<f64>() / des.len() as f64;
    des.sort_by(|x, y| x.total_cmp(y));
    q.p99_de = des[((des.len() as f64 * 0.99) as usize).min(des.len() - 1)];
    q
}

fn luma(px: &[[u8; 3]]) -> Vec<f64> {
    px.iter()
        .map(|c| 0.299 * c[0] as f64 + 0.587 * c[1] as f64 + 0.114 * c[2] as f64)
        .collect()
}

/// Mean SSIM over 8×8 windows with stride 4 (Wang et al. 2004 constants).
fn ssim(a: &[f64], b: &[f64], w: usize, h: usize) -> f64 {
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let win = 8.min(w).min(h);
    let step = (win / 2).max(1);
    let (mut sum, mut cnt) = (0.0, 0usize);
    let mut y = 0;
    while y + win <= h {
        let mut x = 0;
        while x + win <= w {
            let (mut ma, mut mb) = (0.0, 0.0);
            for yy in y..y + win {
                for xx in x..x + win {
                    ma += a[yy * w + xx];
                    mb += b[yy * w + xx];
                }
            }
            let n = (win * win) as f64;
            ma /= n;
            mb /= n;
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for yy in y..y + win {
                for xx in x..x + win {
                    let da = a[yy * w + xx] - ma;
                    let db = b[yy * w + xx] - mb;
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            va /= n - 1.0;
            vb /= n - 1.0;
            cov /= n - 1.0;
            sum += ((2.0 * ma * mb + C1) * (2.0 * cov + C2))
                / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            cnt += 1;
            x += step;
        }
        y += step;
    }
    if cnt == 0 {
        1.0
    } else {
        sum / cnt as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_is_perfect() {
        let img = Rgba {
            w: 16,
            h: 16,
            data: (0..16 * 16 * 4).map(|i| (i * 7 % 256) as u8).collect(),
        };
        let q = compare(&img, &img);
        assert!((q.ssim - 1.0).abs() < 1e-9);
        assert_eq!(q.mean_de, 0.0);
    }

    #[test]
    fn render_roundtrip() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path fill="#ff0000" d="M0 0h10v10H0z"/></svg>"##;
        let img = render_svg(svg, 10, 10).unwrap();
        assert_eq!(&img.data[..4], &[255, 0, 0, 255]);
    }
}
