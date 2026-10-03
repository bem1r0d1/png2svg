//! Regenerates `corpus/png` from `corpus/src/*.svg` plus synthetic cases.
//!
//! Rendering vector sources gives anti-aliased rasters with a known ground
//! truth, which is exactly what real-world exports from Figma/Illustrator look
//! like. Extra variants cover small sizes, JPEG artefacts, soft (upscaled) edges
//! and hard-edged pixel art.
//!
//! Usage: cargo run -p png2svg-metrics --bin gen_corpus [-- <corpus dir>]

use std::fs;
use std::path::PathBuf;

use image::{imageops, ImageBuffer, Rgba as Px, RgbaImage};
use png2svg_metrics::{render_svg, save_png, Result, Rgba};

fn to_image(r: &Rgba) -> RgbaImage {
    ImageBuffer::from_raw(r.w, r.h, r.data.clone()).unwrap()
}
fn from_image(i: &RgbaImage) -> Rgba {
    Rgba {
        w: i.width(),
        h: i.height(),
        data: i.as_raw().clone(),
    }
}

fn svg_size(svg: &str) -> (u32, u32) {
    let attr = |name: &str| -> u32 {
        let key = format!("{name}=\"");
        let s = svg
            .find(&key)
            .map(|i| &svg[i + key.len()..])
            .expect("width/height attribute");
        s[..s.find('"').unwrap()].parse::<f32>().unwrap() as u32
    };
    (attr("width"), attr("height"))
}

const SPRITE: [&str; 16] = [
    "................",
    ".....######.....",
    "...##oooooo##...",
    "..#oooooooooo#..",
    ".#oo##oooo##oo#.",
    ".#o#ww#oo#ww#o#.",
    "#oo#wk#oo#wk#oo#",
    "#ooo##oooo##ooo#",
    "#oooooooooooooo#",
    "#oorrooooooorro#",
    ".#oorrrrrrrroo#.",
    ".#ooorrrrrrooo#.",
    "..#oooooooooo#..",
    "...##oooooo##...",
    ".....######.....",
    "................",
];

fn sprite(scale: u32) -> RgbaImage {
    let color = |c: char| -> [u8; 4] {
        match c {
            '#' => [40, 24, 16, 255],
            'o' => [255, 170, 40, 255],
            'w' => [255, 255, 255, 255],
            'k' => [20, 20, 20, 255],
            'r' => [200, 40, 50, 255],
            _ => [0, 0, 0, 0],
        }
    };
    let mut img = RgbaImage::new(16 * scale, 16 * scale);
    for (y, row) in SPRITE.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            for dy in 0..scale {
                for dx in 0..scale {
                    img.put_pixel(x as u32 * scale + dx, y as u32 * scale + dy, Px(color(c)));
                }
            }
        }
    }
    img
}

