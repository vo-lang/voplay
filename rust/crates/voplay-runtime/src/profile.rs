use std::collections::{BTreeMap, BTreeSet};

use crate::supervisor::Role;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProfileAlias {
    Core,
    TwoD,
    ThreeD,
    Full,
    Editor,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    Core,
    Render2d,
    Text,
    Image,
    Render3d,
    Gltf,
    Physics2d,
    Physics3d,
    Animation,
    Audio,
    Pack,
    Readback,
    Inspection,
    FrameDebugCapture,
    ShaderDiagnostics,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PlacementClass {
    LogicIsland,
    AssetIsland,
    RenderIsland,
    AudioIsland,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleArtifact {
    pub role: Role,
    pub placement: PlacementClass,
    pub artifact_name: String,
    pub target: String,
    pub digest: [u8; 32],
    pub schema_fingerprint: [u8; 32],
    pub shader_abi: Option<[u8; 32]>,
    pub realtime_abi: Option<[u8; 32]>,
    pub capabilities: BTreeSet<Capability>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProfile {
    pub alias: ProfileAlias,
    pub capabilities: BTreeSet<Capability>,
    pub required_roles: BTreeSet<Role>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildRecipe {
    pub profile: ResolvedProfile,
    pub vo_packages: BTreeSet<&'static str>,
    pub rust_crates: BTreeSet<&'static str>,
    pub web_chunks: BTreeSet<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiMinimalRenderRecipe {
    pub published: bool,
    pub capabilities: BTreeSet<Capability>,
    pub required_roles: BTreeSet<Role>,
    pub vo_packages: BTreeSet<&'static str>,
    pub rust_crates: BTreeSet<&'static str>,
    pub web_chunks: BTreeSet<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ArtifactComponent {
    Engine,
    World,
    Schedule,
    Input,
    RawAssetGraph,
    Headless,
    RenderCore,
    Render2d,
    Text,
    Image,
    Render3d,
    Gltf,
    Physics2d,
    Physics3d,
    Animation,
    Audio,
    Pack,
    Readback,
    Inspection,
    FrameDebugCapture,
    ShaderDiagnostics,
    Vogui,
    Racing,
    Terrain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    InvalidBuildRecipe,
    MissingRole(Role),
    UnexpectedRole(Role),
    DuplicateRole(Role),
    InvalidArtifact(Role),
    MissingComponent(ArtifactComponent),
    ForbiddenComponent(ArtifactComponent),
    ArtifactCapabilityMismatch(Role),
}

pub fn validate_build_recipe(recipe: &BuildRecipe) -> Result<(), ProfileError> {
    if *recipe != build_recipe(recipe.profile.alias) {
        return Err(ProfileError::InvalidBuildRecipe);
    }
    Ok(())
}

pub fn resolve(alias: ProfileAlias) -> ResolvedProfile {
    use Capability as C;
    let mut capabilities = BTreeSet::from([C::Core]);
    match alias {
        ProfileAlias::Core => {}
        ProfileAlias::TwoD => {
            capabilities.extend([C::Render2d, C::Text, C::Image, C::Physics2d, C::Readback]);
        }
        ProfileAlias::ThreeD => {
            capabilities.extend([
                C::Render2d,
                C::Text,
                C::Image,
                C::Render3d,
                C::Gltf,
                C::Animation,
                C::Physics3d,
                C::Readback,
            ]);
        }
        ProfileAlias::Full | ProfileAlias::Editor => {
            capabilities.extend([
                C::Render2d,
                C::Text,
                C::Image,
                C::Render3d,
                C::Gltf,
                C::Physics2d,
                C::Physics3d,
                C::Animation,
                C::Audio,
                C::Pack,
                C::Readback,
            ]);
            if alias == ProfileAlias::Editor {
                capabilities.extend([C::Inspection, C::FrameDebugCapture, C::ShaderDiagnostics]);
            }
        }
    }
    let mut required_roles = BTreeSet::from([Role::Logic, Role::Asset]);
    if capabilities.contains(&C::Render2d) || capabilities.contains(&C::Render3d) {
        required_roles.insert(Role::Render);
    }
    if capabilities.contains(&C::Audio) {
        required_roles.insert(Role::Audio);
    }
    ResolvedProfile {
        alias,
        capabilities,
        required_roles,
    }
}

pub fn build_recipe(alias: ProfileAlias) -> BuildRecipe {
    use Capability as C;
    let profile = resolve(alias);
    let mut vo_packages = BTreeSet::from([
        "voplay/core",
        "voplay/world",
        "voplay/schedule",
        "voplay/input",
        "voplay/assets",
    ]);
    let mut rust_crates = BTreeSet::from(["voplay-protocol", "voplay-runtime", "voplay-assets"]);
    let mut web_chunks = BTreeSet::new();

    let packages = [
        (C::Render2d, "voplay/render2d"),
        (C::Text, "voplay/render"),
        (C::Image, "voplay/render"),
        (C::Render3d, "voplay/render3d"),
        (C::Gltf, "voplay/render3d"),
        (C::Physics2d, "voplay/physics2d"),
        (C::Physics3d, "voplay/physics3d"),
        (C::Animation, "voplay/animation"),
        (C::Audio, "voplay/audio"),
        (C::Inspection, "voplay/diagnostics"),
        (C::FrameDebugCapture, "voplay/editor"),
        (C::ShaderDiagnostics, "voplay/editor"),
    ];
    for (capability, package) in packages {
        if profile.capabilities.contains(&capability) {
            vo_packages.insert(package);
        }
    }
    if profile.required_roles.contains(&Role::Render) {
        rust_crates.insert("voplay-render-core");
        web_chunks.insert("voplay-render-worker");
    }
    if profile.capabilities.contains(&C::Render2d) {
        rust_crates.insert("voplay-render-2d");
    }
    if profile.capabilities.contains(&C::Render3d) {
        rust_crates.insert("voplay-render-3d");
    }
    if profile.capabilities.contains(&C::Gltf) {
        rust_crates.insert("voplay-import-gltf");
    }
    if profile.capabilities.contains(&C::Physics2d) {
        rust_crates.insert("voplay-physics-2d");
    }
    if profile.capabilities.contains(&C::Physics3d) {
        rust_crates.insert("voplay-physics-3d");
    }
    if profile.capabilities.contains(&C::Animation) {
        rust_crates.insert("voplay-animation");
    }
    if profile.required_roles.contains(&Role::Audio) {
        rust_crates.insert("voplay-audio");
        web_chunks.insert("voplay-audio-worker");
    }
    if profile.alias == ProfileAlias::Editor {
        rust_crates.insert("voplay-vogui-editor");
        web_chunks.insert("voplay-editor-inspection");
    }
    BuildRecipe {
        profile,
        vo_packages,
        rust_crates,
        web_chunks,
    }
}

pub fn ci_minimal_render_recipe() -> CiMinimalRenderRecipe {
    CiMinimalRenderRecipe {
        published: false,
        capabilities: BTreeSet::from([Capability::Core, Capability::Render2d]),
        required_roles: BTreeSet::from([Role::Logic, Role::Asset, Role::Render]),
        vo_packages: BTreeSet::from([
            "voplay/core",
            "voplay/world",
            "voplay/schedule",
            "voplay/input",
            "voplay/assets",
            "voplay/render2d",
        ]),
        rust_crates: BTreeSet::from([
            "voplay-protocol",
            "voplay-runtime",
            "voplay-assets",
            "voplay-render-core",
            "voplay-render-2d",
        ]),
        web_chunks: BTreeSet::from(["voplay-render-worker"]),
    }
}

pub fn validate_ci_minimal_render_recipe(
    recipe: &CiMinimalRenderRecipe,
) -> Result<(), ProfileError> {
    if recipe != &ci_minimal_render_recipe()
        || recipe.published
        || recipe.capabilities.contains(&Capability::Physics2d)
        || recipe.capabilities.contains(&Capability::Text)
        || recipe.capabilities.contains(&Capability::Image)
        || recipe.capabilities.contains(&Capability::Render3d)
        || recipe.capabilities.contains(&Capability::Audio)
        || recipe.required_roles.contains(&Role::Audio)
    {
        return Err(ProfileError::InvalidBuildRecipe);
    }
    Ok(())
}

pub fn render_build_recipe(recipe: &BuildRecipe) -> String {
    fn joined(values: &BTreeSet<&'static str>) -> String {
        values.iter().copied().collect::<Vec<_>>().join(",")
    }
    format!(
        "profile={:?}\ncapabilities={:?}\nroles={:?}\nvo={}\nrust={}\nweb={}\n",
        recipe.profile.alias,
        recipe.profile.capabilities,
        recipe.profile.required_roles,
        joined(&recipe.vo_packages),
        joined(&recipe.rust_crates),
        joined(&recipe.web_chunks),
    )
}

pub fn validate_role_artifacts(
    profile: &ResolvedProfile,
    artifacts: Vec<RoleArtifact>,
) -> Result<BTreeMap<Role, RoleArtifact>, ProfileError> {
    let mut by_role = BTreeMap::new();
    for artifact in artifacts {
        validate_artifact(&artifact)?;
        if artifact.capabilities != profile.capabilities {
            return Err(ProfileError::ArtifactCapabilityMismatch(artifact.role));
        }
        let role = artifact.role;
        if by_role.insert(role, artifact).is_some() {
            return Err(ProfileError::DuplicateRole(role));
        }
    }
    for role in &profile.required_roles {
        if !by_role.contains_key(role) {
            return Err(ProfileError::MissingRole(*role));
        }
    }
    for role in by_role.keys() {
        if !profile.required_roles.contains(role) {
            return Err(ProfileError::UnexpectedRole(*role));
        }
    }
    Ok(by_role)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileSymbolSize {
    pub name: String,
    pub owner: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProfileDependencyKind {
    VoImport,
    RustCrate,
    JsChunk,
    NativeLibrary,
    RuntimeRole,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProfileDependencyEdge {
    pub from: String,
    pub to: String,
    pub kind: ProfileDependencyKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileChunkSize {
    pub name: String,
    pub raw_bytes: u64,
    pub gzip_bytes: u64,
    pub brotli_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileArtifactMeasurement {
    pub artifact: RoleArtifact,
    pub raw_bytes: u64,
    pub gzip_bytes: u64,
    pub brotli_bytes: u64,
    pub cold_build_millis: u64,
    pub shader_pipeline_count: u32,
    pub top_symbols: Vec<ProfileSymbolSize>,
    pub chunks: Vec<ProfileChunkSize>,
    pub dependencies: BTreeSet<String>,
    pub dependency_edges: BTreeSet<ProfileDependencyEdge>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProfileSizeTotals {
    pub raw_bytes: u64,
    pub gzip_bytes: u64,
    pub brotli_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileReport {
    pub format: u16,
    pub build_identity: [u8; 32],
    pub profile: ResolvedProfile,
    pub artifacts: BTreeMap<Role, ProfileArtifactMeasurement>,
    pub download: ProfileSizeTotals,
    pub placement_residency: BTreeMap<PlacementClass, u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileReportError {
    InvalidBuildIdentity,
    InvalidArtifacts(ProfileError),
    InvalidMeasurement(Role),
    DuplicateSymbol(Role, String),
    DuplicateChunk(Role, String),
    DependencyCycle(Role),
    SizeOverflow,
}

pub fn build_profile_report(
    build_identity: [u8; 32],
    profile: &ResolvedProfile,
    measurements: Vec<ProfileArtifactMeasurement>,
) -> Result<ProfileReport, ProfileReportError> {
    if build_identity.iter().all(|byte| *byte == 0) {
        return Err(ProfileReportError::InvalidBuildIdentity);
    }
    validate_role_artifacts(
        profile,
        measurements
            .iter()
            .map(|measurement| measurement.artifact.clone())
            .collect(),
    )
    .map_err(ProfileReportError::InvalidArtifacts)?;
    let mut artifacts = BTreeMap::new();
    let mut download = ProfileSizeTotals::default();
    let mut placement_residency = BTreeMap::<PlacementClass, u64>::new();
    for measurement in measurements {
        let role = measurement.artifact.role;
        let edge_dependencies = measurement
            .dependency_edges
            .iter()
            .map(|edge| edge.to.clone())
            .collect::<BTreeSet<_>>();
        if measurement.raw_bytes == 0
            || measurement.top_symbols.iter().any(|symbol| {
                symbol.name.is_empty()
                    || symbol.name.len() > 512
                    || symbol.owner.is_empty()
                    || symbol.owner.len() > 512
                    || symbol.bytes > measurement.raw_bytes
            })
            || measurement.chunks.iter().any(|chunk| {
                chunk.name.is_empty()
                    || chunk.name.len() > 512
                    || chunk.raw_bytes > measurement.raw_bytes
            })
            || measurement
                .dependencies
                .iter()
                .any(|dependency| dependency.is_empty() || dependency.len() > 512)
            || measurement.dependency_edges.iter().any(|edge| {
                edge.from.is_empty()
                    || edge.from.len() > 512
                    || edge.to.is_empty()
                    || edge.to.len() > 512
                    || edge.from == edge.to
            })
            || edge_dependencies != measurement.dependencies
            || measurement
                .top_symbols
                .iter()
                .try_fold(0_u64, |bytes, symbol| bytes.checked_add(symbol.bytes))
                .is_none_or(|bytes| bytes > measurement.raw_bytes)
        {
            return Err(ProfileReportError::InvalidMeasurement(role));
        }
        if dependency_cycle(&measurement.dependency_edges) {
            return Err(ProfileReportError::DependencyCycle(role));
        }
        reject_duplicate_names(
            role,
            measurement.top_symbols.iter().map(|symbol| &symbol.name),
            true,
        )?;
        reject_duplicate_names(
            role,
            measurement.chunks.iter().map(|chunk| &chunk.name),
            false,
        )?;
        download.raw_bytes = download
            .raw_bytes
            .checked_add(measurement.raw_bytes)
            .ok_or(ProfileReportError::SizeOverflow)?;
        download.gzip_bytes = download
            .gzip_bytes
            .checked_add(measurement.gzip_bytes)
            .ok_or(ProfileReportError::SizeOverflow)?;
        download.brotli_bytes = download
            .brotli_bytes
            .checked_add(measurement.brotli_bytes)
            .ok_or(ProfileReportError::SizeOverflow)?;
        let residency = placement_residency
            .entry(measurement.artifact.placement)
            .or_default();
        *residency = residency
            .checked_add(measurement.raw_bytes)
            .ok_or(ProfileReportError::SizeOverflow)?;
        artifacts.insert(role, measurement);
    }
    Ok(ProfileReport {
        format: 2,
        build_identity,
        profile: profile.clone(),
        artifacts,
        download,
        placement_residency,
    })
}

fn reject_duplicate_names<'a>(
    role: Role,
    names: impl Iterator<Item = &'a String>,
    symbols: bool,
) -> Result<(), ProfileReportError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            return Err(if symbols {
                ProfileReportError::DuplicateSymbol(role, name.clone())
            } else {
                ProfileReportError::DuplicateChunk(role, name.clone())
            });
        }
    }
    Ok(())
}

fn dependency_cycle(edges: &BTreeSet<ProfileDependencyEdge>) -> bool {
    fn visit(
        node: &str,
        graph: &BTreeMap<&str, Vec<&str>>,
        states: &mut BTreeMap<String, u8>,
    ) -> bool {
        match states.get(node).copied() {
            Some(1) => return true,
            Some(2) => return false,
            _ => {}
        }
        states.insert(node.to_owned(), 1);
        if graph
            .get(node)
            .into_iter()
            .flatten()
            .any(|next| visit(next, graph, states))
        {
            return true;
        }
        states.insert(node.to_owned(), 2);
        false
    }
    let mut graph = BTreeMap::<&str, Vec<&str>>::new();
    for edge in edges {
        graph
            .entry(edge.from.as_str())
            .or_default()
            .push(edge.to.as_str());
    }
    let mut states = BTreeMap::new();
    graph
        .keys()
        .copied()
        .any(|node| visit(node, &graph, &mut states))
}

pub fn audit_components(
    profile: &ResolvedProfile,
    components: &BTreeSet<ArtifactComponent>,
) -> Result<(), ProfileError> {
    let required = required_components(profile);
    for component in required {
        if !components.contains(&component) {
            return Err(ProfileError::MissingComponent(component));
        }
    }
    for component in components {
        if forbidden_component(profile, *component) {
            return Err(ProfileError::ForbiddenComponent(*component));
        }
    }
    Ok(())
}

fn validate_artifact(artifact: &RoleArtifact) -> Result<(), ProfileError> {
    let placement_matches = matches!(
        (artifact.role, artifact.placement),
        (Role::Logic, PlacementClass::LogicIsland)
            | (Role::Asset, PlacementClass::AssetIsland)
            | (Role::Render, PlacementClass::RenderIsland)
            | (Role::Audio, PlacementClass::AudioIsland)
    );
    let abi_matches = match artifact.role {
        Role::Logic | Role::Asset => {
            artifact.shader_abi.is_none() && artifact.realtime_abi.is_none()
        }
        Role::Render => artifact.shader_abi.is_some() && artifact.realtime_abi.is_none(),
        Role::Audio => artifact.shader_abi.is_none() && artifact.realtime_abi.is_some(),
    };
    if !placement_matches
        || !abi_matches
        || !valid_artifact_label(&artifact.artifact_name)
        || !valid_target(&artifact.target)
        || artifact.digest.iter().all(|byte| *byte == 0)
        || artifact.schema_fingerprint.iter().all(|byte| *byte == 0)
    {
        return Err(ProfileError::InvalidArtifact(artifact.role));
    }
    Ok(())
}

fn valid_artifact_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

fn valid_target(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn required_components(profile: &ResolvedProfile) -> BTreeSet<ArtifactComponent> {
    use ArtifactComponent as A;
    use Capability as C;
    let mut required = BTreeSet::from([
        A::Engine,
        A::World,
        A::Schedule,
        A::Input,
        A::RawAssetGraph,
        A::Headless,
    ]);
    let mapping = [
        (C::Render2d, A::Render2d),
        (C::Text, A::Text),
        (C::Image, A::Image),
        (C::Render3d, A::Render3d),
        (C::Gltf, A::Gltf),
        (C::Physics2d, A::Physics2d),
        (C::Physics3d, A::Physics3d),
        (C::Animation, A::Animation),
        (C::Audio, A::Audio),
        (C::Pack, A::Pack),
        (C::Readback, A::Readback),
        (C::Inspection, A::Inspection),
        (C::FrameDebugCapture, A::FrameDebugCapture),
        (C::ShaderDiagnostics, A::ShaderDiagnostics),
    ];
    for (capability, component) in mapping {
        if profile.capabilities.contains(&capability) {
            required.insert(component);
        }
    }
    if profile.required_roles.contains(&Role::Render) {
        required.insert(A::RenderCore);
    }
    if profile.capabilities.contains(&C::Render3d) {
        required.insert(A::Terrain);
    }
    required
}

fn forbidden_component(profile: &ResolvedProfile, component: ArtifactComponent) -> bool {
    use ArtifactComponent as A;
    use Capability as C;
    if matches!(component, A::Vogui | A::Racing) {
        return true;
    }
    if profile.alias == ProfileAlias::Core {
        return matches!(
            component,
            A::RenderCore
                | A::Render2d
                | A::Text
                | A::Image
                | A::Render3d
                | A::Gltf
                | A::Physics2d
                | A::Physics3d
                | A::Animation
                | A::Audio
                | A::Pack
                | A::Readback
                | A::Inspection
                | A::FrameDebugCapture
                | A::ShaderDiagnostics
                | A::Terrain
        );
    }
    if profile.alias == ProfileAlias::TwoD
        && matches!(
            component,
            A::Render3d
                | A::Gltf
                | A::Physics3d
                | A::Animation
                | A::Audio
                | A::Inspection
                | A::FrameDebugCapture
                | A::ShaderDiagnostics
                | A::Terrain
        )
    {
        return true;
    }
    if profile.alias == ProfileAlias::ThreeD
        && matches!(
            component,
            A::Physics2d | A::Audio | A::Inspection | A::FrameDebugCapture | A::ShaderDiagnostics
        )
    {
        return true;
    }
    if profile.alias == ProfileAlias::Full
        && matches!(
            component,
            A::Inspection | A::FrameDebugCapture | A::ShaderDiagnostics
        )
    {
        return true;
    }
    !match component {
        A::Render2d => profile.capabilities.contains(&C::Render2d),
        A::Text => profile.capabilities.contains(&C::Text),
        A::Image => profile.capabilities.contains(&C::Image),
        A::Render3d => profile.capabilities.contains(&C::Render3d),
        A::Gltf => profile.capabilities.contains(&C::Gltf),
        A::Physics2d => profile.capabilities.contains(&C::Physics2d),
        A::Physics3d => profile.capabilities.contains(&C::Physics3d),
        A::Animation => profile.capabilities.contains(&C::Animation),
        A::Audio => profile.capabilities.contains(&C::Audio),
        A::Pack => profile.capabilities.contains(&C::Pack),
        A::Readback => profile.capabilities.contains(&C::Readback),
        A::Inspection => profile.capabilities.contains(&C::Inspection),
        A::FrameDebugCapture => profile.capabilities.contains(&C::FrameDebugCapture),
        A::ShaderDiagnostics => profile.capabilities.contains(&C::ShaderDiagnostics),
        A::Engine
        | A::World
        | A::Schedule
        | A::Input
        | A::RawAssetGraph
        | A::Headless
        | A::RenderCore => true,
        A::Terrain => profile.capabilities.contains(&C::Render3d),
        A::Vogui | A::Racing => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(role: Role, profile: &ResolvedProfile) -> RoleArtifact {
        RoleArtifact {
            role,
            placement: match role {
                Role::Logic => PlacementClass::LogicIsland,
                Role::Asset => PlacementClass::AssetIsland,
                Role::Render => PlacementClass::RenderIsland,
                Role::Audio => PlacementClass::AudioIsland,
            },
            artifact_name: format!("voplay-{role:?}").to_ascii_lowercase(),
            target: "wasm32-unknown-unknown".to_owned(),
            digest: [1; 32],
            schema_fingerprint: [2; 32],
            shader_abi: (role == Role::Render).then_some([3; 32]),
            realtime_abi: (role == Role::Audio).then_some([4; 32]),
            capabilities: profile.capabilities.clone(),
        }
    }

    #[test]
    fn every_declared_profile_has_an_exact_valid_recipe_and_component_set() {
        for alias in [
            ProfileAlias::Core,
            ProfileAlias::TwoD,
            ProfileAlias::ThreeD,
            ProfileAlias::Full,
            ProfileAlias::Editor,
        ] {
            let recipe = build_recipe(alias);
            assert_eq!(recipe.profile, resolve(alias));
            validate_build_recipe(&recipe).unwrap();
            audit_components(&recipe.profile, &required_components(&recipe.profile)).unwrap();
        }
        validate_ci_minimal_render_recipe(&ci_minimal_render_recipe()).unwrap();
    }

    #[test]
    fn recipe_and_component_tampering_fail_closed() {
        let mut recipe = build_recipe(ProfileAlias::Core);
        recipe.rust_crates.insert("voplay-render-3d");
        assert_eq!(
            validate_build_recipe(&recipe),
            Err(ProfileError::InvalidBuildRecipe)
        );
        let profile = resolve(ProfileAlias::Core);
        let mut components = required_components(&profile);
        components.insert(ArtifactComponent::Render3d);
        assert_eq!(
            audit_components(&profile, &components),
            Err(ProfileError::ForbiddenComponent(
                ArtifactComponent::Render3d
            ))
        );
        components.remove(&ArtifactComponent::Render3d);
        components.remove(&ArtifactComponent::Headless);
        assert_eq!(
            audit_components(&profile, &components),
            Err(ProfileError::MissingComponent(ArtifactComponent::Headless))
        );
    }

    #[test]
    fn role_artifacts_require_exact_roles_placement_abis_and_capabilities() {
        let profile = resolve(ProfileAlias::Full);
        let artifacts = profile
            .required_roles
            .iter()
            .copied()
            .map(|role| artifact(role, &profile))
            .collect::<Vec<_>>();
        let validated = validate_role_artifacts(&profile, artifacts.clone()).unwrap();
        assert_eq!(
            validated.keys().copied().collect::<BTreeSet<_>>(),
            profile.required_roles
        );

        let mut invalid = artifacts;
        let render = invalid
            .iter_mut()
            .find(|artifact| artifact.role == Role::Render)
            .unwrap();
        render.shader_abi = None;
        assert_eq!(
            validate_role_artifacts(&profile, invalid),
            Err(ProfileError::InvalidArtifact(Role::Render))
        );
    }

    #[test]
    fn profile_report_binds_build_artifacts_sizes_and_placement() {
        let profile = resolve(ProfileAlias::Full);
        let measurements = profile
            .required_roles
            .iter()
            .copied()
            .map(|role| ProfileArtifactMeasurement {
                artifact: artifact(role, &profile),
                raw_bytes: 100,
                gzip_bytes: 60,
                brotli_bytes: 50,
                cold_build_millis: 10,
                shader_pipeline_count: u32::from(role == Role::Render),
                top_symbols: vec![ProfileSymbolSize {
                    name: format!("{role:?}-entry"),
                    owner: String::from("voplay-runtime"),
                    bytes: 25,
                }],
                chunks: vec![ProfileChunkSize {
                    name: format!("{role:?}-chunk"),
                    raw_bytes: 100,
                    gzip_bytes: 60,
                    brotli_bytes: 50,
                }],
                dependencies: BTreeSet::from([String::from("vo-app-runtime")]),
                dependency_edges: BTreeSet::from([ProfileDependencyEdge {
                    from: String::from("voplay-runtime"),
                    to: String::from("vo-app-runtime"),
                    kind: ProfileDependencyKind::RustCrate,
                }]),
            })
            .collect::<Vec<_>>();

        let report = build_profile_report([9; 32], &profile, measurements).unwrap();
        assert_eq!(report.artifacts.len(), profile.required_roles.len());
        assert_eq!(
            report.download.raw_bytes,
            100 * profile.required_roles.len() as u64
        );
        assert_eq!(
            report.placement_residency.values().sum::<u64>(),
            report.download.raw_bytes
        );

        let mut duplicate = report.artifacts.values().next().unwrap().clone();
        duplicate.top_symbols.push(duplicate.top_symbols[0].clone());
        assert!(matches!(
            build_profile_report([9; 32], &profile, vec![duplicate]),
            Err(ProfileReportError::InvalidArtifacts(_))
                | Err(ProfileReportError::DuplicateSymbol(_, _))
        ));
    }
}
