//! WebAssembly entry points used by the Figma plugin (runs in a Web Worker).
//!
//! A deliberately tiny C ABI so the JS side needs no generated glue:
//!
//! ```text
//! ptr = p2s_alloc(len)                  // allocate input buffers, write into memory
//! ok  = p2s_convert(rgba, rgba_len, w, h, opts, opts_len)   // 1 = ok, 0 = error
//! p2s_result_ptr() / p2s_result_len()   // UTF-8 JSON: Output on success, {"error"} otherwise
//! p2s_free(ptr, len)                    // release input buffers
//! ```

use std::cell::RefCell;

use png2svg_core::{convert, Options};

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Allocates `len` bytes inside WASM memory.
#[no_mangle]
pub extern "C" fn p2s_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Frees a buffer returned by [`p2s_alloc`].
///
/// # Safety
/// `ptr`/`len` must come from a single `p2s_alloc(len)` call.
#[no_mangle]
pub unsafe extern "C" fn p2s_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

/// Converts straight RGBA8 pixels; options are (partial) camelCase JSON.
///
/// # Safety
/// Pointers must reference initialised buffers of the given lengths.
#[no_mangle]
pub unsafe extern "C" fn p2s_convert(
    rgba: *const u8,
    rgba_len: usize,
    width: u32,
    height: u32,
    opts: *const u8,
    opts_len: usize,
) -> u32 {
    let rgba = std::slice::from_raw_parts(rgba, rgba_len);
    let opts = std::slice::from_raw_parts(opts, opts_len);
    let (ok, json) = match convert_json(rgba, width, height, opts) {
        Ok(j) => (1, j),
        Err(e) => (0, serde_json::json!({ "error": e }).to_string()),
    };
    RESULT.with(|r| *r.borrow_mut() = json.into_bytes());
    ok
}

#[no_mangle]
pub extern "C" fn p2s_result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn p2s_result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

/// Safe core of [`p2s_convert`].
pub fn convert_json(rgba: &[u8], width: u32, height: u32, opts: &[u8]) -> Result<String, String> {
    let opts: Options = if opts.iter().all(u8::is_ascii_whitespace) {
        Options::default()
    } else {
        serde_json::from_slice(opts).map_err(|e| format!("bad options: {e}"))?
    };
    let out = convert(rgba, width, height, &opts).map_err(|e| e.to_string())?;
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_options_json() {
        let px = [255u8, 0, 0, 255].repeat(16);
        let json = convert_json(&px, 4, 4, br#"{"preset":"logo","smoothness":0.3}"#).unwrap();
        assert!(json.contains("\"svg\""));
        assert!(json.contains("#ff0000"));
    }

    #[test]
    fn bad_options_reported() {
        let px = [0u8; 16];
        assert!(convert_json(&px, 2, 2, b"{\"preset\":\"nope\"}").is_err());
    }

    #[test]
    fn abi_roundtrip() {
        let px = [0u8, 0, 255, 255].repeat(9);
        let opts = b"{}";
        unsafe {
            let p = p2s_alloc(px.len());
            std::ptr::copy_nonoverlapping(px.as_ptr(), p, px.len());
            assert_eq!(p2s_convert(p, px.len(), 3, 3, opts.as_ptr(), opts.len()), 1);
            p2s_free(p, px.len());
        }
        let json = unsafe { std::slice::from_raw_parts(p2s_result_ptr(), p2s_result_len()) };
        assert!(std::str::from_utf8(json).unwrap().contains("#0000ff"));
    }
}
