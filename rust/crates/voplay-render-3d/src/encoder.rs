use std::collections::{BTreeMap, BTreeSet};

use voplay_runtime::{
    presentation::{FrameCommandEncoder, FrameEncodeRequest, FrameEncoderError, RenderFrameDelta},
    render_commands::{
        decode_object_commands, encode_frame_commands, encode_target_frame_bundle, DrawCommand,
        TargetDrawCommands,
    },
    render_control::{ClearPolicy, ResolvedRenderViewFrame},
    render_world::{Aabb3, RenderWorld, RenderWorldConfig, RenderWorldError, ViewVisibility},
    RenderEntity,
};

pub type RenderFeatureFactoryBuilder =
    fn() -> Box<dyn voplay_runtime::render_feature::FeatureFactory>;

use crate::{
    build_camera_frame, build_directional_shadow_cascades, build_terrain_mesh_3d,
    decode_heightmap_artifact_3d, decode_light, decode_material_3d, decode_mesh_artifact_3d,
    decode_mesh_asset_3d, decode_mesh_instance, decode_skin_palette_artifact_3d,
    encode_scene_frame_3d, packed_entity_handle, project_decal_mesh_3d, projected_decal_mesh_id,
    CameraFrame3d, DecalTargetMesh3d, HeightmapArtifact3d, HeightmapArtifact3dConfig,
    MaterialDescriptor, MeshArtifact3d, MeshArtifact3dConfig, MeshDescriptor,
    PortableFeatureResolver3d, PortableScene3d, PortableScene3dConfig, ProjectedDecalMesh3d,
    Render3dPlanner, Render3dPlannerConfig, SceneFrame3d, SceneViewFrame3d,
    SkinPaletteArtifact3dConfig, TerrainHeightmap3d, TerrainMeshArtifact3d, Wire3dError,
    WireDecal3d, WireTerrainPatch3d, BUILTIN_DECAL_MESH_3D, BUILTIN_TERRAIN_MESH_3D,
    LIGHT_COMPONENT, MESH_INSTANCE_COMPONENT,
};

pub const RENDER_ASSET_MESH_3D: u32 = 2;
pub const RENDER_ASSET_MATERIAL_3D: u32 = 3;
pub const RENDER_ASSET_SKIN_PALETTE_3D: u32 = 4;
pub const RENDER_ASSET_HEIGHTMAP_3D: u32 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scene3dFrameEncoderConfig {
    pub world: RenderWorldConfig,
    pub planner: Render3dPlannerConfig,
    pub max_commands: usize,
    pub max_encoded_bytes: usize,
    pub allow_missing_cameras: bool,
}

impl Default for Scene3dFrameEncoderConfig {
    fn default() -> Self {
        Self {
            world: RenderWorldConfig::default(),
            planner: Render3dPlannerConfig::default(),
            max_commands: 1_000_000,
            max_encoded_bytes: 64 * 1024 * 1024,
            allow_missing_cameras: false,
        }
    }
}

#[derive(Clone)]
pub struct Scene3dFrameEncoder {
    config: Scene3dFrameEncoderConfig,
    world: RenderWorld,
    meshes: BTreeMap<u64, MeshDescriptor>,
    mesh_artifacts: BTreeMap<u64, MeshArtifact3d>,
    materials: BTreeMap<u64, MaterialDescriptor>,
    material_wires: BTreeMap<u64, Vec<u8>>,
    skin_palettes: BTreeSet<u64>,
    heightmaps: BTreeMap<u64, HeightmapArtifact3d>,
    feature_scene: PortableScene3d,
    feature_signature: u64,
    feature_tick: u64,
    render_feature_wires: Vec<Vec<u8>>,
    render_feature_factories:
        BTreeMap<voplay_runtime::render_feature::FeatureFactoryId, RenderFeatureFactoryBuilder>,
    closed: bool,
}

impl Scene3dFrameEncoder {
    pub fn new(
        engine: voplay_protocol::EngineId,
        config: Scene3dFrameEncoderConfig,
    ) -> Result<Self, FrameEncoderError> {
        if config.max_commands == 0 || config.max_encoded_bytes < 8 {
            return Err(FrameEncoderError::InvalidInput);
        }
        Ok(Self {
            world: RenderWorld::new(engine, 1, config.world).map_err(map_world_error)?,
            config,
            meshes: BTreeMap::new(),
            mesh_artifacts: BTreeMap::new(),
            materials: BTreeMap::new(),
            material_wires: BTreeMap::new(),
            skin_palettes: BTreeSet::new(),
            heightmaps: BTreeMap::new(),
            feature_scene: PortableScene3d::new(PortableScene3dConfig::default())
                .map_err(|_| FrameEncoderError::InvalidInput)?,
            feature_signature: 0,
            feature_tick: 0,
            render_feature_wires: Vec::new(),
            render_feature_factories: BTreeMap::new(),
            closed: false,
        })
    }

