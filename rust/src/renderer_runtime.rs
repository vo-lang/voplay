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
    Ready(Box<Renderer>),
    #[cfg(feature = "wasm")]
    Failed(String),
}

/// Owns one hosted renderer lifecycle. Embedders may create isolated runtime
/// instances; module-level functions below delegate to a compatibility default.
pub struct EngineRuntime {
    #[cfg(feature = "wasm")]
    state: RefCell<HostedRendererState>,
    #[cfg(not(feature = "wasm"))]
    state: Mutex<HostedRendererState>,
}

impl EngineRuntime {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "wasm")]
            state: RefCell::new(HostedRendererState {
                generation: 0,
                renderer: HostedRenderer::Empty,
            }),
            #[cfg(not(feature = "wasm"))]
            state: Mutex::new(HostedRendererState {
                generation: 0,
                renderer: HostedRenderer::Empty,
            }),
        }
    }

    #[cfg(feature = "wasm")]
    fn with_state_mut<R>(
        &self,
        operation: impl FnOnce(&mut HostedRendererState) -> R,
    ) -> Result<R, String> {
        let mut state = self.state.borrow_mut();
        Ok(operation(&mut state))
    }

    #[cfg(not(feature = "wasm"))]
    fn with_state_mut<R>(
        &self,
        operation: impl FnOnce(&mut HostedRendererState) -> R,
    ) -> Result<R, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "voplay: engine runtime renderer mutex poisoned".to_string())?;
        Ok(operation(&mut state))
    }

    #[cfg(feature = "wasm")]
    fn with_state<R>(
        &self,
        operation: impl FnOnce(&HostedRendererState) -> R,
    ) -> Result<R, String> {
        let state = self.state.borrow();
        Ok(operation(&state))
    }

    #[cfg(not(feature = "wasm"))]
    fn with_state<R>(
        &self,
        operation: impl FnOnce(&HostedRendererState) -> R,
    ) -> Result<R, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "voplay: engine runtime renderer mutex poisoned".to_string())?;
        Ok(operation(&state))
    }

    pub fn reset_renderer(&self) -> Result<u64, String> {
        self.with_state_mut(|state| {
            state.generation = state.generation.wrapping_add(1);
            state.renderer = HostedRenderer::Empty;
            state.generation
        })
    }

    #[cfg(feature = "wasm")]
    pub fn begin_renderer_init(&self, generation: u64) -> Result<bool, String> {
        self.with_state_mut(|state| {
            if state.generation != generation {
                return Ok(false);
            }
            match &state.renderer {
                HostedRenderer::Empty => {
                    state.renderer = HostedRenderer::Initializing;
                    Ok(true)
                }
                HostedRenderer::Initializing | HostedRenderer::Ready(_) => Ok(false),
                HostedRenderer::Failed(message) => Err(message.clone()),
            }
        })?
    }

    #[cfg(feature = "wasm")]
    pub fn fail_renderer_init(&self, generation: u64, message: String) -> Result<(), String> {
        self.with_state_mut(|state| {
            if state.generation == generation {
                state.renderer = HostedRenderer::Failed(message);
            }
        })
    }

    #[cfg(feature = "wasm")]
    pub fn set_renderer_for_generation(
        &self,
        generation: u64,
        renderer: Renderer,
    ) -> Result<(), String> {
        self.with_state_mut(|state| {
            if state.generation == generation {
                state.renderer = HostedRenderer::Ready(Box::new(renderer));
            }
        })
    }

    #[cfg(not(feature = "wasm"))]
    pub fn set_renderer(&self, renderer: Renderer) -> Result<(), String> {
        self.with_state_mut(|state| {
            state.renderer = HostedRenderer::Ready(Box::new(renderer));
        })
    }

    pub fn with_renderer<R>(
        &self,
        operation: impl FnOnce(&mut Renderer) -> R,
    ) -> Result<R, String> {
        self.with_state_mut(|state| match &mut state.renderer {
            HostedRenderer::Ready(renderer) => Ok(operation(renderer.as_mut())),
            #[cfg(feature = "wasm")]
            HostedRenderer::Initializing => Err("voplay: renderer is initializing".to_string()),
            #[cfg(feature = "wasm")]
            HostedRenderer::Failed(message) => Err(message.clone()),
            HostedRenderer::Empty => Err("voplay: renderer not initialized".to_string()),
        })?
    }

    pub fn renderer_ready(&self) -> Result<bool, String> {
        self.with_state(|state| match &state.renderer {
            HostedRenderer::Ready(_) => Ok(true),
            #[cfg(feature = "wasm")]
            HostedRenderer::Initializing => Ok(false),
            #[cfg(feature = "wasm")]
            HostedRenderer::Failed(message) => Err(message.clone()),
            HostedRenderer::Empty => Err("voplay: renderer not initialized".to_string()),
        })?
    }

    pub fn submit_frame(&self, data: &[u8]) -> Result<(), String> {
        self.with_renderer(|renderer| renderer.submit_frame(data))?
    }

    pub fn last_perf_packet(&self) -> Result<Vec<u8>, String> {
        self.with_renderer(|renderer| renderer.last_perf_packet().to_vec())
    }

    pub fn set_perf_stats_enabled(&self, enabled: bool) -> Result<(), String> {
        self.with_renderer(|renderer| renderer.set_perf_stats_enabled(enabled))
    }
}

