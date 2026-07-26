use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphNodeId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphResourceId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TextureFormat {
    Rgba8,
    Rgba16Float,
    Depth24,
    Depth32Float,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceLifetime {
    Transient,
    Persistent,
    ExternalSurface,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphQueue {
    Graphics,
    Compute,
    Copy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceUsage(pub u32);

impl ResourceUsage {
    pub const SAMPLED: Self = Self(1 << 0);
    pub const STORAGE_READ: Self = Self(1 << 1);
    pub const STORAGE_WRITE: Self = Self(1 << 2);
    pub const COLOR_ATTACHMENT: Self = Self(1 << 3);
    pub const DEPTH_ATTACHMENT: Self = Self(1 << 4);
    pub const COPY_SRC: Self = Self(1 << 5);
    pub const COPY_DST: Self = Self(1 << 6);
    pub const PRESENT: Self = Self(1 << 7);
    pub const READBACK: Self = Self(1 << 8);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphResourceDesc {
    pub id: GraphResourceId,
    pub format: TextureFormat,
    pub sample_count: u8,
    pub allowed_usage: ResourceUsage,
    pub lifetime: ResourceLifetime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceRead {
    pub resource: GraphResourceId,
    pub version: u32,
    pub usage: ResourceUsage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceWrite {
    pub resource: GraphResourceId,
    pub base_version: u32,
    pub usage: ResourceUsage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNodeSpec {
    pub id: GraphNodeId,
    pub stable_type: u64,
    pub label: String,
    pub queue: GraphQueue,
    pub dependencies: Vec<GraphNodeId>,
    pub reads: Vec<ResourceRead>,
    pub writes: Vec<ResourceWrite>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderGraphConfig {
    pub max_nodes: usize,
    pub max_resources: usize,
    pub max_accesses: usize,
    pub max_label_bytes: usize,
}

impl Default for RenderGraphConfig {
    fn default() -> Self {
        Self {
            max_nodes: 1024,
            max_resources: 2048,
            max_accesses: 16_384,
            max_label_bytes: 256,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderGraphError {
    InvalidConfig,
    Capacity,
    InvalidIdentity,
    DuplicateNode,
    DuplicateResource,
    UnknownDependency,
    UnknownResource,
    Cycle,
    UnorderedHazard,
    DuplicateAccess,
    Feedback,
    VersionMismatch,
    UsageMismatch,
    FormatMismatch,
    LifetimeViolation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledRenderGraph {
    pub ordered_nodes: Vec<GraphNodeId>,
    pub final_versions: BTreeMap<GraphResourceId, u32>,
    pub allocations: BTreeMap<GraphResourceId, u32>,
    pub signature: u64,
}

pub fn compile(
    resources: Vec<GraphResourceDesc>,
    nodes: Vec<GraphNodeSpec>,
    config: RenderGraphConfig,
) -> Result<CompiledRenderGraph, RenderGraphError> {
    if config.max_nodes == 0
        || config.max_resources == 0
        || config.max_accesses == 0
        || config.max_label_bytes == 0
    {
        return Err(RenderGraphError::InvalidConfig);
    }
    let access_count = nodes.iter().try_fold(0_usize, |total, node| {
        total
            .checked_add(node.reads.len())?
            .checked_add(node.writes.len())
    });
    if nodes.len() > config.max_nodes
        || resources.len() > config.max_resources
        || access_count
            .filter(|count| *count <= config.max_accesses)
            .is_none()
    {
        return Err(RenderGraphError::Capacity);
    }
    let resources = collect_resources(resources)?;
    let nodes = collect_nodes(nodes, &config, &resources)?;
    let edges = dependency_edges(&nodes)?;
    let ordered_nodes = stable_topological_order(&edges)?;
    validate_hazards(&nodes, &edges)?;
    let (final_versions, lifetimes) = validate_versions(&resources, &nodes, &ordered_nodes)?;
    let allocations = allocate_resources(&resources, &lifetimes);
    let signature = graph_signature(&resources, &nodes, &ordered_nodes, &allocations);
    Ok(CompiledRenderGraph {
        ordered_nodes,
        final_versions,
        allocations,
        signature,
    })
}

fn collect_resources(
    resources: Vec<GraphResourceDesc>,
) -> Result<BTreeMap<GraphResourceId, GraphResourceDesc>, RenderGraphError> {
    let mut result = BTreeMap::new();
    for resource in resources {
        if resource.id.0 == 0 || resource.sample_count == 0 || resource.allowed_usage.0 == 0 {
            return Err(RenderGraphError::InvalidIdentity);
        }
        if is_depth(resource.format)
            && resource
                .allowed_usage
                .contains(ResourceUsage::COLOR_ATTACHMENT)
        {
            return Err(RenderGraphError::FormatMismatch);
        }
        if !is_depth(resource.format)
            && resource
                .allowed_usage
                .contains(ResourceUsage::DEPTH_ATTACHMENT)
        {
            return Err(RenderGraphError::FormatMismatch);
        }
        if resource.lifetime == ResourceLifetime::Transient
            && (resource.allowed_usage.contains(ResourceUsage::PRESENT)
                || resource.allowed_usage.contains(ResourceUsage::READBACK))
        {
            return Err(RenderGraphError::LifetimeViolation);
        }
        if resource.lifetime != ResourceLifetime::ExternalSurface
            && resource.allowed_usage.contains(ResourceUsage::PRESENT)
        {
            return Err(RenderGraphError::LifetimeViolation);
        }
        if result.insert(resource.id, resource).is_some() {
            return Err(RenderGraphError::DuplicateResource);
        }
    }
    Ok(result)
}

fn collect_nodes(
    nodes: Vec<GraphNodeSpec>,
    config: &RenderGraphConfig,
    resources: &BTreeMap<GraphResourceId, GraphResourceDesc>,
) -> Result<BTreeMap<GraphNodeId, GraphNodeSpec>, RenderGraphError> {
    let mut result = BTreeMap::new();
    for mut node in nodes {
        if node.id.0 == 0
            || node.stable_type == 0
            || node.label.is_empty()
            || node.label.len() > config.max_label_bytes
        {
            return Err(RenderGraphError::InvalidIdentity);
        }
        let reads = node
            .reads
            .iter()
            .map(|item| item.resource)
            .collect::<BTreeSet<_>>();
        let writes = node
            .writes
            .iter()
            .map(|item| item.resource)
            .collect::<BTreeSet<_>>();
        if reads.len() != node.reads.len() || writes.len() != node.writes.len() {
            return Err(RenderGraphError::DuplicateAccess);
        }
        if reads.iter().any(|resource| writes.contains(resource)) {
            return Err(RenderGraphError::Feedback);
        }
        for (resource, usage) in node
            .reads
            .iter()
            .map(|item| (item.resource, item.usage))
            .chain(node.writes.iter().map(|item| (item.resource, item.usage)))
        {
            let resource = resources
                .get(&resource)
                .ok_or(RenderGraphError::UnknownResource)?;
            if usage.0 == 0 || !resource.allowed_usage.contains(usage) {
                return Err(RenderGraphError::UsageMismatch);
            }
        }
        node.dependencies.sort();
        node.reads.sort_by_key(|item| item.resource);
        node.writes.sort_by_key(|item| item.resource);
        if result.insert(node.id, node).is_some() {
            return Err(RenderGraphError::DuplicateNode);
        }
    }
    Ok(result)
}

fn dependency_edges(
    nodes: &BTreeMap<GraphNodeId, GraphNodeSpec>,
) -> Result<BTreeMap<GraphNodeId, BTreeSet<GraphNodeId>>, RenderGraphError> {
    let mut edges = nodes
        .keys()
        .map(|id| (*id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for node in nodes.values() {
        let mut dependencies = BTreeSet::new();
        for dependency in &node.dependencies {
            if !nodes.contains_key(dependency) {
                return Err(RenderGraphError::UnknownDependency);
            }
            if !dependencies.insert(*dependency) {
                return Err(RenderGraphError::DuplicateAccess);
            }
            edges.get_mut(dependency).unwrap().insert(node.id);
        }
    }
    Ok(edges)
}

fn stable_topological_order(
    edges: &BTreeMap<GraphNodeId, BTreeSet<GraphNodeId>>,
) -> Result<Vec<GraphNodeId>, RenderGraphError> {
    let mut incoming = edges
        .keys()
        .map(|id| (*id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    for targets in edges.values() {
        for target in targets {
            *incoming.get_mut(target).unwrap() += 1;
        }
    }
    let mut ready = incoming
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(edges.len());
    while let Some(id) = ready.pop_first() {
        order.push(id);
        for target in &edges[&id] {
            let count = incoming.get_mut(target).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.insert(*target);
            }
        }
    }
    if order.len() != edges.len() {
        return Err(RenderGraphError::Cycle);
    }
    Ok(order)
}

fn validate_hazards(
    nodes: &BTreeMap<GraphNodeId, GraphNodeSpec>,
    edges: &BTreeMap<GraphNodeId, BTreeSet<GraphNodeId>>,
) -> Result<(), RenderGraphError> {
    let ids = nodes.keys().copied().collect::<Vec<_>>();
    for (position, left_id) in ids.iter().enumerate() {
        for right_id in ids.iter().skip(position + 1) {
            if has_hazard(&nodes[left_id], &nodes[right_id])
                && !reachable(edges, *left_id, *right_id)
                && !reachable(edges, *right_id, *left_id)
            {
                return Err(RenderGraphError::UnorderedHazard);
            }
        }
    }
    Ok(())
}

fn has_hazard(left: &GraphNodeSpec, right: &GraphNodeSpec) -> bool {
    let left_writes = left
        .writes
        .iter()
        .map(|item| item.resource)
        .collect::<BTreeSet<_>>();
    let right_writes = right
        .writes
        .iter()
        .map(|item| item.resource)
        .collect::<BTreeSet<_>>();
    let left_reads = left
        .reads
        .iter()
        .map(|item| item.resource)
        .collect::<BTreeSet<_>>();
    let right_reads = right
        .reads
        .iter()
        .map(|item| item.resource)
        .collect::<BTreeSet<_>>();
    left_writes
        .iter()
        .any(|resource| right_writes.contains(resource) || right_reads.contains(resource))
        || right_writes
            .iter()
            .any(|resource| left_reads.contains(resource))
}

fn reachable(
    edges: &BTreeMap<GraphNodeId, BTreeSet<GraphNodeId>>,
    source: GraphNodeId,
    target: GraphNodeId,
) -> bool {
    let mut pending = vec![source];
    let mut visited = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node) {
            continue;
        }
        for next in &edges[&node] {
            if *next == target {
                return true;
            }
            pending.push(*next);
        }
    }
    false
}

fn validate_versions(
    resources: &BTreeMap<GraphResourceId, GraphResourceDesc>,
    nodes: &BTreeMap<GraphNodeId, GraphNodeSpec>,
    order: &[GraphNodeId],
) -> Result<
    (
        BTreeMap<GraphResourceId, u32>,
        BTreeMap<GraphResourceId, (usize, usize)>,
    ),
    RenderGraphError,
> {
    let mut versions = resources
        .keys()
        .map(|id| (*id, 0_u32))
        .collect::<BTreeMap<_, _>>();
    let mut lifetimes = BTreeMap::new();
    for (position, id) in order.iter().enumerate() {
        let node = &nodes[id];
        for read in &node.reads {
            if versions[&read.resource] != read.version {
                return Err(RenderGraphError::VersionMismatch);
            }
            touch_lifetime(&mut lifetimes, read.resource, position);
        }
        for write in &node.writes {
            if versions[&write.resource] != write.base_version {
                return Err(RenderGraphError::VersionMismatch);
            }
            versions.insert(
                write.resource,
                write
                    .base_version
                    .checked_add(1)
                    .ok_or(RenderGraphError::VersionMismatch)?,
            );
            touch_lifetime(&mut lifetimes, write.resource, position);
        }
    }
    Ok((versions, lifetimes))
}

fn touch_lifetime(
    lifetimes: &mut BTreeMap<GraphResourceId, (usize, usize)>,
    resource: GraphResourceId,
    position: usize,
) {
    lifetimes
        .entry(resource)
        .and_modify(|range| range.1 = position)
        .or_insert((position, position));
}

fn allocate_resources(
    resources: &BTreeMap<GraphResourceId, GraphResourceDesc>,
    lifetimes: &BTreeMap<GraphResourceId, (usize, usize)>,
) -> BTreeMap<GraphResourceId, u32> {
    let mut allocations = BTreeMap::new();
    let mut slots: Vec<(u32, GraphResourceDesc, usize)> = Vec::new();
    for resource in resources.values() {
        let Some((first, last)) = lifetimes.get(&resource.id).copied() else {
            continue;
        };
        if resource.lifetime != ResourceLifetime::Transient {
            let slot = slots.len() as u32;
            slots.push((slot, resource.clone(), last));
            allocations.insert(resource.id, slot);
            continue;
        }
        let reusable = slots.iter_mut().find(|(_, current, current_last)| {
            current.lifetime == ResourceLifetime::Transient
                && *current_last < first
                && current.format == resource.format
                && current.sample_count == resource.sample_count
                && current.allowed_usage == resource.allowed_usage
        });
        let slot = if let Some((slot, _, current_last)) = reusable {
            *current_last = last;
            *slot
        } else {
            let slot = slots.len() as u32;
            slots.push((slot, resource.clone(), last));
            slot
        };
        allocations.insert(resource.id, slot);
    }
    allocations
}

fn graph_signature(
    resources: &BTreeMap<GraphResourceId, GraphResourceDesc>,
    nodes: &BTreeMap<GraphNodeId, GraphNodeSpec>,
    order: &[GraphNodeId],
    allocations: &BTreeMap<GraphResourceId, u32>,
) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for resource in resources.values() {
        hash = mix(hash, &resource.id.0.to_le_bytes());
        hash = mix(
            hash,
            &[
                resource.format as u8,
                resource.sample_count,
                resource.lifetime as u8,
            ],
        );
        hash = mix(hash, &resource.allowed_usage.0.to_le_bytes());
        hash = mix(
            hash,
            &allocations
                .get(&resource.id)
                .copied()
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
    }
    for id in order {
        let node = &nodes[id];
        hash = mix(hash, &id.0.to_le_bytes());
        hash = mix(hash, &node.stable_type.to_le_bytes());
        hash = mix(hash, node.label.as_bytes());
        hash = mix(hash, &[node.queue as u8]);
        for dependency in &node.dependencies {
            hash = mix(hash, &dependency.0.to_le_bytes());
        }
        for read in &node.reads {
            hash = mix(hash, &read.resource.0.to_le_bytes());
            hash = mix(hash, &read.version.to_le_bytes());
            hash = mix(hash, &read.usage.0.to_le_bytes());
        }
        for write in &node.writes {
            hash = mix(hash, &write.resource.0.to_le_bytes());
            hash = mix(hash, &write.base_version.to_le_bytes());
            hash = mix(hash, &write.usage.0.to_le_bytes());
        }
    }
    hash
}

fn mix(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

const fn is_depth(format: TextureFormat) -> bool {
    matches!(format, TextureFormat::Depth24 | TextureFormat::Depth32Float)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(id: u32, lifetime: ResourceLifetime, usage: ResourceUsage) -> GraphResourceDesc {
        GraphResourceDesc {
            id: GraphResourceId(id),
            format: TextureFormat::Rgba8,
            sample_count: 1,
            allowed_usage: usage,
            lifetime,
        }
    }

    fn node(id: u32, dependencies: Vec<u32>) -> GraphNodeSpec {
        GraphNodeSpec {
            id: GraphNodeId(id),
            stable_type: id as u64,
            label: format!("node-{id}"),
            queue: GraphQueue::Graphics,
            dependencies: dependencies.into_iter().map(GraphNodeId).collect(),
            reads: Vec::new(),
            writes: Vec::new(),
        }
    }

    fn valid_graph() -> (Vec<GraphResourceDesc>, Vec<GraphNodeSpec>) {
        let transient_usage = ResourceUsage::COLOR_ATTACHMENT.union(ResourceUsage::SAMPLED);
        let resources = vec![
            resource(1, ResourceLifetime::Transient, transient_usage),
            resource(2, ResourceLifetime::Transient, transient_usage),
            resource(
                3,
                ResourceLifetime::ExternalSurface,
                ResourceUsage::COLOR_ATTACHMENT.union(ResourceUsage::PRESENT),
            ),
        ];
        let mut one = node(1, Vec::new());
        one.writes.push(ResourceWrite {
            resource: GraphResourceId(1),
            base_version: 0,
            usage: ResourceUsage::COLOR_ATTACHMENT,
        });
        let mut two = node(2, vec![1]);
        two.reads.push(ResourceRead {
            resource: GraphResourceId(1),
            version: 1,
            usage: ResourceUsage::SAMPLED,
        });
        let mut three = node(3, vec![2]);
        three.writes.push(ResourceWrite {
            resource: GraphResourceId(2),
            base_version: 0,
            usage: ResourceUsage::COLOR_ATTACHMENT,
        });
        let mut four = node(4, vec![3]);
        four.reads.push(ResourceRead {
            resource: GraphResourceId(2),
            version: 1,
            usage: ResourceUsage::SAMPLED,
        });
        four.writes.push(ResourceWrite {
            resource: GraphResourceId(3),
            base_version: 0,
            usage: ResourceUsage::COLOR_ATTACHMENT,
        });
        (resources, vec![one, two, three, four])
    }

    #[test]
    fn stable_compile_versions_and_aliases_non_overlapping_transients() {
        let (resources, nodes) = valid_graph();
        let first = compile(
            resources.clone(),
            nodes.clone(),
            RenderGraphConfig::default(),
        )
        .unwrap();
        let mut reversed_resources = resources;
        reversed_resources.reverse();
        let mut reversed_nodes = nodes;
        reversed_nodes.reverse();
        let second = compile(
            reversed_resources,
            reversed_nodes,
            RenderGraphConfig::default(),
        )
        .unwrap();
        assert_eq!(first.signature, second.signature);
        assert_eq!(
            first.ordered_nodes,
            vec![
                GraphNodeId(1),
                GraphNodeId(2),
                GraphNodeId(3),
                GraphNodeId(4)
            ]
        );
        assert_eq!(first.final_versions[&GraphResourceId(1)], 1);
        assert_eq!(first.final_versions[&GraphResourceId(3)], 1);
        assert_eq!(
            first.allocations[&GraphResourceId(1)],
            first.allocations[&GraphResourceId(2)]
        );
        assert_ne!(
            first.allocations[&GraphResourceId(1)],
            first.allocations[&GraphResourceId(3)]
        );
    }

    #[test]
    fn cycle_unordered_hazard_feedback_and_version_mismatch_are_rejected() {
        let resource = resource(
            1,
            ResourceLifetime::Persistent,
            ResourceUsage::COLOR_ATTACHMENT.union(ResourceUsage::SAMPLED),
        );
        assert_eq!(
            compile(
                vec![resource.clone()],
                vec![node(1, vec![2]), node(2, vec![1])],
                RenderGraphConfig::default(),
            ),
            Err(RenderGraphError::Cycle)
        );
        let mut left = node(1, Vec::new());
        left.writes.push(ResourceWrite {
            resource: GraphResourceId(1),
            base_version: 0,
            usage: ResourceUsage::COLOR_ATTACHMENT,
        });
        let mut right = node(2, Vec::new());
        right.writes = left.writes.clone();
        assert_eq!(
            compile(
                vec![resource.clone()],
                vec![left.clone(), right],
                RenderGraphConfig::default(),
            ),
            Err(RenderGraphError::UnorderedHazard)
        );
        left.reads.push(ResourceRead {
            resource: GraphResourceId(1),
            version: 0,
            usage: ResourceUsage::SAMPLED,
        });
        assert_eq!(
            compile(
                vec![resource.clone()],
                vec![left],
                RenderGraphConfig::default(),
            ),
            Err(RenderGraphError::Feedback)
        );
        let mut version = node(1, Vec::new());
        version.reads.push(ResourceRead {
            resource: GraphResourceId(1),
            version: 1,
            usage: ResourceUsage::SAMPLED,
        });
        assert_eq!(
            compile(vec![resource], vec![version], RenderGraphConfig::default()),
            Err(RenderGraphError::VersionMismatch)
        );
    }

    #[test]
    fn format_usage_and_lifetime_fail_during_preflight() {
        let depth = GraphResourceDesc {
            id: GraphResourceId(1),
            format: TextureFormat::Depth24,
            sample_count: 1,
            allowed_usage: ResourceUsage::COLOR_ATTACHMENT,
            lifetime: ResourceLifetime::Persistent,
        };
        assert_eq!(
            compile(vec![depth], Vec::new(), RenderGraphConfig::default()),
            Err(RenderGraphError::FormatMismatch)
        );
        let transient_present = resource(1, ResourceLifetime::Transient, ResourceUsage::PRESENT);
        assert_eq!(
            compile(
                vec![transient_present],
                Vec::new(),
                RenderGraphConfig::default(),
            ),
            Err(RenderGraphError::LifetimeViolation)
        );
        let sampled_only = resource(1, ResourceLifetime::Persistent, ResourceUsage::SAMPLED);
        let mut writer = node(1, Vec::new());
        writer.writes.push(ResourceWrite {
            resource: GraphResourceId(1),
            base_version: 0,
            usage: ResourceUsage::COLOR_ATTACHMENT,
        });
        assert_eq!(
            compile(
                vec![sampled_only],
                vec![writer],
                RenderGraphConfig::default(),
            ),
            Err(RenderGraphError::UsageMismatch)
        );
    }
}