    pub const fn world(&self) -> &RenderWorld {
        &self.world
    }

    pub fn register_render_feature_factory(
        &mut self,
        builder: RenderFeatureFactoryBuilder,
    ) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        let factory = builder();
        let id = factory.id();
        if id.0 == 0
            || factory.version() == 0
            || !factory.feature().is_valid()
            || self.render_feature_factories.contains_key(&id)
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        self.render_feature_factories.insert(id, builder);
        Ok(())
    }
}

impl FrameCommandEncoder for Scene3dFrameEncoder {
    fn owner_snapshot(&self) -> voplay_runtime::presentation::FrameEncoderOwnerSnapshot {
        let world = self.world.owner_snapshot();
        let feature_scene = self.feature_scene.owner_snapshot();
        voplay_runtime::presentation::FrameEncoderOwnerSnapshot {
            closed: self.closed,
            retained_objects: world
                .objects
                .saturating_add(feature_scene.terrain_patches)
                .saturating_add(feature_scene.decals)
                .saturating_add(feature_scene.scatter_fields)
                .saturating_add(feature_scene.scatter_instances)
                .saturating_add(feature_scene.particle_emitters)
                .saturating_add(feature_scene.particles),
            retained_assets: self
                .meshes
                .len()
                .saturating_add(self.mesh_artifacts.len())
                .saturating_add(self.materials.len())
                .saturating_add(self.material_wires.len())
                .saturating_add(self.skin_palettes.len())
                .saturating_add(self.heightmaps.len())
                .saturating_add(feature_scene.materials)
                .saturating_add(self.render_feature_wires.len())
                .saturating_add(self.render_feature_factories.len()),
            retained_bytes: world
                .component_bytes
                .saturating_add(self.material_wires.values().map(Vec::len).sum::<usize>())
                .saturating_add(
                    self.render_feature_wires
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>(),
                ),
        }
    }

    fn shutdown(&mut self) {
        self.closed = true;
        self.world.shutdown();
        self.meshes.clear();
        self.mesh_artifacts.clear();
        self.materials.clear();
        self.material_wires.clear();
        self.skin_palettes.clear();
        self.heightmaps.clear();
        self.feature_scene.shutdown();
        self.feature_signature = 0;
        self.feature_tick = 0;
        self.render_feature_wires.clear();
        self.render_feature_factories.clear();
    }