impl Default for EngineRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "wasm")]
thread_local! {
    static DEFAULT_ENGINE_RUNTIME: EngineRuntime = EngineRuntime::new();
}

#[cfg(not(feature = "wasm"))]
static DEFAULT_ENGINE_RUNTIME: OnceLock<EngineRuntime> = OnceLock::new();

#[cfg(feature = "wasm")]
fn with_default_runtime<R>(operation: impl FnOnce(&EngineRuntime) -> R) -> R {
    DEFAULT_ENGINE_RUNTIME.with(operation)
}

#[cfg(not(feature = "wasm"))]
fn with_default_runtime<R>(operation: impl FnOnce(&EngineRuntime) -> R) -> R {
    operation(DEFAULT_ENGINE_RUNTIME.get_or_init(EngineRuntime::new))
}

#[cfg(feature = "wasm")]
pub fn reset_renderer() -> Result<u64, String> {
    with_default_runtime(EngineRuntime::reset_renderer)
}

#[cfg(feature = "wasm")]
pub fn begin_renderer_init(generation: u64) -> Result<bool, String> {
    with_default_runtime(|runtime| runtime.begin_renderer_init(generation))
}

#[cfg(feature = "wasm")]
pub fn fail_renderer_init(generation: u64, message: String) -> Result<(), String> {
    with_default_runtime(|runtime| runtime.fail_renderer_init(generation, message))
}

#[cfg(feature = "wasm")]
pub fn set_renderer_for_generation(generation: u64, renderer: Renderer) -> Result<(), String> {
    with_default_runtime(|runtime| runtime.set_renderer_for_generation(generation, renderer))
}

#[cfg(not(feature = "wasm"))]
pub fn set_renderer(renderer: Renderer) -> Result<(), String> {
    with_default_runtime(|runtime| runtime.set_renderer(renderer))
}

pub fn with_renderer<R>(operation: impl FnOnce(&mut Renderer) -> R) -> Result<R, String> {
    with_default_runtime(|runtime| runtime.with_renderer(operation))
}

#[cfg(not(feature = "wasm"))]
pub fn renderer_ready() -> bool {
    renderer_ready_result().unwrap_or(false)
}

pub fn renderer_ready_result() -> Result<bool, String> {
    with_default_runtime(EngineRuntime::renderer_ready)
}

pub fn submit_renderer_frame(data: &[u8]) -> Result<(), String> {
    with_default_runtime(|runtime| runtime.submit_frame(data))
}

pub fn last_renderer_perf_packet() -> Result<Vec<u8>, String> {
    with_default_runtime(EngineRuntime::last_perf_packet)
}

pub fn set_renderer_perf_stats_enabled(enabled: bool) -> Result<(), String> {
    with_default_runtime(|runtime| runtime.set_perf_stats_enabled(enabled))
}
