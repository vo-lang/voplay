//! Native window + game loop support for voplay.
//!
//! Architecture: Vo drives the game loop via a tight `for {}`.
//! Each frame, Vo calls `nativeFrame(cmds)` which:
//!   1. Pumps winit events non-blocking (input, resize, close)
//!   2. Applies pending resize to renderer
//!   3. Submits draw commands directly to GPU (NOT via RedrawRequested —
//!      that event has platform-inconsistent timing on Windows)
//!   4. Returns (dt, closed) to Vo
//!
//! Uses winit's `pump_app_events` API (macOS, Windows, Linux, Android).

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use winit::window::{Window, WindowId};

use crate::input;
use crate::renderer::Renderer;

// ---------------------------------------------------------------------------
// Thread-local state (split into two to avoid borrow conflicts in pump)
// ---------------------------------------------------------------------------

thread_local! {
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
    static APP: RefCell<NativeApp> = const { RefCell::new(NativeApp::new()) };
}

/// Application handler for winit events.
/// Holds the window, renderer, and per-frame state.
struct NativeApp {
    // Set during init, before first pump
    init_request: Option<InitRequest>,
    // Set after window creation (inside `resumed`)
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    // Cursor position cache (MouseInput events don't carry position)
    cursor_x: f64,
    cursor_y: f64,
    // Frame state
    last_frame_time: Option<Instant>,
    should_close: bool,
    frame_dt: f64,
    // Resize tracking
    new_size: Option<(u32, u32)>,
    // Init error
    init_error: Option<String>,
}

struct InitRequest {
    width: u32,
    height: u32,
    title: String,
}

impl NativeApp {
    const fn new() -> Self {
        Self {
            init_request: None,
            window: None,
            renderer: None,
            cursor_x: 0.0,
            cursor_y: 0.0,
            last_frame_time: None,
            should_close: false,
            frame_dt: 0.0,
            new_size: None,
            init_error: None,
        }
    }

    fn is_initialized(&self) -> bool {
        self.renderer.is_some()
    }
}

impl ApplicationHandler for NativeApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // Already created
        }

        let req = match self.init_request.take() {
            Some(r) => r,
            None => return,
        };

        let attrs = Window::default_attributes()
            .with_title(&req.title)
            .with_inner_size(winit::dpi::LogicalSize::new(req.width, req.height));

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_error = Some(format!("create_window: {}", e));
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = match instance.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                self.init_error = Some(format!("create_surface: {}", e));
                event_loop.exit();
                return;
            }
        };

        let renderer = match pollster::block_on(Renderer::new(
            &instance,
            surface,
            size.width,
            size.height,
        )) {
            Ok(r) => r,
            Err(e) => {
                self.init_error = Some(e);
                event_loop.exit();
                return;
            }
        };

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.last_frame_time = Some(Instant::now());
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.should_close = true;
                event_loop.exit();
            }

            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    self.new_size = Some((size.width, size.height));
                }
            }

            WindowEvent::RedrawRequested => {
                // System-initiated redraw (expose, restore, etc).
                // Handle resize; actual game frame rendering is done in frame().
                if let Some((w, h)) = self.new_size.take() {
                    if let Some(renderer) = self.renderer.as_mut() {
                        renderer.resize(w, h);
                    }
                }
            }

            // Keyboard input
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                let key_name = match event.logical_key {
                    Key::Named(named) => named_key_string(named),
                    Key::Character(ref s) => s.to_string(),
                    _ => return,
                };
                if !key_name.is_empty() {
                    input::push_key_event(down, &key_name);
                }
            }

            // Pointer input
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_x = position.x;
                self.cursor_y = position.y;
                input::push_pointer_event(input::POINTER_MOVE, position.x, position.y, 0);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let kind = if state == ElementState::Pressed {
                    input::POINTER_DOWN
                } else {
                    input::POINTER_UP
                };
                let btn = match button {
                    winit::event::MouseButton::Left => 0,
                    winit::event::MouseButton::Right => 1,
                    winit::event::MouseButton::Middle => 2,
                    _ => 0,
                };
                input::push_pointer_event(kind, self.cursor_x, self.cursor_y, btn);
            }

            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Public API called from externs
// ---------------------------------------------------------------------------

/// Initialize the native window + renderer.
/// Creates the EventLoop and pumps once to trigger `resumed` → window + GPU init.
pub fn init(width: u32, height: u32, title: &str) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("EventLoop::new: {}", e))?;

    APP.with(|app| {
        let mut app = app.borrow_mut();
        app.init_request = Some(InitRequest {
            width,
            height,
            title: title.to_string(),
        });
    });

    // Pump once to trigger `resumed` callback which creates window + renderer.
    // Use None timeout so it blocks until the window is ready.
    EVENT_LOOP.with(|el| {
        *el.borrow_mut() = Some(event_loop);
    });

    EVENT_LOOP.with(|el| {
        let mut el_opt = el.borrow_mut();
        let event_loop = el_opt.as_mut().unwrap();
        APP.with(|app| {
            let mut app = app.borrow_mut();
            event_loop.pump_app_events(None, &mut *app);
        });
    });

    // Check for init errors
    APP.with(|app| {
        let mut app = app.borrow_mut();
        if let Some(err) = app.init_error.take() {
            return Err(err);
        }
        if !app.is_initialized() {
            return Err("native init: renderer not created".to_string());
        }
        Ok(())
    })
}