    fn register_render_feature(&mut self, bytes: &[u8]) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        let registration = voplay_runtime::render_feature::decode_feature_registration(
            bytes,
            voplay_runtime::render_feature::FeatureRegistryConfig::default(),
        )
        .map_err(|_| FrameEncoderError::InvalidInput)?;
        let id = match registration {
            voplay_runtime::render_feature::RenderFeatureRegistration::Compiled {
                descriptor,
                ..
            } => descriptor.id,
            voplay_runtime::render_feature::RenderFeatureRegistration::Data(source) => {
                source.descriptor.id
            }
        };
        if self.render_feature_wires.iter().any(|current| {
            voplay_runtime::render_feature::decode_feature_registration(
                current,
                voplay_runtime::render_feature::FeatureRegistryConfig::default(),
            )
            .ok()
            .is_some_and(|registration| match registration {
                voplay_runtime::render_feature::RenderFeatureRegistration::Compiled {
                    descriptor,
                    ..
                } => descriptor.id == id,
                voplay_runtime::render_feature::RenderFeatureRegistration::Data(source) => {
                    source.descriptor.id == id
                }
            })
        }) {
            return Err(FrameEncoderError::InvalidInput);
        }
        self.render_feature_wires.push(bytes.to_vec());
        Ok(())
    }

    fn upsert_asset(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        match kind {
            RENDER_ASSET_MESH_3D => {
                if matches!(asset, BUILTIN_TERRAIN_MESH_3D | BUILTIN_DECAL_MESH_3D) {
                    return Err(FrameEncoderError::InvalidInput);
                }
                let mesh = if bytes.get(..4) == Some(b"VMG1") {
                    let artifact = decode_mesh_artifact_3d(bytes, MeshArtifact3dConfig::default())
                        .map_err(|_| FrameEncoderError::InvalidInput)?;
                    let descriptor = artifact.descriptor;
                    self.mesh_artifacts.insert(asset, artifact);
                    descriptor
                } else {
                    self.mesh_artifacts.remove(&asset);
                    decode_mesh_asset_3d(bytes).map_err(|_| FrameEncoderError::InvalidInput)?
                };
                if mesh.id != asset {
                    return Err(FrameEncoderError::InvalidInput);
                }
                self.meshes.insert(asset, mesh);
            }
            RENDER_ASSET_MATERIAL_3D => {
                let material =
                    decode_material_3d(bytes).map_err(|_| FrameEncoderError::InvalidInput)?;
                if material.id != asset || material.revision != revision {
                    return Err(FrameEncoderError::InvalidInput);
                }
                let pipeline_key = 1
                    | (u64::from(matches!(material.alpha, crate::WireAlphaMode3d::Blend)) << 1)
                    | (u64::from(material.double_sided) << 2)
                    | (u64::from(material.unlit) << 3);
                self.materials.insert(
                    asset,
                    material
                        .material_descriptor(pipeline_key)
                        .map_err(|_| FrameEncoderError::InvalidInput)?,
                );
                self.material_wires.insert(asset, bytes.to_vec());
            }
            RENDER_ASSET_SKIN_PALETTE_3D => {
                let palette = decode_skin_palette_artifact_3d(
                    bytes,
                    self.world.engine(),
                    SkinPaletteArtifact3dConfig::default(),
                )
                .map_err(|_| FrameEncoderError::InvalidInput)?;
                if packed_entity_handle(palette.entity) != asset {
                    return Err(FrameEncoderError::InvalidInput);
                }
                self.skin_palettes.insert(asset);
            }
            RENDER_ASSET_HEIGHTMAP_3D => {
                let heightmap =
                    decode_heightmap_artifact_3d(bytes, HeightmapArtifact3dConfig::default())
                        .map_err(|_| FrameEncoderError::InvalidInput)?;
                if heightmap.id != asset {
                    return Err(FrameEncoderError::InvalidInput);
                }
                self.heightmaps.insert(asset, heightmap);
            }
            _ => return Err(FrameEncoderError::Unsupported),
        }
        Ok(())
    }

    fn remove_asset(
        &mut self,
        kind: u32,
        asset: u64,
        _revision: u64,
    ) -> Result<(), FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        match kind {
            RENDER_ASSET_MESH_3D => {
                self.meshes.remove(&asset);
                self.mesh_artifacts.remove(&asset);
            }
            RENDER_ASSET_MATERIAL_3D => {
                self.materials.remove(&asset);
                self.material_wires.remove(&asset);
            }
            RENDER_ASSET_SKIN_PALETTE_3D => {
                self.skin_palettes.remove(&asset);
            }
            RENDER_ASSET_HEIGHTMAP_3D => {
                self.heightmaps.remove(&asset);
            }
            _ => return Err(FrameEncoderError::Unsupported),
        }
        Ok(())
    }

    fn encode(&mut self, request: FrameEncodeRequest<'_>) -> Result<Vec<u8>, FrameEncoderError> {
        if self.closed {
            return Err(FrameEncoderError::Closed);
        }
        if request.engine != self.world.engine()
            || request.domain.engine != request.engine
            || request.pulse_id == 0
            || request.graph_signature == 0
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        let control = request
            .control_frame
            .ok_or(FrameEncoderError::InvalidInput)?;
        if control.engine != request.engine
            || control.domain != request.domain
            || control.views.is_empty()
        {
            return Err(FrameEncoderError::InvalidInput);
        }
        let delta = RenderFrameDelta {
            revision: request.render_revision,
            full_snapshot: request.full_snapshot,
            objects: request
                .objects
                .iter()
                .map(|object| (object.entity, object.bytes.to_vec()))
                .collect(),
            despawned: request.despawned.to_vec(),
        };
        let mut world = self.world.clone();
        world.apply_delta(&delta).map_err(map_world_error)?;
        let next_feature_signature = feature_signature(&world, &self.material_wires);
        let mut feature_scene = if next_feature_signature == self.feature_signature {
            self.feature_scene.clone()
        } else {
            rebuild_feature_scene(&world, &self.material_wires)?
        };
        let mut feature_tick = self.feature_tick;
        if next_feature_signature != self.feature_signature {
            feature_scene
                .synchronize_particle_clock(request.render_revision)
                .map_err(|_| FrameEncoderError::InvalidInput)?;
            feature_tick = request.render_revision;
        } else {
            if request.render_revision < feature_tick {
                return Err(FrameEncoderError::InvalidInput);
            }
            if request.render_revision > feature_tick {
                let first = feature_tick
                    .checked_add(1)
                    .ok_or(FrameEncoderError::Capacity)?;
                for tick in first..=request.render_revision {
                    feature_scene
                        .tick_particles(tick)
                        .map_err(|_| FrameEncoderError::InvalidInput)?;
                }
            }
            feature_tick = request.render_revision;
        }
        let terrain_meshes = build_terrain_meshes(&feature_scene, &self.heightmaps)?;
        let decal_meshes = build_decal_meshes(
            &feature_scene,
            &world,
            &self.mesh_artifacts,
            &terrain_meshes,
            self.config.planner,
        )?;
        let dynamic_meshes = terrain_meshes
            .values()
            .map(terrain_artifact_to_mesh)
            .chain(decal_meshes.values().map(decal_artifact_to_mesh))
            .collect::<Vec<_>>();
        let mut render_feature_registry = build_render_feature_registry(
            &self.render_feature_wires,
            &self.render_feature_factories,
        )?;

        let mut target_commands =
            BTreeMap::<voplay_runtime::control::StableControlRef, Vec<DrawCommand>>::new();
        let mut planner = Render3dPlanner::new(self.config.planner)
            .map_err(|_| FrameEncoderError::InvalidInput)?;
        for mesh in self.meshes.values().copied() {
            planner
                .register_mesh(mesh)
                .map_err(|_| FrameEncoderError::InvalidInput)?;
        }
        for material in self.materials.values().copied() {
            planner
                .register_material(material)
                .map_err(|_| FrameEncoderError::InvalidInput)?;
        }
        let mut scene_views = Vec::new();
        let mut encoded_views = 0_usize;
        for view in &control.views {
            let camera_entity = RenderEntity {
                engine: world.engine(),
                entity: view.descriptor.camera.entity,
            };
            if world.object(camera_entity).is_none() && self.config.allow_missing_cameras {
                continue;
            }
            let camera = build_camera_frame(
                &world,
                camera_entity,
                view.descriptor.viewport.width,
                view.descriptor.viewport.height,
            )
            .map_err(|_| FrameEncoderError::InvalidInput)?;
            let visibility = ViewVisibility {
                view: view.view.handle,
                view_revision: control.control_revision,
                world_revision: world.revision(),
                visible: world
                    .objects()
                    .filter(|object| {
                        object.entity != camera_entity
                            && object.layers & view.descriptor.layer_mask != 0
                    })
                    .map(|object| object.entity)
                    .collect(),
            };
            let plan = planner
                .build(
                    &world,
                    &visibility,
                    camera.position_milli,
                    view.descriptor.layer_mask,
                )
                .map_err(|_| FrameEncoderError::InvalidInput)?;
            let resolved_target = control
                .targets
                .iter()
                .find(|target| target.target == view.target)
                .ok_or(FrameEncoderError::InvalidInput)?;
            let mut resolver = RetainedFeatureResolver {
                terrain_meshes: &terrain_meshes,
                decal_meshes: &decal_meshes,
            };
            let features = feature_scene
                .compose_features(view.descriptor.layer_mask, &mut resolver)
                .map_err(|_| FrameEncoderError::InvalidInput)?;
            let shadow_cascades = if plan.shadow_casters.is_empty() {
                Vec::new()
            } else {
                build_directional_shadow_cascades(
                    camera,
                    &plan.lights,
                    resolved_target.width,
                    resolved_target.height,
                    2_048,
                    4,
                )
                .map_err(|_| FrameEncoderError::InvalidInput)?
            };
            let render_features = realize_data_render_features(
                render_feature_registry.as_mut(),
                request.graph_signature,
                view.descriptor.graph_template,
            )?;
            scene_views.push(SceneViewFrame3d {
                target: view.target,
                width: resolved_target.width,
                height: resolved_target.height,
                viewport: [
                    view.descriptor.viewport.x,
                    view.descriptor.viewport.y,
                    view.descriptor.viewport.width,
                    view.descriptor.viewport.height,
                ],
                external_surface: resolved_target.external_surface,
                color_format: resolved_target.color_format,
                depth_format: resolved_target.depth_format,
                sample_count: resolved_target.sample_count,
                usage: resolved_target.usage,
                clear_color: clear_color_f32(view.descriptor.clear),
                camera_position_milli: camera.position_milli,
                view_projection: camera.view_projection,
                plan,
                features,
                render_features,
                shadow_cascades,
            });
            let mut commands = Vec::new();
            encode_view(
                &world,
                view,
                self.config.planner,
                &self.meshes,
                &self.materials,
                &mut commands,
            )?;
            target_commands
                .entry(view.target)
                .or_default()
                .extend(commands);
            encoded_views += 1;
            if target_commands.values().map(Vec::len).sum::<usize>() > self.config.max_commands {
                return Err(FrameEncoderError::Capacity);
            }
        }
        if encoded_views == 0 && !self.config.allow_missing_cameras {
            return Err(FrameEncoderError::InvalidInput);
        }
        if let Some(transient) = request.transient {
            let target = control
                .targets
                .iter()
                .filter(|target| target.external_surface)
                .map(|target| target.target)
                .min()
                .or_else(|| control.targets.first().map(|target| target.target))
                .ok_or(FrameEncoderError::InvalidInput)?;
            target_commands
                .entry(target)
                .or_default()
                .extend(decode_object_commands(&transient.bytes)?);
        }
        if target_commands.values().map(Vec::len).sum::<usize>() > self.config.max_commands {
            return Err(FrameEncoderError::Capacity);
        }
        let fallback_commands = if target_commands.is_empty() {
            encode_frame_commands(&[], self.config.max_encoded_bytes)?
        } else {
            let targets = target_commands
                .into_iter()
                .map(|(target, commands)| {
                    let resolved = control
                        .targets
                        .iter()
                        .find(|resolved| resolved.target == target)
                        .ok_or(FrameEncoderError::InvalidInput)?;
                    Ok(TargetDrawCommands {
                        target,
                        width: resolved.width,
                        height: resolved.height,
                        external_surface: resolved.external_surface,
                        color_format: resolved.color_format,
                        depth_format: resolved.depth_format,
                        sample_count: resolved.sample_count,
                        usage: resolved.usage,
                        commands,
                    })
                })
                .collect::<Result<Vec<_>, FrameEncoderError>>()?;
            encode_target_frame_bundle(
                &targets,
                control.targets.len().max(1),
                self.config.max_commands,
                self.config.max_encoded_bytes,
            )?
        };
        let bytes = if scene_views.is_empty() {
            fallback_commands
        } else {
            encode_scene_frame_3d(
                &SceneFrame3d {
                    fallback_commands,
                    native_overlay_commands: encode_frame_commands(
                        &[],
                        self.config.max_encoded_bytes,
                    )?,
                    dynamic_meshes,
                    views: scene_views,
                },
                control.views.len().max(1),
                self.config.max_commands,
                self.config.planner.max_lights,
                self.config.max_encoded_bytes,
            )
            .map_err(|error| match error {
                crate::SceneFrame3dError::Capacity => FrameEncoderError::Capacity,
                _ => FrameEncoderError::InvalidInput,
            })?
        };
        self.world = world;
        self.feature_scene = feature_scene;
        self.feature_signature = next_feature_signature;
        self.feature_tick = feature_tick;
        Ok(bytes)
    }
}

