use std::collections::BTreeMap;

use crate::buffer_lease::BufferLeaseMetrics;
use crate::device_hub::{DeviceHub, DeviceHubMetrics};
use crate::endpoint::EndpointQueueMetrics;
use crate::game_engine::Game;
use crate::host_render_backend::HostRenderTransportMetrics;
use crate::presentation::FrameCommandEncoder;
use crate::runtime::{VoplayRuntime, VoplayRuntimeOwnerSnapshot};
use crate::supervisor::Role;
use crate::surface::RenderBackend;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EndpointQueueAggregateMetrics {
    pub queues: usize,
    pub packets: usize,
    pub aggregate_peak_packets: usize,
    pub bytes: usize,
    pub aggregate_peak_bytes: usize,
    pub capacity_rejections: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VoplayInstrumentationSnapshot {
    pub owner: VoplayRuntimeOwnerSnapshot,
    pub device_hub: DeviceHubMetrics,
    pub endpoint_queues: BTreeMap<Role, EndpointQueueMetrics>,
    pub endpoint_total: EndpointQueueAggregateMetrics,
    pub host_render: HostRenderTransportMetrics,
    pub buffer_leases: BufferLeaseMetrics,
    pub asset: Option<crate::asset::AssetServerOwnerSnapshot>,
    pub asset_endpoint_driver: Option<crate::asset_endpoint::AssetEndpointDriverOwnerSnapshot>,
    pub audio: Option<crate::audio_mixer::AudioMixerOwnerSnapshot>,
    pub audio_pipeline: Option<crate::audio_pipeline::AudioPipelineOwnerSnapshot>,
    #[cfg(feature = "audio")]
    pub audio_endpoint_driver: Option<crate::audio_endpoint::AudioEndpointDriverOwnerSnapshot>,
    #[cfg(feature = "render")]
    pub render_endpoint_driver: Option<crate::render_endpoint::RenderEndpointDriverOwnerSnapshot>,
    pub logic_endpoint_driver: Option<crate::logic_endpoint::LogicEndpointDriverOwnerSnapshot>,
    pub hosted_logic_endpoint_driver:
        Option<crate::hosted_logic_endpoint::HostedLogicEndpointOwnerSnapshot>,
    pub host_render_backend: Option<crate::host_render_backend::HostRenderBackendOwnerSnapshot>,
    #[cfg(feature = "render")]
    pub platform_render_backend:
        Option<crate::platform_backend::PlatformRenderBackendOwnerSnapshot>,
    pub headless_render_backend:
        Option<crate::headless_renderer::HeadlessRenderBackendOwnerSnapshot>,
    #[cfg(feature = "app-session")]
    pub hosted_runtime: Option<crate::app_session_adapter::HostedVoplayOwnerSnapshot>,
    #[cfg(feature = "pack")]
    pub world_partition: Option<crate::world_partition::WorldPartitionOwnerSnapshot>,
    #[cfg(feature = "pack")]
    pub partition_io: Option<crate::partition_io::PartitionIoOwnerSnapshot>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VoplayLeakSummary {
    pub roles: usize,
    pub entities: usize,
    pub surfaces: usize,
    pub input_scopes: usize,
    pub pending_endpoint_events: usize,
    pub pending_logic_io_commands: usize,
    pub hosted_logic_pending_control_forwards: usize,
    pub hosted_logic_control_entries: usize,
    pub hosted_logic_output_packets: usize,
    pub hosted_logic_output_bytes: usize,
    pub host_pending_web_controls: usize,
    pub host_pending_web_frames: usize,
    pub host_haptics_requests: usize,
    pub host_haptics_commands: usize,
    pub host_audio_device_leases: usize,
    pub backend_surfaces: usize,
    pub backend_pending_frames: usize,
    pub backend_readbacks: usize,
    pub backend_targets: usize,
    pub backend_textures: usize,
    pub backend_texture_bytes: usize,
    pub pending_render_ops: usize,
    pub acknowledged_render_objects: usize,
    pub presentation_domains: usize,
    pub presentation_entries: usize,
    pub presentation_bytes: usize,
    pub simulation_resources: usize,
    pub simulation_resource_bytes: usize,
    pub simulation_events: usize,
    pub simulation_event_bytes: usize,
    pub encoder_objects: usize,
    pub encoder_assets: usize,
    pub encoder_bytes: usize,
    pub endpoint_packets: usize,
    pub buffer_leases: usize,
    pub device_bindings: usize,
    pub pending_readbacks: usize,
    pub ready_readbacks: usize,
    pub pending_frame_traces: usize,
    pub ready_frame_traces: usize,
    pub assets: usize,
    pub asset_scopes: usize,
    pub asset_tickets: usize,
    pub asset_leases: usize,
    pub audio_buses: usize,
    pub audio_sources: usize,
    pub audio_voices: usize,
    pub audio_streams: usize,
    pub audio_pending_decodes: usize,
    pub audio_ring_samples: usize,
    pub audio_queued_events: usize,
    pub audio_queued_event_bytes: usize,
    pub audio_resident_assets: usize,
    pub audio_resident_asset_bytes: usize,
    pub audio_one_shots: usize,
    pub audio_control_entries: usize,
    pub audio_retiring_control_entries: usize,
    pub audio_recovery_stops: usize,
    pub render_objects: usize,
    pub render_pending_changes: usize,
    pub render_staged_events: usize,
    pub render_completed_events: usize,
    pub render_control_entries: usize,
    pub render_retiring_targets: usize,
    pub render_transient_domains: usize,
    pub render_transient_bytes: usize,
    pub render_texture_assets: usize,
    pub render_texture_bytes: usize,
    pub render_profile_assets: usize,
    pub render_profile_asset_bytes: usize,
    pub partitions: usize,
    pub partition_asset_tickets: usize,
    pub partition_io_requests: usize,
    pub partition_retired_io_requests: usize,
    pub partition_installed_chunks: usize,
}

impl VoplayLeakSummary {
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

impl VoplayInstrumentationSnapshot {
    pub fn capture<G, B, E>(
        runtime: &VoplayRuntime<G, B, E>,
        device_hub: &DeviceHub,
        endpoint_queues: impl IntoIterator<Item = (Role, EndpointQueueMetrics)>,
        host_render: HostRenderTransportMetrics,
        buffer_leases: BufferLeaseMetrics,
    ) -> Self
    where
        G: Game,
        B: RenderBackend,
        E: FrameCommandEncoder,
    {
        let endpoint_queues = endpoint_queues.into_iter().collect::<BTreeMap<_, _>>();
        let mut endpoint_total = EndpointQueueAggregateMetrics::default();
        for metrics in endpoint_queues.values() {
            endpoint_total.queues = endpoint_total.queues.saturating_add(1);
            endpoint_total.packets = endpoint_total.packets.saturating_add(metrics.packets);
            endpoint_total.aggregate_peak_packets = endpoint_total
                .aggregate_peak_packets
                .saturating_add(metrics.peak_packets);
            endpoint_total.bytes = endpoint_total.bytes.saturating_add(metrics.bytes);
            endpoint_total.aggregate_peak_bytes = endpoint_total
                .aggregate_peak_bytes
                .saturating_add(metrics.peak_bytes);
            endpoint_total.capacity_rejections = endpoint_total
                .capacity_rejections
                .saturating_add(metrics.capacity_rejections);
        }
        Self {
            owner: runtime.owner_snapshot(),
            device_hub: device_hub.metrics(),
            endpoint_queues,
            endpoint_total,
            host_render,
            buffer_leases,
            asset: None,
            asset_endpoint_driver: None,
            audio: None,
            audio_pipeline: None,
            #[cfg(feature = "audio")]
            audio_endpoint_driver: None,
            #[cfg(feature = "render")]
            render_endpoint_driver: None,
            logic_endpoint_driver: None,
            hosted_logic_endpoint_driver: None,
            host_render_backend: None,
            #[cfg(feature = "render")]
            platform_render_backend: None,
            headless_render_backend: None,
            #[cfg(feature = "app-session")]
            hosted_runtime: None,
            #[cfg(feature = "pack")]
            world_partition: None,
            #[cfg(feature = "pack")]
            partition_io: None,
        }
    }

    pub fn with_asset(mut self, asset: &crate::asset::AssetServer) -> Self {
        self.asset = Some(asset.owner_snapshot());
        self
    }

    pub fn with_asset_endpoint_driver(
        mut self,
        driver: &crate::asset_endpoint::AssetEndpointDriver,
    ) -> Self {
        self.asset_endpoint_driver = Some(driver.owner_snapshot());
        self
    }

    pub fn with_audio(mut self, audio: &crate::audio_mixer::AudioMixerEndpoint) -> Self {
        self.audio = Some(audio.owner_snapshot());
        self
    }

    pub fn with_audio_pipeline(mut self, pipeline: &crate::audio_pipeline::AudioPipeline) -> Self {
        self.audio_pipeline = Some(pipeline.owner_snapshot());
        self
    }

    #[cfg(feature = "audio")]
    pub fn with_audio_endpoint_driver(
        mut self,
        driver: &crate::audio_endpoint::AudioEndpointDriver,
    ) -> Self {
        self.audio_endpoint_driver = Some(driver.owner_snapshot());
        self
    }

    #[cfg(feature = "render")]
    pub fn with_render_endpoint_driver(
        mut self,
        driver: &crate::render_endpoint::RenderEndpointDriver,
    ) -> Self {
        self.render_endpoint_driver = Some(driver.owner_snapshot());
        self
    }

    pub fn with_logic_endpoint_driver<G: crate::game_engine::Game>(
        mut self,
        driver: &crate::logic_endpoint::LogicEndpointDriver<G>,
    ) -> Self {
        self.logic_endpoint_driver = Some(driver.owner_snapshot());
        self
    }

    pub fn with_hosted_logic_endpoint_driver(
        mut self,
        driver: &crate::hosted_logic_endpoint::HostedLogicEndpointDriver,
    ) -> Self {
        self.hosted_logic_endpoint_driver = Some(driver.owner_snapshot());
        self
    }

    pub fn with_host_render_backend(
        mut self,
        backend: &crate::host_render_backend::HostRenderBackend,
    ) -> Self {
        self.host_render_backend = Some(backend.owner_snapshot());
        self
    }

    #[cfg(feature = "render")]
    pub fn with_platform_render_backend<A: crate::platform_backend::PlatformSurfaceAdapter>(
        mut self,
        backend: &crate::platform_backend::PlatformRenderBackend<A>,
    ) -> Self {
        self.platform_render_backend = Some(backend.owner_snapshot());
        self
    }

    pub fn with_headless_render_backend(
        mut self,
        backend: &crate::headless_renderer::HeadlessRenderBackend,
    ) -> Self {
        self.headless_render_backend = Some(backend.owner_snapshot());
        self
    }

    #[cfg(feature = "app-session")]
    pub fn with_hosted_runtime<G, B, E>(
        mut self,
        runtime: &crate::app_session_adapter::HostedVoplayRuntime<G, B, E>,
    ) -> Self
    where
        G: crate::game_engine::Game,
        B: crate::surface::RenderBackend,
        E: crate::presentation::FrameCommandEncoder,
    {
        self.hosted_runtime = Some(runtime.owner_snapshot());
        self
    }

    #[cfg(feature = "pack")]
    pub fn with_world_partition(
        mut self,
        partition: &crate::world_partition::WorldPartition,
    ) -> Self {
        self.world_partition = Some(partition.owner_snapshot());
        self
    }

    #[cfg(feature = "pack")]
    pub fn with_partition_io(mut self, io: &crate::partition_io::PartitionIoScheduler) -> Self {
        self.partition_io = Some(io.owner_snapshot());
        self
    }

    pub fn leak_summary(&self) -> VoplayLeakSummary {
        #[cfg(feature = "render")]
        let platform_backend = self.platform_render_backend.map_or((0, 0, 0), |backend| {
            (backend.surfaces, backend.pending_frames, backend.readbacks)
        });
        #[cfg(not(feature = "render"))]
        let platform_backend = (0, 0, 0);

        #[cfg(feature = "render")]
        let render_endpoint = self.render_endpoint_driver.map_or(
            (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
            |driver| {
                (
                    driver.encoder.retained_objects,
                    driver.encoder.retained_assets,
                    driver.encoder.retained_bytes,
                    driver.engine.render_objects,
                    driver
                        .engine
                        .pending_upserts
                        .saturating_add(driver.engine.pending_spawns)
                        .saturating_add(driver.engine.pending_despawns),
                    driver.engine.staged_events,
                    driver.engine.completed_events,
                    driver.control_entries,
                    driver.retiring_render_targets,
                    driver.transient_domains,
                    driver.transient_bytes,
                    driver.texture_assets,
                    driver.texture_bytes,
                    driver.profile_assets,
                    driver.profile_asset_bytes,
                )
            },
        );
        #[cfg(not(feature = "render"))]
        let render_endpoint = (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);

        #[cfg(feature = "audio")]
        let audio_endpoint =
            self.audio_endpoint_driver
                .map_or((0, 0, 0, 0, 0, 0, 0, 0), |driver| {
                    (
                        driver.queued_events,
                        driver.queued_event_bytes,
                        driver.resident_assets,
                        driver.resident_asset_bytes,
                        driver.one_shots,
                        driver.control_entries,
                        driver.retiring_control_entries,
                        driver.recovery_stopped_sources,
                    )
                });
        #[cfg(not(feature = "audio"))]
        let audio_endpoint = (0, 0, 0, 0, 0, 0, 0, 0);

        VoplayLeakSummary {
            roles: self
                .logic_endpoint_driver
                .map_or(self.owner.game.live_roles, |driver| driver.game.live_roles),
            entities: self
                .logic_endpoint_driver
                .map_or(self.owner.game.live_entities, |driver| {
                    driver.game.live_entities
                }),
            surfaces: self.owner.surfaces,
            input_scopes: self.owner.input_scopes,
            pending_endpoint_events: self
                .logic_endpoint_driver
                .map_or(self.owner.game.pending_endpoint_events, |driver| {
                    driver.game.pending_endpoint_events
                }),
            pending_logic_io_commands: self
                .logic_endpoint_driver
                .map_or(self.owner.game.pending_logic_io_commands, |driver| {
                    driver.game.pending_logic_io_commands
                }),
            hosted_logic_pending_control_forwards: self.hosted_logic_endpoint_driver.map_or(
                0,
                |driver| {
                    driver.pending_render_control as usize + driver.pending_audio_control as usize
                },
            ),
            hosted_logic_control_entries: self.hosted_logic_endpoint_driver.map_or(0, |driver| {
                driver
                    .render_control_entries
                    .saturating_add(driver.audio_control_entries)
                    .saturating_add(driver.control_store.render_refs)
                    .saturating_add(driver.control_store.audio_refs)
            }),
            hosted_logic_output_packets: self
                .hosted_logic_endpoint_driver
                .map_or(0, |driver| driver.output.packets),
            hosted_logic_output_bytes: self
                .hosted_logic_endpoint_driver
                .map_or(0, |driver| driver.output.bytes),
            #[cfg(feature = "app-session")]
            host_pending_web_controls: self
                .hosted_runtime
                .map_or(0, |runtime| runtime.pending_web_controls),
            #[cfg(not(feature = "app-session"))]
            host_pending_web_controls: 0,
            #[cfg(feature = "app-session")]
            host_pending_web_frames: self
                .hosted_runtime
                .map_or(0, |runtime| runtime.pending_web_frames),
            #[cfg(not(feature = "app-session"))]
            host_pending_web_frames: 0,
            #[cfg(feature = "app-session")]
            host_haptics_requests: self
                .hosted_runtime
                .map_or(0, |runtime| runtime.haptics.pending_requests),
            #[cfg(not(feature = "app-session"))]
            host_haptics_requests: 0,
            #[cfg(feature = "app-session")]
            host_haptics_commands: self
                .hosted_runtime
                .map_or(0, |runtime| runtime.haptics.queued_commands),
            #[cfg(not(feature = "app-session"))]
            host_haptics_commands: 0,
            #[cfg(feature = "app-session")]
            host_audio_device_leases: self
                .hosted_runtime
                .map_or(0, |runtime| runtime.audio_device_active as usize),
            #[cfg(not(feature = "app-session"))]
            host_audio_device_leases: 0,
            backend_surfaces: self
                .host_render_backend
                .map_or(0, |backend| backend.surfaces)
                .saturating_add(platform_backend.0)
                .saturating_add(
                    self.headless_render_backend
                        .map_or(0, |backend| backend.surfaces),
                ),
            backend_pending_frames: self
                .host_render_backend
                .map_or(0, |backend| backend.pending_frames)
                .saturating_add(platform_backend.1)
                .saturating_add(
                    self.headless_render_backend
                        .map_or(0, |backend| backend.pending_frames),
                ),
            backend_readbacks: self
                .host_render_backend
                .map_or(0, |backend| {
                    backend
                        .synchronization
                        .pending_readbacks
                        .saturating_add(backend.synchronization.ready_readbacks)
                        .saturating_add(backend.synchronization.pending_frame_traces)
                        .saturating_add(backend.synchronization.ready_frame_traces)
                })
                .saturating_add(platform_backend.2)
                .saturating_add(
                    self.headless_render_backend
                        .map_or(0, |backend| backend.readbacks),
                ),
            backend_targets: self
                .headless_render_backend
                .map_or(0, |backend| backend.targets),
            backend_textures: self
                .headless_render_backend
                .map_or(0, |backend| backend.textures),
            backend_texture_bytes: self
                .headless_render_backend
                .map_or(0, |backend| backend.texture_bytes),
            pending_render_ops: self.owner.game.render_outbox.pending_ops,
            acknowledged_render_objects: self.owner.game.render_outbox.acked_objects,
            presentation_domains: self.owner.game.presentation_state.domains,
            presentation_entries: self.owner.game.presentation_state.entries,
            presentation_bytes: self.owner.game.presentation_state.bytes,
            simulation_resources: self.owner.game.simulation_state.resources,
            simulation_resource_bytes: self.owner.game.simulation_state.resource_bytes,
            simulation_events: self.owner.game.simulation_state.events,
            simulation_event_bytes: self.owner.game.simulation_state.event_bytes,
            encoder_objects: self
                .owner
                .encoder
                .retained_objects
                .saturating_add(render_endpoint.0),
            encoder_assets: self
                .owner
                .encoder
                .retained_assets
                .saturating_add(render_endpoint.1),
            encoder_bytes: self
                .owner
                .encoder
                .retained_bytes
                .saturating_add(render_endpoint.2),
            endpoint_packets: self.endpoint_total.packets,
            buffer_leases: self.buffer_leases.live,
            device_bindings: self.device_hub.engine_bindings,
            pending_readbacks: self.host_render.synchronization.pending_readbacks,
            ready_readbacks: self.host_render.synchronization.ready_readbacks,
            pending_frame_traces: self.host_render.synchronization.pending_frame_traces,
            ready_frame_traces: self.host_render.synchronization.ready_frame_traces,
            assets: self
                .asset_endpoint_driver
                .map(|driver| driver.server.assets)
                .or_else(|| self.asset.map(|asset| asset.assets))
                .unwrap_or(0),
            asset_scopes: self
                .asset_endpoint_driver
                .map(|driver| driver.server.scopes)
                .or_else(|| self.asset.map(|asset| asset.scopes))
                .unwrap_or(0),
            asset_tickets: self
                .asset_endpoint_driver
                .map(|driver| driver.server.tickets)
                .or_else(|| self.asset.map(|asset| asset.tickets))
                .unwrap_or(0),
            asset_leases: self
                .asset_endpoint_driver
                .map(|driver| driver.server.asset_leases)
                .or_else(|| self.asset.map(|asset| asset.asset_leases))
                .unwrap_or(0),
            audio_buses: self.audio.map_or(0, |audio| audio.buses),
            audio_sources: self.audio.map_or(0, |audio| audio.persistent_sources),
            audio_voices: self.audio.map_or(0, |audio| audio.active_voices),
            audio_streams: self
                .audio_pipeline
                .map_or(0, |pipeline| pipeline.streaming.live_streams),
            audio_pending_decodes: self
                .audio_pipeline
                .map_or(0, |pipeline| pipeline.streaming.pending_decodes),
            audio_ring_samples: self
                .audio_pipeline
                .map_or(0, |pipeline| pipeline.streaming.reserved_ring_samples),
            audio_queued_events: audio_endpoint.0,
            audio_queued_event_bytes: audio_endpoint.1,
            audio_resident_assets: audio_endpoint.2,
            audio_resident_asset_bytes: audio_endpoint.3,
            audio_one_shots: audio_endpoint.4,
            audio_control_entries: audio_endpoint.5,
            audio_retiring_control_entries: audio_endpoint.6,
            audio_recovery_stops: audio_endpoint.7,
            render_objects: (render_endpoint.3 as usize)
                .saturating_add(self.owner.presentation.engine.render_objects),
            render_pending_changes: render_endpoint.4,
            render_staged_events: render_endpoint.5,
            render_completed_events: render_endpoint.6,
            render_control_entries: render_endpoint.7,
            render_retiring_targets: render_endpoint.8,
            render_transient_domains: render_endpoint.9,
            render_transient_bytes: render_endpoint.10,
            render_texture_assets: render_endpoint.11,
            render_texture_bytes: render_endpoint.12,
            render_profile_assets: render_endpoint.13,
            render_profile_asset_bytes: render_endpoint.14,
            #[cfg(feature = "pack")]
            partitions: self
                .world_partition
                .map_or(0, |partition| partition.partitions),
            #[cfg(not(feature = "pack"))]
            partitions: 0,
            #[cfg(feature = "pack")]
            partition_asset_tickets: self
                .world_partition
                .map_or(0, |partition| partition.asset_tickets),
            #[cfg(not(feature = "pack"))]
            partition_asset_tickets: 0,
            #[cfg(feature = "pack")]
            partition_io_requests: self.partition_io.map_or(0, |io| io.live_requests),
            #[cfg(not(feature = "pack"))]
            partition_io_requests: 0,
            #[cfg(feature = "pack")]
            partition_retired_io_requests: self
                .world_partition
                .map_or(0, |partition| partition.retired_io_requests),
            #[cfg(not(feature = "pack"))]
            partition_retired_io_requests: 0,
            #[cfg(feature = "pack")]
            partition_installed_chunks: self
                .world_partition
                .map_or(0, |partition| partition.installed_chunks),
            #[cfg(not(feature = "pack"))]
            partition_installed_chunks: 0,
        }
    }
}