/// Pump OS events + submit draw commands + return dt.
/// Returns (dt_seconds, should_close).
///
/// Frame model:
///   1. pump_app_events(ZERO) — processes input, resize, close (non-blocking)
///   2. Apply pending resize to renderer
///   3. Submit draw commands directly (NOT via RedrawRequested — that
///      event has platform-inconsistent timing on Windows)
///   4. Compute dt from wall clock
///   5. Return (dt, closed) to Vo
pub fn frame(cmds: Vec<u8>) -> (f64, bool) {
    // 1. Pump OS events (input, resize, close).
    //    We do NOT rely on RedrawRequested for game rendering — that event
    //    is platform-inconsistent (Windows doesn't guarantee it fires on
    //    request_redraw inside the handler). Instead we render directly below.
    let mut exited = false;
    EVENT_LOOP.with(|el| {
        let mut el_opt = el.borrow_mut();
        if let Some(event_loop) = el_opt.as_mut() {
            APP.with(|app| {
                let mut app = app.borrow_mut();
                let status = event_loop.pump_app_events(Some(Duration::ZERO), &mut *app);
                if let PumpStatus::Exit(_) = status {
                    exited = true;
                }
            });
        }
    });

    // 2. Handle pending resize + submit draw commands directly.
    //    This runs outside pump_app_events, which is fine on Windows/Linux.
    //    On macOS this may cause minor resize artifacts (acceptable trade-off
    //    vs not rendering at all on Windows).
    APP.with(|app| {
        let mut app = app.borrow_mut();

        // Apply pending resize
        if let Some((w, h)) = app.new_size.take() {
            if let Some(renderer) = app.renderer.as_mut() {
                renderer.resize(w, h);
            }
        }

        // Submit draw commands
        if !cmds.is_empty() {
            if let Some(renderer) = app.renderer.as_mut() {
                if let Err(e) = renderer.submit_frame(&cmds) {
                    log::error!("voplay: submit_frame error: {}", e);
                }
            }
        }

        // Compute dt
        let now = Instant::now();
        if let Some(last) = app.last_frame_time {
            app.frame_dt = now.duration_since(last).as_secs_f64();
        }
        app.last_frame_time = Some(now);

        let dt = app.frame_dt;
        let closed = app.should_close || exited;
        (dt, closed)
    })
}

/// Get current window size.
pub fn window_size() -> (u32, u32) {
    APP.with(|app| {
        let app = app.borrow();
        match app.window.as_ref() {
            Some(w) => {
                let size = w.inner_size();
                (size.width, size.height)
            }
            None => (0, 0),
        }
    })
}

// ---------------------------------------------------------------------------
// Texture management (delegated to renderer)
// ---------------------------------------------------------------------------

/// Load a texture from a file path via the native renderer.
pub fn load_texture(path: &str) -> Result<u32, String> {
    APP.with(|app| {
        let mut app = app.borrow_mut();
        match app.renderer.as_mut() {
            Some(r) => r.load_texture(path),
            None => Err("voplay: renderer not initialized".to_string()),
        }
    })
}

/// Load a texture from encoded image bytes via the native renderer.
pub fn load_texture_bytes(data: &[u8]) -> Result<u32, String> {
    APP.with(|app| {
        let mut app = app.borrow_mut();
        match app.renderer.as_mut() {
            Some(r) => r.load_texture_bytes(data),
            None => Err("voplay: renderer not initialized".to_string()),
        }
    })
}

/// Free a texture by ID via the native renderer.
pub fn free_texture(id: u32) {
    APP.with(|app| {
        let mut app = app.borrow_mut();
        if let Some(r) = app.renderer.as_mut() {
            r.free_texture(id);
        }
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn named_key_string(key: NamedKey) -> String {
    match key {
        NamedKey::ArrowUp => "ArrowUp".to_string(),
        NamedKey::ArrowDown => "ArrowDown".to_string(),
        NamedKey::ArrowLeft => "ArrowLeft".to_string(),
        NamedKey::ArrowRight => "ArrowRight".to_string(),
        NamedKey::Enter => "Enter".to_string(),
        NamedKey::Space => "Space".to_string(),
        NamedKey::Escape => "Escape".to_string(),
        NamedKey::Backspace => "Backspace".to_string(),
        NamedKey::Tab => "Tab".to_string(),
        NamedKey::Shift => "Shift".to_string(),
        NamedKey::Control => "Control".to_string(),
        NamedKey::Alt => "Alt".to_string(),
        NamedKey::Meta => "Meta".to_string(),
        NamedKey::Delete => "Delete".to_string(),
        NamedKey::Home => "Home".to_string(),
        NamedKey::End => "End".to_string(),
        NamedKey::PageUp => "PageUp".to_string(),
        NamedKey::PageDown => "PageDown".to_string(),
        NamedKey::F1 => "F1".to_string(),
        NamedKey::F2 => "F2".to_string(),
        NamedKey::F3 => "F3".to_string(),
        NamedKey::F4 => "F4".to_string(),
        NamedKey::F5 => "F5".to_string(),
        NamedKey::F6 => "F6".to_string(),
        NamedKey::F7 => "F7".to_string(),
        NamedKey::F8 => "F8".to_string(),
        NamedKey::F9 => "F9".to_string(),
        NamedKey::F10 => "F10".to_string(),
        NamedKey::F11 => "F11".to_string(),
        NamedKey::F12 => "F12".to_string(),
        _ => String::new(),
    }
}
