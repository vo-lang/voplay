use std::collections::{BTreeMap, BTreeSet};

use crate::{AlphaMode, DrawPacket3d, MaterialDescriptor, Render3dError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialInstance {
    pub id: u64,
    pub base: u64,
    pub base_color: [u8; 4],
    pub metallic_q16: u16,
    pub roughness_q16: u16,
    pub emissive: [u16; 3],
    pub normal_scale_q16: u16,
    pub occlusion_strength_q16: u16,
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub unlit: bool,
    pub texture_assets: [u64; 5],
    pub revision: u64,
}

pub struct MaterialRegistry {
    bases: BTreeMap<u64, MaterialDescriptor>,
    instances: BTreeMap<u64, MaterialInstance>,
    max_bases: usize,
    max_instances: usize,
    max_textures_per_instance: usize,
    closed: bool,
}

impl MaterialRegistry {
    pub fn new(
        max_bases: usize,
        max_instances: usize,
        max_textures_per_instance: usize,
    ) -> Result<Self, Render3dError> {
        if max_bases == 0 || max_instances == 0 || max_textures_per_instance == 0 {
            return Err(Render3dError::InvalidConfig);
        }
        Ok(Self {
            bases: BTreeMap::new(),
            instances: BTreeMap::new(),
            max_bases,
            max_instances,
            max_textures_per_instance,
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> (usize, usize) {
        let released = (self.bases.len(), self.instances.len());
        self.closed = true;
        self.bases.clear();
        self.instances.clear();
        released
    }

    pub fn register_base(&mut self, descriptor: MaterialDescriptor) -> Result<(), Render3dError> {
        self.ensure_open()?;
        if descriptor.id == 0
            || descriptor.pipeline_key == 0
            || self.bases.contains_key(&descriptor.id)
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        if self.bases.len() >= self.max_bases {
            return Err(Render3dError::Capacity);
        }
        self.bases.insert(descriptor.id, descriptor);
        Ok(())
    }

    pub fn upsert_instance(&mut self, instance: MaterialInstance) -> Result<(), Render3dError> {
        self.ensure_open()?;
        if instance.id == 0
            || instance.base == 0
            || !self.bases.contains_key(&instance.base)
            || instance
                .texture_assets
                .iter()
                .filter(|texture| **texture != 0)
                .count()
                > self.max_textures_per_instance
            || has_duplicate_nonzero(&instance.texture_assets)
            || instance.revision == 0
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        if let Some(current) = self.instances.get(&instance.id) {
            if instance.revision <= current.revision {
                return Err(Render3dError::InvalidDescriptor);
            }
        } else if self.instances.len() >= self.max_instances {
            return Err(Render3dError::Capacity);
        }
        self.instances.insert(instance.id, instance);
        Ok(())
    }

    pub fn resolved(&self, id: u64) -> Option<ResolvedMaterial> {
        if self.closed {
            return None;
        }
        if let Some(instance) = self.instances.get(&id) {
            let base = self.bases.get(&instance.base)?;
            return Some(ResolvedMaterial {
                id: instance.id,
                pipeline_key: base.pipeline_key,
                base_color: instance.base_color,
                metallic_q16: instance.metallic_q16,
                roughness_q16: instance.roughness_q16,
                emissive: instance.emissive,
                normal_scale_q16: instance.normal_scale_q16,
                occlusion_strength_q16: instance.occlusion_strength_q16,
                alpha: instance.alpha,
                double_sided: instance.double_sided,
                unlit: instance.unlit,
                texture_assets: instance.texture_assets,
            });
        }
        self.bases.get(&id).map(|base| ResolvedMaterial {
            id: base.id,
            pipeline_key: base.pipeline_key,
            base_color: base.base_color,
            metallic_q16: base.metallic_q16,
            roughness_q16: base.roughness_q16,
            emissive: base.emissive_q16,
            normal_scale_q16: base.normal_scale_q16,
            occlusion_strength_q16: base.occlusion_strength_q16,
            alpha: base.alpha,
            double_sided: base.double_sided,
            unlit: base.unlit,
            texture_assets: base.textures,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedMaterial {
    pub id: u64,
    pub pipeline_key: u64,
    pub base_color: [u8; 4],
    pub metallic_q16: u16,
    pub roughness_q16: u16,
    pub emissive: [u16; 3],
    pub normal_scale_q16: u16,
    pub occlusion_strength_q16: u16,
    pub alpha: AlphaMode,
    pub double_sided: bool,
    pub unlit: bool,
    pub texture_assets: [u64; 5],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plane {
    pub normal_q20: [i64; 3],
    pub distance_q20: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bounds {
    pub center_milli: [i64; 3],
    pub half_extent_milli: [u32; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisibilityInput {
    pub draw: DrawPacket3d,
    pub bounds: Bounds,
    pub occlusion_cell: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewFrustum {
    pub planes: [Plane; 6],
    pub camera_milli: [i64; 3],
    pub layer_mask: u64,
    pub maximum_distance_milli: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShadowCascade {
    pub index: u32,
    pub near_milli: u64,
    pub far_milli: u64,
    pub texel_world_milli: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisibilityPlan {
    pub opaque: Vec<DrawPacket3d>,
    pub transparent: Vec<DrawPacket3d>,
    pub shadow_cascades: Vec<(ShadowCascade, Vec<DrawPacket3d>)>,
    pub culled_frustum: usize,
    pub culled_occlusion: usize,
    pub culled_distance: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisibilityConfig {
    pub max_draws: usize,
    pub max_occlusion_cells: usize,
    pub max_cascades: usize,
}

pub struct VisibilityPlanner {
    config: VisibilityConfig,
    occluded_cells: BTreeSet<u64>,
    closed: bool,
}

impl VisibilityPlanner {
    pub fn new(config: VisibilityConfig) -> Result<Self, Render3dError> {
        if config.max_draws == 0
            || config.max_occlusion_cells == 0
            || config.max_cascades == 0
            || config.max_cascades > 8
        {
            return Err(Render3dError::InvalidConfig);
        }
        Ok(Self {
            config,
            occluded_cells: BTreeSet::new(),
            closed: false,
        })
    }

    pub fn shutdown(&mut self) -> usize {
        let released = self.occluded_cells.len();
        self.closed = true;
        self.occluded_cells.clear();
        released
    }

    pub fn update_occlusion(
        &mut self,
        cells: impl IntoIterator<Item = u64>,
    ) -> Result<(), Render3dError> {
        self.ensure_open()?;
        let cells = cells.into_iter().collect::<BTreeSet<_>>();
        if cells.len() > self.config.max_occlusion_cells || cells.contains(&0) {
            return Err(Render3dError::Capacity);
        }
        self.occluded_cells = cells;
        Ok(())
    }

    pub fn build(
        &self,
        inputs: &[VisibilityInput],
        frustum: ViewFrustum,
        cascades: &[ShadowCascade],
        materials: &MaterialRegistry,
    ) -> Result<VisibilityPlan, Render3dError> {
        self.ensure_open()?;
        if inputs.len() > self.config.max_draws
            || frustum.layer_mask == 0
            || frustum.maximum_distance_milli == 0
            || cascades.len() > self.config.max_cascades
            || !valid_cascades(cascades)
        {
            return Err(Render3dError::InvalidDescriptor);
        }
        let mut plan = VisibilityPlan {
            opaque: Vec::new(),
            transparent: Vec::new(),
            shadow_cascades: cascades
                .iter()
                .copied()
                .map(|cascade| (cascade, Vec::new()))
                .collect(),
            culled_frustum: 0,
            culled_occlusion: 0,
            culled_distance: 0,
        };
        for input in inputs {
            if input.occlusion_cell != 0 && self.occluded_cells.contains(&input.occlusion_cell) {
                plan.culled_occlusion += 1;
                continue;
            }
            if !inside_frustum(input.bounds, &frustum.planes) {
                plan.culled_frustum += 1;
                continue;
            }
            let distance = distance_milli(input.bounds.center_milli, frustum.camera_milli);
            if distance > frustum.maximum_distance_milli {
                plan.culled_distance += 1;
                continue;
            }
            let material = materials
                .resolved(input.draw.material)
                .ok_or(Render3dError::MissingMaterial)?;
            let mut draw = input.draw;
            draw.pipeline_key = material.pipeline_key;
            draw.depth_key = i64::try_from(distance).unwrap_or(i64::MAX);
            if material.alpha == AlphaMode::Blend {
                plan.transparent.push(draw);
            } else {
                plan.opaque.push(draw);
            }
            if draw.casts_shadow {
                for (cascade, draws) in &mut plan.shadow_cascades {
                    if distance >= cascade.near_milli && distance <= cascade.far_milli {
                        draws.push(draw);
                    }
                }
            }
        }
        plan.opaque
            .sort_by_key(|draw| (draw.pipeline_key, draw.material, draw.mesh, draw.entity));
        plan.transparent.sort_by(|left, right| {
            right
                .depth_key
                .cmp(&left.depth_key)
                .then_with(|| left.pipeline_key.cmp(&right.pipeline_key))
                .then_with(|| left.entity.cmp(&right.entity))
        });
        for (_, draws) in &mut plan.shadow_cascades {
            draws.sort_by_key(|draw| (draw.pipeline_key, draw.mesh, draw.entity));
        }
        Ok(plan)
    }

    fn ensure_open(&self) -> Result<(), Render3dError> {
        if self.closed {
            Err(Render3dError::Closed)
        } else {
            Ok(())
        }
    }
}

fn valid_cascades(cascades: &[ShadowCascade]) -> bool {
    cascades.iter().enumerate().all(|(index, cascade)| {
        cascade.index == index as u32
            && cascade.near_milli < cascade.far_milli
            && cascade.texel_world_milli != 0
            && (index == 0 || cascades[index - 1].far_milli <= cascade.near_milli)
    })
}

fn inside_frustum(bounds: Bounds, planes: &[Plane; 6]) -> bool {
    planes.iter().all(|plane| {
        let center_distance = (0..3).fold(plane.distance_q20, |sum, axis| {
            sum.saturating_add(plane.normal_q20[axis].saturating_mul(bounds.center_milli[axis]))
        });
        let radius = (0..3).fold(0_i64, |sum, axis| {
            sum.saturating_add(
                plane.normal_q20[axis]
                    .unsigned_abs()
                    .saturating_mul(u64::from(bounds.half_extent_milli[axis]))
                    .min(i64::MAX as u64) as i64,
            )
        });
        center_distance.saturating_add(radius) >= 0
    })
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

fn has_duplicate_nonzero(values: &[u64]) -> bool {
    values
        .iter()
        .enumerate()
        .any(|(index, value)| *value != 0 && values[..index].contains(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;
    use voplay_runtime::RenderEntity;

    fn material(id: u64, pipeline_key: u64, alpha: AlphaMode) -> MaterialDescriptor {
        MaterialDescriptor {
            id,
            pipeline_key,
            alpha,
            double_sided: false,
            unlit: false,
            base_color: [255; 4],
            metallic_q16: 0,
            roughness_q16: u16::MAX,
            emissive_q16: [0; 3],
            normal_scale_q16: u16::MAX,
            occlusion_strength_q16: u16::MAX,
            textures: [0; 5],
        }
    }

    fn draw(entity: u32, material: u64, casts_shadow: bool) -> DrawPacket3d {
        DrawPacket3d {
            entity: RenderEntity {
                engine: Handle {
                    index: 1,
                    generation: 1,
                },
                entity: Handle {
                    index: entity,
                    generation: 1,
                },
            },
            mesh: u64::from(entity) + 10,
            material,
            pipeline_key: 999,
            depth_key: -1,
            casts_shadow,
            receives_shadow: true,
            model_milli: [0; 12],
        }
    }

    fn input(
        entity: u32,
        material: u64,
        center_milli: [i64; 3],
        occlusion_cell: u64,
    ) -> VisibilityInput {
        VisibilityInput {
            draw: draw(entity, material, true),
            bounds: Bounds {
                center_milli,
                half_extent_milli: [10; 3],
            },
            occlusion_cell,
        }
    }

    fn frustum(maximum_distance_milli: u64) -> ViewFrustum {
        ViewFrustum {
            planes: [Plane {
                normal_q20: [0; 3],
                distance_q20: 0,
            }; 6],
            camera_milli: [0; 3],
            layer_mask: 1,
            maximum_distance_milli,
        }
    }

    #[test]
    fn visibility_culls_occlusion_and_distance_and_sorts_transparency_back_to_front() {
        let mut materials = MaterialRegistry::new(4, 4, 4).unwrap();
        materials
            .register_base(material(1, 20, AlphaMode::Opaque))
            .unwrap();
        materials
            .register_base(material(2, 10, AlphaMode::Blend))
            .unwrap();
        let mut planner = VisibilityPlanner::new(VisibilityConfig {
            max_draws: 16,
            max_occlusion_cells: 4,
            max_cascades: 2,
        })
        .unwrap();
        planner.update_occlusion([7]).unwrap();
        let inputs = [
            input(1, 1, [100, 0, 0], 0),
            input(2, 2, [300, 0, 0], 0),
            input(3, 2, [200, 0, 0], 0),
            input(4, 1, [100, 0, 0], 7),
            input(5, 1, [2_000, 0, 0], 0),
        ];
        let cascade = ShadowCascade {
            index: 0,
            near_milli: 0,
            far_milli: 1_000,
            texel_world_milli: 1,
        };
        let plan = planner
            .build(&inputs, frustum(1_000), &[cascade], &materials)
            .unwrap();

        assert_eq!(plan.opaque.len(), 1);
        assert_eq!(
            plan.transparent
                .iter()
                .map(|draw| draw.entity.entity.index)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(plan.culled_occlusion, 1);
        assert_eq!(plan.culled_distance, 1);
        assert_eq!(plan.shadow_cascades[0].1.len(), 3);
        assert_eq!(plan.opaque[0].pipeline_key, 20);
        assert_eq!(plan.transparent[0].pipeline_key, 10);
    }

    #[test]
    fn invalid_occlusion_update_does_not_replace_last_committed_set() {
        let mut planner = VisibilityPlanner::new(VisibilityConfig {
            max_draws: 4,
            max_occlusion_cells: 1,
            max_cascades: 1,
        })
        .unwrap();
        planner.update_occlusion([9]).unwrap();
        assert_eq!(planner.update_occlusion([0]), Err(Render3dError::Capacity));

        let mut materials = MaterialRegistry::new(1, 1, 1).unwrap();
        materials
            .register_base(material(1, 1, AlphaMode::Opaque))
            .unwrap();
        let plan = planner
            .build(&[input(1, 1, [0; 3], 9)], frustum(10), &[], &materials)
            .unwrap();
        assert_eq!(plan.culled_occlusion, 1);
        assert!(plan.opaque.is_empty());
    }
}
