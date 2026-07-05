pub(crate) const STATIC_MESH_SHADER: &str = include_str!("../shaders/mesh.wgsl");
pub(crate) const TERRAIN_MESH_SHADER: &str = include_str!("../shaders/mesh_terrain.wgsl");
pub(crate) const SKINNED_MESH_SHADER: &str = include_str!("../shaders/mesh_skinned.wgsl");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShaderLibrary;

impl ShaderLibrary {
    pub(crate) fn load_static_mesh_source() -> &'static str {
        STATIC_MESH_SHADER
    }

    pub(crate) fn load_terrain_mesh_source() -> &'static str {
        TERRAIN_MESH_SHADER
    }

    pub(crate) fn load_skinned_mesh_source() -> &'static str {
        SKINNED_MESH_SHADER
    }
}
