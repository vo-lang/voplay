use std::collections::BTreeMap;

use crate::{MaterialDescriptor, Render3dError};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TerrainPatchId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerrainPatch {
    pub id: TerrainPatchId,
    pub height_asset: u64,
    pub material: u64,
    pub origin_milli: [i64; 3],
    pub size_milli: [u64; 2],
    pub height_scale_milli: i64,
    pub lod_distances_milli: Vec<u64>,
    pub layer_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DecalId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecalComposition {
    BeforeLighting,
    AfterLighting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decal {
    pub id: DecalId,
    pub material: u64,
    pub transform: [i64; 12],
    pub half_extent_milli: [u32; 3],
    pub layer_mask: u64,
    pub composition: DecalComposition,
    pub order: i32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ScatterFieldId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScatterField {
    pub id: ScatterFieldId,
    pub seed: u64,
    pub mesh: u64,
    pub material: u64,
    pub bounds_min_milli: [i64; 3],
    pub bounds_max_milli: [i64; 3],
    pub count: u32,
    pub minimum_scale_milli: u32,
    pub maximum_scale_milli: u32,
    pub layer_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScatterInstance {
    pub field: ScatterFieldId,
    pub serial: u32,
    pub position_milli: [i64; 3],
    pub yaw_q16: u16,
    pub scale_milli: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ParticleEmitter3dId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticleEmitter3d {
    pub id: ParticleEmitter3dId,
    pub seed: u64,
    pub material: u64,
    pub position_milli: [i64; 3],
    pub velocity_min_milli: [i32; 3],
    pub velocity_max_milli: [i32; 3],
    pub spawn_per_tick: u32,
    pub lifetime_ticks: u32,
    pub layer_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Particle3d {
    pub emitter: ParticleEmitter3dId,
    pub serial: u64,
    pub age: u32,
    pub lifetime: u32,
    pub position_milli: [i64; 3],
    pub velocity_milli: [i32; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToneMap {
    None,
    Reinhard,
    Aces,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AntiAlias {
    None,
    Fxaa,
    Taa,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Environment3d {
    pub fog_color: [u16; 3],
    pub fog_start_milli: u64,
    pub fog_end_milli: u64,
    pub exposure_milli: i32,
    pub tone_map: ToneMap,
    pub bloom_threshold_milli: u32,
    pub bloom_intensity_milli: u32,
    pub anti_alias: AntiAlias,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtendedFeatureConfig {
    pub max_terrain_patches: usize,
    pub max_decals: usize,
    pub max_scatter_fields: usize,
    pub max_scatter_instances: usize,
    pub max_particle_emitters: usize,
    pub max_particles: usize,
}

impl Default for ExtendedFeatureConfig {
    fn default() -> Self {
        Self {
            max_terrain_patches: 16_384,
            max_decals: 65_536,
            max_scatter_fields: 16_384,
            max_scatter_instances: 1_000_000,
            max_particle_emitters: 16_384,
            max_particles: 1_000_000,
        }
    }
}

pub struct ExtendedFeatureWorld {
    config: ExtendedFeatureConfig,
    terrain: BTreeMap<TerrainPatchId, TerrainPatch>,
    decals: BTreeMap<DecalId, Decal>,
    scatter: BTreeMap<ScatterFieldId, (ScatterField, Vec<ScatterInstance>)>,
    emitters: BTreeMap<ParticleEmitter3dId, ParticleEmitter3d>,
    particles: Vec<Particle3d>,
    environment: Environment3d,
    last_tick: u64,
    next_particle: u64,
    revision: u64,
    closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtendedFeatureOwnerSnapshot {
    pub closed: bool,
    pub terrain_patches: usize,
    pub decals: usize,
    pub scatter_fields: usize,
    pub scatter_instances: usize,
    pub particle_emitters: usize,
    pub particles: usize,
}

impl ExtendedFeatureWorld {
    pub fn new(config: ExtendedFeatureConfig) -> Result<Self, Render3dError> {
        if config.max_terrain_patches == 0
            || config.max_decals == 0
            || config.max_scatter_fields == 0
            || config.max_scatter_instances == 0
            || config.max_particle_emitters == 0
            || config.max_particles == 0
        {
            return Err(Render3dError::InvalidConfig);
        }
        Ok(Self {
            config,
            terrain: BTreeMap::new(),
            decals: BTreeMap::new(),
            scatter: BTreeMap::new(),
            emitters: BTreeMap::new(),
            particles: Vec::new(),
            environment: Environment3d {
                fog_color: [0; 3],
                fog_start_milli: 0,
                fog_end_milli: u64::MAX,
                exposure_milli: 1000,
                tone_map: ToneMap::Aces,
                bloom_threshold_milli: 1000,
                bloom_intensity_milli: 0,
                anti_alias: AntiAlias::Fxaa,
            },
            last_tick: 0,
            next_particle: 1,
            revision: 0,
            closed: false,
        })
    }

    pub fn owner_snapshot(&self) -> ExtendedFeatureOwnerSnapshot {
        ExtendedFeatureOwnerSnapshot {
            closed: self.closed,
            terrain_patches: self.terrain.len(),
            decals: self.decals.len(),
            scatter_fields: self.scatter.len(),
            scatter_instances: self
                .scatter
                .values()
                .map(|(_, instances)| instances.len())
                .sum(),
            particle_emitters: self.emitters.len(),
            particles: self.particles.len(),
        }
    }

    pub fn shutdown(&mut self) -> ExtendedFeatureOwnerSnapshot {
        let before = self.owner_snapshot();
        self.closed = true;
        self.terrain.clear();
        self.decals.clear();
        self.scatter.clear();
        self.emitters.clear();
        self.particles.clear();
        before
    }

    pub fn upsert_terrain(&mut self, patch: TerrainPatch) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        if patch.id.0 == 0
            || patch.height_asset == 0
            || patch.material == 0
            || patch.size_milli.contains(&0)
            || patch.height_scale_milli == 0
            || patch.layer_mask == 0
            || patch.lod_distances_milli.is_empty()
            || !strictly_increasing(&patch.lod_distances_milli)
            || (!self.terrain.contains_key(&patch.id)
                && self.terrain.len() >= self.config.max_terrain_patches)
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        self.terrain.insert(patch.id, patch);
        self.next_revision()
    }

    pub fn upsert_decal(&mut self, decal: Decal) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        if decal.id.0 == 0
            || decal.material == 0
            || decal.half_extent_milli.contains(&0)
            || decal.layer_mask == 0
            || (!self.decals.contains_key(&decal.id) && self.decals.len() >= self.config.max_decals)
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        self.decals.insert(decal.id, decal);
        self.next_revision()
    }

    pub fn upsert_scatter(&mut self, field: ScatterField) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        if field.id.0 == 0
            || field.seed == 0
            || field.mesh == 0
            || field.material == 0
            || field.count == 0
            || field.minimum_scale_milli == 0
            || field.minimum_scale_milli > field.maximum_scale_milli
            || field.layer_mask == 0
            || (0..3).any(|axis| field.bounds_min_milli[axis] > field.bounds_max_milli[axis])
            || (!self.scatter.contains_key(&field.id)
                && self.scatter.len() >= self.config.max_scatter_fields)
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        let current_count = self
            .scatter
            .get(&field.id)
            .map_or(0, |(_, instances)| instances.len());
        let _total_instances = self
            .scatter
            .values()
            .try_fold(0_usize, |total, (_, instances)| {
                total.checked_add(instances.len())
            })
            .and_then(|total| total.checked_sub(current_count))
            .and_then(|total| total.checked_add(field.count as usize))
            .filter(|total| *total <= self.config.max_scatter_instances)
            .ok_or(Render3dError::Capacity)?;
        let instances = build_scatter_instances(&field)?;
        self.scatter.insert(field.id, (field, instances));
        self.next_revision()
    }

    pub fn upsert_emitter(&mut self, emitter: ParticleEmitter3d) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        if emitter.id.0 == 0
            || emitter.seed == 0
            || emitter.material == 0
            || emitter.spawn_per_tick == 0
            || emitter.lifetime_ticks == 0
            || emitter.layer_mask == 0
            || (0..3)
                .any(|axis| emitter.velocity_min_milli[axis] > emitter.velocity_max_milli[axis])
            || (!self.emitters.contains_key(&emitter.id)
                && self.emitters.len() >= self.config.max_particle_emitters)
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        self.emitters.insert(emitter.id, emitter);
        self.next_revision()
    }

    pub fn set_environment(&mut self, environment: Environment3d) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        if environment.fog_start_milli > environment.fog_end_milli
            || environment.exposure_milli <= 0
            || environment.bloom_intensity_milli > 10_000
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        self.environment = environment;
        self.next_revision()
    }

    pub fn tick_particles(&mut self, tick: u64) -> Result<&[Particle3d], Render3dError> {
        self.ensure_open()?;
        if tick != self.last_tick + 1 {
            return Err(Render3dError::TickSequence);
        }
        let mut particles = Vec::with_capacity(
            self.config
                .max_particles
                .min(self.particles.len().saturating_add(self.emitters.len())),
        );
        for current in &self.particles {
            let age = current
                .age
                .checked_add(1)
                .ok_or(Render3dError::ArithmeticOverflow)?;
            if age >= current.lifetime {
                continue;
            }
            let mut particle = *current;
            particle.age = age;
            for axis in 0..3 {
                particle.position_milli[axis] = particle.position_milli[axis]
                    .checked_add(i64::from(particle.velocity_milli[axis]))
                    .ok_or(Render3dError::ArithmeticOverflow)?;
            }
            particles.push(particle);
        }
        let mut next_particle = self.next_particle;
        for emitter in self.emitters.values() {
            for ordinal in 0..emitter.spawn_per_tick {
                if particles.len() >= self.config.max_particles {
                    break;
                }
                let random = split_mix64(emitter.seed ^ tick.rotate_left(19) ^ u64::from(ordinal));
                let mut velocity = [0; 3];
                for axis in 0..3 {
                    velocity[axis] = random_range(
                        random.rotate_left((axis * 19) as u32),
                        emitter.velocity_min_milli[axis],
                        emitter.velocity_max_milli[axis],
                    );
                }
                let serial = next_particle;
                next_particle = next_particle
                    .checked_add(1)
                    .ok_or(Render3dError::ArithmeticOverflow)?;
                particles.push(Particle3d {
                    emitter: emitter.id,
                    serial,
                    age: 0,
                    lifetime: emitter.lifetime_ticks,
                    position_milli: emitter.position_milli,
                    velocity_milli: velocity,
                });
            }
        }
        particles.sort_by_key(|particle| (particle.emitter, particle.serial));
        self.particles = particles;
        self.next_particle = next_particle;
        self.last_tick = tick;
        Ok(&self.particles)
    }

    pub fn remove_terrain(&mut self, id: TerrainPatchId) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        self.terrain.remove(&id).ok_or(Render3dError::Unknown)?;
        self.next_revision()
    }

    pub fn remove_decal(&mut self, id: DecalId) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        self.decals.remove(&id).ok_or(Render3dError::Unknown)?;
        self.next_revision()
    }

    pub fn remove_scatter(&mut self, id: ScatterFieldId) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        self.scatter.remove(&id).ok_or(Render3dError::Unknown)?;
        self.next_revision()
    }

    pub fn remove_emitter(&mut self, id: ParticleEmitter3dId) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        self.emitters.remove(&id).ok_or(Render3dError::Unknown)?;
        self.particles.retain(|particle| particle.emitter != id);
        self.next_revision()
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub fn terrain(&self) -> impl Iterator<Item = &TerrainPatch> {
        self.terrain.values()
    }

    pub fn decals(&self, composition: DecalComposition) -> Vec<&Decal> {
        let mut decals = self
            .decals
            .values()
            .filter(|decal| decal.composition == composition)
            .collect::<Vec<_>>();
        decals.sort_by_key(|decal| (decal.order, decal.id));
        decals
    }

    pub fn scatter_instances(&self) -> impl Iterator<Item = &ScatterInstance> {
        self.scatter.values().flat_map(|(_, instances)| instances)
    }

    pub const fn environment(&self) -> Environment3d {
        self.environment
    }

    pub fn material_supported(&self, materials: &BTreeMap<u64, MaterialDescriptor>) -> bool {
        self.terrain
            .values()
            .all(|patch| materials.contains_key(&patch.material))
            && self
                .decals
                .values()
                .all(|decal| materials.contains_key(&decal.material))
            && self
                .scatter
                .values()
                .all(|(field, _)| materials.contains_key(&field.material))
            && self
                .emitters
                .values()
                .all(|emitter| materials.contains_key(&emitter.material))
    }

    fn next_revision(&mut self) -> Result<u64, Render3dError> {
        self.ensure_open()?;
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(Render3dError::ArithmeticOverflow)?;
        Ok(self.revision)
    }

    fn ensure_open(&self) -> Result<(), Render3dError> {
        if self.closed {
            Err(Render3dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn build_scatter_instances(field: &ScatterField) -> Result<Vec<ScatterInstance>, Render3dError> {
    (0..field.count)
        .map(|serial| {
            let random = split_mix64(field.seed ^ u64::from(serial));
            let mut position = [0; 3];
            for axis in 0..3 {
                position[axis] = random_i64_inclusive(
                    random.rotate_left((axis * 17) as u32),
                    field.bounds_min_milli[axis],
                    field.bounds_max_milli[axis],
                )?;
            }
            let scale_span = u64::from(field.maximum_scale_milli - field.minimum_scale_milli) + 1;
            Ok(ScatterInstance {
                field: field.id,
                serial,
                position_milli: position,
                yaw_q16: random as u16,
                scale_milli: field.minimum_scale_milli
                    + u32::try_from(random.rotate_left(37) % scale_span)
                        .map_err(|_| Render3dError::ArithmeticOverflow)?,
            })
        })
        .collect()
}

fn random_i64_inclusive(random: u64, minimum: i64, maximum: i64) -> Result<i64, Render3dError> {
    let span = (i128::from(maximum) - i128::from(minimum) + 1) as u128;
    let offset = u128::from(random) % span;
    i64::try_from(i128::from(minimum) + offset as i128)
        .map_err(|_| Render3dError::ArithmeticOverflow)
}

fn strictly_increasing(values: &[u64]) -> bool {
    values
        .windows(2)
        .all(|pair| pair[0] != 0 && pair[0] < pair[1])
        && values.last().is_some_and(|value| *value != 0)
}

fn split_mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

fn random_range(random: u64, minimum: i32, maximum: i32) -> i32 {
    let span = (i64::from(maximum) - i64::from(minimum) + 1) as u64;
    i32::try_from(i64::from(minimum) + i64::try_from(random % span).unwrap()).unwrap()
}