fn main() -> Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("corpus"));
    let src = root.join("src");
    let out = root.join("png");
    fs::create_dir_all(&out)?;
    let mut names: Vec<PathBuf> = fs::read_dir(&src)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "svg"))
        .collect();
    names.sort();

    let write = |name: &str, img: &Rgba| -> Result<()> {
        let p = out.join(format!("{name}.png"));
        save_png(&p, img)?;
        println!("{}  {}x{}", p.display(), img.w, img.h);
        Ok(())
    };
    // Degraded inputs are scored against the clean render (`<name>.ref.png`):
    // the converter should remove the defect, not reproduce it.
    let degraded = |name: &str, img: &Rgba, reference: &Rgba| -> Result<()> {
        write(name, img)?;
        write(&format!("{name}.ref"), reference)
    };

    for path in &names {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let svg = fs::read_to_string(path)?;
        let (w, h) = svg_size(&svg);
        let img = render_svg(&svg, w, h)?;
        write(&stem, &img)?;
        if stem.starts_with("icon-") {
            write(&format!("{stem}-24px"), &render_svg(&svg, 24, 24)?)?;
        }
        match stem.as_str() {
            "flat-illustration" => {
                degraded(&format!("{stem}-jpeg"), &jpeg_roundtrip(&img, 55)?, &img)?
            }
            "badge" => {
                // JPEG has no alpha: the badge is exported on white.
                let white = over_white(&img);
                degraded(
                    &format!("{stem}-jpeg"),
                    &jpeg_roundtrip(&white, 35)?,
                    &white,
                )?
            }
            "logo-circles" => {
                // Soft edges: rendered small, then upscaled 4× with bilinear filtering.
                let small = to_image(&render_svg(&svg, w / 4, h / 4)?);
                let soft = imageops::resize(&small, w, h, imageops::FilterType::Triangle);
                degraded(&format!("{stem}-soft"), &from_image(&soft), &img)?;
                // Pixelated: rendered small, upscaled 4× with nearest neighbour.
                let px = imageops::resize(&small, w, h, imageops::FilterType::Nearest);
                degraded(&format!("{stem}-pixelated"), &from_image(&px), &img)?;
                // Aliased: no anti-aliasing at all (hard staircase edges).
                degraded(
                    &format!("{stem}-aliased"),
                    &render_svg(&crisp(&svg), w, h)?,
                    &img,
                )?;
                // Sensor-like noise + JPEG.
                degraded(
                    &format!("{stem}-noisy"),
                    &jpeg_roundtrip(&add_noise(&img, 12.0), 70)?,
                    &img,
                )?;
            }
            "icon-heart" => {
                let small = to_image(&render_svg(&svg, 16, 16)?);
                let px = imageops::resize(&small, 128, 128, imageops::FilterType::Nearest);
                degraded(
                    &format!("{stem}-pixelated"),
                    &from_image(&px),
                    &render_svg(&svg, 128, 128)?,
                )?;
            }
            "styled-lettering" => {
                // Like generated / upscaled artwork: rendered at half size,
                // upscaled with bilinear filtering, slightly noisy, JPEG.
                let small = to_image(&render_svg(&svg, w / 2, h / 2)?);
                let up = from_image(&imageops::resize(
                    &small,
                    w * 2,
                    h * 2,
                    imageops::FilterType::Triangle,
                ));
                let big = render_svg(&svg, w * 2, h * 2)?;
                degraded(
                    &format!("{stem}-generated"),
                    &jpeg_roundtrip(&add_noise(&up, 3.0), 85)?,
                    &big,
                )?;
            }
            "glyphs" => degraded(
                &format!("{stem}-aliased"),
                &render_svg(&crisp(&svg), w, h)?,
                &img,
            )?,
            _ => {}
        }
    }
    // Pixel art has no "correct" smooth version: visual check only.
    write("pixel-sprite", &from_image(&sprite(8)))?;
    fs::write(out.join("pixel-sprite.noref"), "")?;
    Ok(())
}

fn over_white(img: &Rgba) -> Rgba {
    let mut out = img.clone();
    for px in out.data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        for c in &mut px[..3] {
            *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        px[3] = 255;
    }
    out
}

/// Disables anti-aliasing for the whole document.
fn crisp(svg: &str) -> String {
    svg.replacen("<svg ", r#"<svg shape-rendering="crispEdges" "#, 1)
}

/// Deterministic Gaussian-ish noise (sum of uniforms) with the given σ.
fn add_noise(img: &Rgba, sigma: f64) -> Rgba {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut uni = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = img.clone();
    for px in out.data.chunks_exact_mut(4) {
        for c in &mut px[..3] {
            let n: f64 = (0..4).map(|_| uni()).sum::<f64>() - 2.0; // var = 1/3
            *c = (*c as f64 + n * sigma * 3f64.sqrt())
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn jpeg_roundtrip(img: &Rgba, quality: u8) -> Result<Rgba> {
    let rgb = image::DynamicImage::ImageRgba8(to_image(img)).to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality).encode_image(&rgb)?;
    let back = image::load_from_memory(&buf)?.to_rgba8();
    Ok(from_image(&back))
}
