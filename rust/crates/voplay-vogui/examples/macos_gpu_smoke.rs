#[cfg(all(target_os = "macos", feature = "macos-gpu-host"))]
fn main() {
    use std::collections::BTreeSet;
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread;
    use std::time::Duration;

    use vo_app_host_native::{MacOsGpuWindow, MacOsGpuWindowConfig};
    use vo_app_protocol::{SurfaceHandle, ViewHandle, WindowHandle};
    use vo_app_runtime::{
        NativeCompositionFrame, NativeCompositionOutcome, NativeLayerSubmission, SurfaceGeometry,
        SurfaceInputPolicy, SurfaceKind,
    };
    use vogui_runtime::{
        accessibility::PlatformAccessibilityAdapter,
        semantics::{
            NodeId, SemanticAction, SemanticBounds, SemanticNode, SemanticRole, SemanticSnapshot,
            SemanticStates, UiRootId,
        },
    };
    use voplay_vogui::{MacOsGpuTopology, MacOsGpuTopologyConfig};

    struct ThreadWake(thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => thread::park_timeout(Duration::from_millis(10)),
            }
        }
    }

    let window_handle = WindowHandle {
        index: 0,
        generation: 1,
    };
    let view_handle = ViewHandle {
        index: 0,
        generation: 1,
    };
    let surface_handle = SurfaceHandle {
        index: 0,
        generation: 1,
    };
    let mut window = MacOsGpuWindow::new(
        window_handle,
        view_handle,
        MacOsGpuWindowConfig {
            title: String::from("Volang native GPU smoke"),
            width_points: 640.0,
            height_points: 360.0,
            ..MacOsGpuWindowConfig::default()
        },
    )
    .expect("create AppKit window");
    window.show();
    window
        .set_ime_caret_rect(12.0, 18.0, 2.0, 20.0, 0, 0)
        .expect("set native IME caret");
    let metrics = window.metrics();
    assert!(metrics.visible);
    assert!(metrics.width_points > 0.0 && metrics.height_points > 0.0);

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..Default::default()
    });
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("request Metal adapter");
    let (device, queue) = block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("volang-native-smoke"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
        },
        None,
    ))
    .expect("request Metal device");
    let adapter = Arc::new(adapter);
    let device = Arc::new(device);
    let queue = Arc::new(queue);
    let mut topology = MacOsGpuTopology::attach(
        &window,
        &instance,
        adapter,
        Arc::clone(&device),
        Arc::clone(&queue),
        1,
        MacOsGpuTopologyConfig::default(),
    )
    .expect("attach production GPU topology");

    let semantic_root = UiRootId {
        index: 10,
        generation: 1,
    };
    let semantic_button = NodeId {
        index: 20,
        generation: 1,
    };
    topology
        .accessibility_mut()
        .apply_snapshot(&SemanticSnapshot {
            root: semantic_root,
            tree_revision: 1,
            semantic_revision: 1,
            root_node: semantic_button,
            nodes: vec![SemanticNode {
                node: semantic_button,
                role: SemanticRole::Button,
                label: String::from("Run native smoke"),
                description: String::from("Vogui AppKit accessibility smoke action"),
                value: String::new(),
                locale: String::from("en-US"),
                bounds: SemanticBounds {
                    x_milli: 12_000,
                    y_milli: 18_000,
                    width_milli: 160_000,
                    height_milli: 36_000,
                },
                states: SemanticStates::default(),
                actions: BTreeSet::from([SemanticAction::Press]),
                relations: BTreeSet::new(),
                children: Vec::new(),
                focus_order: Some(0),
                live: false,
            }],
        })
        .expect("commit AppKit accessibility tree");
    let committed_accessibility = topology
        .accessibility()
        .committed()
        .expect("committed accessibility tree");
    assert_eq!(committed_accessibility.semantic_revision, 1);
    assert_eq!(committed_accessibility.nodes.len(), 1);
    assert!(topology.accessibility().platform().sink().dispatch_action(
        semantic_button,
        SemanticAction::Press,
        Vec::new()
    ));
    let accessibility_action = topology
        .accessibility_actions()
        .try_recv()
        .expect("receive AppKit accessibility action")
        .expect("AppKit action is queued");
    assert_eq!(accessibility_action.root, semantic_root);
    assert_eq!(accessibility_action.semantic_revision, 1);
    assert_eq!(accessibility_action.node, semantic_button);
    assert_eq!(accessibility_action.action, SemanticAction::Press);

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("volang-native-smoke-layer"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let pixels = vec![0x30_u8, 0x90, 0xe0, 0xff].repeat(64 * 64);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(64 * 4),
            rows_per_image: Some(64),
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    topology
        .compositor_mut()
        .adapter_mut()
        .expect("open compositor")
        .register_layer_texture(surface_handle, 1, 1, texture)
        .expect("register smoke texture");
    let mut outcomes = Vec::with_capacity(3);
    for pulse_id in 1..=3 {
        let outcome = topology
            .submit_composition(
                NativeCompositionFrame {
                    view: view_handle,
                    pulse_id,
                    device_generation: 1,
                    viewport_width_milli: (metrics.width_points * 1_000.0).round() as u32,
                    viewport_height_milli: (metrics.height_points * 1_000.0).round() as u32,
                    layers: vec![NativeLayerSubmission {
                        surface: surface_handle,
                        kind: SurfaceKind::Game,
                        z_order: 0,
                        input: SurfaceInputPolicy::Interactive,
                        content_revision: pulse_id,
                        texture_token: 1,
                        device_generation: 1,
                        geometry: SurfaceGeometry::default(),
                    }],
                },
                pulse_id,
                u64::MAX,
            )
            .expect("submit and present native composition");
        assert_eq!(outcome, NativeCompositionOutcome::Presented);
        outcomes.push(outcome);
    }
    let snapshot = topology.owner_snapshot();
    assert_eq!(snapshot.view, view_handle);
    assert_eq!(snapshot.device_generation, 1);
    topology.close().expect("close GPU topology");
    window.close();

    println!(
        "{{\"passed\":true,\"window_visible\":{},\"width_points\":{},\"height_points\":{},\"scale_factor\":{},\"presented_frames\":{},\"accessibility_revision\":{}}}",
        metrics.visible,
        metrics.width_points,
        metrics.height_points,
        metrics.scale_factor,
        outcomes.len(),
        accessibility_action.semantic_revision
    );
}

#[cfg(not(all(target_os = "macos", feature = "macos-gpu-host")))]
fn main() {
    panic!("macos_gpu_smoke requires macOS and the macos-gpu-host feature");
}