fn build_render_feature_registry(
    wires: &[Vec<u8>],
    factories: &BTreeMap<
        voplay_runtime::render_feature::FeatureFactoryId,
        RenderFeatureFactoryBuilder,
    >,
) -> Result<Option<voplay_runtime::render_feature::RenderFeatureRegistry>, FrameEncoderError> {
    use voplay_runtime::render_feature::{
        FeatureRegistryConfig, RenderFeatureRegistration, RenderFeatureRegistry,
    };
    if wires.is_empty() {
        return Ok(None);
    }
    let mut registrations = Vec::with_capacity(wires.len());
    let mut capabilities = BTreeSet::new();
    for wire in wires {
        let registration = voplay_runtime::render_feature::decode_feature_registration(
            wire,
            FeatureRegistryConfig::default(),
        )
        .map_err(|_| FrameEncoderError::InvalidInput)?;
        let descriptor = match &registration {
            RenderFeatureRegistration::Compiled { descriptor, .. } => descriptor,
            RenderFeatureRegistration::Data(source) => &source.descriptor,
        };
        capabilities.extend(descriptor.required_capabilities.iter().copied());
        registrations.push(registration);
    }
    let mut registry = RenderFeatureRegistry::new(FeatureRegistryConfig::default(), capabilities)
        .map_err(|_| FrameEncoderError::InvalidInput)?;
    for builder in factories.values() {
        registry
            .register_factory(builder())
            .map_err(|_| FrameEncoderError::InvalidInput)?;
    }
    for registration in registrations {
        match registration {
            RenderFeatureRegistration::Data(source) => registry
                .register_data(source)
                .map_err(|_| FrameEncoderError::InvalidInput)?,
            RenderFeatureRegistration::Compiled {
                descriptor,
                closure,
            } => registry
                .register_compiled(descriptor, closure)
                .map_err(|_| FrameEncoderError::InvalidInput)?,
        }
    }
    registry
        .freeze()
        .map_err(|_| FrameEncoderError::InvalidInput)?;
    Ok(Some(registry))
}

