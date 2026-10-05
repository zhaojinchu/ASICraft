//! The compiler core as a WebAssembly module, for browsers and for checking
//! that the WebAssembly build gives the same output as the native one.
//!
//! The interface is three plain functions and JSON text, so no binding
//! generator is needed:
//!
//! - `redfab_alloc(len)` returns memory for the caller to write `len` bytes into.
//! - `redfab_build(ptr, len)` takes a JSON request `{"mapped", "top", "name"}`
//!   (`mapped` is the text of a Yosys JSON netlist) and returns a pointer to the
//!   reply: four bytes of little-endian length, then that many bytes of JSON,
//!   either `{"ok": true, "design": <design file text>, "structure": <hex of
//!   the .nbt file>, "report": ...}` or `{"ok": false, "error": ...}`.
//! - `redfab_free(ptr, len)` releases memory from either call.

use std::fmt::Write as _;

fn respond(request: &[u8]) -> String {
    let result = (|| -> Result<serde_json::Value, String> {
        let request: serde_json::Value =
            serde_json::from_slice(request).map_err(|e| format!("request: {e}"))?;
        let text = |key: &str| {
            request[key]
                .as_str()
                .ok_or_else(|| format!("request is missing {key:?}"))
        };
        let built = redfab_core::build::build(text("mapped")?, text("top")?, text("name")?)?;
        let mut hex = String::with_capacity(built.structure.len() * 2);
        for byte in &built.structure {
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Ok(
            serde_json::json!({"ok": true, "design": built.design.to_json(), "structure": hex, "report": built.report}),
        )
    })();
    result
        .unwrap_or_else(|error| serde_json::json!({"ok": false, "error": error}))
        .to_string()
}

#[unsafe(no_mangle)]
pub extern "C" fn redfab_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// # Safety
/// `ptr` and `len` must come from `redfab_alloc(len)`, or from `redfab_build`
/// (whose reply is `4 + length` bytes long), and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn redfab_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn redfab_build(ptr: *const u8, len: usize) -> *mut u8 {
    let request = unsafe { std::slice::from_raw_parts(ptr, len) };
    let reply = respond(request);
    let mut out = Vec::with_capacity(4 + reply.len());
    out.extend_from_slice(&(reply.len() as u32).to_le_bytes());
    out.extend_from_slice(reply.as_bytes());
    out.shrink_to_fit();
    let ptr = out.as_mut_ptr();
    std::mem::forget(out);
    ptr
}
