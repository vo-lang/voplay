use std::path::Path;

pub fn read_bytes(path: impl AsRef<Path>) -> Result<Vec<u8>, String> {
    let path = path.as_ref();
    let display = path.display().to_string();

    #[cfg(feature = "wasm")]
    {
        return read_bytes_wasm(path).map_err(|e| format!("read file '{}': {}", display, e));
    }

    #[cfg(not(feature = "wasm"))]
    {
        std::fs::read(path).map_err(|e| format!("read file '{}': {}", display, e))
    }
}

#[cfg(feature = "wasm")]
fn read_bytes_wasm(path: &Path) -> Result<Vec<u8>, String> {
    use wasm_bindgen::{JsCast, JsValue};

    let window = web_sys::window().ok_or_else(|| "no global window".to_string())?;
    let binding = js_sys::Reflect::get(window.as_ref(), &JsValue::from_str("_vfsReadFile"))
        .map_err(|_| "window._vfsReadFile lookup failed".to_string())?;
    let read_file = binding
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "window._vfsReadFile is not installed".to_string())?;
    let path_str = path.to_string_lossy();
    let result = read_file
        .call1(window.as_ref(), &JsValue::from_str(&path_str))
        .map_err(|e| format!("window._vfsReadFile threw: {:?}", e))?;

    let data =
        js_sys::Reflect::get_u32(&result, 0).map_err(|_| "invalid VFS read result".to_string())?;
    let err =
        js_sys::Reflect::get_u32(&result, 1).map_err(|_| "invalid VFS read result".to_string())?;

    if !err.is_null() && !err.is_undefined() {
        return Err(err
            .as_string()
            .unwrap_or_else(|| "unknown VFS read error".to_string()));
    }

    if data.is_null() || data.is_undefined() {
        return Ok(Vec::new());
    }

    Ok(js_sys::Uint8Array::new(&data).to_vec())
}
