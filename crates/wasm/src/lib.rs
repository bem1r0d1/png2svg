//! WebAssembly bindings used by the Figma plugin (runs inside a Web Worker).

use png2svg_core::{convert as core_convert, Options};
use wasm_bindgen::prelude::*;

/// Converts straight RGBA8 pixels to SVG.
///
/// `options_json` is a (possibly partial) JSON object matching `Options`
/// (camelCase keys); missing fields use defaults. Returns a JSON string with
/// `{ svg, width, height, layers, stats }`.
#[wasm_bindgen]
pub fn convert(
    rgba: &[u8],
    width: u32,
    height: u32,
    options_json: &str,
) -> Result<String, JsError> {
    let opts: Options = if options_json.trim().is_empty() {
        Options::default()
    } else {
        serde_json::from_str(options_json)?
    };
    let out = core_convert(rgba, width, height, &opts)?;
    Ok(serde_json::to_string(&out)?)
}

#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_options_json() {
        let px = [255u8, 0, 0, 255].repeat(16);
        let json = convert(&px, 4, 4, r#"{"preset":"logo","smoothness":0.3}"#).unwrap();
        assert!(json.contains("\"svg\""));
        assert!(json.contains("#ff0000"));
    }
}
