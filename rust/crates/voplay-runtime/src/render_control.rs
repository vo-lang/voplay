use std::collections::{BTreeMap, BTreeSet};

use voplay_protocol::EngineId;

use crate::{
    control::{
        control_kind_tag, write_stable_binding, ControlDependencyRef, ControlDomain, ControlError,
        ControlKind, ControlSnapshotState, ControlStateSnapshot, ControlTxnBuilder,
        ControlTxnIdentity, DescriptorDependency, StableControlRef, STABLE_BINDING_BYTES,
    },
    outbox::PresentationDomainId,
    render_graph::{
        CompiledRenderGraph, RenderGraphConfig, ResourceLifetime, ResourceUsage, TextureFormat,
    },
    render_graph_wire::decode_render_graph_program,
    surface::SurfaceMetrics,
    world::WorldEntity,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SizePolicy {
    Fixed { width: u32, height: u32 },
    MatchSurface,
    SurfaceScale { numerator: u32, denominator: u32 },
    MatchTarget(StableControlRef),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClearPolicy {
    Load,
    ClearColor([u16; 4]),
    ClearDepth(u16),
    Discard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderTargetDescriptor {
    pub size: SizePolicy,
    pub color_format: TextureFormat,
    pub depth_format: Option<TextureFormat>,
    pub sample_count: u8,
    pub usage: ResourceUsage,
    pub lifetime: ResourceLifetime,
    pub clear: ClearPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderViewDescriptor {
    pub camera: WorldEntity,
    pub presentation_domain: PresentationDomainId,
    pub viewport: Viewport,
    pub layer_mask: u64,
    pub graph_template: u64,
    pub quality_profile: u32,
    pub clear: ClearPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderGraphDescriptor {
    pub signature: u64,
    pub nodes: Vec<u8>,
    pub required_features: Vec<u64>,
}

#[derive(Debug)]
pub struct ProvisionalRenderTargetRef {
    builder: ControlTxnIdentity,
    token: u32,
}

#[derive(Debug)]
pub struct ProvisionalRenderViewRef {
    _token: u32,
}

#[derive(Debug)]
pub struct ProvisionalRenderGraphRef {
    _token: u32,
}

impl ProvisionalRenderTargetRef {
    pub const fn promotion_token(&self) -> u32 {
        self.token
    }
}

impl ProvisionalRenderViewRef {
    pub const fn promotion_token(&self) -> u32 {
        self._token
    }
}

impl ProvisionalRenderGraphRef {
    pub const fn promotion_token(&self) -> u32 {
        self._token
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderControlError {
    InvalidConfig,
    InvalidTarget,
    InvalidView,
    WrongEngine,
    WrongDomain,
    WrongBuilder,
    InvalidControlRevision,
    EntryCapacity,
    DescriptorCapacity,
    DuplicateEntry,
    MissingTarget,
    MissingGraph,
    TargetCycle,
    TargetPixelCapacity,
    ViewportOutOfBounds,
    Malformed,
    Control(ControlError),
}

impl From<ControlError> for RenderControlError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}

pub struct RenderControlBuilder {
    engine: EngineId,
    identity: ControlTxnIdentity,
    issued_targets: BTreeSet<u32>,
    inner: ControlTxnBuilder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderControlDecodeConfig {
    pub max_entries: usize,
    pub max_descriptor_bytes: usize,
    pub max_targets: usize,
    pub max_views: usize,
}

impl Default for RenderControlDecodeConfig {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_descriptor_bytes: 16 * 1024 * 1024,
            max_targets: 2048,
            max_views: 2048,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealizedRenderTarget {
    pub target: StableControlRef,
    pub descriptor: RenderTargetDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealizedRenderView {
    pub view: StableControlRef,
    pub target: StableControlRef,
    pub descriptor: RenderViewDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealizedRenderGraph {
    pub graph: StableControlRef,
    pub descriptor: RenderGraphDescriptor,
    pub compiled: CompiledRenderGraph,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderControlState {
    pub engine: EngineId,
    pub revision: u64,
    pub targets: Vec<RealizedRenderTarget>,
    pub views: Vec<RealizedRenderView>,
    pub graphs: Vec<RealizedRenderGraph>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderControlFrameConfig {
    pub max_targets: usize,
    pub max_views: usize,
    pub max_total_pixels: u64,
}

impl Default for RenderControlFrameConfig {
    fn default() -> Self {
        Self {
            max_targets: 2048,
            max_views: 2048,
            max_total_pixels: 268_435_456,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedRenderTargetFrame {
    pub target: StableControlRef,
    pub width: u32,
    pub height: u32,
    pub descriptor_index: usize,
    pub external_surface: bool,
    pub color_format: TextureFormat,
    pub depth_format: Option<TextureFormat>,
    pub sample_count: u8,
    pub usage: ResourceUsage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedRenderViewFrame {
    pub view: StableControlRef,
    pub target: StableControlRef,
    pub target_width: u32,
    pub target_height: u32,
    pub descriptor_index: usize,
    pub descriptor: RenderViewDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedRenderGraphFrame {
    pub graph: StableControlRef,
    pub template_signature: u64,
    pub compiled_signature: u64,
    pub descriptor_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderControlFramePlan {
    pub engine: EngineId,
    pub control_revision: u64,
    pub domain: PresentationDomainId,
    pub targets: Vec<ResolvedRenderTargetFrame>,
    pub views: Vec<ResolvedRenderViewFrame>,
    pub graphs: Vec<ResolvedRenderGraphFrame>,
    pub signature: u64,
}

impl RenderControlState {
    pub fn resolve_frame(
        &self,
        domain: PresentationDomainId,
        surface: SurfaceMetrics,
        config: RenderControlFrameConfig,
    ) -> Result<RenderControlFramePlan, RenderControlError> {
        if domain.engine != self.engine || !domain.handle.is_valid() {
            return Err(RenderControlError::WrongDomain);
        }
        if !surface.is_valid()
            || config.max_targets == 0
            || config.max_views == 0
            || config.max_total_pixels == 0
        {
            return Err(RenderControlError::InvalidConfig);
        }
        let target_descriptors = self
            .targets
            .iter()
            .enumerate()
            .map(|(index, target)| (target.target, (index, &target.descriptor)))
            .collect::<BTreeMap<_, _>>();
        let selected_views = self
            .views
            .iter()
            .enumerate()
            .filter(|(_, view)| view.descriptor.presentation_domain == domain)
            .collect::<Vec<_>>();
        if !self.graphs.is_empty()
            && selected_views.iter().any(|(_, view)| {
                !self
                    .graphs
                    .iter()
                    .any(|graph| graph.descriptor.signature == view.descriptor.graph_template)
            })
        {
            return Err(RenderControlError::MissingGraph);
        }
        if selected_views.is_empty() {
            return Err(RenderControlError::InvalidView);
        }
        if selected_views.len() > config.max_views {
            return Err(RenderControlError::EntryCapacity);
        }

        let mut resolved = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        for (_, view) in &selected_views {
            resolve_target_size(
                view.target,
                surface,
                &target_descriptors,
                &mut resolved,
                &mut visiting,
            )?;
        }
        if resolved.len() > config.max_targets {
            return Err(RenderControlError::EntryCapacity);
        }
        let mut total_pixels = 0_u64;
        let mut targets = Vec::with_capacity(resolved.len());
        for (target, (width, height)) in &resolved {
            total_pixels = total_pixels
                .checked_add(u64::from(*width) * u64::from(*height))
                .filter(|pixels| *pixels <= config.max_total_pixels)
                .ok_or(RenderControlError::TargetPixelCapacity)?;
            targets.push(ResolvedRenderTargetFrame {
                target: *target,
                width: *width,
                height: *height,
                descriptor_index: target_descriptors[target].0,
                external_surface: target_descriptors[target].1.lifetime
                    == ResourceLifetime::ExternalSurface,
                color_format: target_descriptors[target].1.color_format,
                depth_format: target_descriptors[target].1.depth_format,
                sample_count: target_descriptors[target].1.sample_count,
                usage: target_descriptors[target].1.usage,
            });
        }

        let mut views = Vec::with_capacity(selected_views.len());
        for (index, view) in selected_views {
            let (target_width, target_height) = resolved
                .get(&view.target)
                .copied()
                .ok_or(RenderControlError::MissingTarget)?;
            let viewport = view.descriptor.viewport;
            if viewport
                .x
                .checked_add(viewport.width)
                .is_none_or(|end| end > target_width)
                || viewport
                    .y
                    .checked_add(viewport.height)
                    .is_none_or(|end| end > target_height)
            {
                return Err(RenderControlError::ViewportOutOfBounds);
            }
            views.push(ResolvedRenderViewFrame {
                view: view.view,
                target: view.target,
                target_width,
                target_height,
                descriptor_index: index,
                descriptor: view.descriptor,
            });
        }
        views.sort_by_key(|view| view.view);
        let mut graph_templates = views
            .iter()
            .map(|view| view.descriptor.graph_template)
            .collect::<BTreeSet<_>>();
        let mut graphs = Vec::with_capacity(graph_templates.len());
        for (descriptor_index, graph) in self.graphs.iter().enumerate() {
            if graph_templates.remove(&graph.descriptor.signature) {
                graphs.push(ResolvedRenderGraphFrame {
                    graph: graph.graph,
                    template_signature: graph.descriptor.signature,
                    compiled_signature: graph.compiled.signature,
                    descriptor_index,
                });
            }
        }
        if !graph_templates.is_empty() && !self.graphs.is_empty() {
            return Err(RenderControlError::MissingGraph);
        }
        graphs.sort_by_key(|graph| graph.graph);
        let signature = frame_plan_signature(self, domain, surface, &targets, &views, &graphs);
        Ok(RenderControlFramePlan {
            engine: self.engine,
            control_revision: self.revision,
            domain,
            targets,
            views,
            graphs,
            signature,
        })
    }
}

fn resolve_target_size(
    target: StableControlRef,
    surface: SurfaceMetrics,
    descriptors: &BTreeMap<StableControlRef, (usize, &RenderTargetDescriptor)>,
    resolved: &mut BTreeMap<StableControlRef, (u32, u32)>,
    visiting: &mut BTreeSet<StableControlRef>,
) -> Result<(u32, u32), RenderControlError> {
    if let Some(size) = resolved.get(&target) {
        return Ok(*size);
    }
    if !visiting.insert(target) {
        return Err(RenderControlError::TargetCycle);
    }
    let descriptor = descriptors
        .get(&target)
        .map(|(_, descriptor)| *descriptor)
        .ok_or(RenderControlError::MissingTarget)?;
    let size = match descriptor.size {
        SizePolicy::Fixed { width, height } => (width, height),
        SizePolicy::MatchSurface => (surface.width, surface.height),
        SizePolicy::SurfaceScale {
            numerator,
            denominator,
        } => (
            scaled_dimension(surface.width, numerator, denominator)?,
            scaled_dimension(surface.height, numerator, denominator)?,
        ),
        SizePolicy::MatchTarget(dependency) => {
            resolve_target_size(dependency, surface, descriptors, resolved, visiting)?
        }
    };
    visiting.remove(&target);
    if size.0 == 0 || size.1 == 0 {
        return Err(RenderControlError::InvalidTarget);
    }
    resolved.insert(target, size);
    Ok(size)
}

fn scaled_dimension(
    value: u32,
    numerator: u32,
    denominator: u32,
) -> Result<u32, RenderControlError> {
    let scaled = u64::from(value)
        .checked_mul(u64::from(numerator))
        .ok_or(RenderControlError::TargetPixelCapacity)?
        / u64::from(denominator);
    u32::try_from(scaled.max(1)).map_err(|_| RenderControlError::TargetPixelCapacity)
}

fn frame_plan_signature(
    state: &RenderControlState,
    domain: PresentationDomainId,
    surface: SurfaceMetrics,
    targets: &[ResolvedRenderTargetFrame],
    views: &[ResolvedRenderViewFrame],
    graphs: &[ResolvedRenderGraphFrame],
) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for value in [
        state.revision,
        u64::from(domain.handle.index),
        u64::from(domain.handle.generation),
        u64::from(surface.width),
        u64::from(surface.height),
        u64::from(surface.scale_numerator),
        u64::from(surface.scale_denominator),
    ] {
        hash = (hash ^ value).wrapping_mul(0x100000001b3);
    }
    for target in targets {
        for value in [
            u64::from(target.target.handle.index),
            u64::from(target.target.handle.generation),
            u64::from(target.width),
            u64::from(target.height),
        ] {
            hash = (hash ^ value).wrapping_mul(0x100000001b3);
        }
    }
    for view in views {
        let descriptor = &state.views[view.descriptor_index].descriptor;
        for value in [
            u64::from(view.view.handle.index),
            u64::from(view.view.handle.generation),
            u64::from(view.target.handle.index),
            u64::from(view.target.handle.generation),
            descriptor.graph_template,
            u64::from(descriptor.quality_profile),
            descriptor.layer_mask,
            u64::from(descriptor.viewport.x),
            u64::from(descriptor.viewport.y),
            u64::from(descriptor.viewport.width),
            u64::from(descriptor.viewport.height),
        ] {
            hash = (hash ^ value).wrapping_mul(0x100000001b3);
        }
    }
    for graph in graphs {
        for value in [
            u64::from(graph.graph.handle.index),
            u64::from(graph.graph.handle.generation),
            graph.template_signature,
            graph.compiled_signature,
        ] {
            hash = (hash ^ value).wrapping_mul(0x100000001b3);
        }
    }
    hash.max(1)
}

impl RenderControlBuilder {
    pub fn new(engine: EngineId, inner: ControlTxnBuilder) -> Result<Self, RenderControlError> {
        if !engine.is_valid() {
            return Err(RenderControlError::WrongEngine);
        }
        let identity = inner.identity();
        Ok(Self {
            engine,
            identity,
            issued_targets: BTreeSet::new(),
            inner,
        })
    }

    pub fn create_target(
        &mut self,
        descriptor: &RenderTargetDescriptor,
    ) -> Result<ProvisionalRenderTargetRef, RenderControlError> {
        let encoded = encode_target(self.engine, descriptor)?;
        let token = self.inner.create_with_dependencies(
            ControlKind::RenderTarget,
            encoded.bytes,
            encoded.dependencies,
        )?;
        self.issued_targets.insert(token);
        Ok(ProvisionalRenderTargetRef {
            builder: self.identity,
            token,
        })
    }

    pub fn create_graph(
        &mut self,
        descriptor: &RenderGraphDescriptor,
    ) -> Result<ProvisionalRenderGraphRef, RenderControlError> {
        let encoded = encode_graph(self.engine, descriptor)?;
        Ok(ProvisionalRenderGraphRef {
            _token: self.inner.create(ControlKind::RenderGraph, encoded)?,
        })
    }

    pub fn create_view_for_provisional_target(
        &mut self,
        target: &ProvisionalRenderTargetRef,
        descriptor: &RenderViewDescriptor,
    ) -> Result<ProvisionalRenderViewRef, RenderControlError> {
        if target.builder != self.identity || !self.issued_targets.contains(&target.token) {
            return Err(RenderControlError::WrongBuilder);
        }
        let encoded = encode_view(
            self.engine,
            TargetBinding::Provisional(target.token),
            descriptor,
        )?;
        Ok(ProvisionalRenderViewRef {
            _token: self.inner.create_with_dependencies(
                ControlKind::RenderView,
                encoded.bytes,
                encoded.dependencies,
            )?,
        })
    }

    pub fn create_view_for_stable_target(
        &mut self,
        target: StableControlRef,
        descriptor: &RenderViewDescriptor,
    ) -> Result<ProvisionalRenderViewRef, RenderControlError> {
        if target.engine != self.engine {
            return Err(RenderControlError::WrongEngine);
        }
        if target.kind != ControlKind::RenderTarget {
            return Err(RenderControlError::InvalidTarget);
        }
        let encoded = encode_view(self.engine, TargetBinding::Stable(target), descriptor)?;
        Ok(ProvisionalRenderViewRef {
            _token: self.inner.create_with_dependencies(
                ControlKind::RenderView,
                encoded.bytes,
                encoded.dependencies,
            )?,
        })
    }

    pub fn finish(self) -> ControlTxnBuilder {
        self.inner
    }
}

enum TargetBinding {
    Provisional(u32),
    Stable(StableControlRef),
}

struct EncodedDescriptor {
    bytes: Vec<u8>,
    dependencies: Vec<DescriptorDependency>,
}

fn encode_target(
    engine: EngineId,
    descriptor: &RenderTargetDescriptor,
) -> Result<EncodedDescriptor, RenderControlError> {
    if !matches!(descriptor.sample_count, 1 | 2 | 4 | 8)
        || descriptor.usage.0 == 0
        || matches!(
            descriptor.color_format,
            TextureFormat::Depth24 | TextureFormat::Depth32Float
        )
        || descriptor.depth_format.is_some_and(|format| {
            !matches!(format, TextureFormat::Depth24 | TextureFormat::Depth32Float)
        })
    {
        return Err(RenderControlError::InvalidTarget);
    }
    if descriptor.lifetime == ResourceLifetime::Transient
        && (descriptor.usage.0 & (ResourceUsage::PRESENT.0 | ResourceUsage::READBACK.0) != 0)
    {
        return Err(RenderControlError::InvalidTarget);
    }
    if descriptor.lifetime != ResourceLifetime::ExternalSurface
        && descriptor.usage.0 & ResourceUsage::PRESENT.0 != 0
    {
        return Err(RenderControlError::InvalidTarget);
    }
    if (descriptor.usage.0 & ResourceUsage::DEPTH_ATTACHMENT.0 != 0)
        != descriptor.depth_format.is_some()
        || matches!(descriptor.clear, ClearPolicy::ClearColor(_))
            && descriptor.usage.0 & ResourceUsage::COLOR_ATTACHMENT.0 == 0
        || matches!(descriptor.clear, ClearPolicy::ClearDepth(_))
            && descriptor.depth_format.is_none()
    {
        return Err(RenderControlError::InvalidTarget);
    }
    let mut bytes = Vec::new();
    let mut dependencies = Vec::new();
    bytes.extend_from_slice(&engine.index.to_le_bytes());
    bytes.extend_from_slice(&engine.generation.to_le_bytes());
    match &descriptor.size {
        SizePolicy::Fixed { width, height } if *width > 0 && *height > 0 => {
            bytes.push(1);
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
        }
        SizePolicy::MatchSurface => bytes.push(2),
        SizePolicy::SurfaceScale {
            numerator,
            denominator,
        } if *numerator > 0 && *denominator > 0 => {
            bytes.push(3);
            bytes.extend_from_slice(&numerator.to_le_bytes());
            bytes.extend_from_slice(&denominator.to_le_bytes());
        }
        SizePolicy::MatchTarget(target)
            if target.engine == engine && target.kind == ControlKind::RenderTarget =>
        {
            bytes.push(4);
            let offset = bytes.len();
            bytes.resize(offset + STABLE_BINDING_BYTES, 0);
            write_stable_binding(&mut bytes, offset, *target)?;
            dependencies.push(DescriptorDependency {
                offset,
                reference: ControlDependencyRef::Stable(*target),
            });
        }
        _ => return Err(RenderControlError::InvalidTarget),
    }
    bytes.push(texture_format_tag(descriptor.color_format));
    bytes.push(
        descriptor
            .depth_format
            .map(texture_format_tag)
            .unwrap_or(u8::MAX),
    );
    bytes.push(descriptor.sample_count);
    bytes.extend_from_slice(&descriptor.usage.0.to_le_bytes());
    bytes.push(resource_lifetime_tag(descriptor.lifetime));
    encode_clear(&mut bytes, descriptor.clear);
    Ok(EncodedDescriptor {
        bytes,
        dependencies,
    })
}

fn encode_view(
    engine: EngineId,
    target: TargetBinding,
    descriptor: &RenderViewDescriptor,
) -> Result<EncodedDescriptor, RenderControlError> {
    if descriptor.camera.world.engine != engine
        || descriptor.presentation_domain.engine != engine
        || descriptor.viewport.width == 0
        || descriptor.viewport.height == 0
        || descriptor.layer_mask == 0
        || descriptor.graph_template == 0
        || descriptor.quality_profile == 0
    {
        return Err(RenderControlError::InvalidView);
    }
    let mut bytes = Vec::new();
    let mut dependencies = Vec::new();
    bytes.extend_from_slice(&engine.index.to_le_bytes());
    bytes.extend_from_slice(&engine.generation.to_le_bytes());
    match target {
        TargetBinding::Provisional(token) => {
            let offset = bytes.len();
            bytes.push(1);
            bytes.push(control_kind_tag(ControlKind::RenderTarget));
            bytes.extend_from_slice(&token.to_le_bytes());
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            dependencies.push(DescriptorDependency {
                offset,
                reference: ControlDependencyRef::Provisional {
                    token,
                    kind: ControlKind::RenderTarget,
                },
            });
        }
        TargetBinding::Stable(target) => {
            let offset = bytes.len();
            bytes.resize(offset + STABLE_BINDING_BYTES, 0);
            write_stable_binding(&mut bytes, offset, target)?;
            dependencies.push(DescriptorDependency {
                offset,
                reference: ControlDependencyRef::Stable(target),
            });
        }
    }
    for value in [
        descriptor.camera.world.handle.index,
        descriptor.camera.world.handle.generation,
        descriptor.camera.entity.index,
        descriptor.camera.entity.generation,
        descriptor.presentation_domain.handle.index,
        descriptor.presentation_domain.handle.generation,
        descriptor.viewport.x,
        descriptor.viewport.y,
        descriptor.viewport.width,
        descriptor.viewport.height,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&descriptor.layer_mask.to_le_bytes());
    bytes.extend_from_slice(&descriptor.graph_template.to_le_bytes());
    bytes.extend_from_slice(&descriptor.quality_profile.to_le_bytes());
    encode_clear(&mut bytes, descriptor.clear);
    Ok(EncodedDescriptor {
        bytes,
        dependencies,
    })
}

const fn texture_format_tag(format: TextureFormat) -> u8 {
    match format {
        TextureFormat::Rgba8 => 1,
        TextureFormat::Rgba16Float => 2,
        TextureFormat::Depth24 => 3,
        TextureFormat::Depth32Float => 4,
    }
}

const fn resource_lifetime_tag(lifetime: ResourceLifetime) -> u8 {
    match lifetime {
        ResourceLifetime::Transient => 1,
        ResourceLifetime::Persistent => 2,
        ResourceLifetime::ExternalSurface => 3,
    }
}

fn encode_clear(bytes: &mut Vec<u8>, clear: ClearPolicy) {
    match clear {
        ClearPolicy::Load => bytes.push(1),
        ClearPolicy::ClearColor(color) => {
            bytes.push(2);
            for channel in color {
                bytes.extend_from_slice(&channel.to_le_bytes());
            }
        }
        ClearPolicy::ClearDepth(depth) => {
            bytes.push(3);
            bytes.extend_from_slice(&depth.to_le_bytes());
        }
        ClearPolicy::Discard => bytes.push(4),
    }
}

pub fn decode_render_control_snapshot(
    snapshot: &ControlStateSnapshot,
    config: RenderControlDecodeConfig,
) -> Result<RenderControlState, RenderControlError> {
    validate_decode_config(config)?;
    if !snapshot.engine.is_valid() {
        return Err(RenderControlError::WrongEngine);
    }
    if snapshot.domain != ControlDomain::Render {
        return Err(RenderControlError::WrongDomain);
    }
    if snapshot.revision == 0 {
        return Err(RenderControlError::InvalidControlRevision);
    }
    if snapshot.entries.len() > config.max_entries {
        return Err(RenderControlError::EntryCapacity);
    }
    let mut descriptor_bytes = 0_usize;
    let mut identities = BTreeSet::new();
    let mut handles = BTreeSet::new();
    let mut targets = BTreeMap::new();
    let mut encoded_views = Vec::new();
    let mut graphs = Vec::new();
    let mut graph_signatures = BTreeSet::new();
    for entry in &snapshot.entries {
        if entry.stable.engine != snapshot.engine || !entry.stable.handle.is_valid() {
            return Err(RenderControlError::WrongEngine);
        }
        if entry.stable.kind.domain() != ControlDomain::Render {
            return Err(RenderControlError::WrongDomain);
        }
        if !identities.insert(entry.stable) || !handles.insert(entry.stable.handle) {
            return Err(RenderControlError::DuplicateEntry);
        }
        descriptor_bytes = descriptor_bytes
            .checked_add(entry.descriptor.len())
            .filter(|bytes| *bytes <= config.max_descriptor_bytes)
            .ok_or(RenderControlError::DescriptorCapacity)?;
        if matches!(entry.state, ControlSnapshotState::Tombstone { .. }) {
            continue;
        }
        match entry.stable.kind {
            ControlKind::RenderTarget => {
                if targets.len() == config.max_targets {
                    return Err(RenderControlError::EntryCapacity);
                }
                let descriptor = decode_target(snapshot.engine, &entry.descriptor)?;
                targets.insert(
                    entry.stable,
                    RealizedRenderTarget {
                        target: entry.stable,
                        descriptor,
                    },
                );
            }
            ControlKind::RenderView => {
                if encoded_views.len() == config.max_views {
                    return Err(RenderControlError::EntryCapacity);
                }
                encoded_views.push((entry.stable, entry.descriptor.as_slice()));
            }
            ControlKind::RenderGraph => {
                let descriptor = decode_graph(snapshot.engine, &entry.descriptor)?;
                let compiled =
                    decode_render_graph_program(&descriptor.nodes, RenderGraphConfig::default())
                        .map_err(|_| RenderControlError::Malformed)?;
                if !graph_signatures.insert(descriptor.signature) {
                    return Err(RenderControlError::DuplicateEntry);
                }
                graphs.push(RealizedRenderGraph {
                    graph: entry.stable,
                    descriptor,
                    compiled,
                });
            }
            _ => return Err(RenderControlError::WrongDomain),
        }
    }
    let mut views = Vec::with_capacity(encoded_views.len());
    for (stable, bytes) in encoded_views {
        let (target, descriptor) = decode_view(snapshot.engine, bytes)?;
        if !targets.contains_key(&target) {
            return Err(RenderControlError::MissingTarget);
        }
        views.push(RealizedRenderView {
            view: stable,
            target,
            descriptor,
        });
    }
    Ok(RenderControlState {
        engine: snapshot.engine,
        revision: snapshot.revision,
        targets: targets.into_values().collect(),
        views,
        graphs,
    })
}

fn decode_target(
    engine: EngineId,
    bytes: &[u8],
) -> Result<RenderTargetDescriptor, RenderControlError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_engine(engine)?;
    let size = match cursor.u8()? {
        1 => SizePolicy::Fixed {
            width: cursor.u32()?,
            height: cursor.u32()?,
        },
        2 => SizePolicy::MatchSurface,
        3 => SizePolicy::SurfaceScale {
            numerator: cursor.u32()?,
            denominator: cursor.u32()?,
        },
        4 => SizePolicy::MatchTarget(cursor.stable(engine, ControlKind::RenderTarget)?),
        _ => return Err(RenderControlError::Malformed),
    };
    let color_format = decode_texture_format(cursor.u8()?)?;
    let depth_format = match cursor.u8()? {
        u8::MAX => None,
        tag => Some(decode_texture_format(tag)?),
    };
    let descriptor = RenderTargetDescriptor {
        size,
        color_format,
        depth_format,
        sample_count: cursor.u8()?,
        usage: ResourceUsage(cursor.u32()?),
        lifetime: decode_resource_lifetime(cursor.u8()?)?,
        clear: cursor.clear()?,
    };
    cursor.finish()?;
    if encode_target(engine, &descriptor)?.bytes != bytes {
        return Err(RenderControlError::Malformed);
    }
    Ok(descriptor)
}

fn decode_view(
    engine: EngineId,
    bytes: &[u8],
) -> Result<(StableControlRef, RenderViewDescriptor), RenderControlError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_engine(engine)?;
    let target = cursor.stable(engine, ControlKind::RenderTarget)?;
    let camera = WorldEntity {
        world: crate::world::WorldId {
            engine,
            handle: cursor.handle()?,
        },
        entity: cursor.handle()?,
    };
    let presentation_domain = PresentationDomainId {
        engine,
        handle: cursor.handle()?,
    };
    let descriptor = RenderViewDescriptor {
        camera,
        presentation_domain,
        viewport: Viewport {
            x: cursor.u32()?,
            y: cursor.u32()?,
            width: cursor.u32()?,
            height: cursor.u32()?,
        },
        layer_mask: cursor.u64()?,
        graph_template: cursor.u64()?,
        quality_profile: cursor.u32()?,
        clear: cursor.clear()?,
    };
    cursor.finish()?;
    if encode_view(engine, TargetBinding::Stable(target), &descriptor)?.bytes != bytes {
        return Err(RenderControlError::Malformed);
    }
    Ok((target, descriptor))
}

fn decode_graph(
    engine: EngineId,
    bytes: &[u8],
) -> Result<RenderGraphDescriptor, RenderControlError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_engine(engine)?;
    if cursor.u8()? != 3 {
        return Err(RenderControlError::Malformed);
    }
    let signature = cursor.u64()?;
    let node_bytes = cursor.u32()? as usize;
    let feature_count = cursor.u32()? as usize;
    if signature == 0 || node_bytes == 0 || feature_count > 4096 {
        return Err(RenderControlError::Malformed);
    }
    let nodes = cursor.take(node_bytes)?.to_vec();
    let mut required_features = Vec::with_capacity(feature_count);
    let mut unique_features = BTreeSet::new();
    for _ in 0..feature_count {
        let feature = cursor.u64()?;
        if feature == 0 || !unique_features.insert(feature) {
            return Err(RenderControlError::Malformed);
        }
        required_features.push(feature);
    }
    cursor.finish()?;
    let descriptor = RenderGraphDescriptor {
        signature,
        nodes,
        required_features,
    };
    if encode_graph(engine, &descriptor)? != bytes {
        return Err(RenderControlError::Malformed);
    }
    Ok(descriptor)
}

fn encode_graph(
    engine: EngineId,
    descriptor: &RenderGraphDescriptor,
) -> Result<Vec<u8>, RenderControlError> {
    if descriptor.signature == 0
        || descriptor.nodes.is_empty()
        || descriptor.nodes.len() > u32::MAX as usize
        || descriptor.required_features.len() > 4096
    {
        return Err(RenderControlError::Malformed);
    }
    let mut unique_features = BTreeSet::new();
    if descriptor
        .required_features
        .iter()
        .any(|feature| *feature == 0 || !unique_features.insert(*feature))
    {
        return Err(RenderControlError::Malformed);
    }
    let mut bytes =
        Vec::with_capacity(25 + descriptor.nodes.len() + descriptor.required_features.len() * 8);
    bytes.extend_from_slice(&engine.index.to_le_bytes());
    bytes.extend_from_slice(&engine.generation.to_le_bytes());
    bytes.push(3);
    bytes.extend_from_slice(&descriptor.signature.to_le_bytes());
    bytes.extend_from_slice(&(descriptor.nodes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(descriptor.required_features.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&descriptor.nodes);
    for feature in &descriptor.required_features {
        bytes.extend_from_slice(&feature.to_le_bytes());
    }
    Ok(bytes)
}

fn validate_decode_config(config: RenderControlDecodeConfig) -> Result<(), RenderControlError> {
    if config.max_entries == 0
        || config.max_descriptor_bytes == 0
        || config.max_targets == 0
        || config.max_views == 0
    {
        return Err(RenderControlError::InvalidConfig);
    }
    Ok(())
}

fn decode_texture_format(tag: u8) -> Result<TextureFormat, RenderControlError> {
    match tag {
        1 => Ok(TextureFormat::Rgba8),
        2 => Ok(TextureFormat::Rgba16Float),
        3 => Ok(TextureFormat::Depth24),
        4 => Ok(TextureFormat::Depth32Float),
        _ => Err(RenderControlError::Malformed),
    }
}

fn decode_resource_lifetime(tag: u8) -> Result<ResourceLifetime, RenderControlError> {
    match tag {
        1 => Ok(ResourceLifetime::Transient),
        2 => Ok(ResourceLifetime::Persistent),
        3 => Ok(ResourceLifetime::ExternalSurface),
        _ => Err(RenderControlError::Malformed),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], RenderControlError> {
        let end = self
            .offset
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(RenderControlError::Malformed)?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, RenderControlError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, RenderControlError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, RenderControlError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, RenderControlError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn handle(&mut self) -> Result<voplay_protocol::Handle, RenderControlError> {
        let handle = voplay_protocol::Handle {
            index: self.u32()?,
            generation: self.u32()?,
        };
        if !handle.is_valid() {
            return Err(RenderControlError::Malformed);
        }
        Ok(handle)
    }

    fn expect_engine(&mut self, engine: EngineId) -> Result<(), RenderControlError> {
        if self.u32()? != engine.index || self.u32()? != engine.generation {
            return Err(RenderControlError::WrongEngine);
        }
        Ok(())
    }

    fn stable(
        &mut self,
        engine: EngineId,
        expected: ControlKind,
    ) -> Result<StableControlRef, RenderControlError> {
        if self.u8()? != 2 || self.u8()? != control_kind_tag(expected) {
            return Err(RenderControlError::Malformed);
        }
        Ok(StableControlRef {
            engine,
            kind: expected,
            handle: self.handle()?,
        })
    }

    fn clear(&mut self) -> Result<ClearPolicy, RenderControlError> {
        match self.u8()? {
            1 => Ok(ClearPolicy::Load),
            2 => Ok(ClearPolicy::ClearColor([
                self.u16()?,
                self.u16()?,
                self.u16()?,
                self.u16()?,
            ])),
            3 => Ok(ClearPolicy::ClearDepth(self.u16()?)),
            4 => Ok(ClearPolicy::Discard),
            _ => Err(RenderControlError::Malformed),
        }
    }

    fn finish(self) -> Result<(), RenderControlError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(RenderControlError::Malformed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ControlDomain, EngineControlConfig, EngineControlStore};
    use crate::world::WorldId;
    use voplay_protocol::Handle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn target_descriptor() -> RenderTargetDescriptor {
        RenderTargetDescriptor {
            size: SizePolicy::Fixed {
                width: 1280,
                height: 720,
            },
            color_format: TextureFormat::Rgba16Float,
            depth_format: None,
            sample_count: 1,
            usage: ResourceUsage::COLOR_ATTACHMENT.union(ResourceUsage::SAMPLED),
            lifetime: ResourceLifetime::Persistent,
            clear: ClearPolicy::ClearColor([0, 0, 0, u16::MAX]),
        }
    }

    fn view_descriptor(engine: EngineId) -> RenderViewDescriptor {
        RenderViewDescriptor {
            camera: WorldEntity {
                world: WorldId {
                    engine,
                    handle: handle(2),
                },
                entity: handle(3),
            },
            presentation_domain: PresentationDomainId {
                engine,
                handle: handle(4),
            },
            viewport: Viewport {
                x: 10,
                y: 20,
                width: 640,
                height: 360,
            },
            layer_mask: 1,
            graph_template: 7,
            quality_profile: 2,
            clear: ClearPolicy::Load,
        }
    }

    fn store(engine: EngineId) -> EngineControlStore {
        EngineControlStore::new(
            engine,
            EngineControlConfig {
                max_leases: 8,
                max_refs: 16,
                max_ops_per_transaction: 8,
                max_descriptor_bytes: 4096,
                max_snapshot_bytes: 4096,
            },
        )
        .unwrap()
    }

    #[test]
    fn provisional_target_is_promoted_inside_the_committed_view_descriptor() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let lease = store.issue_lease(handle(8), logic, 2).unwrap();
        let mut builder =
            RenderControlBuilder::new(engine, lease.begin(ControlDomain::Render, 1, 0)).unwrap();
        let target = builder.create_target(&target_descriptor()).unwrap();
        builder
            .create_view_for_provisional_target(&target, &view_descriptor(engine))
            .unwrap();

        let committed = store
            .commit(builder.finish())
            .unwrap()
            .publish_at_safe_point(logic)
            .unwrap();
        assert_eq!(committed.promotions.len(), 2);
        let target = committed.promotions[0].1;
        let view = committed.promotions[1].1;
        assert_eq!(target.kind, ControlKind::RenderTarget);
        assert_eq!(view.kind, ControlKind::RenderView);

        let descriptor = store.descriptor(view).unwrap();
        assert_eq!(descriptor[8], 2);
        assert_eq!(descriptor[9], control_kind_tag(ControlKind::RenderTarget));
        assert_eq!(&descriptor[10..14], &target.handle.index.to_le_bytes());
        assert_eq!(&descriptor[14..18], &target.handle.generation.to_le_bytes());
        assert_eq!(
            store.snapshot(ControlDomain::Render).unwrap().entries.len(),
            2
        );
    }

    #[test]
    fn provisional_targets_are_builder_local_and_rejection_consumes_no_capacity() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let first_lease = store.issue_lease(handle(7), logic, 2).unwrap();
        let second_lease = store.issue_lease(handle(8), logic, 2).unwrap();
        let mut first =
            RenderControlBuilder::new(engine, first_lease.begin(ControlDomain::Render, 1, 0))
                .unwrap();
        let foreign = first.create_target(&target_descriptor()).unwrap();
        let mut second =
            RenderControlBuilder::new(engine, second_lease.begin(ControlDomain::Render, 1, 0))
                .unwrap();
        let local = second.create_target(&target_descriptor()).unwrap();
        assert!(matches!(
            second.create_view_for_provisional_target(&foreign, &view_descriptor(engine)),
            Err(RenderControlError::WrongBuilder)
        ));
        second
            .create_view_for_provisional_target(&local, &view_descriptor(engine))
            .unwrap();
        assert_eq!(
            store
                .commit(second.finish())
                .unwrap()
                .publish_at_safe_point(logic)
                .unwrap()
                .promotions
                .len(),
            2
        );
    }

    #[test]
    fn stable_target_dependencies_are_owner_and_store_validated_atomically() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let cross_engine = StableControlRef {
            engine: handle(2),
            kind: ControlKind::RenderTarget,
            handle: handle(0),
        };
        let lease = store.issue_lease(handle(7), logic, 1).unwrap();
        let mut builder =
            RenderControlBuilder::new(engine, lease.begin(ControlDomain::Render, 1, 0)).unwrap();
        assert!(matches!(
            builder.create_view_for_stable_target(cross_engine, &view_descriptor(engine)),
            Err(RenderControlError::WrongEngine)
        ));

        let forged = StableControlRef {
            engine,
            kind: ControlKind::RenderTarget,
            handle: handle(11),
        };
        builder
            .create_view_for_stable_target(forged, &view_descriptor(engine))
            .unwrap();
        assert!(matches!(
            store.commit(builder.finish()),
            Err(ControlError::InvalidStableRef)
        ));
        assert_eq!(store.snapshot(ControlDomain::Render).unwrap().revision, 0);
    }

    #[test]
    fn descriptor_validation_rejects_invalid_lifetime_format_and_view_owners() {
        let engine = handle(1);
        let logic = handle(9);
        let mut store = store(engine);
        let lease = store.issue_lease(handle(7), logic, 3).unwrap();
        let mut builder =
            RenderControlBuilder::new(engine, lease.begin(ControlDomain::Render, 1, 0)).unwrap();

        let mut invalid_target = target_descriptor();
        invalid_target.lifetime = ResourceLifetime::Transient;
        invalid_target.usage = invalid_target.usage.union(ResourceUsage::READBACK);
        assert!(matches!(
            builder.create_target(&invalid_target),
            Err(RenderControlError::InvalidTarget)
        ));

        invalid_target = target_descriptor();
        invalid_target.depth_format = Some(TextureFormat::Rgba8);
        assert!(matches!(
            builder.create_target(&invalid_target),
            Err(RenderControlError::InvalidTarget)
        ));

        let target = builder.create_target(&target_descriptor()).unwrap();
        let mut invalid_view = view_descriptor(engine);
        invalid_view.camera.world.engine = handle(2);
        assert!(matches!(
            builder.create_view_for_provisional_target(&target, &invalid_view),
            Err(RenderControlError::InvalidView)
        ));
    }

    #[test]
    fn descriptor_encoding_is_deterministic_and_uses_explicit_wire_tags() {
        let engine = handle(1);
        let first = encode_target(engine, &target_descriptor()).unwrap();
        let second = encode_target(engine, &target_descriptor()).unwrap();
        assert_eq!(first.bytes, second.bytes);
        assert_eq!(first.dependencies.len(), 0);
        assert_eq!(first.bytes[17], 2);
    }
}
