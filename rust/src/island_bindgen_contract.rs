use std::collections::BTreeSet;

const WRAPPER_MARKER: &str = "#[vo_ext::vo_wasm_bindgen_export";
const WRAPPER_SOURCES: [(&str, &str); 5] = [
    (
        "island_bindgen/animation.rs",
        include_str!("island_bindgen/animation.rs"),
    ),
    (
        "island_bindgen/physics2d.rs",
        include_str!("island_bindgen/physics2d.rs"),
    ),
    (
        "island_bindgen/physics3d.rs",
        include_str!("island_bindgen/physics3d.rs"),
    ),
    (
        "island_bindgen/render.rs",
        include_str!("island_bindgen/render.rs"),
    ),
    (
        "island_bindgen/resource.rs",
        include_str!("island_bindgen/resource.rs"),
    ),
];

const VO_EXTERN_SOURCES: [(&str, &str, &str); 10] = [
    (
        "voplay",
        "animation_runtime.vo",
        include_str!("../../animation_runtime.vo"),
    ),
    ("voplay", "host.vo", include_str!("../../host.vo")),
    ("voplay", "resources.vo", include_str!("../../resources.vo")),
    ("voplay", "audio.vo", include_str!("../../audio.vo")),
    (
        "voplay/scene2d",
        "scene2d/physics.vo",
        include_str!("../../scene2d/physics.vo"),
    ),
    (
        "voplay/scene3d",
        "scene3d/physics_backend.vo",
        include_str!("../../scene3d/physics_backend.vo"),
    ),
    (
        "voplay/scene3d",
        "scene3d/impostor.vo",
        include_str!("../../scene3d/impostor.vo"),
    ),
    (
        "voplay/scene3d",
        "scene3d/level.vo",
        include_str!("../../scene3d/level.vo"),
    ),
    (
        "voplay/scene3d",
        "scene3d/terrain.vo",
        include_str!("../../scene3d/terrain.vo"),
    ),
    (
        "voplay/scene3d",
        "scene3d/mesh_terrain.vo",
        include_str!("../../scene3d/mesh_terrain.vo"),
    ),
];

const NATIVE_ENTRY_SOURCE: &str = include_str!("externs/mod.rs");
const NATIVE_PHYSICS3D_SOURCE: &str = include_str!("externs/physics3d.rs");
const PHYSICS_BACKEND_SOURCE: &str = include_str!("../../scene3d/physics_backend.vo");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireType {
    Bool,
    I32,
    I64,
    F32,
    F64,
    String,
    Bytes,
}

#[derive(Clone, Copy)]
enum DecoderEvent {
    One(WireType),
    TerrainSplat,
}

