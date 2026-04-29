#[cfg(feature = "native")]
use std::ffi::c_void;
#[cfg(feature = "native")]
use std::sync::{Mutex, OnceLock};

#[cfg(feature = "native")]
pub const HOST_API_VERSION: u32 = 1;
#[cfg(feature = "native")]
pub const SURFACE_KIND_APPKIT: u32 = 1;
#[cfg(feature = "native")]
pub const SURFACE_KIND_CORE_ANIMATION_LAYER: u32 = 2;

#[cfg(feature = "native")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NativeSurfaceDesc {
    pub kind: u32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub native_handle: *mut c_void,
}

#[cfg(feature = "native")]
pub type InitSurfaceFn = unsafe extern "C" fn(
    canvas_ref_ptr: *const u8,
    canvas_ref_len: usize,
    out_desc: *mut NativeSurfaceDesc,
) -> bool;

#[cfg(feature = "native")]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApi {
    pub version: u32,
    pub init_surface: InitSurfaceFn,
}

#[cfg(feature = "native")]
static HOST_API: OnceLock<Mutex<Option<HostApi>>> = OnceLock::new();

#[cfg(feature = "native")]
fn host_api_slot() -> &'static Mutex<Option<HostApi>> {
    HOST_API.get_or_init(|| Mutex::new(None))
}

#[cfg(feature = "native")]
pub fn request_surface(canvas_ref: &str) -> Result<NativeSurfaceDesc, String> {
    let api = host_api_slot()
        .lock()
        .unwrap()
        .as_ref()
        .copied()
        .ok_or_else(|| "voplay: native host API not installed".to_string())?;
    let mut desc = NativeSurfaceDesc {
        kind: 0,
        width: 0,
        height: 0,
        scale_factor: 0.0,
        native_handle: std::ptr::null_mut(),
    };
    let ok = unsafe { (api.init_surface)(canvas_ref.as_ptr(), canvas_ref.len(), &mut desc) };
    if !ok {
        return Err(format!(
            "voplay: native host surface init failed for '{}'",
            canvas_ref
        ));
    }
    Ok(desc)
}

#[cfg(feature = "native")]
#[no_mangle]
pub extern "C" fn vo_voplay_set_host_api(api: *const HostApi) {
    assert!(!api.is_null(), "voplay: host API pointer must not be null");
    let api = unsafe { *api };
    assert!(
        api.version == HOST_API_VERSION,
        "voplay: unsupported host API version"
    );
    *host_api_slot().lock().unwrap() = Some(api);
}

#[cfg(feature = "native")]
#[no_mangle]
pub extern "C" fn vo_voplay_push_key_event(down: bool, key_ptr: *const u8, key_len: usize) {
    assert!(!key_ptr.is_null(), "voplay: key pointer must not be null");
    let key_bytes = unsafe { std::slice::from_raw_parts(key_ptr, key_len) };
    let key = std::str::from_utf8(key_bytes).expect("voplay: key must be valid UTF-8");
    crate::input::push_key_event(down, key);
}

#[cfg(feature = "native")]
#[no_mangle]
pub extern "C" fn vo_voplay_push_pointer_down(x: f64, y: f64, button: u8) {
    crate::input::push_pointer_event(crate::input::POINTER_DOWN, 0, x, y, button);
}

#[cfg(feature = "native")]
#[no_mangle]
pub extern "C" fn vo_voplay_push_pointer_up(x: f64, y: f64, button: u8) {
    crate::input::push_pointer_event(crate::input::POINTER_UP, 0, x, y, button);
}

#[cfg(feature = "native")]
#[no_mangle]
pub extern "C" fn vo_voplay_push_pointer_move(x: f64, y: f64) {
    crate::input::push_pointer_event(crate::input::POINTER_MOVE, 0, x, y, 0);
}

#[cfg(feature = "native")]
#[no_mangle]
pub extern "C" fn vo_voplay_push_scroll_event(dx: f64, dy: f64) {
    crate::input::push_scroll_event(dx, dy);
}
