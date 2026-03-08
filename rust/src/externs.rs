//! Vo extern implementations for voplay.
//! Registers: initSurface, submitFrame, pollInput, runtimeIsWeb,
//!            loadTexture, loadTextureBytes, freeTexture,
//!            physicsInit, physicsSpawnBody, physicsDestroyBody, physicsStep.

use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use std::sync::{Mutex, OnceLock};
use crate::physics::{PhysicsWorld2D, BodyDesc, PhysBodyType, ColliderKind};
use crate::renderer::Renderer;
use crate::input;

/// Global renderer instance, initialized by initSurface.
static RENDERER: OnceLock<std::sync::Mutex<Renderer>> = OnceLock::new();

#[allow(dead_code)]
fn get_renderer() -> &'static std::sync::Mutex<Renderer> {
    RENDERER.get().expect("voplay: renderer not initialized (call initSurface first)")
}

// --- initSurface ---

#[vo_fn("voplay", "initSurface")]
pub fn init_surface(call: &mut ExternCallContext) -> ExternResult {
    let _canvas_ref = call.arg_str(0).to_string();

    // Phase 0: For native, create a headless/dummy surface for testing.
    // Full surface creation requires platform-specific code (web: canvas element, native: winit).
    // For now, return an error explaining this is not yet implemented.
    // The actual implementation will be platform-specific in Phase 1.

    // TODO: Phase 1 — platform-specific surface creation
    //   Web: find <canvas> element, create wgpu surface
    //   Native: create winit window, create wgpu surface

    write_nil_error(call, 0);
    ExternResult::Ok
}

// --- submitFrame ---

#[vo_fn("voplay", "submitFrame")]
pub fn submit_frame(call: &mut ExternCallContext) -> ExternResult {
    let cmds = call.arg_bytes(0);

    match RENDERER.get() {
        Some(renderer_mutex) => {
            let mut renderer = renderer_mutex.lock().unwrap();
            match renderer.submit_frame(cmds) {
                Ok(()) => write_nil_error(call, 0),
                Err(msg) => write_error_to(call, 0, &msg),
            }
        }
        None => {
            // No renderer yet — silently succeed (Phase 0: no-op before surface init)
            write_nil_error(call, 0);
        }
    }

    ExternResult::Ok
}

// --- pollInput ---

#[vo_fn("voplay", "pollInput")]
pub fn poll_input(call: &mut ExternCallContext) -> ExternResult {
    let events = input::drain_input();
    let slice_ref = call.alloc_bytes(&events);
    call.ret_ref(0, slice_ref);
    ExternResult::Ok
}

// --- float64 bit conversion (Vo lacks math.Float64bits/Float64frombits) ---

#[vo_fn("voplay", "float64Bits")]
pub fn float64_bits(call: &mut ExternCallContext) -> ExternResult {
    let f = call.arg_f64(0);
    call.ret_u64(0, f.to_bits());
    ExternResult::Ok
}

#[vo_fn("voplay", "float64FromBits")]
pub fn float64_from_bits(call: &mut ExternCallContext) -> ExternResult {
    let bits = call.arg_u64(0);
    call.ret_f64(0, f64::from_bits(bits));
    ExternResult::Ok
}

// --- Native game loop externs (only on native feature) ---

#[vo_fn("voplay", "nativeInit")]
pub fn native_init(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let width = call.arg_u64(0) as u32;
        let height = call.arg_u64(1) as u32;
        let title = call.arg_str(2).to_string();
        match crate::native::init(width, height, &title) {
            Ok(()) => write_nil_error(call, 0),
            Err(msg) => write_error_to(call, 0, &msg),
        }
    }
    #[cfg(not(feature = "native"))]
    {
        write_error_to(call, 0, "nativeInit not available on this platform");
    }
    ExternResult::Ok
}

#[vo_fn("voplay", "nativeFrame")]
pub fn native_frame(call: &mut ExternCallContext) -> ExternResult {
    #[cfg(feature = "native")]
    {
        let cmds = call.arg_bytes(0).to_vec();
        let (dt, closed) = crate::native::frame(cmds);
        call.ret_f64(0, dt);
        call.ret_bool(1, closed);
    }
    #[cfg(not(feature = "native"))]
    {
        call.ret_f64(0, 0.0);
        call.ret_bool(1, true);
    }
    ExternResult::Ok
}

// --- loadTexture ---

#[vo_fn("voplay", "loadTexture")]
pub fn load_texture(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();

    #[cfg(feature = "native")]
    {
        match crate::native::load_texture(&path) {
            Ok(id) => {
                call.ret_u64(0, id as u64);
                write_nil_error(call, 1);
            }
            Err(msg) => {
                call.ret_u64(0, 0);
                write_error_to(call, 1, &msg);
            }
        }
    }
    #[cfg(not(feature = "native"))]
    {
        match RENDERER.get() {
            Some(renderer_mutex) => {
                let mut renderer = renderer_mutex.lock().unwrap();
                match renderer.load_texture(&path) {
                    Ok(id) => {
                        call.ret_u64(0, id as u64);
                        write_nil_error(call, 1);
                    }
                    Err(msg) => {
                        call.ret_u64(0, 0);
                        write_error_to(call, 1, &msg);
                    }
                }
            }
            None => {
                call.ret_u64(0, 0);
                write_error_to(call, 1, "voplay: renderer not initialized");
            }
        }
    }

    ExternResult::Ok
}

// --- loadTextureBytes ---

