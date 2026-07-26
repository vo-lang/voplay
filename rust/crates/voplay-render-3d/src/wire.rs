use std::collections::BTreeMap;

use crate::{AlphaMode, MaterialDescriptor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireAlphaMode3d {
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireMaterial3d {
    pub id: u64,
    pub base_material: u64,
    pub revision: u64,
    pub alpha: WireAlphaMode3d,
    pub double_sided: bool,
    pub unlit: bool,
    pub base_color: [u16; 4],
    pub metallic: u16,
    pub roughness: u16,
    pub emissive: [u16; 3],
    pub alpha_cutoff: u16,
    pub textures: [u64; 5],
}

impl WireMaterial3d {
    pub fn material_descriptor(self, pipeline_key: u64) -> Result<MaterialDescriptor, Wire3dError> {
        if pipeline_key == 0 {
            return Err(Wire3dError::InvalidDescriptor);
        }
        Ok(MaterialDescriptor {
            id: self.id,
            pipeline_key,
            alpha: match self.alpha {
                WireAlphaMode3d::Opaque => AlphaMode::Opaque,
                WireAlphaMode3d::Mask => AlphaMode::Mask {
                    cutoff_q16: self.alpha_cutoff,
                },
                WireAlphaMode3d::Blend => AlphaMode::Blend,
            },
            double_sided: self.double_sided,
            unlit: self.unlit,
            base_color: self.base_color.map(q16_to_u8),
            metallic_q16: self.metallic,
            roughness_q16: self.roughness,
            emissive_q16: self.emissive,
            normal_scale_q16: u16::MAX,
            occlusion_strength_q16: u16::MAX,
            textures: self.textures,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireTerrainPatch3d {
    pub id: u64,
    pub heightmap: u64,
    pub material: u64,
    pub origin_milli: [i64; 3],
    pub size: [u32; 2],
    pub height_scale_milli: u32,
    pub lod: u32,
    pub neighbors: [u32; 4],
    pub layers: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireDecalComposition3d {
    BeforeLighting,
    AfterLighting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireDecal3d {
    pub id: u64,
    pub material: u64,
    pub transform_milli: [i64; 16],
    pub composition: WireDecalComposition3d,
    pub layers: u64,
    pub fade_start_milli: u64,
    pub fade_end_milli: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireScatterField3d {
    pub id: u64,
    pub seed: u64,
    pub mesh: u64,
    pub material: u64,
    pub bounds_milli: [i64; 4],
    pub density_per_million: u32,
    pub min_scale_milli: u32,
    pub max_scale_milli: u32,
    pub max_instances: u32,
    pub layers: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireScatterInstance3d {
    pub field: u64,
    pub serial: u32,
    pub position_milli: [i64; 3],
    pub yaw_q16: u16,
    pub scale_milli: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireParticleEmitter3d {
    pub id: u64,
    pub seed: u64,
    pub mesh: u64,
    pub material: u64,
    pub max_particles: u32,
    pub spawn_per_tick: u32,
    pub lifetime_ticks: u32,
    pub position_milli: [i64; 3],
    pub velocity_min_milli: [i64; 3],
    pub velocity_max_milli: [i64; 3],
    pub start_scale_milli: u32,
    pub end_scale_milli: u32,
    pub layers: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireParticle3d {
    pub emitter: u64,
    pub serial: u64,
    pub age: u32,
    pub lifetime: u32,
    pub position_milli: [i64; 3],
    pub velocity_milli: [i64; 3],
    pub scale_milli: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PortableFeaturePhase3d {
    Terrain,
    Scatter,
    DecalBeforeLighting,
    Particle,
    DecalAfterLighting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableFeatureDraw3d {
    pub source: u64,
    pub serial: u64,
    pub phase: PortableFeaturePhase3d,
    pub mesh: u64,
    pub material: u64,
    pub model_milli: [i64; 12],
    pub casts_shadow: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortableFeaturePlan3d {
    pub scene_revision: u64,
    pub draws: Vec<PortableFeatureDraw3d>,
    pub environment: Option<WireEnvironment3d>,
}

pub trait PortableFeatureResolver3d {
    fn terrain_draw(&mut self, patch: &WireTerrainPatch3d)
        -> Result<(u64, [i64; 12]), Wire3dError>;

    fn decal_draw(&mut self, decal: &WireDecal3d) -> Result<Option<(u64, [i64; 12])>, Wire3dError>;
}

pub const BUILTIN_TERRAIN_MESH_3D: u64 = u64::MAX - 1;
pub const BUILTIN_DECAL_MESH_3D: u64 = u64::MAX - 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireToneMap3d {
    Linear,
    Reinhard,
    Aces,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireAntiAlias3d {
    None,
    Fxaa,
    Taa,
    Msaa,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireEnvironment3d {
    pub fog_color: [u16; 3],
    pub fog_start_milli: u64,
    pub fog_end_milli: u64,
    pub exposure_milli: i32,
    pub tone_map: WireToneMap3d,
    pub bloom_threshold: u16,
    pub bloom_strength: u16,
    pub anti_alias: WireAntiAlias3d,
    pub msaa_samples: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wire3dError {
    Closed,
    InvalidMagic,
    Truncated,
    TrailingBytes,
    InvalidDescriptor,
    Capacity,
    ArithmeticOverflow,
    StaleRevision,
}

pub fn decode_material_3d(bytes: &[u8]) -> Result<WireMaterial3d, Wire3dError> {
    let mut reader = Reader::descriptor(bytes, b"VA31", 91)?;
    let id = reader.u64()?;
    let base_material = reader.u64()?;
    let revision = reader.u64()?;
    let alpha = match reader.u8()? {
        1 => WireAlphaMode3d::Opaque,
        2 => WireAlphaMode3d::Mask,
        3 => WireAlphaMode3d::Blend,
        _ => return Err(Wire3dError::InvalidDescriptor),
    };
    let double_sided = reader.boolean()?;
    let unlit = reader.boolean()?;
    let base_color = reader.u16_array()?;
    let metallic = reader.u16()?;
    let roughness = reader.u16()?;
    let emissive = reader.u16_array()?;
    let alpha_cutoff = reader.u16()?;
    let textures = reader.u64_array()?;
    reader.finish()?;
    if id == 0 || revision == 0 || (alpha == WireAlphaMode3d::Mask) != (alpha_cutoff != 0) {
        return Err(Wire3dError::InvalidDescriptor);
    }
    Ok(WireMaterial3d {
        id,
        base_material,
        revision,
        alpha,
        double_sided,
        unlit,
        base_color,
        metallic,
        roughness,
        emissive,
        alpha_cutoff,
        textures,
    })
}

pub fn encode_material_3d(material: WireMaterial3d) -> Result<Vec<u8>, Wire3dError> {
    if material.id == 0
        || material.revision == 0
        || (material.alpha == WireAlphaMode3d::Mask) != (material.alpha_cutoff != 0)
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    let mut bytes = Vec::with_capacity(91);
    bytes.extend_from_slice(b"VA31");
    bytes.extend_from_slice(&material.id.to_le_bytes());
    bytes.extend_from_slice(&material.base_material.to_le_bytes());
    bytes.extend_from_slice(&material.revision.to_le_bytes());
    bytes.push(match material.alpha {
        WireAlphaMode3d::Opaque => 1,
        WireAlphaMode3d::Mask => 2,
        WireAlphaMode3d::Blend => 3,
    });
    bytes.push(u8::from(material.double_sided));
    bytes.push(u8::from(material.unlit));
    for value in material.base_color {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&material.metallic.to_le_bytes());
    bytes.extend_from_slice(&material.roughness.to_le_bytes());
    for value in material.emissive {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&material.alpha_cutoff.to_le_bytes());
    for texture in material.textures {
        bytes.extend_from_slice(&texture.to_le_bytes());
    }
    debug_assert_eq!(bytes.len(), 91);
    Ok(bytes)
}

pub fn decode_terrain_3d(bytes: &[u8]) -> Result<WireTerrainPatch3d, Wire3dError> {
    let mut reader = Reader::descriptor(bytes, b"VR31", 92)?;
    let patch = WireTerrainPatch3d {
        id: reader.u64()?,
        heightmap: reader.u64()?,
        material: reader.u64()?,
        origin_milli: reader.i64_array()?,
        size: reader.u32_array()?,
        height_scale_milli: reader.u32()?,
        lod: reader.u32()?,
        neighbors: reader.u32_array()?,
        layers: reader.u64()?,
    };
    reader.finish()?;
    if patch.id == 0
        || patch.heightmap == 0
        || patch.material == 0
        || patch.size.contains(&0)
        || patch.height_scale_milli == 0
        || patch.lod > 31
        || patch.layers == 0
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    Ok(patch)
}

pub fn decode_decal_3d(bytes: &[u8]) -> Result<WireDecal3d, Wire3dError> {
    let mut reader = Reader::descriptor(bytes, b"VD31", 173)?;
    let composition = match reader.u8()? {
        1 => WireDecalComposition3d::BeforeLighting,
        2 => WireDecalComposition3d::AfterLighting,
        _ => return Err(Wire3dError::InvalidDescriptor),
    };
    let id = reader.u64()?;
    let material = reader.u64()?;
    let layers = reader.u64()?;
    let fade_start_milli = reader.u64()?;
    let fade_end_milli = reader.u64()?;
    let transform_milli = reader.i64_array()?;
    reader.finish()?;
    if id == 0
        || material == 0
        || layers == 0
        || (fade_end_milli != 0 && fade_end_milli <= fade_start_milli)
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    Ok(WireDecal3d {
        id,
        material,
        transform_milli,
        composition,
        layers,
        fade_start_milli,
        fade_end_milli,
    })
}

pub fn decode_scatter_3d(bytes: &[u8]) -> Result<WireScatterField3d, Wire3dError> {
    let mut reader = Reader::descriptor(bytes, b"VS31", 92)?;
    let field = WireScatterField3d {
        id: reader.u64()?,
        seed: reader.u64()?,
        mesh: reader.u64()?,
        material: reader.u64()?,
        bounds_milli: reader.i64_array()?,
        density_per_million: reader.u32()?,
        min_scale_milli: reader.u32()?,
        max_scale_milli: reader.u32()?,
        max_instances: reader.u32()?,
        layers: reader.u64()?,
    };
    reader.finish()?;
    if field.id == 0
        || field.seed == 0
        || field.mesh == 0
        || field.material == 0
        || field.bounds_milli[2] <= 0
        || field.bounds_milli[3] <= 0
        || !(1..=1_000_000).contains(&field.density_per_million)
        || field.min_scale_milli == 0
        || field.max_scale_milli < field.min_scale_milli
        || !(1..=10_000_000).contains(&field.max_instances)
        || field.layers == 0
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    Ok(field)
}

pub fn decode_particle_emitter_3d(bytes: &[u8]) -> Result<WireParticleEmitter3d, Wire3dError> {
    let mut reader = Reader::descriptor(bytes, b"VP31", 136)?;
    let emitter = WireParticleEmitter3d {
        id: reader.u64()?,
        seed: reader.u64()?,
        mesh: reader.u64()?,
        material: reader.u64()?,
        max_particles: reader.u32()?,
        spawn_per_tick: reader.u32()?,
        lifetime_ticks: reader.u32()?,
        position_milli: reader.i64_array()?,
        velocity_min_milli: reader.i64_array()?,
        velocity_max_milli: reader.i64_array()?,
        start_scale_milli: reader.u32()?,
        end_scale_milli: reader.u32()?,
        layers: reader.u64()?,
    };
    reader.finish()?;
    if emitter.id == 0
        || emitter.seed == 0
        || emitter.mesh == 0
        || emitter.material == 0
        || !(1..=1_000_000).contains(&emitter.max_particles)
        || emitter.spawn_per_tick == 0
        || emitter.spawn_per_tick > emitter.max_particles
        || emitter.lifetime_ticks == 0
        || emitter.start_scale_milli == 0
        || emitter.end_scale_milli == 0
        || emitter.layers == 0
        || (0..3).any(|axis| emitter.velocity_min_milli[axis] > emitter.velocity_max_milli[axis])
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    Ok(emitter)
}

pub fn decode_environment_3d(bytes: &[u8]) -> Result<WireEnvironment3d, Wire3dError> {
    let mut reader = Reader::descriptor(bytes, b"VE31", 40)?;
    let tone_map = match reader.u8()? {
        1 => WireToneMap3d::Linear,
        2 => WireToneMap3d::Reinhard,
        3 => WireToneMap3d::Aces,
        _ => return Err(Wire3dError::InvalidDescriptor),
    };
    let anti_alias = match reader.u8()? {
        1 => WireAntiAlias3d::None,
        2 => WireAntiAlias3d::Fxaa,
        3 => WireAntiAlias3d::Taa,
        4 => WireAntiAlias3d::Msaa,
        _ => return Err(Wire3dError::InvalidDescriptor),
    };
    let environment = WireEnvironment3d {
        fog_color: reader.u16_array()?,
        fog_start_milli: reader.u64()?,
        fog_end_milli: reader.u64()?,
        exposure_milli: reader.i32()?,
        tone_map,
        bloom_threshold: reader.u16()?,
        bloom_strength: reader.u16()?,
        anti_alias,
        msaa_samples: reader.u32()?,
    };
    reader.finish()?;
    if (environment.fog_end_milli != 0 && environment.fog_end_milli <= environment.fog_start_milli)
        || match environment.anti_alias {
            WireAntiAlias3d::Msaa => !matches!(environment.msaa_samples, 2 | 4 | 8),
            _ => environment.msaa_samples != 1,
        }
    {
        return Err(Wire3dError::InvalidDescriptor);
    }
    Ok(environment)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableScene3dConfig {
    pub max_materials: usize,
    pub max_terrain_patches: usize,
    pub max_decals: usize,
    pub max_scatter_fields: usize,
    pub max_scatter_instances: usize,
    pub max_particle_emitters: usize,
    pub max_particles: usize,
}

impl Default for PortableScene3dConfig {
    fn default() -> Self {
        Self {
            max_materials: 1_000_000,
            max_terrain_patches: 16_384,
            max_decals: 65_536,
            max_scatter_fields: 16_384,
            max_scatter_instances: 1_000_000,
            max_particle_emitters: 16_384,
            max_particles: 1_000_000,
        }
    }
}

#[derive(Clone)]
pub struct PortableScene3d {
    config: PortableScene3dConfig,
    materials: BTreeMap<u64, WireMaterial3d>,
    terrain: BTreeMap<u64, WireTerrainPatch3d>,
    decals: BTreeMap<u64, WireDecal3d>,
    scatter: BTreeMap<u64, WireScatterField3d>,
    particle_emitters: BTreeMap<u64, WireParticleEmitter3d>,
    scatter_instances: BTreeMap<u64, Vec<WireScatterInstance3d>>,
    particles: Vec<WireParticle3d>,
    environment: Option<WireEnvironment3d>,
    last_particle_tick: u64,
    next_particle_serial: u64,
    revision: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortableScene3dOwnerSnapshot {
    pub closed: bool,
    pub materials: usize,
    pub terrain_patches: usize,
    pub decals: usize,
    pub scatter_fields: usize,
    pub scatter_instances: usize,
    pub particle_emitters: usize,
    pub particles: usize,
}

impl PortableScene3d {
    pub fn new(config: PortableScene3dConfig) -> Result<Self, Wire3dError> {
        if config.max_materials == 0
            || config.max_terrain_patches == 0
            || config.max_decals == 0
            || config.max_scatter_fields == 0
            || config.max_scatter_instances == 0
            || config.max_particle_emitters == 0
            || config.max_particles == 0
        {
            return Err(Wire3dError::InvalidDescriptor);
        }
        Ok(Self {
            config,
            materials: BTreeMap::new(),
            terrain: BTreeMap::new(),
            decals: BTreeMap::new(),
            scatter: BTreeMap::new(),
            particle_emitters: BTreeMap::new(),
            scatter_instances: BTreeMap::new(),
            particles: Vec::new(),
            environment: None,
            last_particle_tick: 0,
            next_particle_serial: 1,
            revision: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> PortableScene3dOwnerSnapshot {
        PortableScene3dOwnerSnapshot {
            closed: self.closed,
            materials: self.materials.len(),
            terrain_patches: self.terrain.len(),
            decals: self.decals.len(),
            scatter_fields: self.scatter.len(),
            scatter_instances: self.scatter_instances.values().map(Vec::len).sum(),
            particle_emitters: self.particle_emitters.len(),
            particles: self.particles.len(),
        }
    }

    pub fn shutdown(&mut self) -> PortableScene3dOwnerSnapshot {
        let before = self.owner_snapshot();
        self.closed = true;
        self.materials.clear();
        self.terrain.clear();
        self.decals.clear();
        self.scatter.clear();
        self.particle_emitters.clear();
        self.scatter_instances.clear();
        self.particles.clear();
        self.environment = None;
        before
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn material(&self, id: u64) -> Option<&WireMaterial3d> {
        self.materials.get(&id)
    }

    pub fn terrain(&self, id: u64) -> Option<&WireTerrainPatch3d> {
        self.terrain.get(&id)
    }

    pub fn terrain_patches(&self) -> impl Iterator<Item = &WireTerrainPatch3d> {
        self.terrain.values()
    }

    pub fn decal(&self, id: u64) -> Option<&WireDecal3d> {
        self.decals.get(&id)
    }

    pub fn decals(&self) -> impl Iterator<Item = &WireDecal3d> {
        self.decals.values()
    }

    pub fn scatter(&self, id: u64) -> Option<&WireScatterField3d> {
        self.scatter.get(&id)
    }

    pub fn particle_emitter(&self, id: u64) -> Option<&WireParticleEmitter3d> {
        self.particle_emitters.get(&id)
    }

    pub fn scatter_instances(&self, field: u64) -> Option<&[WireScatterInstance3d]> {
        self.scatter_instances.get(&field).map(Vec::as_slice)
    }

    pub fn particles(&self) -> &[WireParticle3d] {
        &self.particles
    }

    pub fn compose_features(
        &self,
        layers: u64,
        resolver: &mut impl PortableFeatureResolver3d,
    ) -> Result<PortableFeaturePlan3d, Wire3dError> {
        self.ensure_open()?;
        if layers == 0 {
            return Err(Wire3dError::InvalidDescriptor);
        }
        let mut draws = Vec::new();
        for patch in self.terrain.values() {
            if patch.layers & layers == 0 {
                continue;
            }
            require_material(&self.materials, patch.material)?;
            let (mesh, model_milli) = resolver.terrain_draw(patch)?;
            if mesh == 0 {
                return Err(Wire3dError::InvalidDescriptor);
            }
            draws.push(PortableFeatureDraw3d {
                source: patch.id,
                serial: 0,
                phase: PortableFeaturePhase3d::Terrain,
                mesh,
                material: patch.material,
                model_milli,
                casts_shadow: true,
            });
        }
        for (field_id, field) in &self.scatter {
            if field.layers & layers == 0 {
                continue;
            }
            require_material(&self.materials, field.material)?;
            for instance in self
                .scatter_instances
                .get(field_id)
                .ok_or(Wire3dError::InvalidDescriptor)?
            {
                draws.push(PortableFeatureDraw3d {
                    source: field.id,
                    serial: u64::from(instance.serial),
                    phase: PortableFeaturePhase3d::Scatter,
                    mesh: field.mesh,
                    material: field.material,
                    model_milli: instance_model(
                        instance.position_milli,
                        instance.scale_milli,
                        instance.yaw_q16,
                    ),
                    casts_shadow: true,
                });
            }
        }
        for decal in self.decals.values() {
            if decal.layers & layers == 0 {
                continue;
            }
            require_material(&self.materials, decal.material)?;
            let Some((mesh, model_milli)) = resolver.decal_draw(decal)? else {
                continue;
            };
            if mesh == 0 {
                return Err(Wire3dError::InvalidDescriptor);
            }
            draws.push(PortableFeatureDraw3d {
                source: decal.id,
                serial: 0,
                phase: match decal.composition {
                    WireDecalComposition3d::BeforeLighting => {
                        PortableFeaturePhase3d::DecalBeforeLighting
                    }
                    WireDecalComposition3d::AfterLighting => {
                        PortableFeaturePhase3d::DecalAfterLighting
                    }
                },
                mesh,
                material: decal.material,
                model_milli,
                casts_shadow: false,
            });
        }
        for particle in &self.particles {
            let emitter = self
                .particle_emitters
                .get(&particle.emitter)
                .ok_or(Wire3dError::InvalidDescriptor)?;
            if emitter.layers & layers == 0 {
                continue;
            }
            require_material(&self.materials, emitter.material)?;
            draws.push(PortableFeatureDraw3d {
                source: emitter.id,
                serial: particle.serial,
                phase: PortableFeaturePhase3d::Particle,
                mesh: emitter.mesh,
                material: emitter.material,
                model_milli: instance_model(particle.position_milli, particle.scale_milli, 0),
                casts_shadow: false,
            });
        }
        draws.sort_by_key(|draw| {
            (
                draw.phase,
                draw.material,
                draw.mesh,
                draw.source,
                draw.serial,
            )
        });
        Ok(PortableFeaturePlan3d {
            scene_revision: self.revision,
            draws,
            environment: self.environment,
        })
    }

    pub const fn environment(&self) -> Option<WireEnvironment3d> {
        self.environment
    }

    pub fn apply_material(&mut self, bytes: &[u8]) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        let value = decode_material_3d(bytes)?;
        if self
            .materials
            .get(&value.id)
            .is_some_and(|current| current.revision >= value.revision)
        {
            return Err(Wire3dError::StaleRevision);
        }
        insert(
            &mut self.materials,
            value.id,
            value,
            self.config.max_materials,
        )?;
        self.bump()
    }

    pub fn apply_terrain(&mut self, bytes: &[u8]) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        let value = decode_terrain_3d(bytes)?;
        insert(
            &mut self.terrain,
            value.id,
            value,
            self.config.max_terrain_patches,
        )?;
        self.bump()
    }

    pub fn apply_decal(&mut self, bytes: &[u8]) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        let value = decode_decal_3d(bytes)?;
        insert(&mut self.decals, value.id, value, self.config.max_decals)?;
        self.bump()
    }

    pub fn apply_scatter(&mut self, bytes: &[u8]) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        let value = decode_scatter_3d(bytes)?;
        let replaced = self.scatter_instances.get(&value.id).map_or(0, Vec::len);
        let occupied = self
            .scatter_instances
            .values()
            .try_fold(0_usize, |total, instances| {
                total.checked_add(instances.len())
            })
            .and_then(|total| total.checked_sub(replaced))
            .ok_or(Wire3dError::ArithmeticOverflow)?;
        let available = self
            .config
            .max_scatter_instances
            .checked_sub(occupied)
            .ok_or(Wire3dError::Capacity)?;
        let instances = build_scatter_instances(value, available)?;
        insert(
            &mut self.scatter,
            value.id,
            value,
            self.config.max_scatter_fields,
        )?;
        self.scatter_instances.insert(value.id, instances);
        self.bump()
    }

    pub fn apply_particle_emitter(&mut self, bytes: &[u8]) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        let value = decode_particle_emitter_3d(bytes)?;
        insert(
            &mut self.particle_emitters,
            value.id,
            value,
            self.config.max_particle_emitters,
        )?;
        self.particles
            .retain(|particle| particle.emitter != value.id);
        self.bump()
    }

    pub fn apply_environment(&mut self, bytes: &[u8]) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        self.environment = Some(decode_environment_3d(bytes)?);
        self.bump()
    }

    pub fn remove_material(&mut self, id: u64) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        remove(&mut self.materials, id)?;
        self.bump()
    }

    pub fn remove_terrain(&mut self, id: u64) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        remove(&mut self.terrain, id)?;
        self.bump()
    }

    pub fn remove_decal(&mut self, id: u64) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        remove(&mut self.decals, id)?;
        self.bump()
    }

    pub fn remove_scatter(&mut self, id: u64) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        remove(&mut self.scatter, id)?;
        self.scatter_instances.remove(&id);
        self.bump()
    }

    pub fn remove_particle_emitter(&mut self, id: u64) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        remove(&mut self.particle_emitters, id)?;
        self.particles.retain(|particle| particle.emitter != id);
        self.bump()
    }

    pub fn tick_particles(&mut self, tick: u64) -> Result<&[WireParticle3d], Wire3dError> {
        self.ensure_open()?;
        if self.last_particle_tick.checked_add(1) != Some(tick) {
            return Err(Wire3dError::InvalidDescriptor);
        }
        let mut particles = Vec::with_capacity(
            self.config.max_particles.min(
                self.particles
                    .len()
                    .saturating_add(self.particle_emitters.len()),
            ),
        );
        for particle in &self.particles {
            let age = particle
                .age
                .checked_add(1)
                .ok_or(Wire3dError::ArithmeticOverflow)?;
            if age >= particle.lifetime {
                continue;
            }
            let emitter = self
                .particle_emitters
                .get(&particle.emitter)
                .ok_or(Wire3dError::InvalidDescriptor)?;
            let mut next = *particle;
            next.age = age;
            for axis in 0..3 {
                next.position_milli[axis] = next.position_milli[axis]
                    .checked_add(next.velocity_milli[axis])
                    .ok_or(Wire3dError::ArithmeticOverflow)?;
            }
            next.scale_milli = interpolate_u32(
                emitter.start_scale_milli,
                emitter.end_scale_milli,
                age,
                particle.lifetime,
            );
            particles.push(next);
        }
        let mut next_serial = self.next_particle_serial;
        for emitter in self.particle_emitters.values() {
            let alive = particles
                .iter()
                .filter(|particle| particle.emitter == emitter.id)
                .count();
            let available = usize::try_from(emitter.max_particles)
                .unwrap_or(usize::MAX)
                .saturating_sub(alive);
            for ordinal in 0..usize::try_from(emitter.spawn_per_tick)
                .unwrap_or(usize::MAX)
                .min(available)
            {
                if particles.len() == self.config.max_particles {
                    break;
                }
                let random = split_mix64(emitter.seed ^ tick.rotate_left(19) ^ ordinal as u64);
                let mut velocity_milli = [0; 3];
                for axis in 0..3 {
                    velocity_milli[axis] = random_i64_inclusive(
                        random.rotate_left((axis * 17) as u32),
                        emitter.velocity_min_milli[axis],
                        emitter.velocity_max_milli[axis],
                    )?;
                }
                particles.push(WireParticle3d {
                    emitter: emitter.id,
                    serial: next_serial,
                    age: 0,
                    lifetime: emitter.lifetime_ticks,
                    position_milli: emitter.position_milli,
                    velocity_milli,
                    scale_milli: emitter.start_scale_milli,
                });
                next_serial = next_serial
                    .checked_add(1)
                    .ok_or(Wire3dError::ArithmeticOverflow)?;
            }
        }
        particles.sort_by_key(|particle| (particle.emitter, particle.serial));
        self.particles = particles;
        self.next_particle_serial = next_serial;
        self.last_particle_tick = tick;
        Ok(&self.particles)
    }

    pub fn synchronize_particle_clock(&mut self, tick: u64) -> Result<(), Wire3dError> {
        self.ensure_open()?;
        if !self.particles.is_empty() || tick < self.last_particle_tick {
            return Err(Wire3dError::InvalidDescriptor);
        }
        self.last_particle_tick = tick;
        Ok(())
    }

    fn bump(&mut self) -> Result<u64, Wire3dError> {
        self.ensure_open()?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Wire3dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    fn ensure_open(&self) -> Result<(), Wire3dError> {
        if self.closed {
            Err(Wire3dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn insert<T>(
    values: &mut BTreeMap<u64, T>,
    id: u64,
    value: T,
    maximum: usize,
) -> Result<(), Wire3dError> {
    if !values.contains_key(&id) && values.len() >= maximum {
        return Err(Wire3dError::Capacity);
    }
    values.insert(id, value);
    Ok(())
}

fn remove<T>(values: &mut BTreeMap<u64, T>, id: u64) -> Result<(), Wire3dError> {
    values
        .remove(&id)
        .map(|_| ())
        .ok_or(Wire3dError::InvalidDescriptor)
}

fn q16_to_u8(value: u16) -> u8 {
    ((u32::from(value) * 255 + 32_767) / 65_535) as u8
}

fn require_material(
    materials: &BTreeMap<u64, WireMaterial3d>,
    material: u64,
) -> Result<(), Wire3dError> {
    if materials.contains_key(&material) {
        Ok(())
    } else {
        Err(Wire3dError::InvalidDescriptor)
    }
}

fn instance_model(position_milli: [i64; 3], scale_milli: u32, yaw_q16: u16) -> [i64; 12] {
    let angle = f64::from(yaw_q16) / f64::from(u16::MAX) * std::f64::consts::TAU;
    let scale = f64::from(scale_milli);
    let cosine = (angle.cos() * scale).round() as i64;
    let sine = (angle.sin() * scale).round() as i64;
    [
        cosine,
        0,
        sine,
        position_milli[0],
        0,
        i64::from(scale_milli),
        0,
        position_milli[1],
        sine.saturating_neg(),
        0,
        cosine,
        position_milli[2],
    ]
}

fn build_scatter_instances(
    field: WireScatterField3d,
    maximum: usize,
) -> Result<Vec<WireScatterInstance3d>, Wire3dError> {
    let count = (u64::from(field.max_instances)
        .checked_mul(u64::from(field.density_per_million))
        .ok_or(Wire3dError::ArithmeticOverflow)?
        / 1_000_000)
        .max(1)
        .min(u64::from(field.max_instances));
    let count = usize::try_from(count).map_err(|_| Wire3dError::Capacity)?;
    if count > maximum {
        return Err(Wire3dError::Capacity);
    }
    let mut instances = Vec::with_capacity(count);
    for serial in 0..count {
        let random = split_mix64(field.seed ^ serial as u64);
        let x = random_i64_inclusive(
            random,
            field.bounds_milli[0],
            field.bounds_milli[0].saturating_add(field.bounds_milli[2]),
        )?;
        let z = random_i64_inclusive(
            random.rotate_left(29),
            field.bounds_milli[1],
            field.bounds_milli[1].saturating_add(field.bounds_milli[3]),
        )?;
        let scale_span = u64::from(field.max_scale_milli - field.min_scale_milli) + 1;
        instances.push(WireScatterInstance3d {
            field: field.id,
            serial: u32::try_from(serial).map_err(|_| Wire3dError::Capacity)?,
            position_milli: [x, 0, z],
            yaw_q16: random.rotate_left(43) as u16,
            scale_milli: field.min_scale_milli
                + u32::try_from(random.rotate_left(13) % scale_span)
                    .map_err(|_| Wire3dError::ArithmeticOverflow)?,
        });
    }
    Ok(instances)
}

fn split_mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn random_i64_inclusive(random: u64, minimum: i64, maximum: i64) -> Result<i64, Wire3dError> {
    if minimum > maximum {
        return Err(Wire3dError::InvalidDescriptor);
    }
    let span = (i128::from(maximum) - i128::from(minimum) + 1) as u128;
    let offset = u128::from(random) % span;
    i64::try_from(i128::from(minimum) + offset as i128).map_err(|_| Wire3dError::ArithmeticOverflow)
}

fn interpolate_u32(from: u32, to: u32, elapsed: u32, duration: u32) -> u32 {
    if duration == 0 {
        return to;
    }
    let from = i128::from(from);
    let delta = i128::from(to) - from;
    let value = from + delta * i128::from(elapsed.min(duration)) / i128::from(duration);
    u32::try_from(value).unwrap_or(if value < 0 { 0 } else { u32::MAX })
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn descriptor(bytes: &'a [u8], magic: &[u8; 4], length: usize) -> Result<Self, Wire3dError> {
        if bytes.len() < 4 {
            return Err(Wire3dError::Truncated);
        }
        if &bytes[..4] != magic {
            return Err(Wire3dError::InvalidMagic);
        }
        if bytes.len() < length {
            return Err(Wire3dError::Truncated);
        }
        if bytes.len() > length {
            return Err(Wire3dError::TrailingBytes);
        }
        Ok(Self { bytes, cursor: 4 })
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], Wire3dError> {
        let end = self.cursor.checked_add(N).ok_or(Wire3dError::Capacity)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(Wire3dError::Truncated)?
            .try_into()
            .map_err(|_| Wire3dError::Truncated)?;
        self.cursor = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, Wire3dError> {
        Ok(self.bytes::<1>()?[0])
    }

    fn boolean(&mut self) -> Result<bool, Wire3dError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Wire3dError::InvalidDescriptor),
        }
    }

    fn u16(&mut self) -> Result<u16, Wire3dError> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }

    fn u32(&mut self) -> Result<u32, Wire3dError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }

    fn i32(&mut self) -> Result<i32, Wire3dError> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }

    fn u64(&mut self) -> Result<u64, Wire3dError> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }

    fn i64(&mut self) -> Result<i64, Wire3dError> {
        Ok(i64::from_le_bytes(self.bytes()?))
    }

    fn u16_array<const N: usize>(&mut self) -> Result<[u16; N], Wire3dError> {
        let mut values = [0; N];
        for value in &mut values {
            *value = self.u16()?;
        }
        Ok(values)
    }

    fn u32_array<const N: usize>(&mut self) -> Result<[u32; N], Wire3dError> {
        let mut values = [0; N];
        for value in &mut values {
            *value = self.u32()?;
        }
        Ok(values)
    }

    fn u64_array<const N: usize>(&mut self) -> Result<[u64; N], Wire3dError> {
        let mut values = [0; N];
        for value in &mut values {
            *value = self.u64()?;
        }
        Ok(values)
    }

    fn i64_array<const N: usize>(&mut self) -> Result<[i64; N], Wire3dError> {
        let mut values = [0; N];
        for value in &mut values {
            *value = self.i64()?;
        }
        Ok(values)
    }

    fn finish(self) -> Result<(), Wire3dError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Wire3dError::TrailingBytes)
        }
    }
}
