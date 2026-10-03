//! Dev helper: writes `source | rendered SVG | |diff|` side by side, zoomed.
//! cargo run -p png2svg-metrics --bin sidebyside -- <name> [zoom] [report dir]

use std::path::PathBuf;

use png2svg_metrics::{load_image, save_png, Result, Rgba};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args
        .next()
        .ok_or("usage: sidebyside <name> [zoom] [report dir]")?;
    let zoom: u32 = args.next().map(|z| z.parse()).transpose()?.unwrap_or(2);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "report".into()));
    let a = load_image(&dir.join(format!("{name}.png")))?;
    let b = load_image(&dir.join(format!("{name}.render.png")))?;
    let (w, h) = (a.w * zoom, a.h * zoom);
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
            let (sx, sy) = (x / zoom, y / zoom);
            let pa = over_white(&a, sx, sy);
            let pb = over_white(&b, sx, sy);
            let d = (0..3)
                .map(|c| (pa[c] as i32 - pb[c] as i32).unsigned_abs())
                .max()
                .unwrap();
            let pd = [255u8, (255 - d.min(255) as u8), (255 - d.min(255) as u8)];
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