fn realize_data_render_features(
    registry: Option<&mut voplay_runtime::render_feature::RenderFeatureRegistry>,
    graph_signature: u64,
    material_variant: u64,
) -> Result<Vec<voplay_runtime::render_feature::RealizedFeature>, FrameEncoderError> {
    use voplay_runtime::render_feature::PipelineSpecialization;
    let Some(registry) = registry else {
        return Ok(Vec::new());
    };
    let features = registry.feature_ids().collect::<Vec<_>>();
    features
        .into_iter()
        .map(|feature| {
            registry
                .realize(PipelineSpecialization {
                    feature,
                    graph_signature,
                    target_format: 1,
                    sample_count: 1,
                    material_variant,
                })
                .cloned()
                .map_err(|_| FrameEncoderError::InvalidInput)
        })
        .collect()
}

struct RetainedFeatureResolver<'a> {
    terrain_meshes: &'a BTreeMap<u64, TerrainMeshArtifact3d>,
    decal_meshes: &'a BTreeMap<u64, ProjectedDecalMesh3d>,
}

impl PortableFeatureResolver3d for RetainedFeatureResolver<'_> {
    fn terrain_draw(
        &mut self,
        patch: &WireTerrainPatch3d,
    ) -> Result<(u64, [i64; 12]), Wire3dError> {
        let artifact = self
            .terrain_meshes
            .get(&patch.id)
            .ok_or(Wire3dError::InvalidDescriptor)?;
        Ok((artifact.mesh, artifact.model_milli))
    }

    fn decal_draw(&mut self, decal: &WireDecal3d) -> Result<Option<(u64, [i64; 12])>, Wire3dError> {
        Ok(self
            .decal_meshes
            .get(&decal.id)
            .map(|artifact| (artifact.mesh, artifact.model_milli)))
    }
}

