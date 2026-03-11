#[cfg(feature = "wasm")]
use std::cell::RefCell;
#[cfg(not(feature = "wasm"))]
use std::sync::{Mutex, OnceLock};

use crate::renderer::Renderer;

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
    static HOSTED_RENDERER: RefCell<HostedRenderer> = const { RefCell::new(HostedRenderer::Empty) };
}

#[cfg(not(feature = "wasm"))]
static HOSTED_RENDERER: OnceLock<Mutex<HostedRenderer>> = OnceLock::new();

#[cfg(not(feature = "wasm"))]
fn hosted_renderer() -> &'static Mutex<HostedRenderer> {
    HOSTED_RENDERER.get_or_init(|| Mutex::new(HostedRenderer::Empty))
}

#[cfg(feature = "wasm")]
fn with_hosted_renderer_mut<R>(f: impl FnOnce(&mut HostedRenderer) -> R) -> R {
    HOSTED_RENDERER.with(|state| {
        let mut state = state.borrow_mut();
        f(&mut state)
    })
}

#[cfg(not(feature = "wasm"))]
fn with_hosted_renderer_mut<R>(f: impl FnOnce(&mut HostedRenderer) -> R) -> R {
    let mut state = hosted_renderer().lock().unwrap();
    f(&mut state)
}

#[cfg(feature = "wasm")]
fn with_hosted_renderer_ref<R>(f: impl FnOnce(&HostedRenderer) -> R) -> R {
    HOSTED_RENDERER.with(|state| {
        let state = state.borrow();
        f(&state)
    })
}

#[cfg(not(feature = "wasm"))]
fn with_hosted_renderer_ref<R>(f: impl FnOnce(&HostedRenderer) -> R) -> R {
    let state = hosted_renderer().lock().unwrap();
    f(&state)
}

#[cfg(feature = "wasm")]
pub fn begin_renderer_init() -> Result<bool, String> {
    with_hosted_renderer_mut(|state| match state {
        HostedRenderer::Empty => {
            *state = HostedRenderer::Initializing;
            Ok(true)
        }
        HostedRenderer::Initializing | HostedRenderer::Ready(_) => Ok(false),
        HostedRenderer::Failed(msg) => Err(msg.clone()),
    })
}

#[cfg(feature = "wasm")]
pub fn fail_renderer_init(msg: String) {
    with_hosted_renderer_mut(|state| {
        *state = HostedRenderer::Failed(msg);
    });
}

pub fn set_renderer(renderer: Renderer) {
    with_hosted_renderer_mut(|state| {
        *state = HostedRenderer::Ready(renderer);
    });
}

pub fn with_renderer<R>(f: impl FnOnce(&mut Renderer) -> R) -> Result<R, String> {
    with_hosted_renderer_mut(|state| match state {
        HostedRenderer::Ready(renderer) => Ok(f(renderer)),
        #[cfg(feature = "wasm")]
        HostedRenderer::Initializing => Err("voplay: renderer is initializing".to_string()),
        #[cfg(feature = "wasm")]
        HostedRenderer::Failed(msg) => Err(msg.clone()),
        HostedRenderer::Empty => Err("voplay: renderer not initialized".to_string()),
    })
}

pub fn renderer_ready() -> bool {
    with_hosted_renderer_ref(|state| match state {
        HostedRenderer::Ready(_) => true,
        #[cfg(feature = "wasm")]
        HostedRenderer::Initializing | HostedRenderer::Failed(_) => false,
        HostedRenderer::Empty => false,
    })
}

pub fn submit_renderer_frame(data: &[u8]) -> Result<(), String> {
    with_renderer(|renderer| renderer.submit_frame(data)).and_then(|result| result)
}