fn lex_vo(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let byte = bytes[offset];
        if byte.is_ascii_whitespace() {
            offset += 1;
            continue;
        }
        if byte == b'/' && bytes.get(offset + 1) == Some(&b'/') {
            offset += 2;
            while offset < bytes.len() && bytes[offset] != b'\n' {
                offset += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(offset + 1) == Some(&b'*') {
            offset += 2;
            let mut closed = false;
            while offset + 1 < bytes.len() {
                if bytes[offset] == b'*' && bytes[offset + 1] == b'/' {
                    offset += 2;
                    closed = true;
                    break;
                }
                offset += 1;
            }
            assert!(closed, "unterminated block comment in Vo extern source");
            continue;
        }
        if matches!(byte, b'"' | b'\'' | b'`') {
            let quote = byte;
            offset += 1;
            let mut closed = false;
            while offset < bytes.len() {
                if quote != b'`' && bytes[offset] == b'\\' {
                    offset = offset.saturating_add(2);
                    continue;
                }
                if bytes[offset] == quote {
                    offset += 1;
                    closed = true;
                    break;
                }
                offset += 1;
            }
            assert!(closed, "unterminated literal in Vo extern source");
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = offset;
            offset += 1;
            while offset < bytes.len()
                && (bytes[offset].is_ascii_alphanumeric() || bytes[offset] == b'_')
            {
                offset += 1;
            }
            tokens.push(source[start..offset].to_string());
            continue;
        }
        if byte.is_ascii() {
            tokens.push(char::from(byte).to_string());
        }
        offset += 1;
    }
    tokens
}

fn function_param_tokens(source: &str, function: &str) -> Option<Vec<String>> {
    let tokens = lex_vo(source);
    let mut found = None;
    for start in 0..tokens.len().saturating_sub(2) {
        if tokens[start] != "func" || tokens[start + 1] != function || tokens[start + 2] != "(" {
            continue;
        }
        assert!(
            found.is_none(),
            "duplicate Vo extern declaration for {function}"
        );
        let mut depth = 1usize;
        let mut end = start + 3;
        while end < tokens.len() && depth != 0 {
            match tokens[end].as_str() {
                "(" => depth += 1,
                ")" => depth -= 1,
                _ => {}
            }
            end += 1;
        }
        assert_eq!(depth, 0, "unterminated parameter list for {function}");
        found = Some(tokens[start + 3..end - 1].to_vec());
    }
    found
}

fn is_identifier(token: &str) -> bool {
    let mut bytes = token.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn primitive_wire_type(name: &str) -> Option<WireType> {
    match name {
        "bool" => Some(WireType::Bool),
        "int8" | "int16" | "int32" | "uint8" | "uint16" | "uint32" | "byte" | "rune" => {
            Some(WireType::I32)
        }
        "int" | "int64" | "uint" | "uint64" => Some(WireType::I64),
        "float32" => Some(WireType::F32),
        "float64" => Some(WireType::F64),
        "string" => Some(WireType::String),
        _ => None,
    }
}

fn segment_wire_type(segment: &[String]) -> Option<(WireType, usize)> {
    if segment.len() >= 3 && segment[segment.len() - 3..] == ["[", "]", "byte"] {
        return Some((WireType::Bytes, 3));
    }
    segment
        .last()
        .and_then(|name| primitive_wire_type(name))
        .map(|wire_type| (wire_type, 1))
}

fn wire_types_from_param_tokens(tokens: &[String], identity: &str) -> Vec<WireType> {
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::<Vec<String>>::new();
    let mut current = Vec::new();
    let mut nesting = 0usize;
    for token in tokens {
        match token.as_str() {
            "(" | "[" | "{" => nesting += 1,
            ")" | "]" | "}" => {
                nesting = nesting
                    .checked_sub(1)
                    .unwrap_or_else(|| panic!("unbalanced parameter type in {identity}"));
            }
            "," if nesting == 0 => {
                assert!(!current.is_empty(), "empty parameter segment in {identity}");
                segments.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(token.clone());
    }
    assert_eq!(nesting, 0, "unbalanced parameter type in {identity}");
    if !current.is_empty() {
        segments.push(current);
    }

    let mut pending_names = 0usize;
    let mut wire_types = Vec::new();
    for segment in segments {
        if let Some((wire_type, suffix_len)) = segment_wire_type(&segment) {
            let name_tokens = &segment[..segment.len() - suffix_len];
            assert_eq!(
                name_tokens.len(),
                1,
                "malformed typed parameter segment {segment:?} in {identity}",
            );
            assert!(
                is_identifier(&name_tokens[0]),
                "invalid parameter name in {identity}: {}",
                name_tokens[0],
            );
            for _ in 0..=pending_names {
                wire_types.push(wire_type);
            }
            pending_names = 0;
            continue;
        }

        assert_eq!(
            segment.len(),
            1,
            "unknown or malformed Vo extern parameter type in {identity}: {segment:?}",
        );
        assert!(
            is_identifier(&segment[0]),
            "invalid grouped parameter name in {identity}: {}",
            segment[0],
        );
        pending_names += 1;
    }
    assert_eq!(
        pending_names, 0,
        "grouped Vo extern parameters lack a closing type in {identity}",
    );
    wire_types
}

fn declared_wire_types(package: &str, function: &str) -> Vec<WireType> {
    let identity = format!("{package}.{function}");
    let mut found = Vec::new();
    for (source_package, source_path, source) in VO_EXTERN_SOURCES {
        if source_package != package {
            continue;
        }
        if let Some(tokens) = function_param_tokens(source, function) {
            found.push((
                source_path,
                wire_types_from_param_tokens(&tokens, &identity),
            ));
        }
    }
    assert_eq!(
        found.len(),
        1,
        "{identity} must have exactly one Vo extern declaration, found {found:?}",
    );
    found.pop().expect("one declaration").1
}

fn quoted_wrapper_identity<'a>(wrapper: &'a str, source_path: &str) -> (&'a str, &'a str) {
    let quoted: Vec<_> = wrapper.split('"').collect();
    assert!(
        quoted.len() >= 4,
        "malformed vo_wasm_bindgen_export attribute in {source_path}",
    );
    (quoted[1], quoted[3])
}

fn decoded_wire_types(wrapper: &str, identity: &str) -> Vec<WireType> {
    let decode_region = wrapper
        .split_once("pos.finish();")
        .unwrap_or_else(|| panic!("{identity} has no explicit input finish"))
        .0;
    let direct_decoders = [
        ("in_bool(input, &mut pos)", WireType::Bool),
        ("in_i32(input, &mut pos)", WireType::I32),
        ("in_value(input, &mut pos)", WireType::I64),
        ("in_f32(input, &mut pos)", WireType::F32),
        ("in_f64(input, &mut pos)", WireType::F64),
        ("in_str(input, &mut pos)", WireType::String),
        ("in_bytes(input, &mut pos)", WireType::Bytes),
    ];

    let mut events = Vec::<(usize, DecoderEvent)>::new();
    for (marker, wire_type) in direct_decoders {
        events.extend(
            decode_region
                .match_indices(marker)
                .map(|(offset, _)| (offset, DecoderEvent::One(wire_type))),
        );
    }
    let terrain_splat_marker = "decode_terrain_splat_input(input, &mut pos)";
    events.extend(
        decode_region
            .match_indices(terrain_splat_marker)
            .map(|(offset, _)| (offset, DecoderEvent::TerrainSplat)),
    );

    for (offset, _) in decode_region.match_indices("in_") {
        if offset > 0 {
            let previous = decode_region.as_bytes()[offset - 1];
            if previous.is_ascii_alphanumeric() || previous == b'_' {
                continue;
            }
        }
        assert!(
            events
                .iter()
                .any(|(known_offset, _)| *known_offset == offset),
            "{identity} uses an unknown input decoder near {:?}",
            &decode_region[offset..decode_region.len().min(offset + 48)],
        );
    }

    events.sort_by_key(|(offset, _)| *offset);
    let mut wire_types = Vec::new();
    for (_, event) in events {
        match event {
            DecoderEvent::One(wire_type) => wire_types.push(wire_type),
            DecoderEvent::TerrainSplat => {
                wire_types.push(WireType::I64);
                wire_types.push(WireType::Bytes);
            }
        }
    }
    wire_types
}

fn native_entry_identities() -> BTreeSet<String> {
    let marker = "vo_ext::vo_extension_entry!";
    NATIVE_ENTRY_SOURCE
        .split(marker)
        .skip(1)
        .map(|entry| {
            let end = entry
                .find(')')
                .expect("unterminated vo_extension_entry invocation");
            let quoted: Vec<_> = entry[..end].split('"').collect();
            assert_eq!(
                quoted.len(),
                5,
                "vo_extension_entry must contain exactly package and function strings",
            );
            format!("{}\0{}", quoted[1], quoted[3])
        })
        .collect()
}

fn wrapper_for<'a>(source: &'a str, package: &str, function: &str) -> &'a str {
    source
        .split(WRAPPER_MARKER)
        .skip(1)
        .find(|wrapper| quoted_wrapper_identity(wrapper, "wrapper lookup") == (package, function))
        .unwrap_or_else(|| panic!("missing wrapper for {package}.{function}"))
}

#[test]
fn all_87_wrappers_match_vo_parameter_wire_types_and_native_identities() {
    let mut wrapper_identities = BTreeSet::new();
    let mut wrapper_count = 0usize;
    for (source_path, source) in WRAPPER_SOURCES {
        for wrapper in source.split(WRAPPER_MARKER).skip(1) {
            wrapper_count += 1;
            let (package, function) = quoted_wrapper_identity(wrapper, source_path);
            let identity = format!("{package}.{function}");
            assert!(
                wrapper_identities.insert(format!("{package}\0{function}")),
                "duplicate island wrapper identity {identity}",
            );
            assert_eq!(
                wrapper.matches("DecodePosition::new(input)").count(),
                1,
                "{identity} must install exactly one complete-input guard",
            );
            assert_eq!(
                wrapper.matches("pos.finish();").count(),
                1,
                "{identity} must finish input exactly once",
            );

            let declared = declared_wire_types(package, function);
            let decoded = decoded_wire_types(wrapper, &identity);
            assert_eq!(
                decoded, declared,
                "{identity} browser decoder order diverges from its Vo declaration",
            );
        }
    }

    assert_eq!(wrapper_count, 87);
    assert_eq!(wrapper_identities.len(), 87);
    let native_identities = native_entry_identities();
    assert_eq!(native_identities.len(), 87);
    assert_eq!(wrapper_identities, native_identities);
}

#[test]
fn vo_parameter_parser_is_group_aware_comment_aware_and_fail_closed() {
    let source = r#"
        func mixed(
            first, // the grouped int32 parameter may span lines
            second int32,
            wide int64,
            compact float32,
            precise float64,
            enabled bool,
            name string,
            payload []byte,
        )
        func zero(/* comments are legal between delimiters */)
    "#;
    let mixed = function_param_tokens(source, "mixed").expect("mixed declaration");
    assert_eq!(
        wire_types_from_param_tokens(&mixed, "test.mixed"),
        vec![
            WireType::I32,
            WireType::I32,
            WireType::I64,
            WireType::F32,
            WireType::F64,
            WireType::Bool,
            WireType::String,
            WireType::Bytes,
        ],
    );
    let zero = function_param_tokens(source, "zero").expect("zero declaration");
    assert!(wire_types_from_param_tokens(&zero, "test.zero").is_empty());

    let unknown = function_param_tokens("func invalid(value uintptr)", "invalid")
        .expect("invalid declaration");
    assert!(
        std::panic::catch_unwind(|| wire_types_from_param_tokens(&unknown, "test.invalid"))
            .is_err(),
    );
}

#[test]
fn sleep_state_and_heightfield_keep_native_and_browser_contracts_aligned() {
    assert!(PHYSICS_BACKEND_SOURCE
        .contains("physicsSetBodySleepState(worldID, bodyID, command.Sleeping)",));
    assert!(PHYSICS_BACKEND_SOURCE
        .contains("func physicsSetBodySleepState(worldID, bodyID int, sleeping bool)"));

    let native_sleep = NATIVE_PHYSICS3D_SOURCE
        .split("pub fn physics3d_set_body_sleep_state")
        .nth(1)
        .expect("native sleep-state extern")
        .split("#[vo_fn")
        .next()
        .expect("native sleep-state extern body");
    assert!(native_sleep.contains("let sleeping = call.arg_bool(2);"));
    assert!(!native_sleep.contains("call.arg_u64(2)"));

    let physics3d_source = WRAPPER_SOURCES[2].1;
    let browser_sleep = wrapper_for(
        physics3d_source,
        "voplay/scene3d",
        "physicsSetBodySleepState",
    );
    assert_eq!(
        decoded_wire_types(browser_sleep, "voplay/scene3d.physicsSetBodySleepState"),
        vec![WireType::I64, WireType::I64, WireType::Bool],
    );

    let heightfield = wrapper_for(
        physics3d_source,
        "voplay/scene3d",
        "physicsSpawnHeightfield",
    );
    assert!(
        heightfield.contains("crate::externs::physics3d::decode_heightfield_data(height_bytes)")
    );
    assert!(!heightfield.contains(".chunks_exact(4)"));
}
