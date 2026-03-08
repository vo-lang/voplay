//! Visual test: opens a window and renders 2D shapes using voplay's Pipeline2D.
//! Run with: cargo run --example test_shapes

use std::sync::Arc;
use vo_voplay::Renderer;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

fn main() {
    let event_loop = EventLoop::new().unwrap();
    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}

#[derive(Default)]
struct App {
    state: Option<RenderState>,
}

struct RenderState {
    window: Arc<Window>,
    renderer: Renderer,
    frame: u64,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("VoPlay Shape Test")
            .with_inner_size(winit::dpi::LogicalSize::new(800, 600));
        let window = Arc::new(event_loop.create_window(attrs).unwrap());

        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).unwrap();

        let renderer = pollster::block_on(Renderer::new(&instance, surface, size.width, size.height))
            .expect("Failed to create renderer");

        self.state = Some(RenderState {
            window,
            renderer,
            frame: 0,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = match self.state.as_mut() {
            Some(s) => s,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.renderer.resize(size.width, size.height);
            }
            WindowEvent::RedrawRequested => {
                state.frame += 1;

                // Build a command stream manually (same binary format as Vo's DrawCtx)
                let cmds = build_test_commands(state.frame);
                if let Err(e) = state.renderer.submit_frame(&cmds) {
                    eprintln!("submit_frame error: {}", e);
                }
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

/// Build a binary command stream matching draw.vo's encoding.
/// Opcodes + f64 wire format (same as Vo side).
fn build_test_commands(frame: u64) -> Vec<u8> {
    let mut buf = Vec::new();

    // Clear: opcode 0x01 + 4x f64 (RGBA)
    let t = frame as f64 * 0.01;
    let bg_r = 0.1 + 0.05 * (t * 0.7).sin();
    let bg_g = 0.1 + 0.05 * (t * 1.1).sin();
    let bg_b = 0.3;
    buf.push(0x01); // opClear
    write_f64(&mut buf, bg_r);
    write_f64(&mut buf, bg_g);
    write_f64(&mut buf, bg_b);
    write_f64(&mut buf, 1.0);

    // DrawRect: opcode 0x11 + x,y,w,h,r,g,b,a (8x f64)
    buf.push(0x11);
    write_f64(&mut buf, 50.0);   // x
    write_f64(&mut buf, 50.0);   // y
    write_f64(&mut buf, 200.0);  // w
    write_f64(&mut buf, 120.0);  // h
    write_f64(&mut buf, 1.0);    // r (red)
    write_f64(&mut buf, 0.2);    // g
    write_f64(&mut buf, 0.2);    // b
    write_f64(&mut buf, 1.0);    // a

    // DrawCircle: opcode 0x12 + cx,cy,radius,r,g,b,a (7x f64)
    buf.push(0x12);
    write_f64(&mut buf, 400.0);  // cx
    write_f64(&mut buf, 200.0);  // cy
    write_f64(&mut buf, 80.0);   // radius
    write_f64(&mut buf, 0.2);    // r
    write_f64(&mut buf, 0.8);    // g (green)
    write_f64(&mut buf, 0.2);    // b
    write_f64(&mut buf, 1.0);    // a

    // DrawLine: opcode 0x13 + x1,y1,x2,y2,thickness,r,g,b,a (9x f64)
    buf.push(0x13);
    write_f64(&mut buf, 100.0);  // x1
    write_f64(&mut buf, 400.0);  // y1
    write_f64(&mut buf, 600.0);  // x2
    write_f64(&mut buf, 350.0);  // y2
    write_f64(&mut buf, 4.0);    // thickness
    write_f64(&mut buf, 1.0);    // r
    write_f64(&mut buf, 1.0);    // g (yellow)
    write_f64(&mut buf, 0.0);    // b
    write_f64(&mut buf, 1.0);    // a

    // Semi-transparent white rect
    buf.push(0x11);
    write_f64(&mut buf, 300.0);
    write_f64(&mut buf, 100.0);
    write_f64(&mut buf, 250.0);
    write_f64(&mut buf, 180.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 0.3);

    // Thick cyan line at bottom
    buf.push(0x13);
    write_f64(&mut buf, 50.0);
    write_f64(&mut buf, 500.0);
    write_f64(&mut buf, 750.0);
    write_f64(&mut buf, 500.0);
    write_f64(&mut buf, 8.0);
    write_f64(&mut buf, 0.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 1.0);

    // DrawText: opcode 0x14 + x,y,size,r,g,b,a (7x f64) + u16 len + utf8 bytes
    let text = b"Hello VoPlay!";
    buf.push(0x14);
    write_f64(&mut buf, 50.0);    // x
    write_f64(&mut buf, 550.0);   // y
    write_f64(&mut buf, 24.0);    // size
    write_f64(&mut buf, 1.0);     // r (white)
    write_f64(&mut buf, 1.0);     // g
    write_f64(&mut buf, 1.0);     // b
    write_f64(&mut buf, 1.0);     // a
    buf.extend_from_slice(&(text.len() as u16).to_le_bytes());
    buf.extend_from_slice(text);

    // Larger yellow text
    let text2 = b"Frame count shown below";
    buf.push(0x14);
    write_f64(&mut buf, 50.0);
    write_f64(&mut buf, 30.0);
    write_f64(&mut buf, 16.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 1.0);
    write_f64(&mut buf, 0.0);
    write_f64(&mut buf, 1.0);
    buf.extend_from_slice(&(text2.len() as u16).to_le_bytes());
    buf.extend_from_slice(text2);

    buf
}

fn write_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_le_bytes());
}