fn build_terrain_meshes(
    scene: &PortableScene3d,
    heightmaps: &BTreeMap<u64, HeightmapArtifact3d>,
) -> Result<BTreeMap<u64, TerrainMeshArtifact3d>, FrameEncoderError> {
    let mut meshes = BTreeMap::new();
    for patch in scene.terrain_patches() {
        let heightmap = heightmaps
            .get(&patch.heightmap)
            .ok_or(FrameEncoderError::InvalidInput)?;
        let artifact = build_terrain_mesh_3d(
            patch,
            TerrainHeightmap3d {
                width: heightmap.width,
                height: heightmap.height,
                samples: &heightmap.samples,
            },
        )
        .map_err(|_| FrameEncoderError::InvalidInput)?;
        meshes.insert(patch.id, artifact);
    }
    Ok(meshes)
}

fn terrain_artifact_to_mesh(artifact: &TerrainMeshArtifact3d) -> MeshArtifact3d {
    MeshArtifact3d {
        descriptor: MeshDescriptor {
            id: artifact.mesh,
            vertex_count: artifact.positions.len() as u32,
            index_count: artifact.indices.len() as u32,
            skinned: false,
        },
        positions: artifact.positions.clone(),
        normals: artifact.normals.clone(),
        texcoords: artifact.texcoords.clone(),
        joints: None,
        weights: None,
        indices: artifact.indices.clone(),
    }
}

fn build_decal_meshes(
    scene: &PortableScene3d,
    world: &RenderWorld,
    mesh_artifacts: &BTreeMap<u64, MeshArtifact3d>,
    terrain_meshes: &BTreeMap<u64, TerrainMeshArtifact3d>,
    planner: Render3dPlannerConfig,
) -> Result<BTreeMap<u64, ProjectedDecalMesh3d>, FrameEncoderError> {
    let mut result = BTreeMap::new();
    for decal in scene.decals() {
        let mut combined = ProjectedDecalMesh3d {
            mesh: projected_decal_mesh_id(decal, u64::MAX),
            model_milli: [1_000, 0, 0, 0, 0, 1_000, 0, 0, 0, 0, 1_000, 0],
            positions: Vec::new(),
            normals: Vec::new(),
            texcoords: Vec::new(),
            indices: Vec::new(),
        };
        for object in world.objects() {
            if object.layers & decal.layers == 0 {
                continue;
            }
            let Some(component) = object.components.get(&MESH_INSTANCE_COMPONENT) else {
                continue;
            };
            let instance = decode_mesh_instance(&component.bytes, planner)
                .map_err(|_| FrameEncoderError::InvalidInput)?;
            let Some(mesh) = instance
                .lods
                .first()
                .and_then(|lod| mesh_artifacts.get(&lod.mesh))
            else {
                continue;
            };
            let projected = project_decal_mesh_3d(
                decal,
                DecalTargetMesh3d {
                    id: packed_entity_handle(object.entity),
                    model_milli: object.transform.current,
                    positions: &mesh.positions,
                    normals: &mesh.normals,
                    indices: &mesh.indices,
                },
            )
            .map_err(|_| FrameEncoderError::InvalidInput)?;
            append_projected_decal(&mut combined, projected)?;
        }
        for terrain in terrain_meshes.values() {
            let projected = project_decal_mesh_3d(
                decal,
                DecalTargetMesh3d {
                    id: terrain.mesh,
                    model_milli: terrain.model_milli,
                    positions: &terrain.positions,
                    normals: &terrain.normals,
                    indices: &terrain.indices,
                },
            )
            .map_err(|_| FrameEncoderError::InvalidInput)?;
            append_projected_decal(&mut combined, projected)?;
        }
        if !combined.indices.is_empty() {
            result.insert(decal.id, combined);
        }
    }
    Ok(result)
}

fn append_projected_decal(
    destination: &mut ProjectedDecalMesh3d,
    source: ProjectedDecalMesh3d,
) -> Result<(), FrameEncoderError> {
    let base =
        u32::try_from(destination.positions.len()).map_err(|_| FrameEncoderError::Capacity)?;
    destination.positions.extend(source.positions);
    destination.normals.extend(source.normals);
    destination.texcoords.extend(source.texcoords);
    destination.indices.reserve(source.indices.len());
    for index in source.indices {
        destination
            .indices
            .push(base.checked_add(index).ok_or(FrameEncoderError::Capacity)?);
    }
    Ok(())
}

