#[cfg(feature = "wasm")]
use std::cell::RefCell;
#[cfg(not(feature = "wasm"))]
use std::sync::{Mutex, OnceLock};

use crate::renderer::Renderer;

struct HostedRendererState {
    generation: u64,
    renderer: HostedRenderer,
}

enum HostedRenderer {
    Empty,
    #[cfg(feature = "wasm")]
    Initializing,
    Ready(Renderer),
    #[cfg(feature = "wasm")]
    Failed(String),
}

#[cfg(feature = "wasm")]
thread_local! {
    static HOSTED_RENDERER: RefCell<HostedRendererState> = const {
        RefCell::new(HostedRendererState {
            generation: 0,
            renderer: HostedRenderer::Empty,
        })
    };
}

#[cfg(not(feature = "wasm"))]
static HOSTED_RENDERER: OnceLock<Mutex<HostedRendererState>> = OnceLock::new();

#[cfg(not(feature = "wasm"))]
fn hosted_renderer() -> &'static Mutex<HostedRendererState> {
    HOSTED_RENDERER.get_or_init(|| {
        Mutex::new(HostedRendererState {
            generation: 0,
            renderer: HostedRenderer::Empty,
        })
    })
}

#[cfg(feature = "wasm")]
fn with_hosted_renderer_mut<R>(f: impl FnOnce(&mut HostedRendererState) -> R) -> R {
    HOSTED_RENDERER.with(|state| {
        let mut state = state.borrow_mut();
        f(&mut state)
    })
}

#[cfg(not(feature = "wasm"))]
fn with_hosted_renderer_mut<R>(f: impl FnOnce(&mut HostedRendererState) -> R) -> R {
    let mut state = hosted_renderer().lock().unwrap();
    f(&mut state)
}

#[cfg(feature = "wasm")]
fn with_hosted_renderer_ref<R>(f: impl FnOnce(&HostedRendererState) -> R) -> R {
    HOSTED_RENDERER.with(|state| {
        let state = state.borrow();
        f(&state)
    })
}

#[cfg(not(feature = "wasm"))]
fn with_hosted_renderer_ref<R>(f: impl FnOnce(&HostedRendererState) -> R) -> R {
    let state = hosted_renderer().lock().unwrap();
    f(&state)
}

pub fn reset_renderer() -> u64 {
    with_hosted_renderer_mut(|state| {
        state.generation = state.generation.wrapping_add(1);
        state.renderer = HostedRenderer::Empty;
        state.generation
    })
}

#[cfg(feature = "wasm")]
pub fn begin_renderer_init(generation: u64) -> Result<bool, String> {
    with_hosted_renderer_mut(|state| {
        if state.generation != generation {
            return Ok(false);
        }
        match &state.renderer {
            HostedRenderer::Empty => {
                state.renderer = HostedRenderer::Initializing;
                Ok(true)
            }
            HostedRenderer::Initializing | HostedRenderer::Ready(_) => Ok(false),
            HostedRenderer::Failed(msg) => Err(msg.clone()),
        }
    })
}

#[cfg(feature = "wasm")]
pub fn fail_renderer_init(generation: u64, msg: String) {
    with_hosted_renderer_mut(|state| {
        if state.generation == generation {
            state.renderer = HostedRenderer::Failed(msg);
        }
    });
}

#[cfg(feature = "wasm")]
pub fn set_renderer_for_generation(generation: u64, renderer: Renderer) {
    with_hosted_renderer_mut(|state| {
        if state.generation == generation {
            state.renderer = HostedRenderer::Ready(renderer);
        }
    });
}

#[cfg(not(feature = "wasm"))]
pub fn set_renderer(renderer: Renderer) {
    with_hosted_renderer_mut(|state| {
        state.renderer = HostedRenderer::Ready(renderer);
    });
}

pub fn with_renderer<R>(f: impl FnOnce(&mut Renderer) -> R) -> Result<R, String> {
    with_hosted_renderer_mut(|state| match &mut state.renderer {
        HostedRenderer::Ready(renderer) => Ok(f(renderer)),
        #[cfg(feature = "wasm")]
        HostedRenderer::Initializing => Err("voplay: renderer is initializing".to_string()),
        #[cfg(feature = "wasm")]
        HostedRenderer::Failed(msg) => Err(msg.clone()),
        HostedRenderer::Empty => Err("voplay: renderer not initialized".to_string()),
    })
}

pub fn renderer_ready() -> bool {
    renderer_ready_result().unwrap_or(false)
}

pub fn renderer_ready_result() -> Result<bool, String> {
    with_hosted_renderer_ref(|state| match &state.renderer {
        HostedRenderer::Ready(_) => Ok(true),
        #[cfg(feature = "wasm")]
        HostedRenderer::Initializing => Ok(false),
        #[cfg(feature = "wasm")]
        HostedRenderer::Failed(msg) => Err(msg.clone()),
        HostedRenderer::Empty => Err("voplay: renderer not initialized".to_string()),
    })
}

pub fn submit_renderer_frame(data: &[u8]) -> Result<(), String> {
    with_renderer(|renderer| renderer.submit_frame(data)).and_then(|result| result)
}
