use std::collections::BTreeMap;

use voplay_protocol::EngineId;

use crate::{
    decode_heightmap_artifact_3d, decode_material_3d, decode_mesh_artifact_3d,
    decode_skin_palette_artifact_3d, packed_entity_handle, HeightmapArtifact3dConfig,
    MeshArtifact3dConfig, SkinPaletteArtifact3dConfig, SkinPaletteUpload3d, WgpuSceneRenderer,
    WgpuSceneRendererError, BUILTIN_DECAL_MESH_3D, BUILTIN_TERRAIN_MESH_3D,
    RENDER_ASSET_HEIGHTMAP_3D, RENDER_ASSET_MATERIAL_3D, RENDER_ASSET_MESH_3D,
    RENDER_ASSET_SKIN_PALETTE_3D,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WgpuAssetResidency3dConfig {
    pub max_assets: usize,
    pub max_bytes: usize,
    pub mesh: MeshArtifact3dConfig,
    pub skin_palette: SkinPaletteArtifact3dConfig,
}

impl Default for WgpuAssetResidency3dConfig {
    fn default() -> Self {
        Self {
            max_assets: 65_536,
            max_bytes: 1024 * 1024 * 1024,
            mesh: MeshArtifact3dConfig::default(),
            skin_palette: SkinPaletteArtifact3dConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WgpuAssetResidency3dError {
    InvalidConfig,
    InvalidAsset,
    StaleRevision,
    Capacity,
    ByteCapacity,
    Renderer(WgpuSceneRendererError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WgpuAssetResidency3dMetrics {
    pub live_assets: usize,
    pub peak_live_assets: usize,
    pub desired_bytes: usize,
    pub peak_desired_bytes: usize,
    pub upserts: u64,
    pub removals: u64,
    pub recovery_uploads: u64,
    pub capacity_rejections: u64,
    pub stale_rejections: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WgpuAssetResidency3dOwnerSnapshot {
    pub engine: EngineId,
    pub metrics: WgpuAssetResidency3dMetrics,
}

impl From<WgpuSceneRendererError> for WgpuAssetResidency3dError {
    fn from(value: WgpuSceneRendererError) -> Self {
        Self::Renderer(value)
    }
}

#[derive(Clone)]
struct DesiredAsset {
    revision: u64,
    bytes: Vec<u8>,
}

pub struct WgpuAssetResidency3d {
    engine: EngineId,
    config: WgpuAssetResidency3dConfig,
    revisions: BTreeMap<(u32, u64), u64>,
    desired: BTreeMap<(u32, u64), DesiredAsset>,
    desired_bytes: usize,
    metrics: WgpuAssetResidency3dMetrics,
}

impl WgpuAssetResidency3d {
    pub fn new(
        engine: EngineId,
        config: WgpuAssetResidency3dConfig,
    ) -> Result<Self, WgpuAssetResidency3dError> {
        if !engine.is_valid() || config.max_assets == 0 || config.max_bytes == 0 {
            return Err(WgpuAssetResidency3dError::InvalidConfig);
        }
        Ok(Self {
            engine,
            config,
            revisions: BTreeMap::new(),
            desired: BTreeMap::new(),
            desired_bytes: 0,
            metrics: WgpuAssetResidency3dMetrics::default(),
        })
    }

    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    pub fn revision(&self, kind: u32, asset: u64) -> Option<u64> {
        self.revisions.get(&(kind, asset)).copied()
    }

    pub const fn desired_bytes(&self) -> usize {
        self.desired_bytes
    }

    pub const fn owner_snapshot(&self) -> WgpuAssetResidency3dOwnerSnapshot {
        WgpuAssetResidency3dOwnerSnapshot {
            engine: self.engine,
            metrics: self.metrics,
        }
    }

    pub fn shutdown(&mut self) -> WgpuAssetResidency3dOwnerSnapshot {
        let before = self.owner_snapshot();
        self.revisions.clear();
        self.desired.clear();
        self.desired_bytes = 0;
        self.metrics.live_assets = 0;
        self.metrics.desired_bytes = 0;
        before
    }

    pub fn upsert(
        &mut self,
        renderer: &mut WgpuSceneRenderer,
        kind: u32,
        asset: u64,
        revision: u64,
        bytes: &[u8],
    ) -> Result<(), WgpuAssetResidency3dError> {
        self.validate_revision(kind, asset, revision)?;
        if bytes.is_empty() {
            return Err(WgpuAssetResidency3dError::InvalidAsset);
        }
        let key = (kind, asset);
        let previous_bytes = self.desired.get(&key).map_or(0, |entry| entry.bytes.len());
        if (!self.desired.contains_key(&key) && self.desired.len() == self.config.max_assets)
            || self
                .desired_bytes
                .checked_sub(previous_bytes)
                .and_then(|total| total.checked_add(bytes.len()))
                .is_none_or(|total| total > self.config.max_bytes)
        {
            self.metrics.capacity_rejections = self.metrics.capacity_rejections.saturating_add(1);
            return Err(if self.desired.len() == self.config.max_assets {
                WgpuAssetResidency3dError::Capacity
            } else {
                WgpuAssetResidency3dError::ByteCapacity
            });
        }
        apply_upsert(renderer, self.engine, self.config, kind, asset, bytes)?;
        self.desired_bytes = self.desired_bytes - previous_bytes + bytes.len();
        self.desired.insert(
            key,
            DesiredAsset {
                revision,
                bytes: bytes.to_vec(),
            },
        );
        self.revisions.insert(key, revision);
        self.metrics.live_assets = self.desired.len();
        self.metrics.peak_live_assets = self.metrics.peak_live_assets.max(self.metrics.live_assets);
        self.metrics.desired_bytes = self.desired_bytes;
        self.metrics.peak_desired_bytes = self
            .metrics
            .peak_desired_bytes
            .max(self.metrics.desired_bytes);
        self.metrics.upserts = self.metrics.upserts.saturating_add(1);
        Ok(())
    }

    pub fn remove(
        &mut self,
        renderer: &mut WgpuSceneRenderer,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), WgpuAssetResidency3dError> {
        self.validate_revision(kind, asset, revision)?;
        apply_remove(renderer, self.engine, kind, asset)?;
        if let Some(previous) = self.desired.remove(&(kind, asset)) {
            self.desired_bytes -= previous.bytes.len();
        }
        self.revisions.insert((kind, asset), revision);
        self.metrics.live_assets = self.desired.len();
        self.metrics.desired_bytes = self.desired_bytes;
        self.metrics.removals = self.metrics.removals.saturating_add(1);
        Ok(())
    }

    pub fn recover(
        &mut self,
        renderer: &mut WgpuSceneRenderer,
    ) -> Result<(), WgpuAssetResidency3dError> {
        for ((kind, asset), desired) in &self.desired {
            debug_assert_eq!(
                self.revisions.get(&(*kind, *asset)),
                Some(&desired.revision)
            );
            apply_upsert(
                renderer,
                self.engine,
                self.config,
                *kind,
                *asset,
                &desired.bytes,
            )?;
        }
        self.metrics.recovery_uploads = self
            .metrics
            .recovery_uploads
            .saturating_add(self.desired.len() as u64);
        Ok(())
    }

    pub fn realize_remove(
        &self,
        renderer: &mut WgpuSceneRenderer,
        kind: u32,
        asset: u64,
    ) -> Result<(), WgpuAssetResidency3dError> {
        apply_remove(renderer, self.engine, kind, asset)
    }

    fn validate_revision(
        &mut self,
        kind: u32,
        asset: u64,
        revision: u64,
    ) -> Result<(), WgpuAssetResidency3dError> {
        if asset == 0
            || revision == 0
            || (kind == RENDER_ASSET_MESH_3D
                && matches!(asset, BUILTIN_TERRAIN_MESH_3D | BUILTIN_DECAL_MESH_3D))
            || !matches!(
                kind,
                RENDER_ASSET_MESH_3D
                    | RENDER_ASSET_MATERIAL_3D
                    | RENDER_ASSET_SKIN_PALETTE_3D
                    | RENDER_ASSET_HEIGHTMAP_3D
            )
        {
            return Err(WgpuAssetResidency3dError::InvalidAsset);
        }
        if self
            .revisions
            .get(&(kind, asset))
            .is_some_and(|current| *current >= revision)
        {
            self.metrics.stale_rejections = self.metrics.stale_rejections.saturating_add(1);
            return Err(WgpuAssetResidency3dError::StaleRevision);
        }
        Ok(())
    }
}

fn apply_upsert(
    renderer: &mut WgpuSceneRenderer,
    engine: EngineId,
    config: WgpuAssetResidency3dConfig,
    kind: u32,
    asset: u64,
    bytes: &[u8],
) -> Result<(), WgpuAssetResidency3dError> {
    match kind {
        RENDER_ASSET_MESH_3D => {
            let artifact = decode_mesh_artifact_3d(bytes, config.mesh)
                .map_err(|_| WgpuAssetResidency3dError::InvalidAsset)?;
            if artifact.descriptor.id != asset {
                return Err(WgpuAssetResidency3dError::InvalidAsset);
            }
            renderer.upsert_mesh_artifact(&artifact)?;
        }
        RENDER_ASSET_MATERIAL_3D => {
            let wire =
                decode_material_3d(bytes).map_err(|_| WgpuAssetResidency3dError::InvalidAsset)?;
            if wire.id != asset {
                return Err(WgpuAssetResidency3dError::InvalidAsset);
            }
            let pipeline_key = 1
                | (u64::from(matches!(wire.alpha, crate::WireAlphaMode3d::Blend)) << 1)
                | (u64::from(wire.double_sided) << 2)
                | (u64::from(wire.unlit) << 3);
            renderer.upsert_material(
                wire.material_descriptor(pipeline_key)
                    .map_err(|_| WgpuAssetResidency3dError::InvalidAsset)?,
            )?;
        }
        RENDER_ASSET_SKIN_PALETTE_3D => {
            let artifact = decode_skin_palette_artifact_3d(bytes, engine, config.skin_palette)
                .map_err(|_| WgpuAssetResidency3dError::InvalidAsset)?;
            if packed_entity_handle(artifact.entity) != asset {
                return Err(WgpuAssetResidency3dError::InvalidAsset);
            }
            renderer.upload_skin_palette(SkinPaletteUpload3d {
                entity: artifact.entity,
                joint_matrices: &artifact.joint_matrices,
            })?;
        }
        RENDER_ASSET_HEIGHTMAP_3D => {
            let artifact =
                decode_heightmap_artifact_3d(bytes, HeightmapArtifact3dConfig::default())
                    .map_err(|_| WgpuAssetResidency3dError::InvalidAsset)?;
            if artifact.id != asset {
                return Err(WgpuAssetResidency3dError::InvalidAsset);
            }
        }
        _ => return Err(WgpuAssetResidency3dError::InvalidAsset),
    }
    Ok(())
}

fn apply_remove(
    renderer: &mut WgpuSceneRenderer,
    engine: EngineId,
    kind: u32,
    asset: u64,
) -> Result<(), WgpuAssetResidency3dError> {
    match kind {
        RENDER_ASSET_MESH_3D => {
            renderer.remove_mesh(asset);
        }
        RENDER_ASSET_MATERIAL_3D => {
            renderer.remove_material(asset);
        }
        RENDER_ASSET_SKIN_PALETTE_3D => {
            renderer.remove_skin_palette(voplay_runtime::RenderEntity {
                engine,
                entity: voplay_protocol::Handle {
                    index: asset as u32,
                    generation: (asset >> 32) as u32,
                },
            });
        }
        RENDER_ASSET_HEIGHTMAP_3D => {}
        _ => return Err(WgpuAssetResidency3dError::InvalidAsset),
    }
    Ok(())
}