fn decal_artifact_to_mesh(artifact: &ProjectedDecalMesh3d) -> MeshArtifact3d {
    MeshArtifact3d {
        descriptor: MeshDescriptor {
            id: artifact.mesh,
            vertex_count: artifact.positions.len() as u32,
            index_count: artifact.indices.len() as u32,
            skinned: false,
        },
        positions: artifact.positions.clone(),
        normals: artifact.normals.clone(),
        texcoords: artifact.texcoords.clone(),
        joints: None,
        weights: None,
        indices: artifact.indices.clone(),
    }
}

fn rebuild_feature_scene(
    world: &RenderWorld,
    material_wires: &BTreeMap<u64, Vec<u8>>,
) -> Result<PortableScene3d, FrameEncoderError> {
    let mut scene = PortableScene3d::new(PortableScene3dConfig::default())
        .map_err(|_| FrameEncoderError::InvalidInput)?;
    for bytes in material_wires.values() {
        scene
            .apply_material(bytes)
            .map_err(|_| FrameEncoderError::InvalidInput)?;
    }
    for object in world.objects() {
        for component in object.components.values() {
            match component.bytes.get(..4) {
                Some(b"VR31") => scene.apply_terrain(&component.bytes),
                Some(b"VD31") => scene.apply_decal(&component.bytes),
                Some(b"VS31") => scene.apply_scatter(&component.bytes),
                Some(b"VP31") => scene.apply_particle_emitter(&component.bytes),
                Some(b"VE31") => scene.apply_environment(&component.bytes),
                _ => continue,
            }
            .map_err(|_| FrameEncoderError::InvalidInput)?;
        }
    }
    Ok(scene)
}

fn feature_signature(world: &RenderWorld, material_wires: &BTreeMap<u64, Vec<u8>>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for (id, bytes) in material_wires {
        hash_bytes(&mut hash, &id.to_le_bytes());
        hash_bytes(&mut hash, bytes);
    }
    for object in world.objects() {
        for component in object.components.values() {
            if matches!(
                component.bytes.get(..4),
                Some(b"VR31" | b"VD31" | b"VS31" | b"VP31" | b"VE31")
            ) {
                hash_bytes(&mut hash, &object.entity.entity.index.to_le_bytes());
                hash_bytes(&mut hash, &object.entity.entity.generation.to_le_bytes());
                hash_bytes(&mut hash, &component.kind.0.to_le_bytes());
                hash_bytes(&mut hash, &component.bytes);
            }
        }
    }
    hash.max(1)
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
    }
}

fn encode_view(
    world: &RenderWorld,
    view: &ResolvedRenderViewFrame,
    planner: Render3dPlannerConfig,
    meshes: &BTreeMap<u64, MeshDescriptor>,
    materials: &BTreeMap<u64, MaterialDescriptor>,
    commands: &mut Vec<DrawCommand>,
) -> Result<(), FrameEncoderError> {
    let viewport = view.descriptor.viewport;
    let camera = RenderEntity {
        engine: world.engine(),
        entity: view.descriptor.camera.entity,
    };
    let camera = build_camera_frame(world, camera, viewport.width, viewport.height)
        .map_err(|_| FrameEncoderError::InvalidInput)?;
    commands.push(DrawCommand::SolidRect {
        x: viewport.x,
        y: viewport.y,
        width: viewport.width,
        height: viewport.height,
        color: clear_color(view.descriptor.clear),
    });

    let mut draws = Vec::new();
    let mut lights = Vec::new();
    for object in world.objects() {
        if object.entity == camera.entity || object.layers & view.descriptor.layer_mask == 0 {
            continue;
        }
        if let Some(component) = object.components.get(&MESH_INSTANCE_COMPONENT) {
            let instance = decode_mesh_instance(&component.bytes, planner)
                .map_err(|_| FrameEncoderError::InvalidInput)?;
            if instance.layers & view.descriptor.layer_mask != 0 {
                if !instance
                    .lods
                    .iter()
                    .any(|lod| meshes.contains_key(&lod.mesh))
                {
                    return Err(FrameEncoderError::InvalidInput);
                }
                let material = materials
                    .get(&instance.material)
                    .ok_or(FrameEncoderError::InvalidInput)?;
                if let Some(rect) = project_bounds(&camera, object.bounds, viewport) {
                    draws.push((
                        object.transform.current[11],
                        object.entity,
                        DrawCommand::SolidRect {
                            x: rect[0],
                            y: rect[1],
                            width: rect[2],
                            height: rect[3],
                            color: material.base_color,
                        },
                    ));
                }
            }
        }
        if let Some(component) = object.components.get(&LIGHT_COMPONENT) {
            let light =
                decode_light(&component.bytes).map_err(|_| FrameEncoderError::InvalidInput)?;
            let position = [
                object.transform.current[3],
                object.transform.current[7],
                object.transform.current[11],
            ];
            if let Some(point) = project_point(&camera, position, viewport) {
                lights.push(DrawCommand::SolidRect {
                    x: point[0].saturating_sub(2),
                    y: point[1].saturating_sub(2),
                    width: 5,
                    height: 5,
                    color: [
                        q16_to_u8(light.color[0]),
                        q16_to_u8(light.color[1]),
                        q16_to_u8(light.color[2]),
                        255,
                    ],
                });
            }
        }
    }
    draws.sort_by_key(|(depth, entity, _)| (*depth, *entity));
    commands.extend(draws.into_iter().map(|(_, _, command)| command));
    commands.extend(lights);
    Ok(())
}

