//! png2svg command-line tool.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use png2svg_core::{convert, GroupBy, Options, Preset};
use png2svg_metrics::{compare, load_image, render_svg};

const USAGE: &str = "\
png2svg — high-quality raster to vector conversion

USAGE:
    png2svg <input.png> [-o output.svg] [options]

OPTIONS:
    -o, --output <file>        output path (default: input with .svg extension, '-' = stdout)
    -p, --preset <name>        auto | logo | icon | illustration   (default: auto)
    -c, --colors <n>           fixed palette size (default: automatic)
    -d, --detail <0..1>        colour separation / small-detail retention (default: 0.5)
    -s, --smoothness <0..1>    curve tolerance, higher = fewer nodes (default: 0.5)
        --corner <degrees>     corner angle threshold (default: 60)
        --speckle <px>         minimum region area (default: from detail)
        --no-snap              do not snap near-axis lines to horizontal/vertical
        --precision <0..4>     decimals in path data (default: 2)
        --no-shapes            do not detect circles / ellipses / (rounded) rectangles
        --flatten-shapes       write detected shapes as paths instead of <circle>/<ellipse>/<rect>
        --group-by <mode>      shape (one element per shape, default) | color (one path per colour)
        --palette <colors>     comma-separated #rrggbb list; close colours snap to it exactly
        --palette-tolerance <ΔE>  snapping distance, ΔE OKLab×100 (default: 3; 1000 = force palette)
    -m, --metrics              render the SVG back and print SSIM / ΔE vs. the input
    -h, --help                 show this help
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<String> = None;
    let mut opts = Options::default();
    let mut metrics = false;
    let mut args = std::env::args().skip(1);
    let val = |args: &mut dyn Iterator<Item = String>, flag: &str| -> Result<String, String> {
        args.next().ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-o" | "--output" => output = Some(val(&mut args, &a)?),
            "-p" | "--preset" => {
                opts.preset = match val(&mut args, &a)?.as_str() {
                    "auto" => Preset::Auto,
                    "logo" => Preset::Logo,
                    "icon" => Preset::Icon,
                    "illustration" => Preset::Illustration,
                    p => return Err(format!("unknown preset '{p}'").into()),
                }
            }
            "-c" | "--colors" => opts.colors = Some(val(&mut args, &a)?.parse()?),
            "-d" | "--detail" => opts.detail = val(&mut args, &a)?.parse()?,
            "-s" | "--smoothness" => opts.smoothness = val(&mut args, &a)?.parse()?,
            "--corner" => opts.corner_threshold = val(&mut args, &a)?.parse()?,
            "--speckle" => opts.speckle_area = Some(val(&mut args, &a)?.parse()?),
            "--no-snap" => opts.snap_axes = false,
            "--precision" => opts.precision = val(&mut args, &a)?.parse()?,
            "--no-shapes" => opts.shapes = false,
            "--flatten-shapes" => opts.flatten_shapes = true,
            "--group-by" => {
                opts.group_by = match val(&mut args, &a)?.as_str() {
                    "shape" => GroupBy::Shape,
                    "color" | "colour" => GroupBy::Color,
                    g => return Err(format!("unknown group-by '{g}'").into()),
                }
            }
            "--palette" => {
                opts.palette = val(&mut args, &a)?
                    .split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect()
            }
            "--palette-tolerance" => opts.palette_tolerance = val(&mut args, &a)?.parse()?,
            "-m" | "--metrics" => metrics = true,
            s if s.starts_with('-') && s != "-" => {
                return Err(format!("unknown option '{s}'\n\n{USAGE}").into())
            }
            _ if input.is_none() => input = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument '{a}'").into()),
        }
    }
    let input = input.ok_or(format!("missing input file\n\n{USAGE}"))?;
    let img = load_image(&input)?;
    let t0 = Instant::now();
    let out = convert(&img.data, img.w, img.h, &opts)?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let output =
        output.unwrap_or_else(|| input.with_extension("svg").to_string_lossy().into_owned());
    if output == "-" {
        print!("{}", out.svg);
    } else {
        std::fs::write(&output, &out.svg)?;
    }
    let s = &out.stats;
    eprintln!(
        "{} → {}: {}x{}, preset {}, {} colours, {} layers, {} subpaths, {} segments, {} shapes, {} bytes, {:.1} ms",
        input.display(),
        output,
        img.w,
        img.h,
        s.preset,
        s.colors,
        s.layers,
        s.subpaths,
        s.segments,
        s.primitives,
        s.bytes,
        ms
    );
    if metrics {
        let back = render_svg(&out.svg, img.w, img.h)?;
        let q = compare(&img, &back);
        eprintln!(
            "quality: SSIM {:.4}, ΔE mean {:.3}, ΔE p99 {:.2}",
            q.ssim, q.mean_de, q.p99_de
        );
    }
    Ok(())
}
