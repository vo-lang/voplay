use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use vo_module::profile::{resolve_source_recipes, ArtifactRole, CapabilitySet};
use vo_module::schema::modfile::ModFile;

fn main() {
    println!("cargo:rerun-if-env-changed=VOPLAY_RENDER_FEATURE_FACTORIES");
    println!("cargo:rerun-if-changed=../../../vo.mod");
    for feature in [
        "PROFILE_CORE",
        "PROFILE_2D",
        "PROFILE_3D",
        "PROFILE_FULL",
        "PROFILE_EDITOR",
    ] {
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_{feature}");
    }

    let selected = [
        ("core", "PROFILE_CORE"),
        ("2d", "PROFILE_2D"),
        ("3d", "PROFILE_3D"),
        ("full", "PROFILE_FULL"),
        ("editor", "PROFILE_EDITOR"),
    ]
    .into_iter()
    .filter(|(_, feature)| env::var_os(format!("CARGO_FEATURE_{feature}")).is_some())
    .collect::<Vec<_>>();
    if selected.len() != 1 {
        panic!(
            "voplay-extension requires exactly one profile-* feature, found {}",
            selected.len()
        );
    }
    let profile_name = selected[0].0;
    let target = env::var("TARGET").expect("Cargo provides TARGET");
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir")).join("../../..");
    let mod_file =
        ModFile::parse(&fs::read_to_string(root.join("vo.mod")).expect("read Voplay vo.mod"))
            .expect("parse Voplay vo.mod");
    let capabilities = mod_file
        .profiles
        .resolve(
            Some(profile_name),
            &CapabilitySet::default(),
            "voplay-extension build profile",
        )
        .expect("resolve Voplay profile");
    let recipes = resolve_source_recipes(
        &mod_file
            .extension
            .as_ref()
            .expect("Voplay extension contract")
            .source_recipes,
        &mod_file.profiles,
        "voplay source recipes",
    )
    .expect("validate Voplay source recipes");
    let recipe = recipes
        .iter()
        .find(|recipe| {
            recipe.capabilities == capabilities
                && recipe.target == target
                && recipe.toolchain == vo_module::TOOLCHAIN_VERSION
        })
        .unwrap_or_else(|| {
            panic!(
                "Voplay profile {profile_name} has no source recipe for {target} and toolchain {}",
                vo_module::TOOLCHAIN_VERSION,
            )
        });

    let roles = recipe
        .role_outputs
        .iter()
        .map(|output| output.role.clone())
        .collect::<BTreeSet<_>>();
    let schema = digest_array(&recipe.schema);
    let abi = digest_array(&recipe.abi);
    let capability = digest_array(&recipe.capabilities.digest());
    let mut generated = String::new();
    writeln!(
        generated,
        "pub const PROFILE_NAME: &str = {profile_name:?};"
    )
    .unwrap();
    writeln!(
        generated,
        "pub const PROFILE_SCHEMA: [u8; 32] = {schema:?};"
    )
    .unwrap();
    writeln!(generated, "pub const PROFILE_ABI: [u8; 32] = {abi:?};").unwrap();
    writeln!(
        generated,
        "pub const PROFILE_CAPABILITIES: [u8; 32] = {capability:?};"
    )
    .unwrap();
    writeln!(
        generated,
        "pub static PROVIDER_FACTORIES: [vo_app_runtime::provider_abi::ProviderFactoryDescriptorV2; {}] = [",
        roles.len(),
    )
    .unwrap();
    for role in roles {
        let role_name = role.as_str();
        let factory_id = stable_factory_id(role_name);
        writeln!(
            generated,
            "vo_app_runtime::provider_abi::ProviderFactoryDescriptorV2 {{ struct_size: core::mem::size_of::<vo_app_runtime::provider_abi::ProviderFactoryDescriptorV2>() as u32, abi_version: vo_app_runtime::provider_abi::PROVIDER_FACTORY_ABI_VERSION, factory_id: {factory_id}, role: {}, abi_fingerprint: PROFILE_ABI, schema_fingerprint: PROFILE_SCHEMA, capability_digest: PROFILE_CAPABILITIES, create: Some({}) }},",
            role_abi(&role),
            create_function(&role),
        )
        .unwrap();
    }
    writeln!(generated, "];").unwrap();
    let render_feature_factories = render_feature_factory_paths();
    writeln!(
        generated,
        "#[cfg(any(feature = \"profile-3d\", feature = \"profile-full\", feature = \"profile-editor\"))]"
    )
    .unwrap();
    writeln!(
        generated,
        "pub static PROFILE_RENDER_FEATURE_FACTORIES: [voplay_render_3d::RenderFeatureFactoryBuilder; {}] = [",
        render_feature_factories.len(),
    )
    .unwrap();
    for factory in render_feature_factories {
        writeln!(generated, "{factory},").unwrap();
    }
    writeln!(generated, "];").unwrap();
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("out dir")).join("provider_profile.rs"),
        generated,
    )
    .expect("write generated provider profile");
}

fn render_feature_factory_paths() -> Vec<String> {
    let Some(value) = env::var_os("VOPLAY_RENDER_FEATURE_FACTORIES") else {
        return Vec::new();
    };
    value
        .to_string_lossy()
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| {
            if !valid_rust_path(path) {
                panic!("invalid statically linked Voplay RenderFeature factory path {path:?}");
            }
            path.to_owned()
        })
        .collect()
}

fn valid_rust_path(path: &str) -> bool {
    path.split("::").all(|segment| {
        let mut chars = segment.chars();
        chars
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    })
}

fn digest_array(digest: &vo_module::digest::Digest) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (index, pair) in digest.hex().as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex(pair[0]) << 4) | hex(pair[1]);
    }
    bytes
}

fn hex(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => panic!("invalid digest hex"),
    }
}

fn stable_factory_id(role: &str) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in b"github.com/vo-lang/voplay"
        .iter()
        .chain([0].iter())
        .chain(role.as_bytes())
    {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash.max(1)
}

fn role_abi(role: &ArtifactRole) -> &'static str {
    match role {
        ArtifactRole::Logic => "vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_LOGIC",
        ArtifactRole::Asset => "vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_ASSET",
        ArtifactRole::Render => "vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_RENDERER",
        ArtifactRole::Audio => "vo_app_runtime::provider_abi::PROVIDER_ROLE_GAME_AUDIO",
        _ => panic!("unsupported Voplay provider role {}", role.as_str()),
    }
}

fn create_function(role: &ArtifactRole) -> &'static str {
    match role {
        ArtifactRole::Logic => "create_game_logic_provider",
        ArtifactRole::Asset => "create_game_asset_provider",
        ArtifactRole::Render => "create_game_render_provider",
        ArtifactRole::Audio => "create_game_audio_provider",
        _ => panic!("unsupported Voplay provider role {}", role.as_str()),
    }
}