fn clear_color(clear: ClearPolicy) -> [u8; 4] {
    match clear {
        ClearPolicy::ClearColor(color) => color.map(q16_to_u8),
        ClearPolicy::Discard => [0, 0, 0, 0],
        ClearPolicy::Load | ClearPolicy::ClearDepth(_) => [0, 0, 0, 255],
    }
}

fn clear_color_f32(clear: ClearPolicy) -> [f32; 4] {
    clear_color(clear).map(|value| f32::from(value) / 255.0)
}

fn project_bounds(
    camera: &CameraFrame3d,
    bounds: Aabb3,
    viewport: voplay_runtime::render_control::Viewport,
) -> Option<[u32; 4]> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut projected = 0_u8;
    for x in [bounds.min[0], bounds.max[0]] {
        for y in [bounds.min[1], bounds.max[1]] {
            for z in [bounds.min[2], bounds.max[2]] {
                let Some(ndc) = project_ndc(camera, [x, y, z]) else {
                    continue;
                };
                min[0] = min[0].min(ndc[0]);
                min[1] = min[1].min(ndc[1]);
                max[0] = max[0].max(ndc[0]);
                max[1] = max[1].max(ndc[1]);
                projected += 1;
            }
        }
    }
    if projected == 0 || max[0] < -1.0 || min[0] > 1.0 || max[1] < -1.0 || min[1] > 1.0 {
        return None;
    }
    let min = [min[0].clamp(-1.0, 1.0), min[1].clamp(-1.0, 1.0)];
    let max = [max[0].clamp(-1.0, 1.0), max[1].clamp(-1.0, 1.0)];
    let left = viewport.x + (((min[0] + 1.0) * 0.5) * viewport.width as f32) as u32;
    let right = viewport.x + (((max[0] + 1.0) * 0.5) * viewport.width as f32) as u32;
    let top = viewport.y + (((1.0 - max[1]) * 0.5) * viewport.height as f32) as u32;
    let bottom = viewport.y + (((1.0 - min[1]) * 0.5) * viewport.height as f32) as u32;
    Some([
        left,
        top,
        right.saturating_sub(left).max(1),
        bottom.saturating_sub(top).max(1),
    ])
}

fn project_point(
    camera: &CameraFrame3d,
    point: [i64; 3],
    viewport: voplay_runtime::render_control::Viewport,
) -> Option<[u32; 2]> {
    let ndc = project_ndc(camera, point)?;
    if !(-1.0..=1.0).contains(&ndc[0]) || !(-1.0..=1.0).contains(&ndc[1]) {
        return None;
    }
    Some([
        viewport.x + (((ndc[0] + 1.0) * 0.5) * viewport.width as f32) as u32,
        viewport.y + (((1.0 - ndc[1]) * 0.5) * viewport.height as f32) as u32,
    ])
}

fn project_ndc(camera: &CameraFrame3d, point_milli: [i64; 3]) -> Option<[f32; 2]> {
    let point = [
        point_milli[0] as f32 / 1_000.0,
        point_milli[1] as f32 / 1_000.0,
        point_milli[2] as f32 / 1_000.0,
        1.0,
    ];
    let mut clip = [0.0; 4];
    for row in 0..4 {
        clip[row] = (0..4)
            .map(|column| camera.view_projection[column][row] * point[column])
            .sum();
    }
    if !clip.iter().all(|value| value.is_finite()) || clip[3] <= f32::EPSILON {
        return None;
    }
    Some([clip[0] / clip[3], clip[1] / clip[3]])
}

fn q16_to_u8(value: u16) -> u8 {
    ((u32::from(value) * 255 + 32_767) / 65_535) as u8
}

fn map_world_error(error: RenderWorldError) -> FrameEncoderError {
    match error {
        RenderWorldError::ObjectCapacity
        | RenderWorldError::ComponentCapacity
        | RenderWorldError::ComponentByteCapacity
        | RenderWorldError::TotalByteCapacity
        | RenderWorldError::DirtyCapacity
        | RenderWorldError::ViewCapacity
        | RenderWorldError::SpatialCapacity => FrameEncoderError::Capacity,
        _ => FrameEncoderError::InvalidInput,
    }
}