#[vo_fn("voplay", "loadTextureBytes")]
pub fn load_texture_bytes(call: &mut ExternCallContext) -> ExternResult {
    let data = call.arg_bytes(0).to_vec();

    #[cfg(feature = "native")]
    {
        match crate::native::load_texture_bytes(&data) {
            Ok(id) => {
                call.ret_u64(0, id as u64);
                write_nil_error(call, 1);
            }
            Err(msg) => {
                call.ret_u64(0, 0);
                write_error_to(call, 1, &msg);
            }
        }
    }
    #[cfg(not(feature = "native"))]
    {
        match RENDERER.get() {
            Some(renderer_mutex) => {
                let mut renderer = renderer_mutex.lock().unwrap();
                match renderer.load_texture_bytes(&data) {
                    Ok(id) => {
                        call.ret_u64(0, id as u64);
                        write_nil_error(call, 1);
                    }
                    Err(msg) => {
                        call.ret_u64(0, 0);
                        write_error_to(call, 1, &msg);
                    }
                }
            }
            None => {
                call.ret_u64(0, 0);
                write_error_to(call, 1, "voplay: renderer not initialized");
            }
        }
    }

    ExternResult::Ok
}

// --- freeTexture ---

#[vo_fn("voplay", "freeTexture")]
pub fn free_texture(call: &mut ExternCallContext) -> ExternResult {
    let id = call.arg_u64(0) as u32;

    #[cfg(feature = "native")]
    {
        crate::native::free_texture(id);
    }
    #[cfg(not(feature = "native"))]
    {
        if let Some(renderer_mutex) = RENDERER.get() {
            let mut renderer = renderer_mutex.lock().unwrap();
            renderer.free_texture(id);
        }
    }

    ExternResult::Ok
}

// --- runtimeIsWeb ---

#[vo_fn("voplay", "runtimeIsWeb")]
pub fn runtime_is_web(call: &mut ExternCallContext) -> ExternResult {
    let is_web = cfg!(target_arch = "wasm32");
    call.ret_bool(0, is_web);
    ExternResult::Ok
}

// --- Physics externs ---

static PHYSICS: OnceLock<Mutex<PhysicsWorld2D>> = OnceLock::new();

fn get_physics() -> &'static Mutex<PhysicsWorld2D> {
    PHYSICS.get().expect("voplay: physics not initialized (call physicsInit first)")
}

#[vo_fn("voplay", "physicsInit")]
pub fn physics_init(call: &mut ExternCallContext) -> ExternResult {
    let gx = call.arg_f64(0) as f32;
    let gy = call.arg_f64(1) as f32;
    let _ = PHYSICS.set(Mutex::new(PhysicsWorld2D::new(gx, gy)));
    ExternResult::Ok
}

/// Decode a BodyDesc from bytes.
/// Format: body_type(u8), collider_kind(u8), fixed_rotation(u8),
///         x(f64), y(f64), rotation(f64),
///         collider_args(3x f64), density(f64), friction(f64), restitution(f64),
///         linear_damping(f64)
fn decode_body_desc(body_id: u32, data: &[u8]) -> BodyDesc {
    let mut pos = 0;
    let body_type = match data[pos] {
        1 => PhysBodyType::Static,
        2 => PhysBodyType::Kinematic,
        _ => PhysBodyType::Dynamic,
    };
    pos += 1;
    let collider_kind = match data[pos] {
        2 => ColliderKind::Circle,
        3 => ColliderKind::Capsule,
        _ => ColliderKind::Box,
    };
    pos += 1;
    let fixed_rotation = data[pos] != 0;
    pos += 1;

    let read_f64 = |p: &mut usize| -> f64 {
        let v = f64::from_le_bytes([
            data[*p], data[*p+1], data[*p+2], data[*p+3],
            data[*p+4], data[*p+5], data[*p+6], data[*p+7],
        ]);
        *p += 8;
        v
    };

    let x = read_f64(&mut pos) as f32;
    let y = read_f64(&mut pos) as f32;
    let rotation = read_f64(&mut pos) as f32;
    let ca0 = read_f64(&mut pos) as f32;
    let ca1 = read_f64(&mut pos) as f32;
    let ca2 = read_f64(&mut pos) as f32;
    let density = read_f64(&mut pos) as f32;
    let friction = read_f64(&mut pos) as f32;
    let restitution = read_f64(&mut pos) as f32;
    let linear_damping = read_f64(&mut pos) as f32;

    BodyDesc {
        body_id,
        body_type,
        x, y, rotation,
        collider_kind,
        collider_args: [ca0, ca1, ca2],
        density,
        friction,
        restitution,
        linear_damping,
        fixed_rotation,
    }
}

#[vo_fn("voplay", "physicsSpawnBody")]
pub fn physics_spawn_body(call: &mut ExternCallContext) -> ExternResult {
    let body_id = call.arg_u64(0) as u32;
    let data = call.arg_bytes(1);
    let desc = decode_body_desc(body_id, data);
    get_physics().lock().unwrap().spawn_body(&desc);
    ExternResult::Ok
}

#[vo_fn("voplay", "physicsDestroyBody")]
pub fn physics_destroy_body(call: &mut ExternCallContext) -> ExternResult {
    let body_id = call.arg_u64(0) as u32;
    get_physics().lock().unwrap().destroy_body(body_id);
    ExternResult::Ok
}

#[vo_fn("voplay", "physicsStep")]
pub fn physics_step(call: &mut ExternCallContext) -> ExternResult {
    let dt = call.arg_f64(0) as f32;
    let cmds = call.arg_bytes(1);
    let cmds_owned = cmds.to_vec();

    let mut world = get_physics().lock().unwrap();
    world.apply_commands(&cmds_owned);
    world.step(dt);
    let state = world.serialize_state();

    let slice_ref = call.alloc_bytes(&state);
    call.ret_ref(0, slice_ref);
    ExternResult::Ok
}
