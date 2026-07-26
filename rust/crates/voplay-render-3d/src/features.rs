use std::collections::BTreeMap;

use voplay_runtime::{
    render_world::{RenderComponentKind, RenderWorld, ViewVisibility},
    RenderEntity,
};

pub const MESH_INSTANCE_COMPONENT: RenderComponentKind = RenderComponentKind(0x3001);
pub const LIGHT_COMPONENT: RenderComponentKind = RenderComponentKind(0x3002);
pub const CAMERA_COMPONENT: RenderComponentKind = RenderComponentKind(0x3003);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlphaMode {
    Opaque,
    Mask { cutoff_q16: u16 },
    Blend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaterialDescriptor {
    pub id: u64,
    pub pipeline_key: u64,
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub unlit: bool,
    pub base_color: [u8; 4],
    pub metallic_q16: u16,
    pub roughness_q16: u16,
    pub emissive_q16: [u16; 3],
    pub normal_scale_q16: u16,
    pub occlusion_strength_q16: u16,
    pub textures: [u64; 5],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshDescriptor {
    pub id: u64,
    pub vertex_count: u32,
    pub index_count: u32,
    pub skinned: bool,
}

pub fn encode_mesh_asset_3d(mesh: MeshDescriptor) -> Result<Vec<u8>, Render3dError> {
    if mesh.id == 0 || mesh.vertex_count == 0 || mesh.index_count == 0 {
        return Err(Render3dError::InvalidDescriptor);
    }
    let mut bytes = Vec::with_capacity(21);
    bytes.extend_from_slice(b"VMS1");
    bytes.extend_from_slice(&mesh.id.to_le_bytes());
    bytes.extend_from_slice(&mesh.vertex_count.to_le_bytes());
    bytes.extend_from_slice(&mesh.index_count.to_le_bytes());
    bytes.push(u8::from(mesh.skinned));
    Ok(bytes)
}

pub fn decode_mesh_asset_3d(bytes: &[u8]) -> Result<MeshDescriptor, Render3dError> {
    if bytes.len() != 21 || bytes.get(..4) != Some(b"VMS1") {
        return Err(Render3dError::InvalidDescriptor);
    }
    let mesh = MeshDescriptor {
        id: read_u64(bytes, 4)?,
        vertex_count: read_u32(bytes, 12)?,
        index_count: read_u32(bytes, 16)?,
        skinned: read_bool(bytes[20])?,
    };
    if mesh.id == 0 || mesh.vertex_count == 0 || mesh.index_count == 0 {
        return Err(Render3dError::InvalidDescriptor);
    }
    Ok(mesh)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LodMesh {
    pub mesh: u64,
    pub max_distance_milli: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeshInstanceDescriptor {
    pub material: u64,
    pub layers: u64,
    pub casts_shadow: bool,
    pub receives_shadow: bool,
    pub lods: Vec<LodMesh>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LightKind {
    Directional,
    Point,
    Spot { outer_angle_q16: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightDescriptor {
    pub kind: LightKind,
    pub color: [u16; 3],
    pub intensity_milli: u32,
    pub range_milli: u32,
    pub casts_shadow: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraProjection3d {
    Perspective {
        vertical_fov_millidegrees: u32,
        near_milli: u32,
        far_milli: u32,
    },
    Orthographic {
        vertical_height_milli: u32,
        near_milli: u32,
        far_milli: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraDescriptor3d {
    pub projection: CameraProjection3d,
    pub exposure_milli: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraFrame3d {
    pub entity: RenderEntity,
    pub descriptor: CameraDescriptor3d,
    pub position_milli: [i64; 3],
    pub forward_milli: [i64; 3],
    pub view_projection: [[f32; 4]; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DrawPacket3d {
    pub entity: RenderEntity,
    pub mesh: u64,
    pub material: u64,
    pub pipeline_key: u64,
    pub depth_key: i64,
    pub casts_shadow: bool,
    pub receives_shadow: bool,
    pub model_milli: [i64; 12],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightPacket3d {
    pub entity: RenderEntity,
    pub descriptor: LightDescriptor,
    pub position_milli: [i64; 3],
    pub direction_milli: [i64; 3],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Render3dPlan {
    pub world_revision: u64,
    pub view_revision: u64,
    pub opaque: Vec<DrawPacket3d>,
    pub transparent: Vec<DrawPacket3d>,
    pub shadow_casters: Vec<DrawPacket3d>,
    pub lights: Vec<LightPacket3d>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Render3dPlannerConfig {
    pub max_meshes: usize,
    pub max_materials: usize,
    pub max_draws: usize,
    pub max_lights: usize,
    pub max_lods_per_instance: usize,
}

impl Default for Render3dPlannerConfig {
    fn default() -> Self {
        Self {
            max_meshes: 1_000_000,
            max_materials: 1_000_000,
            max_draws: 1_000_000,
            max_lights: 16_384,
            max_lods_per_instance: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Render3dError {
    Closed,
    InvalidConfig,
    InvalidDescriptor,
    DuplicateAsset,
    Capacity,
    Unknown,
    MissingMesh,
    MissingMaterial,
    InvalidComponent,
    TickSequence,
    ArithmeticOverflow,
}

pub struct Render3dPlanner {
    config: Render3dPlannerConfig,
    meshes: BTreeMap<u64, MeshDescriptor>,
    materials: BTreeMap<u64, MaterialDescriptor>,
    closed: bool,
}

impl Render3dPlanner {
    pub fn new(config: Render3dPlannerConfig) -> Result<Self, Render3dError> {
        if config.max_meshes == 0
            || config.max_materials == 0
            || config.max_draws == 0
            || config.max_lights == 0
            || config.max_lods_per_instance == 0
        {
            return Err(Render3dError::InvalidConfig);
        }
        Ok(Self {
            config,
            meshes: BTreeMap::new(),
            materials: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> (usize, usize) {
        if self.closed {
            return (0, 0);
        }
        self.closed = true;
        let meshes = self.meshes.len();
        let materials = self.materials.len();
        self.meshes.clear();
        self.materials.clear();
        (meshes, materials)
    }

    pub fn register_mesh(&mut self, mesh: MeshDescriptor) -> Result<(), Render3dError> {
        self.ensure_open()?;
        if mesh.id == 0 || mesh.vertex_count == 0 || mesh.index_count == 0 {
            return Err(Render3dError::InvalidDescriptor);
        }
        if self.meshes.contains_key(&mesh.id) {
            return Err(Render3dError::DuplicateAsset);
        }
        if self.meshes.len() == self.config.max_meshes {
            return Err(Render3dError::Capacity);
        }
        self.meshes.insert(mesh.id, mesh);
        Ok(())
    }

    pub fn register_material(&mut self, material: MaterialDescriptor) -> Result<(), Render3dError> {
        self.ensure_open()?;
        if material.id == 0 || material.pipeline_key == 0 {
            return Err(Render3dError::InvalidDescriptor);
        }
        if self.materials.contains_key(&material.id) {
            return Err(Render3dError::DuplicateAsset);
        }
        if self.materials.len() == self.config.max_materials {
            return Err(Render3dError::Capacity);
        }
        self.materials.insert(material.id, material);
        Ok(())
    }

    pub fn build(
        &self,
        world: &RenderWorld,
        visibility: &ViewVisibility,
        camera_position_milli: [i64; 3],
        view_layers: u64,
    ) -> Result<Render3dPlan, Render3dError> {
        self.ensure_open()?;
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        let mut shadow_casters = Vec::new();
        let mut lights = Vec::new();
        for entity in &visibility.visible {
            let object = world
                .object(*entity)
                .ok_or(Render3dError::InvalidComponent)?;
            if let Some(component) = object.components.get(&MESH_INSTANCE_COMPONENT) {
                let instance = decode_mesh_instance(&component.bytes, self.config)?;
                if instance.layers & view_layers != 0 {
                    let position = object_position(object.transform.current);
                    let distance = distance_milli(position, camera_position_milli);
                    let mesh_id = select_lod(&instance.lods, distance)
                        .ok_or(Render3dError::InvalidComponent)?;
                    let mesh = self
                        .meshes
                        .get(&mesh_id)
                        .ok_or(Render3dError::MissingMesh)?;
                    let material = self
                        .materials
                        .get(&instance.material)
                        .ok_or(Render3dError::MissingMaterial)?;
                    if mesh.skinned && object.transform.current_tick == 0 {
                        return Err(Render3dError::InvalidComponent);
                    }
                    let packet = DrawPacket3d {
                        entity: *entity,
                        mesh: mesh.id,
                        material: material.id,
                        pipeline_key: material.pipeline_key,
                        depth_key: distance as i64,
                        casts_shadow: instance.casts_shadow,
                        receives_shadow: instance.receives_shadow,
                        model_milli: object.transform.current,
                    };
                    if material.alpha == AlphaMode::Blend {
                        transparent.push(packet);
                    } else {
                        opaque.push(packet);
                    }
                    if packet.casts_shadow {
                        shadow_casters.push(packet);
                    }
                }
            }
            if let Some(component) = object.components.get(&LIGHT_COMPONENT) {
                if lights.len() == self.config.max_lights {
                    return Err(Render3dError::Capacity);
                }
                lights.push(LightPacket3d {
                    entity: *entity,
                    descriptor: decode_light(&component.bytes)?,
                    position_milli: object_position(object.transform.current),
                    direction_milli: object_forward(object.transform.current),
                });
            }
        }
        if opaque.len().saturating_add(transparent.len()) > self.config.max_draws {
            return Err(Render3dError::Capacity);
        }
        opaque.sort_by_key(|packet| {
            (
                packet.pipeline_key,
                packet.material,
                packet.mesh,
                packet.entity,
            )
        });
        transparent.sort_by(|left, right| {
            right
                .depth_key
                .cmp(&left.depth_key)
                .then_with(|| left.pipeline_key.cmp(&right.pipeline_key))
                .then_with(|| left.entity.cmp(&right.entity))
        });
        shadow_casters.sort_by_key(|packet| (packet.pipeline_key, packet.mesh, packet.entity));
        lights.sort_by_key(|packet| packet.entity);
        Ok(Render3dPlan {
            world_revision: visibility.world_revision,
            view_revision: visibility.view_revision,
            opaque,
            transparent,
            shadow_casters,
            lights,
        })
    }

    fn ensure_open(&self) -> Result<(), Render3dError> {
        if self.closed {
            Err(Render3dError::Closed)
        } else {
            Ok(())
        }
    }
}

pub fn encode_mesh_instance(
    instance: &MeshInstanceDescriptor,
    max_lods: usize,
) -> Result<Vec<u8>, Render3dError> {
    validate_mesh_instance(instance, max_lods)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"VM31");
    bytes.extend_from_slice(&instance.material.to_le_bytes());
    bytes.extend_from_slice(&instance.layers.to_le_bytes());
    bytes.push(u8::from(instance.casts_shadow));
    bytes.push(u8::from(instance.receives_shadow));
    bytes.extend_from_slice(&(instance.lods.len() as u16).to_le_bytes());
    for lod in &instance.lods {
        bytes.extend_from_slice(&lod.mesh.to_le_bytes());
        bytes.extend_from_slice(&lod.max_distance_milli.to_le_bytes());
    }
    Ok(bytes)
}

pub fn decode_mesh_instance(
    bytes: &[u8],
    config: Render3dPlannerConfig,
) -> Result<MeshInstanceDescriptor, Render3dError> {
    if bytes.len() < 24 || &bytes[..4] != b"VM31" {
        return Err(Render3dError::InvalidComponent);
    }
    let material = read_u64(bytes, 4)?;
    let layers = read_u64(bytes, 12)?;
    let casts_shadow = read_bool(bytes[20])?;
    let receives_shadow = read_bool(bytes[21])?;
    let count = u16::from_le_bytes([bytes[22], bytes[23]]) as usize;
    if count > config.max_lods_per_instance || bytes.len() != 24 + count * 16 {
        return Err(Render3dError::InvalidComponent);
    }
    let mut lods = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 24 + index * 16;
        lods.push(LodMesh {
            mesh: read_u64(bytes, offset)?,
            max_distance_milli: read_u64(bytes, offset + 8)?,
        });
    }
    let instance = MeshInstanceDescriptor {
        material,
        layers,
        casts_shadow,
        receives_shadow,
        lods,
    };
    validate_mesh_instance(&instance, config.max_lods_per_instance)?;
    Ok(instance)
}

pub fn encode_light(light: LightDescriptor) -> Result<Vec<u8>, Render3dError> {
    if light.intensity_milli == 0 {
        return Err(Render3dError::InvalidDescriptor);
    }
    let mut bytes = Vec::from(b"VL31");
    match light.kind {
        LightKind::Directional => {
            bytes.push(1);
            bytes.extend_from_slice(&0_u16.to_le_bytes());
        }
        LightKind::Point => {
            bytes.push(2);
            bytes.extend_from_slice(&0_u16.to_le_bytes());
        }
        LightKind::Spot { outer_angle_q16 } if outer_angle_q16 != 0 => {
            bytes.push(3);
            bytes.extend_from_slice(&outer_angle_q16.to_le_bytes());
        }
        LightKind::Spot { .. } => return Err(Render3dError::InvalidDescriptor),
    }
    bytes.push(u8::from(light.casts_shadow));
    for color in light.color {
        bytes.extend_from_slice(&color.to_le_bytes());
    }
    bytes.extend_from_slice(&light.intensity_milli.to_le_bytes());
    bytes.extend_from_slice(&light.range_milli.to_le_bytes());
    Ok(bytes)
}

pub fn encode_camera(camera: CameraDescriptor3d) -> Result<Vec<u8>, Render3dError> {
    validate_camera(camera)?;
    let mut bytes = Vec::with_capacity(21);
    bytes.extend_from_slice(b"VC31");
    match camera.projection {
        CameraProjection3d::Perspective {
            vertical_fov_millidegrees,
            near_milli,
            far_milli,
        } => {
            bytes.push(1);
            bytes.extend_from_slice(&vertical_fov_millidegrees.to_le_bytes());
            bytes.extend_from_slice(&near_milli.to_le_bytes());
            bytes.extend_from_slice(&far_milli.to_le_bytes());
        }
        CameraProjection3d::Orthographic {
            vertical_height_milli,
            near_milli,
            far_milli,
        } => {
            bytes.push(2);
            bytes.extend_from_slice(&vertical_height_milli.to_le_bytes());
            bytes.extend_from_slice(&near_milli.to_le_bytes());
            bytes.extend_from_slice(&far_milli.to_le_bytes());
        }
    }
    bytes.extend_from_slice(&camera.exposure_milli.to_le_bytes());
    Ok(bytes)
}

pub fn decode_camera(bytes: &[u8]) -> Result<CameraDescriptor3d, Render3dError> {
    if bytes.len() != 21 || &bytes[..4] != b"VC31" {
        return Err(Render3dError::InvalidComponent);
    }
    let first = read_u32(bytes, 5)?;
    let near_milli = read_u32(bytes, 9)?;
    let far_milli = read_u32(bytes, 13)?;
    let exposure_milli = i32::from_le_bytes(
        bytes[17..21]
            .try_into()
            .map_err(|_| Render3dError::InvalidComponent)?,
    );
    let projection = match bytes[4] {
        1 => CameraProjection3d::Perspective {
            vertical_fov_millidegrees: first,
            near_milli,
            far_milli,
        },
        2 => CameraProjection3d::Orthographic {
            vertical_height_milli: first,
            near_milli,
            far_milli,
        },
        _ => return Err(Render3dError::InvalidComponent),
    };
    let camera = CameraDescriptor3d {
        projection,
        exposure_milli,
    };
    validate_camera(camera)?;
    Ok(camera)
}

pub fn build_camera_frame(
    world: &RenderWorld,
    camera: RenderEntity,
    viewport_width: u32,
    viewport_height: u32,
) -> Result<CameraFrame3d, Render3dError> {
    if camera.engine != world.engine() || viewport_width == 0 || viewport_height == 0 {
        return Err(Render3dError::InvalidDescriptor);
    }
    let object = world
        .object(camera)
        .ok_or(Render3dError::InvalidComponent)?;
    let descriptor = object
        .components
        .get(&CAMERA_COMPONENT)
        .ok_or(Render3dError::InvalidComponent)
        .and_then(|component| decode_camera(&component.bytes))?;
    let view = rigid_view_matrix(object.transform.current)?;
    let projection = projection_matrix(descriptor.projection, viewport_width, viewport_height)?;
    Ok(CameraFrame3d {
        entity: camera,
        descriptor,
        position_milli: object_position(object.transform.current),
        forward_milli: object_forward(object.transform.current),
        view_projection: multiply_matrix(projection, view),
    })
}

fn validate_camera(camera: CameraDescriptor3d) -> Result<(), Render3dError> {
    if !(-32_000..=32_000).contains(&camera.exposure_milli) {
        return Err(Render3dError::InvalidDescriptor);
    }
    let valid = match camera.projection {
        CameraProjection3d::Perspective {
            vertical_fov_millidegrees,
            near_milli,
            far_milli,
        } => {
            (1_000..179_000).contains(&vertical_fov_millidegrees)
                && near_milli != 0
                && far_milli > near_milli
        }
        CameraProjection3d::Orthographic {
            vertical_height_milli,
            near_milli,
            far_milli,
        } => vertical_height_milli != 0 && far_milli > near_milli,
    };
    valid.then_some(()).ok_or(Render3dError::InvalidDescriptor)
}

fn projection_matrix(
    projection: CameraProjection3d,
    width: u32,
    height: u32,
) -> Result<[[f32; 4]; 4], Render3dError> {
    let aspect = width as f32 / height as f32;
    if !aspect.is_finite() || aspect <= 0.0 {
        return Err(Render3dError::InvalidDescriptor);
    }
    let matrix = match projection {
        CameraProjection3d::Perspective {
            vertical_fov_millidegrees,
            near_milli,
            far_milli,
        } => {
            let radians = (vertical_fov_millidegrees as f32 / 1_000.0).to_radians();
            let focal = 1.0 / (radians * 0.5).tan();
            let near = near_milli as f32 / 1_000.0;
            let far = far_milli as f32 / 1_000.0;
            [
                [focal / aspect, 0.0, 0.0, 0.0],
                [0.0, focal, 0.0, 0.0],
                [0.0, 0.0, far / (near - far), -1.0],
                [0.0, 0.0, near * far / (near - far), 0.0],
            ]
        }
        CameraProjection3d::Orthographic {
            vertical_height_milli,
            near_milli,
            far_milli,
        } => {
            let height = vertical_height_milli as f32 / 1_000.0;
            let width = height * aspect;
            let near = near_milli as f32 / 1_000.0;
            let far = far_milli as f32 / 1_000.0;
            [
                [2.0 / width, 0.0, 0.0, 0.0],
                [0.0, 2.0 / height, 0.0, 0.0],
                [0.0, 0.0, 1.0 / (near - far), 0.0],
                [0.0, 0.0, near / (near - far), 1.0],
            ]
        }
    };
    matrix
        .iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(matrix)
        .ok_or(Render3dError::ArithmeticOverflow)
}

fn rigid_view_matrix(transform: [i64; 12]) -> Result<[[f32; 4]; 4], Render3dError> {
    let scale = |value: i64| value as f32 / 1_000.0;
    let rotation = [
        [
            scale(transform[0]),
            scale(transform[1]),
            scale(transform[2]),
        ],
        [
            scale(transform[4]),
            scale(transform[5]),
            scale(transform[6]),
        ],
        [
            scale(transform[8]),
            scale(transform[9]),
            scale(transform[10]),
        ],
    ];
    for row in &rotation {
        let length = row.iter().map(|value| value * value).sum::<f32>();
        if !length.is_finite() || (length - 1.0).abs() > 0.02 {
            return Err(Render3dError::InvalidComponent);
        }
    }
    for left in 0..3 {
        for right in left + 1..3 {
            let dot = (0..3)
                .map(|axis| rotation[left][axis] * rotation[right][axis])
                .sum::<f32>();
            if dot.abs() > 0.02 {
                return Err(Render3dError::InvalidComponent);
            }
        }
    }
    let position = [
        scale(transform[3]),
        scale(transform[7]),
        scale(transform[11]),
    ];
    let translation = [
        -(rotation[0][0] * position[0]
            + rotation[1][0] * position[1]
            + rotation[2][0] * position[2]),
        -(rotation[0][1] * position[0]
            + rotation[1][1] * position[1]
            + rotation[2][1] * position[2]),
        -(rotation[0][2] * position[0]
            + rotation[1][2] * position[1]
            + rotation[2][2] * position[2]),
    ];
    Ok([
        [rotation[0][0], rotation[0][1], rotation[0][2], 0.0],
        [rotation[1][0], rotation[1][1], rotation[1][2], 0.0],
        [rotation[2][0], rotation[2][1], rotation[2][2], 0.0],
        [translation[0], translation[1], translation[2], 1.0],
    ])
}

fn multiply_matrix(left: [[f32; 4]; 4], right: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            result[column][row] = (0..4)
                .map(|axis| left[axis][row] * right[column][axis])
                .sum();
        }
    }
    result
}

pub fn decode_light(bytes: &[u8]) -> Result<LightDescriptor, Render3dError> {
    if bytes.len() != 22 || &bytes[..4] != b"VL31" {
        return Err(Render3dError::InvalidComponent);
    }
    let angle = u16::from_le_bytes([bytes[5], bytes[6]]);
    let kind = match bytes[4] {
        1 if angle == 0 => LightKind::Directional,
        2 if angle == 0 => LightKind::Point,
        3 if angle != 0 => LightKind::Spot {
            outer_angle_q16: angle,
        },
        _ => return Err(Render3dError::InvalidComponent),
    };
    let descriptor = LightDescriptor {
        kind,
        casts_shadow: read_bool(bytes[7])?,
        color: [
            u16::from_le_bytes([bytes[8], bytes[9]]),
            u16::from_le_bytes([bytes[10], bytes[11]]),
            u16::from_le_bytes([bytes[12], bytes[13]]),
        ],
        intensity_milli: read_u32(bytes, 14)?,
        range_milli: read_u32(bytes, 18)?,
    };
    if descriptor.intensity_milli == 0 {
        return Err(Render3dError::InvalidComponent);
    }
    Ok(descriptor)
}

fn validate_mesh_instance(
    instance: &MeshInstanceDescriptor,
    max_lods: usize,
) -> Result<(), Render3dError> {
    if instance.material == 0
        || instance.layers == 0
        || instance.lods.is_empty()
        || instance.lods.len() > max_lods
    {
        return Err(Render3dError::InvalidDescriptor);
    }
    let mut previous = 0;
    for lod in &instance.lods {
        if lod.mesh == 0 || lod.max_distance_milli <= previous {
            return Err(Render3dError::InvalidDescriptor);
        }
        previous = lod.max_distance_milli;
    }
    Ok(())
}

fn select_lod(lods: &[LodMesh], distance: u64) -> Option<u64> {
    lods.iter()
        .find(|lod| distance <= lod.max_distance_milli)
        .or_else(|| lods.last())
        .map(|lod| lod.mesh)
}

fn object_position(transform: [i64; 12]) -> [i64; 3] {
    [transform[3], transform[7], transform[11]]
}

fn object_forward(transform: [i64; 12]) -> [i64; 3] {
    [
        transform[2].saturating_neg(),
        transform[6].saturating_neg(),
        transform[10].saturating_neg(),
    ]
}

fn distance_milli(left: [i64; 3], right: [i64; 3]) -> u64 {
    let squared = (0..3).fold(0_u128, |sum, axis| {
        let delta = i128::from(left[axis]) - i128::from(right[axis]);
        sum.saturating_add(delta.unsigned_abs().saturating_pow(2))
    });
    integer_sqrt(squared).min(u128::from(u64::MAX)) as u64
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut low = 1;
    let mut high = value.min(u128::from(u64::MAX)) + 1;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if middle <= value / middle {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

fn read_bool(value: u8) -> Result<bool, Render3dError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Render3dError::InvalidComponent),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Render3dError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(Render3dError::InvalidComponent)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Render3dError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|value| value.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(Render3dError::InvalidComponent)
}
