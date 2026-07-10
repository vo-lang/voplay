use super::*;

fn resolve_texture_id(
    textures: &TextureManager,
    preferred: u32,
    fallback: Option<u32>,
) -> (u32, RenderSkipStats) {
    let resolved = crate::texture::resolve_texture_reference(preferred, fallback, |id| {
        textures.get(id).is_some()
    });
    (
        resolved.texture_id,
        RenderSkipStats {
            missing_textures: resolved.missing_count,
            fallback_paths: resolved.fallback_path_count,
            ..RenderSkipStats::default()
        },
    )
}

pub(super) fn resolve_texture_key(
    material: &MaterialOverride,
    mesh_material: &MeshMaterial,
    textures: &TextureManager,
) -> (PrimitiveTextureKey, RenderSkipStats) {
    let (albedo, albedo_skips) = resolve_texture_id(
        textures,
        material.albedo_texture_id,
        mesh_material.texture_id,
    );
    let (normal, normal_skips) = resolve_texture_id(
        textures,
        material.normal_texture_id,
        mesh_material.normal_texture_id,
    );
    let (metallic_roughness, metallic_roughness_skips) = resolve_texture_id(
        textures,
        material.metallic_roughness_texture_id,
        mesh_material.metallic_roughness_texture_id,
    );
    let (emissive, emissive_skips) = resolve_texture_id(
        textures,
        material.emissive_texture_id,
        mesh_material.emissive_texture_id,
    );
    let (toon_ramp, toon_ramp_skips) = resolve_texture_id(
        textures,
        material.toon_ramp_texture_id,
        mesh_material.toon_ramp_texture_id,
    );
    let key = PrimitiveTextureKey {
        albedo,
        normal,
        metallic_roughness,
        emissive,
        toon_ramp,
        sampler: MaterialSamplerKey::resolve(
            material.wrap_mode,
            material.filter_mode,
            mesh_material.sampler,
        ),
    };
    let mut skips = RenderSkipStats::default();
    for field_skips in [
        albedo_skips,
        normal_skips,
        metallic_roughness_skips,
        emissive_skips,
        toon_ramp_skips,
    ] {
        skips.merge(field_skips);
    }
    (key, skips)
}
