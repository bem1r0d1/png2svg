//! Dev helper: `source (nearest zoom) | SVG rendered as vector at zoom | |diff|`.
//!
//! cargo run -p png2svg-metrics --bin sidebyside -- <name> [zoom] [x y size] [--dir report]
//!
//! With a crop (`x y size`, in source pixels) the panel shows that region only,
//! which makes it easy to inspect edge quality at high magnification.

use std::path::PathBuf;

use png2svg_metrics::{load_image, render_svg, save_png, Result, Rgba};

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = PathBuf::from("report");
    if let Some(i) = args.iter().position(|a| a == "--dir") {
        dir = PathBuf::from(args.remove(i + 1));
        args.remove(i);
    }
    let name = args
        .first()
        .ok_or("usage: sidebyside <name> [zoom] [x y size] [--dir report]")?;
    let zoom: u32 = args.get(1).map(|z| z.parse()).transpose()?.unwrap_or(2);
    let src = load_image(&dir.join(format!("{name}.png")))?;
    let (cx, cy, cw, ch) = match (args.get(2), args.get(3), args.get(4)) {
        (Some(x), Some(y), Some(s)) => {
            let s: u32 = s.parse()?;
            (x.parse()?, y.parse()?, s.min(src.w), s.min(src.h))
        }
        _ => (0, 0, src.w, src.h),
    };
    let svg = std::fs::read_to_string(dir.join(format!("{name}.svg")))?;
    // True vector render at the zoomed resolution.
    let vec = render_svg(&svg, src.w * zoom, src.h * zoom)?;

    let (w, h) = (cw * zoom, ch * zoom);
    let mut out = Rgba {
        w: w * 3 + 8,
        h,
        data: vec![255; ((w * 3 + 8) * h * 4) as usize],
    };
    let over_white = |img: &Rgba, x: u32, y: u32| -> [u8; 3] {
        let i = ((y * img.w + x) * 4) as usize;
        let al = img.data[i + 3] as u32;
        [0, 1, 2].map(|c| ((img.data[i + c] as u32 * al + 255 * (255 - al)) / 255) as u8)
    };
    for y in 0..h {
        for x in 0..w {
            let pa = over_white(&src, cx + x / zoom, cy + y / zoom);
            let pb = over_white(&vec, cx * zoom + x, cy * zoom + y);
            let d = (0..3)
                .map(|c| (pa[c] as i32 - pb[c] as i32).unsigned_abs())
                .max()
                .unwrap();
            let pd = [255u8, 255 - d.min(255) as u8, 255 - d.min(255) as u8];
            for (k, p) in [pa, pb, pd].iter().enumerate() {
                let ox = x + k as u32 * (w + 4);
                let i = ((y * out.w + ox) * 4) as usize;
                out.data[i..i + 3].copy_from_slice(p);
                out.data[i + 3] = 255;
            }
        }
    }
    let path = dir.join(format!("{name}.side.png"));
    save_png(&path, &out)?;
    println!("{}", path.display());
    Ok(())
}
