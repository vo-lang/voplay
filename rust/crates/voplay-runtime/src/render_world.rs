use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voplay_protocol::{EngineId, Handle};

use crate::{presentation::RenderFrameDelta, RenderEntity};

const OBJECT_MAGIC: &[u8; 4] = b"VRW1";
const TRANSFORM_VALUES: usize = 12;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RenderComponentKind(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderComponent {
    pub kind: RenderComponentKind,
    pub schema_hash: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransformSample {
    pub previous_tick: u64,
    pub current_tick: u64,
    pub previous: [i64; TRANSFORM_VALUES],
    pub current: [i64; TRANSFORM_VALUES],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Aabb3 {
    pub min: [i64; 3],
    pub max: [i64; 3],
}

impl Aabb3 {
    pub fn is_valid(self) -> bool {
        (0..3).all(|axis| self.min[axis] <= self.max[axis])
    }

    pub fn intersects(self, other: Self) -> bool {
        (0..3).all(|axis| self.min[axis] <= other.max[axis] && self.max[axis] >= other.min[axis])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedRenderObject {
    pub entity: RenderEntity,
    pub transform: TransformSample,
    pub bounds: Aabb3,
    pub layers: u64,
    pub components: BTreeMap<RenderComponentKind, RenderComponent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderWorldConfig {
    pub max_objects: usize,
    pub max_components_per_object: usize,
    pub max_component_bytes: usize,
    pub max_total_component_bytes: usize,
    pub max_dirty_uploads: usize,
    pub max_views: usize,
    pub max_cells_per_object: usize,
    pub max_query_cells: usize,
    pub spatial_cell_extent: i64,
}

impl Default for RenderWorldConfig {
    fn default() -> Self {
        Self {
            max_objects: 1_000_000,
            max_components_per_object: 64,
            max_component_bytes: 4 * 1024 * 1024,
            max_total_component_bytes: 256 * 1024 * 1024,
            max_dirty_uploads: 1_000_000,
            max_views: 256,
            max_cells_per_object: 512,
            max_query_cells: 65_536,
            spatial_cell_extent: 1_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderWorldError {
    Closed,
    InvalidConfig,
    WrongEngine,
    Revision,
    ObjectCapacity,
    ComponentCapacity,
    ComponentByteCapacity,
    TotalByteCapacity,
    DirtyCapacity,
    ViewCapacity,
    SpatialCapacity,
    InvalidObject,
    InvalidPacket,
    DeviceGeneration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirtyUploadKind {
    Transform,
    Component(RenderComponentKind),
    Retire,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirtyUpload {
    pub entity: RenderEntity,
    pub kind: DirtyUploadKind,
    pub revision: u64,
    pub byte_len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewVisibility {
    pub view: Handle,
    pub view_revision: u64,
    pub world_revision: u64,
    pub visible: Vec<RenderEntity>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderWorldInstrumentation {
    pub objects: usize,
    pub component_bytes: usize,
    pub dirty_uploads: usize,
    pub applied_deltas: u64,
    pub decoded_objects: u64,
    pub retired_objects: u64,
    pub queued_dirty_uploads: u64,
    pub visibility_queries: u64,
    pub visibility_candidates: u64,
    pub last_delta_objects: usize,
    pub last_delta_despawns: usize,
}

#[derive(Clone)]
struct ObjectRecord {
    object: RetainedRenderObject,
    cells: Vec<[i64; 3]>,
    component_bytes: usize,
}

#[derive(Clone)]
pub struct RenderWorld {
    engine: EngineId,
    config: RenderWorldConfig,
    revision: u64,
    device_generation: u64,
    component_bytes: usize,
    objects: BTreeMap<RenderEntity, ObjectRecord>,
    spatial: BTreeMap<[i64; 3], BTreeSet<RenderEntity>>,
    dirty: VecDeque<DirtyUpload>,
    visibility: BTreeMap<Handle, ViewVisibility>,
    full_upload_required: bool,
    instrumentation: RenderWorldInstrumentation,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderWorldOwnerSnapshot {
    pub closed: bool,
    pub objects: usize,
    pub component_bytes: usize,
    pub spatial_cells: usize,
    pub spatial_memberships: usize,
    pub dirty_uploads: usize,
    pub visibility_views: usize,
    pub visibility_entities: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderWorldShutdownReport {
    pub released_objects: usize,
    pub released_component_bytes: usize,
    pub released_spatial_cells: usize,
    pub released_spatial_memberships: usize,
    pub released_dirty_uploads: usize,
    pub released_visibility_views: usize,
    pub released_visibility_entities: usize,
}

impl RenderWorld {
    pub fn new(
        engine: EngineId,
        device_generation: u64,
        config: RenderWorldConfig,
    ) -> Result<Self, RenderWorldError> {
        if !engine.is_valid()
            || device_generation == 0
            || config.max_objects == 0
            || config.max_components_per_object == 0
            || config.max_component_bytes == 0
            || config.max_total_component_bytes == 0
            || config.max_dirty_uploads == 0
            || config.max_views == 0
            || config.max_cells_per_object == 0
            || config.max_query_cells == 0
            || config.spatial_cell_extent <= 0
        {
            return Err(RenderWorldError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            revision: 0,
            device_generation,
            component_bytes: 0,
            objects: BTreeMap::new(),
            spatial: BTreeMap::new(),
            dirty: VecDeque::new(),
            visibility: BTreeMap::new(),
            full_upload_required: true,
            instrumentation: RenderWorldInstrumentation::default(),
            closed: false,
        })
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub const fn device_generation(&self) -> u64 {
        self.device_generation
    }

    pub fn instrumentation(&self) -> RenderWorldInstrumentation {
        RenderWorldInstrumentation {
            objects: self.objects.len(),
            component_bytes: self.component_bytes,
            dirty_uploads: self.dirty.len(),
            ..self.instrumentation
        }
    }

    pub fn owner_snapshot(&self) -> RenderWorldOwnerSnapshot {
        RenderWorldOwnerSnapshot {
            closed: self.closed,
            objects: self.objects.len(),
            component_bytes: self.component_bytes,
            spatial_cells: self.spatial.len(),
            spatial_memberships: self.spatial.values().map(BTreeSet::len).sum(),
            dirty_uploads: self.dirty.len(),
            visibility_views: self.visibility.len(),
            visibility_entities: self
                .visibility
                .values()
                .map(|visibility| visibility.visible.len())
                .sum(),
        }
    }

    pub fn shutdown(&mut self) -> RenderWorldShutdownReport {
        let snapshot = self.owner_snapshot();
        self.closed = true;
        self.objects.clear();
        self.component_bytes = 0;
        self.spatial.clear();
        self.dirty.clear();
        self.visibility.clear();
        self.full_upload_required = false;
        RenderWorldShutdownReport {
            released_objects: snapshot.objects,
            released_component_bytes: snapshot.component_bytes,
            released_spatial_cells: snapshot.spatial_cells,
            released_spatial_memberships: snapshot.spatial_memberships,
            released_dirty_uploads: snapshot.dirty_uploads,
            released_visibility_views: snapshot.visibility_views,
            released_visibility_entities: snapshot.visibility_entities,
        }
    }

    pub fn object(&self, entity: RenderEntity) -> Option<&RetainedRenderObject> {
        self.objects.get(&entity).map(|record| &record.object)
    }

    pub fn objects(&self) -> impl ExactSizeIterator<Item = &RetainedRenderObject> {
        self.objects.values().map(|record| &record.object)
    }

    pub fn apply_delta(&mut self, delta: &RenderFrameDelta) -> Result<(), RenderWorldError> {
        self.ensure_open()?;
        if delta.revision < self.revision
            || (!delta.full_snapshot && delta.revision > self.revision.saturating_add(1))
            || (!delta.full_snapshot
                && delta.revision == self.revision
                && (!delta.objects.is_empty() || !delta.despawned.is_empty()))
        {
            return Err(RenderWorldError::Revision);
        }
        let despawned = delta.despawned.iter().copied().collect::<BTreeSet<_>>();
        for entity in &delta.despawned {
            if entity.engine != self.engine {
                return Err(RenderWorldError::WrongEngine);
            }
        }
        let mut staged = Vec::with_capacity(delta.objects.len());
        for (entity, bytes) in &delta.objects {
            if entity.engine != self.engine {
                return Err(RenderWorldError::WrongEngine);
            }
            let object = decode_render_object(*entity, bytes, self.config)?;
            let cells = object_cells(object.bounds, self.config)?;
            let next_component_bytes = object
                .components
                .values()
                .map(|component| component.bytes.len())
                .sum::<usize>();
            staged.push((
                *entity,
                ObjectRecord {
                    object,
                    cells,
                    component_bytes: next_component_bytes,
                },
            ));
        }

        let mut object_count = if delta.full_snapshot {
            0
        } else {
            self.objects.len()
        };
        let mut component_bytes = if delta.full_snapshot {
            0
        } else {
            self.component_bytes
        };
        let mut dirty_additions = 0_usize;
        let mut retired_objects = 0_usize;
        if !delta.full_snapshot {
            for entity in &despawned {
                if let Some(previous) = self.objects.get(entity) {
                    object_count -= 1;
                    component_bytes -= previous.component_bytes;
                    retired_objects += 1;
                    dirty_additions = dirty_additions
                        .checked_add(1)
                        .ok_or(RenderWorldError::DirtyCapacity)?;
                }
            }
        }
        for (entity, record) in &staged {
            let existed_after_despawn = !delta.full_snapshot
                && !despawned.contains(entity)
                && self.objects.contains_key(entity);
            if existed_after_despawn {
                component_bytes -= self.objects[entity].component_bytes;
            } else {
                object_count = object_count
                    .checked_add(1)
                    .ok_or(RenderWorldError::ObjectCapacity)?;
            }
            component_bytes = component_bytes
                .checked_add(record.component_bytes)
                .filter(|bytes| *bytes <= self.config.max_total_component_bytes)
                .ok_or(RenderWorldError::TotalByteCapacity)?;
            if !delta.full_snapshot {
                dirty_additions = dirty_additions
                    .checked_add(1 + record.object.components.len())
                    .ok_or(RenderWorldError::DirtyCapacity)?;
            }
        }
        if object_count > self.config.max_objects {
            return Err(RenderWorldError::ObjectCapacity);
        }
        if !delta.full_snapshot
            && self
                .dirty
                .len()
                .checked_add(dirty_additions)
                .is_none_or(|count| count > self.config.max_dirty_uploads)
        {
            return Err(RenderWorldError::DirtyCapacity);
        }

        if delta.full_snapshot {
            self.objects.clear();
            self.spatial.clear();
            self.dirty.clear();
        } else {
            for entity in &despawned {
                let Some(previous) = self.objects.remove(entity) else {
                    continue;
                };
                remove_cells(&mut self.spatial, *entity, &previous.cells);
                self.dirty.push_back(DirtyUpload {
                    entity: *entity,
                    kind: DirtyUploadKind::Retire,
                    revision: delta.revision,
                    byte_len: 0,
                });
            }
        }
        for (entity, record) in staged {
            if let Some(previous) = self.objects.remove(&entity) {
                remove_cells(&mut self.spatial, entity, &previous.cells);
            }
            insert_cells(&mut self.spatial, entity, &record.cells);
            if !delta.full_snapshot {
                self.dirty.push_back(DirtyUpload {
                    entity,
                    kind: DirtyUploadKind::Transform,
                    revision: delta.revision,
                    byte_len: TRANSFORM_VALUES * 8,
                });
                self.dirty.extend(
                    record
                        .object
                        .components
                        .values()
                        .map(|component| DirtyUpload {
                            entity,
                            kind: DirtyUploadKind::Component(component.kind),
                            revision: delta.revision,
                            byte_len: component.bytes.len(),
                        }),
                );
            }
            self.objects.insert(entity, record);
        }
        self.component_bytes = component_bytes;
        self.revision = delta.revision;
        self.visibility.clear();
        self.full_upload_required |= delta.full_snapshot;
        self.instrumentation.applied_deltas = self.instrumentation.applied_deltas.saturating_add(1);
        self.instrumentation.decoded_objects = self
            .instrumentation
            .decoded_objects
            .saturating_add(delta.objects.len() as u64);
        self.instrumentation.retired_objects = self
            .instrumentation
            .retired_objects
            .saturating_add(retired_objects as u64);
        self.instrumentation.queued_dirty_uploads = self
            .instrumentation
            .queued_dirty_uploads
            .saturating_add(dirty_additions as u64);
        self.instrumentation.last_delta_objects = delta.objects.len();
        self.instrumentation.last_delta_despawns = delta.despawned.len();
        Ok(())
    }

    pub fn visibility(
        &mut self,
        view: Handle,
        view_revision: u64,
        layers: u64,
        bounds: Aabb3,
    ) -> Result<&ViewVisibility, RenderWorldError> {
        self.ensure_open()?;
        if !view.is_valid() || view_revision == 0 || layers == 0 || !bounds.is_valid() {
            return Err(RenderWorldError::InvalidObject);
        }
        if self.visibility.contains_key(&view)
            && self.visibility[&view].view_revision == view_revision
            && self.visibility[&view].world_revision == self.revision
        {
            return Ok(&self.visibility[&view]);
        }
        if !self.visibility.contains_key(&view) && self.visibility.len() == self.config.max_views {
            return Err(RenderWorldError::ViewCapacity);
        }
        let cells = object_cells(bounds, self.config)?;
        if cells.len() > self.config.max_query_cells {
            return Err(RenderWorldError::SpatialCapacity);
        }
        let mut candidates = BTreeSet::new();
        for cell in cells {
            if let Some(entities) = self.spatial.get(&cell) {
                candidates.extend(entities.iter().copied());
            }
        }
        let candidate_count = candidates.len();
        let visible = candidates
            .into_iter()
            .filter(|entity| {
                self.objects.get(entity).is_some_and(|record| {
                    record.object.layers & layers != 0 && record.object.bounds.intersects(bounds)
                })
            })
            .collect();
        self.instrumentation.visibility_queries =
            self.instrumentation.visibility_queries.saturating_add(1);
        self.instrumentation.visibility_candidates = self
            .instrumentation
            .visibility_candidates
            .saturating_add(candidate_count as u64);
        self.visibility.insert(
            view,
            ViewVisibility {
                view,
                view_revision,
                world_revision: self.revision,
                visible,
            },
        );
        Ok(&self.visibility[&view])
    }

    pub fn poll_dirty_uploads(
        &mut self,
        budget: usize,
    ) -> Result<Vec<DirtyUpload>, RenderWorldError> {
        self.ensure_open()?;
        if self.full_upload_required {
            let upload_count = self.objects.values().try_fold(0_usize, |count, record| {
                count.checked_add(1 + record.object.components.len())
            });
            if upload_count.is_none_or(|count| count > self.config.max_dirty_uploads) {
                return Err(RenderWorldError::DirtyCapacity);
            }
            self.dirty.clear();
            for record in self.objects.values() {
                self.dirty.push_back(DirtyUpload {
                    entity: record.object.entity,
                    kind: DirtyUploadKind::Transform,
                    revision: self.revision,
                    byte_len: TRANSFORM_VALUES * 8,
                });
                for component in record.object.components.values() {
                    self.dirty.push_back(DirtyUpload {
                        entity: record.object.entity,
                        kind: DirtyUploadKind::Component(component.kind),
                        revision: self.revision,
                        byte_len: component.bytes.len(),
                    });
                }
            }
            self.full_upload_required = false;
        }
        Ok((0..budget).map_while(|_| self.dirty.pop_front()).collect())
    }

    pub fn restart_device(&mut self, generation: u64) -> Result<(), RenderWorldError> {
        self.ensure_open()?;
        if generation <= self.device_generation {
            return Err(RenderWorldError::DeviceGeneration);
        }
        self.device_generation = generation;
        self.visibility.clear();
        self.dirty.clear();
        self.full_upload_required = true;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), RenderWorldError> {
        if self.closed {
            Err(RenderWorldError::Closed)
        } else {
            Ok(())
        }
    }
}

pub fn encode_render_object(
    object: &RetainedRenderObject,
    config: RenderWorldConfig,
) -> Result<Vec<u8>, RenderWorldError> {
    validate_object(object, config)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(OBJECT_MAGIC);
    bytes.extend_from_slice(&object.layers.to_le_bytes());
    bytes.extend_from_slice(&object.transform.previous_tick.to_le_bytes());
    bytes.extend_from_slice(&object.transform.current_tick.to_le_bytes());
    for value in object.transform.previous {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in object.transform.current {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in object.bounds.min {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in object.bounds.max {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&(object.components.len() as u32).to_le_bytes());
    for component in object.components.values() {
        bytes.extend_from_slice(&component.kind.0.to_le_bytes());
        bytes.extend_from_slice(&component.schema_hash.to_le_bytes());
        bytes.extend_from_slice(&(component.bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&component.bytes);
    }
    Ok(bytes)
}

pub fn decode_render_object(
    entity: RenderEntity,
    bytes: &[u8],
    config: RenderWorldConfig,
) -> Result<RetainedRenderObject, RenderWorldError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(4)? != OBJECT_MAGIC {
        return Err(RenderWorldError::InvalidPacket);
    }
    let layers = cursor.u64()?;
    let previous_tick = cursor.u64()?;
    let current_tick = cursor.u64()?;
    let mut previous = [0; TRANSFORM_VALUES];
    let mut current = [0; TRANSFORM_VALUES];
    for value in &mut previous {
        *value = cursor.i64()?;
    }
    for value in &mut current {
        *value = cursor.i64()?;
    }
    let mut min = [0; 3];
    let mut max = [0; 3];
    for value in &mut min {
        *value = cursor.i64()?;
    }
    for value in &mut max {
        *value = cursor.i64()?;
    }
    let count = cursor.u32()? as usize;
    if count > config.max_components_per_object {
        return Err(RenderWorldError::ComponentCapacity);
    }
    let mut components = BTreeMap::new();
    for _ in 0..count {
        let kind = RenderComponentKind(cursor.u32()?);
        let schema_hash = cursor.u64()?;
        let component_bytes = cursor.bytes32(config.max_component_bytes)?.to_vec();
        let component = RenderComponent {
            kind,
            schema_hash,
            bytes: component_bytes,
        };
        if components.insert(kind, component).is_some() {
            return Err(RenderWorldError::InvalidPacket);
        }
    }
    cursor.finish()?;
    let object = RetainedRenderObject {
        entity,
        transform: TransformSample {
            previous_tick,
            current_tick,
            previous,
            current,
        },
        bounds: Aabb3 { min, max },
        layers,
        components,
    };
    validate_object(&object, config)?;
    Ok(object)
}

fn validate_object(
    object: &RetainedRenderObject,
    config: RenderWorldConfig,
) -> Result<(), RenderWorldError> {
    if !object.entity.engine.is_valid()
        || !object.entity.entity.is_valid()
        || object.layers == 0
        || !object.bounds.is_valid()
        || object.transform.current_tick < object.transform.previous_tick
        || object.components.len() > config.max_components_per_object
        || object.components.values().any(|component| {
            component.kind.0 == 0
                || component.schema_hash == 0
                || component.bytes.len() > config.max_component_bytes
        })
    {
        return Err(RenderWorldError::InvalidObject);
    }
    Ok(())
}

fn object_cells(
    bounds: Aabb3,
    config: RenderWorldConfig,
) -> Result<Vec<[i64; 3]>, RenderWorldError> {
    if !bounds.is_valid() {
        return Err(RenderWorldError::InvalidObject);
    }
    let min = bounds
        .min
        .map(|value| value.div_euclid(config.spatial_cell_extent));
    let max = bounds
        .max
        .map(|value| value.div_euclid(config.spatial_cell_extent));
    let mut cells = Vec::new();
    for x in min[0]..=max[0] {
        for y in min[1]..=max[1] {
            for z in min[2]..=max[2] {
                if cells.len() == config.max_cells_per_object {
                    return Err(RenderWorldError::SpatialCapacity);
                }
                cells.push([x, y, z]);
            }
        }
    }
    Ok(cells)
}

fn remove_cells(
    spatial: &mut BTreeMap<[i64; 3], BTreeSet<RenderEntity>>,
    entity: RenderEntity,
    cells: &[[i64; 3]],
) {
    for cell in cells {
        if let Some(entities) = spatial.get_mut(cell) {
            entities.remove(&entity);
            if entities.is_empty() {
                spatial.remove(cell);
            }
        }
    }
}

fn insert_cells(
    spatial: &mut BTreeMap<[i64; 3], BTreeSet<RenderEntity>>,
    entity: RenderEntity,
    cells: &[[i64; 3]],
) {
    for cell in cells {
        spatial.entry(*cell).or_default().insert(entity);
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], RenderWorldError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(RenderWorldError::InvalidPacket)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(RenderWorldError::InvalidPacket)?;
        self.offset = end;
        Ok(value)
    }

    fn u32(&mut self) -> Result<u32, RenderWorldError> {
        self.take(4)?
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| RenderWorldError::InvalidPacket)
    }

    fn u64(&mut self) -> Result<u64, RenderWorldError> {
        self.take(8)?
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| RenderWorldError::InvalidPacket)
    }

    fn i64(&mut self) -> Result<i64, RenderWorldError> {
        self.take(8)?
            .try_into()
            .map(i64::from_le_bytes)
            .map_err(|_| RenderWorldError::InvalidPacket)
    }

    fn bytes32(&mut self, max: usize) -> Result<&'a [u8], RenderWorldError> {
        let length = self.u32()? as usize;
        if length > max {
            return Err(RenderWorldError::ComponentByteCapacity);
        }
        self.take(length)
    }

    fn finish(self) -> Result<(), RenderWorldError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(RenderWorldError::InvalidPacket)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn entity(engine: EngineId, index: u32) -> RenderEntity {
        RenderEntity {
            engine,
            entity: handle(index),
        }
    }

    fn object(entity: RenderEntity, component_bytes: usize) -> RetainedRenderObject {
        RetainedRenderObject {
            entity,
            transform: TransformSample {
                previous_tick: 0,
                current_tick: 1,
                previous: [0; TRANSFORM_VALUES],
                current: [1; TRANSFORM_VALUES],
            },
            bounds: Aabb3 {
                min: [0, 0, 0],
                max: [1, 1, 1],
            },
            layers: 1,
            components: BTreeMap::from([(
                RenderComponentKind(1),
                RenderComponent {
                    kind: RenderComponentKind(1),
                    schema_hash: 7,
                    bytes: vec![9; component_bytes],
                },
            )]),
        }
    }

    #[test]
    fn incremental_capacity_failure_is_atomic_and_success_tracks_only_delta_work() {
        let engine = handle(0);
        let mut config = RenderWorldConfig::default();
        config.max_total_component_bytes = 10;
        let first = entity(engine, 0);
        let second = entity(engine, 1);
        let mut world = RenderWorld::new(engine, 1, config).unwrap();
        world
            .apply_delta(&RenderFrameDelta {
                revision: 1,
                full_snapshot: true,
                objects: vec![(
                    first,
                    encode_render_object(&object(first, 4), config).unwrap(),
                )],
                despawned: Vec::new(),
            })
            .unwrap();
        let before = world.instrumentation();
        assert_eq!(
            world.apply_delta(&RenderFrameDelta {
                revision: 2,
                full_snapshot: false,
                objects: vec![
                    (
                        first,
                        encode_render_object(&object(first, 6), config).unwrap(),
                    ),
                    (
                        second,
                        encode_render_object(&object(second, 6), config).unwrap(),
                    ),
                ],
                despawned: Vec::new(),
            }),
            Err(RenderWorldError::TotalByteCapacity)
        );
        assert_eq!(world.revision(), 1);
        assert_eq!(world.objects().len(), 1);
        assert_eq!(world.instrumentation(), before);

        world
            .apply_delta(&RenderFrameDelta {
                revision: 2,
                full_snapshot: false,
                objects: vec![(
                    first,
                    encode_render_object(&object(first, 5), config).unwrap(),
                )],
                despawned: Vec::new(),
            })
            .unwrap();
        let after = world.instrumentation();
        assert_eq!(after.last_delta_objects, 1);
        assert_eq!(after.objects, 1);
        assert_eq!(after.component_bytes, 5);
    }
}
