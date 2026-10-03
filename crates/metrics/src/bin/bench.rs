//! Runs the converter over the corpus, measures quality and writes an HTML report.
//!
//! cargo run --release -p png2svg-metrics --bin bench -- [options]
//!   --corpus <dir>     PNG directory (default corpus/png)
//!   --report <dir>     output directory (default report)
//!   --baseline <file>  baseline JSON (default corpus/baseline.json)
//!   --check            fail if any image regresses against the baseline
//!   --update           overwrite the baseline with current results

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use png2svg_core::{convert, Options};
use png2svg_metrics::{compare, load_image, render_svg, save_png, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct Row {
    ssim: f64,
    mean_de: f64,
    p99_de: f64,
    layers: usize,
    segments: usize,
    bytes: usize,
}

// Allowed regressions before `--check` fails.
const SSIM_DROP: f64 = 0.004;
const DE_RISE: f64 = 0.25;
const SEG_RISE: f64 = 0.15;

fn main() -> Result<()> {
    let mut corpus = PathBuf::from("corpus/png");
    let mut report = PathBuf::from("report");
    let mut baseline = PathBuf::from("corpus/baseline.json");
    let (mut check, mut update) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--corpus" => corpus = args.next().ok_or("--corpus <dir>")?.into(),
            "--report" => report = args.next().ok_or("--report <dir>")?.into(),
            "--baseline" => baseline = args.next().ok_or("--baseline <file>")?.into(),
            "--check" => check = true,
            "--update" => update = true,
            _ => return Err(format!("unknown argument {a}").into()),
        }
    }
    fs::create_dir_all(&report)?;
    let mut files: Vec<PathBuf> = fs::read_dir(&corpus)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .filter(|p| !p.to_string_lossy().ends_with(".ref.png"))
        .collect();
    files.sort();

    let base: BTreeMap<String, Row> = fs::read_to_string(&baseline)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut results: BTreeMap<String, Row> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut html = String::from(HTML_HEAD);

    println!(
        "{:<28} {:>7} {:>7} {:>7} {:>6} {:>6} {:>7} {:>7}  preset",
        "image", "SSIM", "ΔE", "ΔE p99", "layers", "segs", "bytes", "ms"
    );
    for f in &files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        let img = load_image(f)?;
        let t0 = Instant::now();
        let out = convert(&img.data, img.w, img.h, &Options::default())?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let back = render_svg(&out.svg, img.w, img.h)?;
        // Degraded inputs are scored against their clean reference.
        let ref_path = f.with_extension("ref.png");
        let reference = if ref_path.exists() {
            load_image(&ref_path)?
        } else {
            img.clone()
        };
        let q = compare(&reference, &back);
        let row = Row {
            ssim: q.ssim,
            mean_de: q.mean_de,
            p99_de: q.p99_de,
            layers: out.stats.layers,
            segments: out.stats.segments,
            bytes: out.stats.bytes,
        };
        println!(
            "{:<28} {:>7.4} {:>7.3} {:>7.2} {:>6} {:>6} {:>7} {:>7.1}  {}",
            name,
            row.ssim,
            row.mean_de,
            row.p99_de,
            row.layers,
            row.segments,
            row.bytes,
            ms,
            out.stats.preset
        );
        fs::write(report.join(format!("{name}.svg")), &out.svg)?;
        fs::copy(f, report.join(format!("{name}.png")))?;
        save_png(&report.join(format!("{name}.render.png")), &back)?;
        html.push_str(&format!(
            r#"<section><h2>{name}</h2><p>SSIM {:.4} · ΔE {:.3} (p99 {:.2}) · {} layers · {} segments · {} bytes · {}</p>
<div class="row"><figure><img src="{name}.png"><figcaption>source</figcaption></figure><figure><img src="{name}.svg"><figcaption>svg</figcaption></figure></div></section>
"#,
            row.ssim, row.mean_de, row.p99_de, row.layers, row.segments, row.bytes, out.stats.preset
        ));

        // Inputs without a meaningful reference are only shown in the report.
        if f.with_extension("noref").exists() {
            continue;
        }
        if let Some(b) = base.get(&name) {
            let mut why = Vec::new();
            if row.ssim < b.ssim - SSIM_DROP {
                why.push(format!("SSIM {:.4} < {:.4}", row.ssim, b.ssim));
            }
            if row.mean_de > b.mean_de + DE_RISE {
                why.push(format!("ΔE {:.3} > {:.3}", row.mean_de, b.mean_de));
            }
            if row.segments as f64 > b.segments as f64 * (1.0 + SEG_RISE) + 2.0 {
                why.push(format!("segments {} > {}", row.segments, b.segments));
            }
            if !why.is_empty() {
                failures.push(format!("{name}: {}", why.join(", ")));
            }
        }
        results.insert(name, row);
    }
    html.push_str("</body></html>\n");
    fs::write(report.join("index.html"), html)?;

    let n = results.len() as f64;
    println!(
        "\nmean SSIM {:.4}, mean ΔE {:.3}; report: {}",
        results.values().map(|r| r.ssim).sum::<f64>() / n,
        results.values().map(|r| r.mean_de).sum::<f64>() / n,
        report.join("index.html").display()
    );
    if update {
        fs::write(&baseline, serde_json::to_string_pretty(&results)? + "\n")?;
        println!("baseline updated: {}", baseline.display());
    }
    if check {
        if failures.is_empty() {
            println!("no regressions against {}", baseline.display());
        } else {
            eprintln!("regressions:\n  {}", failures.join("\n  "));
            std::process::exit(1);
        }
    }
    Ok(())
}

const HTML_HEAD: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>png2svg report</title>
<style>
body{font:14px system-ui,sans-serif;margin:24px;background:#f4f4f5;color:#18181b}
section{background:#fff;border-radius:8px;padding:12px 16px;margin-bottom:16px}
h2{font-size:16px;margin:0 0 4px}.row{display:flex;gap:12px;flex-wrap:wrap}
figure{margin:0}img{width:320px;image-rendering:pixelated;background:repeating-conic-gradient(#ddd 0 25%,#fff 0 50%) 0 0/16px 16px;border:1px solid #ddd}
figcaption{color:#71717a;font-size:12px}
</style></head><body><h1>png2svg corpus report</h1>
"#;
