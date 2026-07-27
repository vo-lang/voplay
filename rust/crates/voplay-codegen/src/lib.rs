use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use vo_schema_compiler::{
    generator_cache_key, schema_source_fingerprint, validate_generated_path, GeneratedArtifact,
    GeneratorCacheInput, GeneratorDiagnostic, GeneratorIdentity, GeneratorOutput,
};

pub const GENERATOR_NAME: &str = "voplay.component-store";
pub const GENERATOR_VERSION: &str = "13";
pub const SCHEMA_KIND: &str = "voplay.components";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodegenError {
    Parse(String),
    InvalidSchema,
    DuplicateComponent,
    DuplicateField,
    ComponentIdCollision,
    UnsupportedFieldType,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComponentId(pub [u8; 16]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedComponent {
    pub component_id: ComponentId,
    pub canonical_name: String,
    pub schema_fingerprint: [u8; 32],
    pub vo_source: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedModule {
    pub module_fingerprint: [u8; 32],
    pub components: Vec<GeneratedComponent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernedGeneration {
    pub module: GeneratedModule,
    pub output: GeneratorOutput,
}

pub fn generate_governed(
    source_path: &str,
    source: &str,
    toolchain: &str,
    target: &str,
    capabilities: &[String],
) -> Result<GovernedGeneration, Vec<GeneratorDiagnostic>> {
    let identity = GeneratorIdentity {
        name: GENERATOR_NAME.to_string(),
        version: GENERATOR_VERSION.to_string(),
        schema_kind: SCHEMA_KIND.to_string(),
    };
    let module = generate(source).map_err(|error| {
        vec![GeneratorDiagnostic {
            code: "VOPLAY-GEN-001",
            stage: "schema",
            source_path: source_path.to_string(),
            span: 0..source.len(),
            message: format!("{error:?}"),
        }]
    })?;
    let schema_fingerprint = schema_source_fingerprint(source.as_bytes());
    let cache_key = generator_cache_key(&GeneratorCacheInput {
        identity: &identity,
        schema_fingerprint,
        toolchain,
        target,
        capabilities,
    });
    let parsed: SourceModule = toml::from_str(source).map_err(|error| {
        vec![GeneratorDiagnostic {
            code: "VOPLAY-GEN-003",
            stage: "schema",
            source_path: source_path.to_string(),
            span: 0..source.len(),
            message: error.to_string(),
        }]
    })?;
    let mut artifacts = Vec::with_capacity(module.components.len() + 2);
    let mut output_paths = BTreeSet::new();
    let mut manifest = format!(
        "format = 1\ngenerator = \"{}\"\ngenerator_version = \"{}\"\nschema_kind = \"{}\"\nmodule_fingerprint = \"{}\"\n",
        GENERATOR_NAME,
        GENERATOR_VERSION,
        SCHEMA_KIND,
        hex(&module.module_fingerprint),
    );
    for component in &module.components {
        let leaf = component
            .canonical_name
            .rsplit('/')
            .next()
            .unwrap_or("component")
            .split('@')
            .next()
            .unwrap_or("component")
            .to_ascii_lowercase();
        let path = format!("{leaf}_component.vo");
        validate_generated_path(&path).map_err(|message| {
            vec![GeneratorDiagnostic {
                code: "VOPLAY-GEN-002",
                stage: "output",
                source_path: source_path.to_string(),
                span: 0..0,
                message: message.to_string(),
            }]
        })?;
        if !output_paths.insert(path.clone()) {
            return Err(vec![GeneratorDiagnostic {
                code: "VOPLAY-GEN-004",
                stage: "output",
                source_path: source_path.to_string(),
                span: 0..source.len(),
                message: format!("multiple components produce {path}"),
            }]);
        }
        manifest.push_str(&format!(
            "\n[[component]]\nname = {:?}\nid = \"{}\"\nschema_fingerprint = \"{}\"\nsource = {:?}\n",
            component.canonical_name,
            hex(&component.component_id.0),
            hex(&component.schema_fingerprint),
            path,
        ));
        artifacts.push(GeneratedArtifact::new(
            path,
            component.vo_source.as_bytes().to_vec(),
        ));
    }
    if let Some(game) = parsed.game {
        validate_game(&game).map_err(|error| {
            vec![GeneratorDiagnostic {
                code: "VOPLAY-GEN-005",
                stage: "schema",
                source_path: source_path.to_string(),
                span: 0..source.len(),
                message: format!("{error:?}"),
            }]
        })?;
        let path = format!("{}_game.vo", snake_case(&game.type_name));
        validate_generated_path(&path).expect("generator owns game output path");
        if !output_paths.insert(path.clone()) {
            return Err(vec![GeneratorDiagnostic {
                code: "VOPLAY-GEN-006",
                stage: "output",
                source_path: source_path.to_string(),
                span: 0..source.len(),
                message: format!("game output collides at {path}"),
            }]);
        }
        let source = render_game_source(&parsed.canonical_module, &game, module.module_fingerprint);
        manifest.push_str(&render_game_manifest(
            &parsed.canonical_module,
            &game,
            module.module_fingerprint,
            &path,
        ));
        artifacts.push(GeneratedArtifact::new(path, source.into_bytes()));
    }
    artifacts.push(GeneratedArtifact::new(
        "generated/voplay_components.manifest".to_string(),
        manifest.into_bytes(),
    ));
    Ok(GovernedGeneration {
        module,
        output: GeneratorOutput {
            cache_key,
            schema_fingerprint,
            artifacts,
        },
    })
}

#[derive(Deserialize)]
struct SourceModule {
    format: u32,
    canonical_module: String,
    component: Vec<SourceComponent>,
    #[serde(default)]
    game: Option<SourceGame>,
}

#[derive(Deserialize)]
struct SourceComponent {
    package: String,
    type_name: String,
    schema_major: u32,
    classification: String,
    #[serde(default)]
    id_override: Option<String>,
    field: Vec<SourceField>,
}

#[derive(Deserialize)]
struct SourceField {
    name: String,
    r#type: String,
    #[serde(default)]
    default: String,
    #[serde(default)]
    editor: String,
}

#[derive(Deserialize)]
struct SourceGame {
    package: String,
    type_name: String,
    init_type: String,
    configure: String,
    start: String,
    execute: String,
    max_init_bytes: usize,
    #[serde(default = "default_world_max_entities")]
    max_world_entities: usize,
    #[serde(default = "default_world_max_commands")]
    max_world_commands: usize,
    #[serde(default = "default_world_max_changes")]
    max_world_changes: usize,
    #[serde(default = "default_world_component_bytes")]
    max_world_component_bytes: usize,
    roles: Vec<String>,
    #[serde(default)]
    init_field: Vec<SourceField>,
}

fn default_world_max_entities() -> usize {
    1_000_000
}

fn default_world_max_commands() -> usize {
    16_777_216
}

fn default_world_max_changes() -> usize {
    1_048_576
}

fn default_world_component_bytes() -> usize {
    16_777_216
}

pub fn generate(source: &str) -> Result<GeneratedModule, CodegenError> {
    let source: SourceModule =
        toml::from_str(source).map_err(|error| CodegenError::Parse(error.to_string()))?;
    if source.format != 1
        || !valid_module_path(&source.canonical_module)
        || source.component.is_empty()
    {
        return Err(CodegenError::InvalidSchema);
    }
    let mut names = BTreeSet::new();
    let mut ids = BTreeMap::new();
    let mut generated = Vec::with_capacity(source.component.len());
    for component in source.component {
        validate_component(&component)?;
        let canonical_name = format!(
            "{}/{}/{}@{}",
            source.canonical_module, component.package, component.type_name, component.schema_major
        );
        if !names.insert(canonical_name.clone()) {
            return Err(CodegenError::DuplicateComponent);
        }
        let component_id = component_id(&canonical_name, component.id_override.as_deref())?;
        if ids.insert(component_id, canonical_name.clone()).is_some() {
            return Err(CodegenError::ComponentIdCollision);
        }
        let canonical_schema = canonical_component(&canonical_name, &component);
        let schema_fingerprint: [u8; 32] = Sha256::digest(canonical_schema.as_bytes()).into();
        let vo_source = generate_vo_source(component_id, &component);
        generated.push(GeneratedComponent {
            component_id,
            canonical_name,
            schema_fingerprint,
            vo_source,
        });
    }
    generated.sort_by(|left, right| left.canonical_name.cmp(&right.canonical_name));
    let mut module_canonical = String::new();
    for component in &generated {
        writeln!(
            module_canonical,
            "{}:{}:{}",
            component.canonical_name,
            hex(&component.component_id.0),
            hex(&component.schema_fingerprint)
        )
        .unwrap();
    }
    Ok(GeneratedModule {
        module_fingerprint: Sha256::digest(module_canonical.as_bytes()).into(),
        components: generated,
    })
}

fn validate_component(component: &SourceComponent) -> Result<(), CodegenError> {
    if !valid_identifier(&component.package)
        || !valid_identifier(&component.type_name)
        || component.schema_major == 0
        || !matches!(
            component.classification.as_str(),
            "simulation" | "presentation"
        )
        || component.field.is_empty()
    {
        return Err(CodegenError::InvalidSchema);
    }
    let mut fields = BTreeSet::new();
    for field in &component.field {
        if !valid_identifier(&field.name) || !fields.insert(field.name.as_str()) {
            return Err(CodegenError::DuplicateField);
        }
        if !matches!(
            field.r#type.as_str(),
            "bool"
                | "int32"
                | "uint32"
                | "int64"
                | "uint64"
                | "float32"
                | "float64"
                | "string"
                | "bytes"
        ) {
            return Err(CodegenError::UnsupportedFieldType);
        }
    }
    Ok(())
}

fn validate_game(game: &SourceGame) -> Result<(), CodegenError> {
    if !valid_identifier(&game.package)
        || !valid_identifier(&game.type_name)
        || !valid_identifier(&game.init_type)
        || !valid_identifier(&game.configure)
        || !valid_identifier(&game.start)
        || !valid_identifier(&game.execute)
        || game.max_init_bytes == 0
        || game.max_world_entities == 0
        || game.max_world_entities > 1_000_000
        || game.max_world_commands == 0
        || game.max_world_commands > 16_777_216
        || game.max_world_changes == 0
        || game.max_world_changes > 1_048_576
        || game.max_world_component_bytes == 0
        || game.max_world_component_bytes > 16_777_216
        || game.roles.is_empty()
    {
        return Err(CodegenError::InvalidSchema);
    }
    let mut roles = BTreeSet::new();
    for role in &game.roles {
        if !matches!(role.as_str(), "logic" | "asset" | "render" | "audio")
            || !roles.insert(role.as_str())
        {
            return Err(CodegenError::InvalidSchema);
        }
    }
    if !roles.contains("logic") {
        return Err(CodegenError::InvalidSchema);
    }
    let mut fields = BTreeSet::new();
    for field in &game.init_field {
        if !valid_identifier(&field.name) || !fields.insert(field.name.as_str()) {
            return Err(CodegenError::DuplicateField);
        }
        if !matches!(
            field.r#type.as_str(),
            "bool" | "int32" | "uint32" | "int64" | "uint64" | "string" | "bytes"
        ) {
            return Err(CodegenError::UnsupportedFieldType);
        }
    }
    Ok(())
}

fn component_id(
    canonical_name: &str,
    id_override: Option<&str>,
) -> Result<ComponentId, CodegenError> {
    if let Some(value) = id_override {
        let bytes = decode_hex_16(value).ok_or(CodegenError::InvalidSchema)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(CodegenError::InvalidSchema);
        }
        return Ok(ComponentId(bytes));
    }
    let digest = Sha256::digest(canonical_name.as_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&digest[..16]);
    Ok(ComponentId(id))
}

fn canonical_component(name: &str, component: &SourceComponent) -> String {
    let mut canonical = format!(
        "name={name};class={};major={}",
        component.classification, component.schema_major
    );
    for field in &component.field {
        write!(
            canonical,
            ";field={},{},{},{}",
            field.name, field.r#type, field.default, field.editor
        )
        .unwrap();
    }
    canonical
}

fn generate_vo_source(id: ComponentId, component: &SourceComponent) -> String {
    let mut source = format!(
        "// governed:voplay-codegen component_id={}\npackage {}\n\n",
        hex(&id.0),
        component.package
    );
    writeln!(source, "type {} struct {{", component.type_name).unwrap();
    for field in &component.field {
        writeln!(
            source,
            "\t{} {}",
            export_name(&field.name),
            vo_type(&field.r#type)
        )
        .unwrap();
    }
    writeln!(source, "}}\n").unwrap();
    writeln!(
        source,
        "type {0}Store struct {{\n\tDenseEntities []uint64\n\tDenseValues []{0}\n\tSparse []uint32\n\tAdded []uint64\n\tChanged []uint64\n\tRemoved []uint64\n\tRevision uint64\n\tMaxEntities uint32\n\tMaxChanges uint32\n}}",
        component.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\ntype {0}Query struct {{\n\tStore *{0}Store\n\tCursor uint32\n\tExpectedRevision uint64\n}}",
        component.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\nfunc New{0}Store(maxEntities uint32, maxChanges uint32) (*{0}Store, bool) {{\n\tif maxEntities == 0 || maxEntities > 1000000 || maxChanges == 0 || maxChanges > 1000000 {{ return nil, false }}\n\treturn &{0}Store{{Sparse: make([]uint32, int(maxEntities)+1), MaxEntities: maxEntities, MaxChanges: maxChanges}}, true\n}}\n\nfunc (store *{0}Store) Get(entity uint64) ({0}, bool) {{\n\tvar zero {0}\n\tif entity == 0 {{ return zero, false }}\n\tindex := uint32(entity)\n\tif index == 0 || index > store.MaxEntities {{ return zero, false }}\n\tslot := store.Sparse[index]\n\tif slot == 0 {{ return zero, false }}\n\tdense := int(slot-1)\n\tif dense >= len(store.DenseEntities) || store.DenseEntities[dense] != entity {{ return zero, false }}\n\treturn store.DenseValues[dense], true\n}}\n\nfunc (store *{0}Store) Upsert(entity uint64, value {0}) bool {{\n\tchangeCount := len(store.Added)+len(store.Changed)+len(store.Removed)\n\tif entity == 0 || store.Revision == 18446744073709551615 || changeCount >= int(store.MaxChanges) {{ return false }}\n\tindex := uint32(entity)\n\tif index == 0 || index > store.MaxEntities {{ return false }}\n\tslot := store.Sparse[index]\n\tif slot != 0 {{\n\t\tdense := int(slot-1)\n\t\tif dense >= len(store.DenseEntities) || store.DenseEntities[dense] != entity {{ return false }}\n\t\tstore.DenseValues[dense] = value\n\t\tstore.Changed = append(store.Changed, entity)\n\t\tstore.Revision++\n\t\treturn true\n\t}}\n\tif len(store.DenseEntities) >= int(store.MaxEntities) {{ return false }}\n\tinsert := 0\n\tfor insert < len(store.DenseEntities) && store.DenseEntities[insert] < entity {{ insert++ }}\n\tstore.DenseEntities = append(store.DenseEntities, 0)\n\tstore.DenseValues = append(store.DenseValues, value)\n\tcursor := len(store.DenseEntities)-1\n\tfor cursor > insert {{\n\t\tstore.DenseEntities[cursor] = store.DenseEntities[cursor-1]\n\t\tstore.DenseValues[cursor] = store.DenseValues[cursor-1]\n\t\tmovedIndex := uint32(store.DenseEntities[cursor])\n\t\tstore.Sparse[movedIndex] = uint32(cursor+1)\n\t\tcursor--\n\t}}\n\tstore.DenseEntities[insert] = entity\n\tstore.DenseValues[insert] = value\n\tstore.Sparse[index] = uint32(insert+1)\n\tstore.Added = append(store.Added, entity)\n\tstore.Revision++\n\treturn true\n}}\n\nfunc (store *{0}Store) Remove(entity uint64) bool {{\n\tchangeCount := len(store.Added)+len(store.Changed)+len(store.Removed)\n\tif entity == 0 || store.Revision == 18446744073709551615 || changeCount >= int(store.MaxChanges) {{ return false }}\n\tindex := uint32(entity)\n\tif index == 0 || index > store.MaxEntities {{ return false }}\n\tslot := store.Sparse[index]\n\tif slot == 0 {{ return false }}\n\tdense := int(slot-1)\n\tif dense >= len(store.DenseEntities) || store.DenseEntities[dense] != entity {{ return false }}\n\tfor cursor := dense; cursor+1 < len(store.DenseEntities); cursor++ {{\n\t\tstore.DenseEntities[cursor] = store.DenseEntities[cursor+1]\n\t\tstore.DenseValues[cursor] = store.DenseValues[cursor+1]\n\t\tmovedIndex := uint32(store.DenseEntities[cursor])\n\t\tstore.Sparse[movedIndex] = uint32(cursor+1)\n\t}}\n\tstore.DenseEntities = store.DenseEntities[:len(store.DenseEntities)-1]\n\tstore.DenseValues = store.DenseValues[:len(store.DenseValues)-1]\n\tstore.Sparse[index] = 0\n\tstore.Removed = append(store.Removed, entity)\n\tstore.Revision++\n\treturn true\n}}\n\nfunc (store *{0}Store) Query() {0}Query {{\n\treturn {0}Query{{Store: store, ExpectedRevision: store.Revision}}\n}}\n\nfunc (query *{0}Query) Next() (uint64, {0}, bool) {{\n\tvar zero {0}\n\tif query.Store == nil || query.ExpectedRevision != query.Store.Revision || int(query.Cursor) >= len(query.Store.DenseEntities) {{ return 0, zero, false }}\n\tindex := int(query.Cursor)\n\tquery.Cursor++\n\treturn query.Store.DenseEntities[index], query.Store.DenseValues[index], true\n}}\n\nfunc (store *{0}Store) TakeChanges() ([]uint64, []uint64, []uint64, uint64) {{\n\tadded := store.Added\n\tchanged := store.Changed\n\tremoved := store.Removed\n\tstore.Added = nil\n\tstore.Changed = nil\n\tstore.Removed = nil\n\treturn added, changed, removed, store.Revision\n}}",
        component.type_name
    )
    .unwrap();
    source
}

fn render_game_source(
    canonical_module: &str,
    game: &SourceGame,
    module_fingerprint: [u8; 32],
) -> String {
    let canonical = canonical_game(canonical_module, game, module_fingerprint);
    let artifact_id: [u8; 32] = Sha256::digest(format!("artifact:{canonical}").as_bytes()).into();
    let schema_fingerprint: [u8; 32] = Sha256::digest(canonical.as_bytes()).into();
    let role_fingerprint: [u8; 32] =
        Sha256::digest(format!("roles:{}", game.roles.join(",")).as_bytes()).into();
    let mut factory_bytes = [0_u8; 8];
    factory_bytes.copy_from_slice(&artifact_id[..8]);
    let mut factory_id = u64::from_le_bytes(factory_bytes);
    if factory_id == 0 {
        factory_id = 1;
    }
    let mut source = format!(
        "// governed:voplay-codegen game_factory_id={factory_id}\npackage {}\n\nimport (\n\t\"errors\"\n\t\"github.com/vo-lang/voplay\"\n\t\"github.com/vo-lang/voplay/vo/world\"\n)\n\n",
        game.package
    );
    writeln!(source, "type {} struct {{", game.init_type).unwrap();
    for field in &game.init_field {
        writeln!(
            source,
            "\t{} {}",
            export_name(&field.name),
            vo_type(&field.r#type)
        )
        .unwrap();
    }
    writeln!(source, "}}\n").unwrap();
    writeln!(
        source,
        "type {}GeneratedGame struct {{\n\tValue {}\n\tInit {}\n\tWorld *world.World\n}}\n",
        game.type_name, game.type_name, game.init_type
    )
    .unwrap();
    writeln!(
        source,
        "func (game *{}GeneratedGame) Configure(builder voplay.Builder) {{\n\t{}(&game.Value, builder)\n}}\n",
        game.type_name, game.configure
    )
    .unwrap();
    writeln!(
        source,
        "func (game *{}GeneratedGame) Start(context voplay.StartContext) error {{\n\treturn {}(&game.Value, game.Init, context)\n}}\n",
        game.type_name, game.start
    )
    .unwrap();
    writeln!(
        source,
        "func (game *{}GeneratedGame) Execute(invocation voplay.SystemInvocation) (voplay.TickOutput, error) {{\n\treturn {}(&game.Value, invocation)\n}}\n",
        game.type_name, game.execute
    )
    .unwrap();
    render_init_codec(&mut source, game);
    render_binary_helpers(&mut source, &game.type_name);
    writeln!(
        source,
        "func New{}EntryDescriptor() voplay.GameEntryDescriptor {{",
        game.type_name
    )
    .unwrap();
    writeln!(source, "\treturn voplay.GameEntryDescriptor{{").unwrap();
    writeln!(source, "\t\tArtifactId: {},", vo_byte_array(&artifact_id)).unwrap();
    writeln!(source, "\t\tFactoryId: {factory_id},").unwrap();
    writeln!(
        source,
        "\t\tSchemaFingerprint: {},",
        vo_byte_array(&schema_fingerprint)
    )
    .unwrap();
    writeln!(
        source,
        "\t\tRoleArtifactSetFingerprint: {},",
        vo_byte_array(&role_fingerprint)
    )
    .unwrap();
    writeln!(source, "\t}}\n}}\n").unwrap();
    writeln!(
        source,
        "func __vo_entry_meta_v1_voplay_{}_{}_{}_{}(initBytes []byte) {{",
        factory_id,
        hex(&artifact_id),
        hex(&schema_fingerprint),
        hex(&role_fingerprint),
    )
    .unwrap();
    writeln!(
        source,
        "\tinit, ok := {}DecodeInit(initBytes)\n\tif !ok {{ panic(\"generated Voplay init codec rejected target-island payload\") }}\n\tengine := voplay.TargetEngine()\n\tworldValue, ok := world.NewWorld(world.WorldRef{{EngineIndex: engine.Engine.Index, EngineGeneration: engine.Engine.Generation, WorldIndex: 1, WorldGeneration: 1}}, {}, {}, {}, {})\n\tif !ok {{ panic(\"generated Voplay World configuration is invalid\") }}\n\tgame := &{}GeneratedGame{{Init: init, World: worldValue}}\n\tbuilder := &{}GeneratedBuilder{{}}\n\tgame.Configure(builder)\n\tif builder.Failure != nil {{ panic(builder.Failure.Error()) }}\n\torderedSystems, scheduleHash, ok := voplay.FreezeSchedule(builder.Systems)\n\tif !ok {{ panic(\"generated Voplay schedule is invalid\") }}\n\tbuilder.Systems = orderedSystems\n\tbuilder.ScheduleHash = scheduleHash\n\tcontext := &{}GeneratedStartContext{{NextHandle: 1, Builder: builder, WorldValue: worldValue}}\n\tif err := game.Start(context); err != nil {{ panic(err.Error()) }}\n\tif builder.Failure != nil {{ panic(builder.Failure.Error()) }}\n\tif err := voplay.TargetStart(builder.Configuration()); err != nil {{ panic(err.Error()) }}\n\tstages := []voplay.Stage{{voplay.StagePreTick, voplay.StageInput, voplay.StageGameplay, voplay.StagePrePhysics, voplay.StagePhysics, voplay.StagePostPhysics, voplay.StagePostTick, voplay.StageExtract}}\n\tfor {{\n\t\ttickBytes, err := voplay.TargetNextTicks()\n\t\tif err != nil {{ panic(err.Error()) }}\n\t\tbatch, ok := {}DecodeTickBatch(tickBytes)\n\t\tif !ok {{ panic(\"generated Voplay tick codec rejected provider payload\") }}\n\t\toutput := voplay.TickOutput{{}}\n\t\tfor offset := uint64(0); offset < batch.Count; offset++ {{\n\t\t\ttick := batch\n\t\t\ttick.FirstTick = batch.FirstTick+offset\n\t\t\ttick.Count = 1\n\t\t\tif offset > 0 {{ tick.InputFrames = nil; tick.RenderReturns = nil; tick.AssetReturns = nil; tick.AudioReturns = nil; tick.InitialEntities = nil; tick.RequestedAssets = nil; tick.RenderViews = nil }}\n\t\t\tfor _, stage := range stages {{\n\t\t\t\tfor _, system := range builder.Systems {{\n\t\t\t\t\tif system.Stage != stage {{ continue }}\n\t\t\t\t\ttickOutput, err := game.Execute(voplay.SystemInvocation{{System: system, Tick: tick, World: game.World}})\n\t\t\t\t\tif err != nil {{ panic(err.Error()) }}\n\t\t\t\t\tpacketCount := len(output.RenderPackets)+len(output.AssetPackets)+len(output.AudioPackets)+len(tickOutput.RenderPackets)+len(tickOutput.AssetPackets)+len(tickOutput.AudioPackets)\n\t\t\t\t\tif packetCount > 4096 {{ panic(\"generated Voplay tick output exceeds provider packet limit\") }}\n\t\t\t\t\toutput.RenderPackets = append(output.RenderPackets, tickOutput.RenderPackets...)\n\t\t\t\t\toutput.AssetPackets = append(output.AssetPackets, tickOutput.AssetPackets...)\n\t\t\t\t\toutput.AudioPackets = append(output.AudioPackets, tickOutput.AudioPackets...)\n\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t\tresult, ok := {}EncodeTickOutput(output)\n\t\tif !ok {{ panic(\"generated Voplay tick output exceeds provider limits\") }}\n\t\tif err := voplay.TargetCommitTicks(batch.FirstTick, batch.Count, result); err != nil {{ panic(err.Error()) }}\n\t}}\n}}\n",
        game.type_name,
        game.max_world_entities,
        game.max_world_commands,
        game.max_world_changes,
        game.max_world_component_bytes,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name
    )
    .unwrap();
    render_target_bootstrap(&mut source, game);
    source = upgrade_generated_tick_input_v4(source, game);
    writeln!(
        source,
        "func New{}OwnedInit(value {}) (voplay.OwnedInitData, bool) {{",
        game.type_name, game.init_type
    )
    .unwrap();
    writeln!(
        source,
        "\tbytes, ok := {}EncodeInit(value)\n\tif !ok {{ return voplay.OwnedInitData{{}}, false }}\n\treturn voplay.OwnedInitData{{Bytes: bytes}}, true\n}}",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\nfunc __VoplayRun{}(game *{}GeneratedGame) error {{",
        game.type_name, game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\tinit, ok := New{}OwnedInit(game.Init)",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\tif !ok {{ return errors.New(\"generated init config exceeds descriptor limit\") }}"
    )
    .unwrap();
    writeln!(
        source,
        "\tdescriptor := New{}EntryDescriptor()",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\tdescriptorBytes := make([]byte, 0, 104)\n\tdescriptorBytes = append(descriptorBytes, descriptor.ArtifactId[:]...)\n\tdescriptorBytes = {}AppendU64(descriptorBytes, descriptor.FactoryId)\n\tdescriptorBytes = append(descriptorBytes, descriptor.SchemaFingerprint[:]...)\n\tdescriptorBytes = append(descriptorBytes, descriptor.RoleArtifactSetFingerprint[:]...)",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\treturn voplay.RunEntry(descriptorBytes, init.Bytes)"
    )
    .unwrap();
    writeln!(source, "}}").unwrap();
    writeln!(
        source,
        "\nfunc __VoplayInstall{}(engine voplay.EngineRef, game *{}GeneratedGame, init voplay.OwnedInitData) (voplay.GameEntryDescriptor, error) {{",
        game.type_name, game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\tdescriptor := New{}EntryDescriptor()",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\tdescriptorBytes := make([]byte, 0, 104)\n\tdescriptorBytes = append(descriptorBytes, descriptor.ArtifactId[:]...)\n\tdescriptorBytes = {}AppendU64(descriptorBytes, descriptor.FactoryId)\n\tdescriptorBytes = append(descriptorBytes, descriptor.SchemaFingerprint[:]...)\n\tdescriptorBytes = append(descriptorBytes, descriptor.RoleArtifactSetFingerprint[:]...)",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "\tif len(init.Bytes) > 16777216 {{ return voplay.GameEntryDescriptor{{}}, errors.New(\"generated init config exceeds descriptor limit\") }}\n\tif err := voplay.InstallEntry(engine, descriptorBytes, init.Bytes); err != nil {{ return voplay.GameEntryDescriptor{{}}, err }}\n\treturn descriptor, nil\n}}"
    )
    .unwrap();
    source
}

fn upgrade_generated_tick_input_v4(mut source: String, game: &SourceGame) -> String {
    source = source.replace(
        "if offset > 0 { tick.RenderReturns = nil;",
        "if offset > 0 { tick.InputFrames = nil; tick.PresentationPulses = nil; tick.RenderReturns = nil;",
    );
    source = source.replace(
        "tick.InputFrames = nil; tick.RenderReturns = nil;",
        "tick.InputFrames = nil; tick.PresentationPulses = nil; tick.RenderReturns = nil;",
    );
    source = source.replace(
        "tick.AudioReturns = nil;",
        "tick.AudioReturns = nil; tick.LogicReturns = nil;",
    );
    source = source.replace("if len(input) < 60 ||", "if len(input) < 72 ||");
    source = source.replace("ReadU32(input, 0) != 3", "ReadU32(input, 0) != 6");
    source = source.replace(
        "\tbatch.RenderReturns, offset, ok = ",
        &format!(
            "\tbatch.InputFrames, offset, ok = {}DecodeTickPackets(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.PresentationPulses, offset, ok = {}DecodeTickPackets(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.RenderReturns, offset, ok = ",
            game.type_name, game.type_name
        ),
    );
    source = source.replace(
        "batch.InitialEntities, offset, ok = ",
        &format!(
            "batch.LogicReturns, offset, ok = {}DecodeTickPackets(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.InitialEntities, offset, ok = ",
            game.type_name
        ),
    );
    source = source.replace(
        "len(batch.RenderReturns)+len(batch.AssetReturns)+len(batch.AudioReturns) > 4096",
        "len(batch.InputFrames)+len(batch.PresentationPulses) > 4096 || len(batch.RenderReturns)+len(batch.AssetReturns)+len(batch.AudioReturns)+len(batch.LogicReturns) > 4096",
    );
    source = source.replace(
        "len(output.AudioPackets)",
        "len(output.AudioPackets)+len(output.LogicPackets)",
    );
    source = source.replace(
        "len(tickOutput.AudioPackets)",
        "len(tickOutput.AudioPackets)+len(tickOutput.LogicPackets)",
    );
    source = source.replace(
        "len(frameOutput.AudioPackets)",
        "len(frameOutput.AudioPackets)+len(frameOutput.LogicPackets)",
    );
    source = source.replace(
        "output.AudioPackets = append(output.AudioPackets, tickOutput.AudioPackets...)",
        "output.AudioPackets = append(output.AudioPackets, tickOutput.AudioPackets...)\n\t\t\t\t\toutput.LogicPackets = append(output.LogicPackets, tickOutput.LogicPackets...)",
    );
    source = source.replace(
        "output.AudioPackets = append(output.AudioPackets, frameOutput.AudioPackets...)",
        "output.AudioPackets = append(output.AudioPackets, frameOutput.AudioPackets...)\n\t\t\t\toutput.LogicPackets = append(output.LogicPackets, frameOutput.LogicPackets...)",
    );
    let audio_output_tail = format!(
        "\t\tresult, ok = {}AppendTickOutputPacket(result, 3, packet)\n\t\tif !ok {{ return nil, false }}\n\t}}\n\treturn result, true\n}}\n\nfunc ",
        game.type_name
    );
    source = source.replace(
        &audio_output_tail,
        &format!(
            "\t\tresult, ok = {}AppendTickOutputPacket(result, 3, packet)\n\t\tif !ok {{ return nil, false }}\n\t}}\n\tfor _, packet := range output.LogicPackets {{\n\t\tvar ok bool\n\t\tresult, ok = {}AppendTickOutputPacket(result, 4, packet)\n\t\tif !ok {{ return nil, false }}\n\t}}\n\treturn result, true\n}}\n\nfunc ",
            game.type_name, game.type_name
        ),
    );
    source = source.replace(
        "stage == voplay.StageStartup || stage == voplay.StageShutdown",
        "stage == voplay.StageStartup || stage == voplay.StageShutdown",
    );
    let commit_marker = format!(
        "\t\tresult, ok := {}EncodeTickOutput(output)",
        game.type_name
    );
    let frame_loop = format!(
        "\t\tfor _, pulse := range batch.PresentationPulses {{\n\t\t\tfor _, system := range builder.Systems {{\n\t\t\t\tif system.Stage != voplay.StageFrame {{ continue }}\n\t\t\t\tframeOutput, err := game.Execute(voplay.SystemInvocation{{System: system, Tick: batch, World: game.World, PresentationPulse: pulse}})\n\t\t\t\tif err != nil {{ panic(err.Error()) }}\n\t\t\t\tpacketCount := len(output.RenderPackets)+len(output.AssetPackets)+len(output.AudioPackets)+len(output.LogicPackets)+len(frameOutput.RenderPackets)+len(frameOutput.AssetPackets)+len(frameOutput.AudioPackets)+len(frameOutput.LogicPackets)\n\t\t\t\tif packetCount > 4096 {{ panic(\"generated Voplay frame output exceeds provider packet limit\") }}\n\t\t\t\toutput.RenderPackets = append(output.RenderPackets, frameOutput.RenderPackets...)\n\t\t\t\toutput.AssetPackets = append(output.AssetPackets, frameOutput.AssetPackets...)\n\t\t\t\toutput.AudioPackets = append(output.AudioPackets, frameOutput.AudioPackets...)\n\t\t\t\toutput.LogicPackets = append(output.LogicPackets, frameOutput.LogicPackets...)\n\t\t\t}}\n\t\t}}\n{commit_marker}"
    );
    source = source.replace(&commit_marker, &frame_loop);
    source
}

fn render_target_bootstrap(source: &mut String, game: &SourceGame) {
    writeln!(
        source,
        "func {}DecodeStartupRecords(input []byte, offset int) ([]voplay.StartupRecord, int, bool) {{\n\tif offset < 0 || offset+4 > len(input) {{ return nil, offset, false }}\n\tcountValue := {}ReadU32(input, offset)\n\toffset += 4\n\tif countValue > 65536 {{ return nil, offset, false }}\n\trecords := make([]voplay.StartupRecord, 0, int(countValue))\n\tfor index := uint32(0); index < countValue; index++ {{\n\t\tif offset+12 > len(input) {{ return nil, offset, false }}\n\t\thandleValue := {}ReadU64(input, offset)\n\t\tlengthValue := {}ReadU32(input, offset+8)\n\t\toffset += 12\n\t\tif handleValue == 0 || handleValue > 4294967295 || lengthValue == 0 || lengthValue > 1048576 {{ return nil, offset, false }}\n\t\tlength := int(lengthValue)\n\t\tif offset+length > len(input) {{ return nil, offset, false }}\n\t\tdescriptor := make([]byte, length)\n\t\tcopy(descriptor, input[offset:offset+length])\n\t\trecords = append(records, voplay.StartupRecord{{ Handle: voplay.Handle{{ Index: uint32(handleValue), Generation: 1 }}, Descriptor: descriptor }})\n\t\toffset += length\n\t}}\n\treturn records, offset, true\n}}\n",
        game.type_name, game.type_name, game.type_name, game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func {}DecodeTickPackets(input []byte, offset int) ([][]byte, int, bool) {{\n\tif offset < 0 || offset+4 > len(input) {{ return nil, offset, false }}\n\tcountValue := {}ReadU32(input, offset)\n\toffset += 4\n\tif countValue > 4096 {{ return nil, offset, false }}\n\tcount := int(countValue)\n\tpackets := make([][]byte, 0, count)\n\tfor index := 0; index < count; index++ {{\n\t\tif offset+4 > len(input) {{ return nil, offset, false }}\n\t\tlengthValue := {}ReadU32(input, offset)\n\t\toffset += 4\n\t\tif lengthValue == 0 || lengthValue > 1048576 {{ return nil, offset, false }}\n\t\tlength := int(lengthValue)\n\t\tif offset+length > len(input) {{ return nil, offset, false }}\n\t\tpacket := make([]byte, length)\n\t\tcopy(packet, input[offset:offset+length])\n\t\tpackets = append(packets, packet)\n\t\toffset += length\n\t}}\n\treturn packets, offset, true\n}}\n\nfunc {}DecodeTickBatch(input []byte) (voplay.TickBatch, bool) {{\n\tif len(input) < 60 || {}ReadU32(input, 0) != 3 {{ return voplay.TickBatch{{}}, false }}\n\tbatch := voplay.TickBatch{{\n\t\tFirstTick: {}ReadU64(input, 4),\n\t\tCount: {}ReadU64(input, 12),\n\t\tFixedTickNanos: {}ReadU64(input, 20),\n\t\tMonotonicNanos: {}ReadU64(input, 28),\n\t}}\n\tif batch.FirstTick == 0 || batch.Count == 0 || batch.FixedTickNanos == 0 {{ return voplay.TickBatch{{}}, false }}\n\toffset := 36\n\tvar ok bool\n\tbatch.RenderReturns, offset, ok = {}DecodeTickPackets(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.AssetReturns, offset, ok = {}DecodeTickPackets(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.AudioReturns, offset, ok = {}DecodeTickPackets(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.InitialEntities, offset, ok = {}DecodeStartupRecords(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.RequestedAssets, offset, ok = {}DecodeStartupRecords(input, offset)\n\tif !ok {{ return voplay.TickBatch{{}}, false }}\n\tbatch.RenderViews, offset, ok = {}DecodeStartupRecords(input, offset)\n\tif !ok || offset != len(input) || len(batch.RenderReturns)+len(batch.AssetReturns)+len(batch.AudioReturns) > 4096 || len(batch.InitialEntities)+len(batch.RequestedAssets)+len(batch.RenderViews) > 65536 {{ return voplay.TickBatch{{}}, false }}\n\tif batch.FirstTick != 1 && len(batch.InitialEntities)+len(batch.RequestedAssets)+len(batch.RenderViews) != 0 {{ return voplay.TickBatch{{}}, false }}\n\treturn batch, true\n}}\n",
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func {}EncodeTickOutput(output voplay.TickOutput) ([]byte, bool) {{\n\tcount := len(output.RenderPackets)+len(output.AssetPackets)+len(output.AudioPackets)\n\tif count > 4096 {{ return nil, false }}\n\tresult := []byte(\"voplay-tick-output-v1\\x00\")\n\tresult = {}AppendU32(result, uint32(count))\n\tfor _, packet := range output.RenderPackets {{\n\t\tvar ok bool\n\t\tresult, ok = {}AppendTickOutputPacket(result, 1, packet)\n\t\tif !ok {{ return nil, false }}\n\t}}\n\tfor _, packet := range output.AssetPackets {{\n\t\tvar ok bool\n\t\tresult, ok = {}AppendTickOutputPacket(result, 2, packet)\n\t\tif !ok {{ return nil, false }}\n\t}}\n\tfor _, packet := range output.AudioPackets {{\n\t\tvar ok bool\n\t\tresult, ok = {}AppendTickOutputPacket(result, 3, packet)\n\t\tif !ok {{ return nil, false }}\n\t}}\n\treturn result, true\n}}\n\nfunc {}AppendTickOutputPacket(result []byte, role byte, packet []byte) ([]byte, bool) {{\n\tif len(packet) == 0 || len(packet) > 1048576 || len(result)+5+len(packet) > 16777216 {{ return nil, false }}\n\tresult = append(result, role)\n\tresult = {}AppendU32(result, uint32(len(packet)))\n\treturn append(result, packet...), true\n}}\n",
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name,
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "type {}GeneratedBuilder struct {{\n\tOperations []byte\n\tCount uint32\n\tSystems []voplay.RegisteredSystem\n\tScheduleHash uint64\n\tFailure error\n}}\n",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func (builder *{}GeneratedBuilder) appendOperation(tag byte, first []byte, second []byte) error {{\n\tif builder.Failure != nil {{ return builder.Failure }}\n\tif builder.Count == 65536 || len(first) > 1048576 || len(second) > 1048576 || len(builder.Operations)+9+len(first)+len(second)+27 > 16777216 {{\n\t\tbuilder.Failure = errors.New(\"generated Voplay configuration exceeds startup limit\")\n\t\treturn builder.Failure\n\t}}\n\tbuilder.Operations = append(builder.Operations, tag)\n\tbuilder.Operations = {}AppendU32(builder.Operations, uint32(len(first)))\n\tbuilder.Operations = {}AppendU32(builder.Operations, uint32(len(second)))\n\tbuilder.Operations = append(builder.Operations, first...)\n\tbuilder.Operations = append(builder.Operations, second...)\n\tbuilder.Count++\n\treturn nil\n}}\n",
        game.type_name, game.type_name, game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func (builder *{}GeneratedBuilder) Configuration() []byte {{\n\tconfiguration := []byte(\"voplay-target-start-v3\\x00\")\n\tconfiguration = {}AppendU64(configuration, builder.ScheduleHash)\n\tconfiguration = {}AppendU32(configuration, builder.Count)\n\treturn append(configuration, builder.Operations...)\n}}\n",
        game.type_name, game.type_name, game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func (builder *{}GeneratedBuilder) RegisterSystem(stage voplay.Stage, descriptor []byte) error {{\n\treturn builder.RegisterScheduledSystem(voplay.SystemSpec{{Stage: stage, Descriptor: descriptor, Deterministic: true}})\n}}\n\nfunc (builder *{}GeneratedBuilder) RegisterScheduledSystem(spec voplay.SystemSpec) error {{\n\tif !spec.Stage.Valid() || spec.Stage == voplay.StageStartup || spec.Stage == voplay.StageShutdown || len(spec.Descriptor) == 0 {{ return errors.New(\"generated Voplay system stage is not driven by the target loop\") }}\n\tid := spec.Id\n\tif id == 0 {{\n\t\tid = uint64(14695981039346656037)\n\t\tfor _, value := range spec.Descriptor {{ id = (id ^ uint64(value)) * 1099511628211 }}\n\t\tid = (id ^ uint64(spec.Stage)) * 1099511628211\n\t\tif id == 0 {{ id = 1 }}\n\t}}\n\tfor _, system := range builder.Systems {{ if system.Id == id {{ return errors.New(\"generated Voplay system identity is duplicated\") }} }}\n\tcopyDescriptor := append([]byte{{}}, spec.Descriptor...)\n\tfirst := {}AppendU32(nil, uint32(spec.Stage))\n\tfirst = {}AppendU64(first, id)\n\tif err := builder.appendOperation(2, first, copyDescriptor); err != nil {{ return err }}\n\tbuilder.Systems = append(builder.Systems, voplay.RegisteredSystem{{Id: id, Stage: spec.Stage, Descriptor: copyDescriptor, Deterministic: spec.Deterministic, SimulationReads: append([]uint64{{}}, spec.SimulationReads...), SimulationWrites: append([]uint64{{}}, spec.SimulationWrites...), PresentationReads: append([]uint64{{}}, spec.PresentationReads...), PresentationWrites: append([]uint64{{}}, spec.PresentationWrites...), Before: append([]uint64{{}}, spec.Before...), After: append([]uint64{{}}, spec.After...)}})\n\treturn nil\n}}\n",
        game.type_name, game.type_name, game.type_name, game.type_name
    )
    .unwrap();
    for (method, tag, argument, first, second) in [
        ("RegisterComponent", 1, "schema []byte", "schema", "nil"),
        (
            "RegisterPlugin",
            3,
            "descriptor []byte",
            "descriptor",
            "nil",
        ),
        (
            "RegisterAssetLoader",
            4,
            "descriptor []byte",
            "descriptor",
            "nil",
        ),
        (
            "RegisterRenderFeature",
            5,
            "descriptor []byte",
            "descriptor",
            "nil",
        ),
    ] {
        writeln!(
            source,
            "func (builder *{}GeneratedBuilder) {}({}) error {{\n\treturn builder.appendOperation({}, {}, {})\n}}\n",
            game.type_name, method, argument, tag, first, second
        )
        .unwrap();
    }
    writeln!(
        source,
        "func (builder *{}GeneratedBuilder) SetFixedTick(nanos uint64, maxCatchUp uint32) error {{\n\tfirst := {}AppendU64(nil, nanos)\n\tsecond := {}AppendU32(nil, maxCatchUp)\n\treturn builder.appendOperation(6, first, second)\n}}\n",
        game.type_name, game.type_name, game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "type {}GeneratedStartContext struct {{\n\tNextHandle uint32\n\tBuilder *{}GeneratedBuilder\n\tWorldValue *world.World\n}}\n",
        game.type_name,
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func (context *{}GeneratedStartContext) Engine() voplay.EngineRef {{\n\treturn voplay.TargetEngine()\n}}\n",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func (context *{}GeneratedStartContext) World() *world.World {{\n\treturn context.WorldValue\n}}\n",
        game.type_name
    )
    .unwrap();
    writeln!(
        source,
        "func (context *{}GeneratedStartContext) Spawn(initialComponents []byte) (voplay.Handle, error) {{\n\tif err := context.Builder.appendOperation(16, initialComponents, nil); err != nil {{ return voplay.Handle{{}}, err }}\n\tcommands, ok := context.WorldValue.Begin()\n\tif !ok || !commands.Spawn(uint64(context.NextHandle), []world.ComponentRecord{{world.ComponentRecord{{Component: 1, Value: initialComponents}}}}) {{ return voplay.Handle{{}}, errors.New(\"generated Voplay World rejected startup entity\") }}\n\tif _, ok = commands.Commit(); !ok {{ return voplay.Handle{{}}, errors.New(\"generated Voplay World failed startup transaction\") }}\n\tentity, ok := context.WorldValue.Entity(uint64(context.NextHandle))\n\tif !ok {{ return voplay.Handle{{}}, errors.New(\"generated Voplay World lost startup entity\") }}\n\thandle := voplay.Handle{{Index: entity.Index, Generation: entity.Generation}}\n\tcontext.NextHandle++\n\treturn handle, nil\n}}\n",
        game.type_name
    )
    .unwrap();
    for (method, tag, argument, payload) in [
        ("RequestAsset", 17, "descriptor []byte", "descriptor"),
        ("CreateRenderView", 18, "descriptor []byte", "descriptor"),
    ] {
        writeln!(
            source,
            "func (context *{}GeneratedStartContext) {}({}) (voplay.Handle, error) {{\n\tif err := context.Builder.appendOperation({}, {}, nil); err != nil {{ return voplay.Handle{{}}, err }}\n\thandle := voplay.Handle{{Index: context.NextHandle, Generation: 1}}\n\tcontext.NextHandle++\n\treturn handle, nil\n}}\n",
            game.type_name, method, argument, tag, payload
        )
        .unwrap();
    }
}

fn canonical_game(
    canonical_module: &str,
    game: &SourceGame,
    module_fingerprint: [u8; 32],
) -> String {
    let mut canonical = format!(
        "module={canonical_module};package={};game={};init={};configure={};start={};execute={};max_init={};world={},{},{},{};components={}",
        game.package,
        game.type_name,
        game.init_type,
        game.configure,
        game.start,
        game.execute,
        game.max_init_bytes,
        game.max_world_entities,
        game.max_world_commands,
        game.max_world_changes,
        game.max_world_component_bytes,
        hex(&module_fingerprint),
    );
    for role in &game.roles {
        write!(canonical, ";role={role}").unwrap();
    }
    for field in &game.init_field {
        write!(
            canonical,
            ";init_field={},{},{},{}",
            field.name, field.r#type, field.default, field.editor
        )
        .unwrap();
    }
    canonical
}

fn render_game_manifest(
    canonical_module: &str,
    game: &SourceGame,
    module_fingerprint: [u8; 32],
    path: &str,
) -> String {
    let canonical = canonical_game(canonical_module, game, module_fingerprint);
    let artifact_id: [u8; 32] = Sha256::digest(format!("artifact:{canonical}").as_bytes()).into();
    let schema_fingerprint: [u8; 32] = Sha256::digest(canonical.as_bytes()).into();
    let role_fingerprint: [u8; 32] =
        Sha256::digest(format!("roles:{}", game.roles.join(",")).as_bytes()).into();
    format!(
        "\n[game]\nname = {:?}\npackage = {:?}\nsource = {:?}\nartifact_id = \"{}\"\nschema_fingerprint = \"{}\"\nrole_artifact_set_fingerprint = \"{}\"\nroles = {:?}\nconfigure = {:?}\nstart = {:?}\nexecute = {:?}\nmax_world_entities = {}\nmax_world_commands = {}\nmax_world_changes = {}\nmax_world_component_bytes = {}\n",
        game.type_name,
        game.package,
        path,
        hex(&artifact_id),
        hex(&schema_fingerprint),
        hex(&role_fingerprint),
        game.roles,
        game.configure,
        game.start,
        game.execute,
        game.max_world_entities,
        game.max_world_commands,
        game.max_world_changes,
        game.max_world_component_bytes,
    )
}

fn render_init_codec(source: &mut String, game: &SourceGame) {
    writeln!(
        source,
        "func {}EncodeInit(value {}) ([]byte, bool) {{",
        game.type_name, game.init_type
    )
    .unwrap();
    writeln!(source, "\toutput := make([]byte, 0)").unwrap();
    for field in &game.init_field {
        render_encode_field(source, &game.type_name, field);
    }
    writeln!(
        source,
        "\tif len(output) > {} {{ return nil, false }}\n\treturn output, true\n}}\n",
        game.max_init_bytes
    )
    .unwrap();
    writeln!(
        source,
        "func {}DecodeInit(input []byte) ({}, bool) {{",
        game.type_name, game.init_type
    )
    .unwrap();
    writeln!(
        source,
        "\tvalue := {}{{}}\n\tif len(input) > {} {{ return value, false }}\n\toffset := 0",
        game.init_type, game.max_init_bytes
    )
    .unwrap();
    for field in &game.init_field {
        render_decode_field(source, &game.type_name, field);
    }
    writeln!(
        source,
        "\tif offset != len(input) {{ return value, false }}\n\treturn value, true\n}}\n"
    )
    .unwrap();
}

fn render_encode_field(source: &mut String, game: &str, field: &SourceField) {
    let name = export_name(&field.name);
    match field.r#type.as_str() {
        "bool" => writeln!(
            source,
            "\tif value.{name} {{ output = append(output, 1) }} else {{ output = append(output, 0) }}"
        )
        .unwrap(),
        "int32" | "uint32" | "float32" => writeln!(
            source,
            "\toutput = {game}AppendU32(output, uint32(value.{name}))"
        )
        .unwrap(),
        "int64" | "uint64" | "float64" => writeln!(
            source,
            "\toutput = {game}AppendU64(output, uint64(value.{name}))"
        )
        .unwrap(),
        "string" => writeln!(
            source,
            "\tif len(value.{name}) > 4294967295 {{ return nil, false }}\n\toutput = {game}AppendU32(output, uint32(len(value.{name})))\n\toutput = append(output, []byte(value.{name})...)"
        )
        .unwrap(),
        "bytes" => writeln!(
            source,
            "\tif len(value.{name}) > 4294967295 {{ return nil, false }}\n\toutput = {game}AppendU32(output, uint32(len(value.{name})))\n\toutput = append(output, value.{name}...)"
        )
        .unwrap(),
        _ => unreachable!("game fields are validated"),
    }
}

fn render_decode_field(source: &mut String, game: &str, field: &SourceField) {
    let name = export_name(&field.name);
    match field.r#type.as_str() {
        "bool" => writeln!(
            source,
            "\tif offset >= len(input) || input[offset] > 1 {{ return value, false }}\n\tvalue.{name} = input[offset] == 1\n\toffset++"
        )
        .unwrap(),
        "int32" => render_fixed_decode(source, game, &name, "int32", 4, "ReadU32"),
        "uint32" => render_fixed_decode(source, game, &name, "", 4, "ReadU32"),
        "int64" => render_fixed_decode(source, game, &name, "int64", 8, "ReadU64"),
        "uint64" => render_fixed_decode(source, game, &name, "", 8, "ReadU64"),
        "string" | "bytes" => {
            let length = format!("{name}Length");
            writeln!(
                source,
                "\tif len(input) - offset < 4 {{ return value, false }}\n\t{length} := int({game}ReadU32(input, offset))\n\toffset += 4\n\tif {length} > len(input) - offset {{ return value, false }}"
            )
            .unwrap();
            if field.r#type == "string" {
                writeln!(
                    source,
                    "\tvalue.{name} = string(input[offset:offset + {length}])\n\toffset += {length}"
                )
                .unwrap();
            } else {
                writeln!(
                    source,
                    "\tvalue.{name} = make([]byte, {length})\n\tcopy(value.{name}, input[offset:offset + {length}])\n\toffset += {length}"
                )
                .unwrap();
            }
        }
        _ => unreachable!("game fields are validated"),
    }
}

fn render_fixed_decode(
    source: &mut String,
    game: &str,
    name: &str,
    cast: &str,
    bytes: usize,
    reader: &str,
) {
    let expression = if cast.is_empty() {
        format!("{game}{reader}(input, offset)")
    } else {
        format!("{cast}({game}{reader}(input, offset))")
    };
    writeln!(
        source,
        "\tif len(input) - offset < {bytes} {{ return value, false }}\n\tvalue.{name} = {expression}\n\toffset += {bytes}"
    )
    .unwrap();
}

fn render_binary_helpers(source: &mut String, game: &str) {
    writeln!(source, "func {game}AppendU32(output []byte, value uint32) []byte {{\n\treturn append(output, byte(value), byte(value >> 8), byte(value >> 16), byte(value >> 24))\n}}\n").unwrap();
    writeln!(source, "func {game}AppendU64(output []byte, value uint64) []byte {{\n\treturn append(output, byte(value), byte(value >> 8), byte(value >> 16), byte(value >> 24), byte(value >> 32), byte(value >> 40), byte(value >> 48), byte(value >> 56))\n}}\n").unwrap();
    writeln!(source, "func {game}ReadU32(input []byte, offset int) uint32 {{\n\treturn uint32(input[offset]) | uint32(input[offset + 1]) << 8 | uint32(input[offset + 2]) << 16 | uint32(input[offset + 3]) << 24\n}}\n").unwrap();
    writeln!(source, "func {game}ReadU64(input []byte, offset int) uint64 {{\n\treturn uint64(input[offset]) | uint64(input[offset + 1]) << 8 | uint64(input[offset + 2]) << 16 | uint64(input[offset + 3]) << 24 | uint64(input[offset + 4]) << 32 | uint64(input[offset + 5]) << 40 | uint64(input[offset + 6]) << 48 | uint64(input[offset + 7]) << 56\n}}\n").unwrap();
}

fn vo_type(source: &str) -> &'static str {
    match source {
        "bool" => "bool",
        "int32" => "int32",
        "uint32" => "uint32",
        "int64" => "int64",
        "uint64" => "uint64",
        "float32" => "float32",
        "float64" => "float64",
        "string" => "string",
        "bytes" => "[]byte",
        _ => unreachable!("field type is validated before generation"),
    }
}

fn valid_module_path(value: &str) -> bool {
    let segments = value.split('/').collect::<Vec<_>>();
    segments.len() >= 2
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && *segment != "."
                && *segment != ".."
                && segment
                    .chars()
                    .next()
                    .is_some_and(|first| first == '_' || first.is_ascii_alphanumeric())
                && segment.chars().all(|character| {
                    character == '_'
                        || character == '-'
                        || character == '.'
                        || character.is_ascii_alphanumeric()
                })
        })
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn export_name(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn snake_case(value: &str) -> String {
    let mut result = String::new();
    for (index, character) in value.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                result.push('_');
            }
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

fn vo_byte_array(bytes: &[u8; 32]) -> String {
    let mut result = String::from("[32]byte{");
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            result.push_str(", ");
        }
        write!(result, "{byte}").unwrap();
    }
    result.push('}');
    result
}

fn decode_hex_16(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 {
        return None;
    }
    let mut result = [0; 16];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}

fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").unwrap();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"
format = 1
canonical_module = "github.com/acme/smoke-game"

[[component]]
package = "game"
type_name = "Position"
schema_major = 1
classification = "simulation"

[[component.field]]
name = "x"
type = "int64"
default = "0"
editor = "distance"

[[component.field]]
name = "y"
type = "int64"
default = "0"
editor = "distance"

[game]
package = "game"
type_name = "Demo"
init_type = "DemoInit"
configure = "ConfigureDemo"
start = "StartDemo"
execute = "ExecuteDemo"
max_init_bytes = 4096
roles = ["logic", "asset", "render"]

[[game.init_field]]
name = "seed"
type = "uint64"
default = "1"
editor = ""
"#;

    #[test]
    fn governed_generation_is_deterministic_and_emits_component_game_and_manifest() {
        let capabilities = vec![String::from("core"), String::from("render2d")];
        let first = generate_governed(
            "game.voplay.toml",
            SCHEMA,
            "vo-1",
            "wasm32-unknown-unknown",
            &capabilities,
        )
        .unwrap();
        let second = generate_governed(
            "game.voplay.toml",
            SCHEMA,
            "vo-1",
            "wasm32-unknown-unknown",
            &capabilities,
        )
        .unwrap();

        assert_eq!(first, second);
        assert_ne!(first.module.module_fingerprint, [0; 32]);
        assert_eq!(
            first
                .output
                .artifacts
                .iter()
                .map(|artifact| artifact.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "position_component.vo",
                "demo_game.vo",
                "generated/voplay_components.manifest",
            ]
        );
        let game = first
            .output
            .artifacts
            .iter()
            .find(|artifact| artifact.path == "demo_game.vo")
            .unwrap();
        let source = std::str::from_utf8(&game.bytes).unwrap();
        assert!(source.contains("func NewDemoEntryDescriptor()"));
        assert!(source.contains("func __VoplayRunDemo("));
        assert!(source.contains("voplay.TargetCommitTicks"));
        assert!(source.contains("PresentationPulses"));
    }

    #[test]
    fn schema_failures_keep_governed_diagnostic_identity_and_source_path() {
        let invalid = SCHEMA.replace("name = \"y\"", "name = \"x\"");
        let diagnostics =
            generate_governed("broken.voplay.toml", &invalid, "vo-1", "native", &[]).unwrap_err();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "VOPLAY-GEN-001");
        assert_eq!(diagnostics[0].stage, "schema");
        assert_eq!(diagnostics[0].source_path, "broken.voplay.toml");
        assert!(diagnostics[0].message.contains("DuplicateField"));
    }

    #[test]
    fn game_role_and_init_contracts_are_rejected_before_artifact_emission() {
        let invalid_roles = SCHEMA.replace(
            "roles = [\"logic\", \"asset\", \"render\"]",
            "roles = [\"asset\", \"render\"]",
        );
        let diagnostics =
            generate_governed("game.voplay.toml", &invalid_roles, "vo-1", "native", &[])
                .unwrap_err();
        assert_eq!(diagnostics[0].code, "VOPLAY-GEN-005");
        assert!(diagnostics[0].message.contains("InvalidSchema"));
    }
}
