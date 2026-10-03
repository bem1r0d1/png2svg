#!/usr/bin/env bash
# Builds the static site for GitHub Pages into ./site:
#   index.html                 – web version of the converter (WASM, runs locally)
#   png2svg-figma-plugin.zip   – Figma plugin (manifest + dist), import via
#                                Plugins → Development → Import plugin from manifest
#   report/                    – corpus quality report (source vs SVG)
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -p png2svg-wasm --target wasm32-unknown-unknown --release
(cd packages/figma-plugin && npm ci --no-audit --no-fund && node build.mjs)
cargo run --release -p png2svg-metrics --bin bench -- --report report >/dev/null

rm -rf site && mkdir -p site
cp packages/figma-plugin/dist/web/index.html site/index.html
(cd packages/figma-plugin && rm -f ../../site/png2svg-figma-plugin.zip &&
  zip -qr ../../site/png2svg-figma-plugin.zip manifest.json dist/code.js dist/ui.html)
cp -r report site/report
touch site/.nojekyll
echo "site ready: $(du -sh site | cut -f1)"
