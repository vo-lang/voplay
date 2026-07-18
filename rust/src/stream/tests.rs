use super::{DrawCommand, DrawStreamError, StreamReader};
use crate::draw_protocol::{
    DRAW_STREAM_FLAGS, DRAW_STREAM_HEADER_SIZE, DRAW_STREAM_MAGIC, DRAW_STREAM_VERSION,
};

fn frame_payload(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(DRAW_STREAM_HEADER_SIZE + payload.len());
    framed.extend_from_slice(&DRAW_STREAM_MAGIC);
    framed.extend_from_slice(&DRAW_STREAM_VERSION.to_le_bytes());
    framed.extend_from_slice(&DRAW_STREAM_FLAGS.to_le_bytes());
    framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    framed.extend_from_slice(payload);
    framed
}

fn decode_framed(data: &[u8]) -> Result<Vec<DrawCommand>, DrawStreamError> {
    let mut reader = StreamReader::new(data)?;
    let mut commands = Vec::new();
    while let Some(command) = reader.next_command()? {
        commands.push(command);
    }
    Ok(commands)
}

fn decode_all(payload: &[u8]) -> Vec<DrawCommand> {
    let framed = frame_payload(payload);
    decode_framed(&framed).expect("test draw stream should decode")
}

#[test]
fn rejects_unknown_opcode_with_stream_offset() {
    let error = decode_framed(&frame_payload(&[0xFF])).unwrap_err();
    assert_eq!(
        error,
        DrawStreamError::UnknownOpcode {
            offset: DRAW_STREAM_HEADER_SIZE,
            opcode: 0xFF,
        }
    );
}

#[test]
fn rejects_truncated_command_without_panicking() {
    let error = decode_framed(&frame_payload(&[0x01, 0, 0, 0])).unwrap_err();
    assert!(matches!(error, DrawStreamError::Truncated { .. }));
}

#[test]
fn rejects_payload_length_mismatch_before_decoding() {
    let mut framed = frame_payload(&[0x04]);
    framed[8..12].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        decode_framed(&framed).unwrap_err(),
        DrawStreamError::PayloadLengthMismatch {
            declared: 2,
            actual: 1,
        }
    );
}

fn push_default_material(data: &mut Vec<u8>) {
    data.extend_from_slice(&0u32.to_le_bytes());
    for value in [1.0f32, 1.0, 1.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0u32, 0, 0, 0, 0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.55f32, 0.0, 0.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&0u32.to_le_bytes());
    for value in [1.0f32, 0.0, 1.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0u32, 0, 0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
}

fn push_primitive_instance(data: &mut Vec<u8>, object_id: u32, model_id: u32, visible: bool) {
    data.extend_from_slice(&object_id.to_le_bytes());
    data.extend_from_slice(&model_id.to_le_bytes());
    for value in [0.0f32, 1.0, 2.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 0.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [1.0f32, 1.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    push_default_material(data);
    data.push(if visible { 1 } else { 0 });
    push_primitive_render_params(data);
}

fn push_primitive_instance_ref(
    data: &mut Vec<u8>,
    object_id: u32,
    model_id: u32,
    material_id: u32,
    visible: bool,
) {
    data.extend_from_slice(&object_id.to_le_bytes());
    data.extend_from_slice(&model_id.to_le_bytes());
    for value in [0.0f32, 1.0, 2.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 0.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [1.0f32, 1.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&material_id.to_le_bytes());
    data.push(if visible { 1 } else { 0 });
    push_primitive_render_params(data);
}

fn push_primitive_instance_key(
    data: &mut Vec<u8>,
    object_id: u32,
    shape_id: u32,
    material_id: u32,
    visible: bool,
) {
    data.extend_from_slice(&object_id.to_le_bytes());
    data.extend_from_slice(&shape_id.to_le_bytes());
    for value in [0.0f32, 1.0, 2.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 0.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [1.0f32, 1.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&material_id.to_le_bytes());
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.push(if visible { 1 } else { 0 });
    push_primitive_render_params(data);
}

fn push_primitive_render_params(data: &mut Vec<u8>) {
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&12.0f32.to_le_bytes());
    data.extend_from_slice(&96.0f32.to_le_bytes());
    data.extend_from_slice(&0.35f32.to_le_bytes());
    for value in [0.25f32, 0.5, 0.125, 0.25] {
        data.extend_from_slice(&value.to_le_bytes());
    }
}

#[test]
fn decodes_color_grading_command() {
    let mut data = Vec::new();
    data.push(0x26);
    data.push(2);
    data.extend_from_slice(&1.08f32.to_le_bytes());
    data.extend_from_slice(&1.04f32.to_le_bytes());
    data.extend_from_slice(&1.06f32.to_le_bytes());
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::SetColorGrading3D {
            tone_map,
            exposure,
            contrast,
            saturation,
        } => {
            assert_eq!(*tone_map, 2);
            assert!((*exposure - 1.08).abs() < 0.0001);
            assert!((*contrast - 1.04).abs() < 0.0001);
            assert!((*saturation - 1.06).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_hemisphere_ambient_lights_command() {
    let mut data = Vec::new();
    data.push(0x23);
    data.extend_from_slice(&0.52f32.to_le_bytes());
    data.extend_from_slice(&0.64f32.to_le_bytes());
    data.extend_from_slice(&0.70f32.to_le_bytes());
    data.extend_from_slice(&0.22f32.to_le_bytes());
    data.extend_from_slice(&0.30f32.to_le_bytes());
    data.extend_from_slice(&0.30f32.to_le_bytes());
    data.push(0);
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::SetLights3D {
            ambient_r,
            ambient_g,
            ambient_b,
            ambient_ground_r,
            ambient_ground_g,
            ambient_ground_b,
            lights,
        } => {
            assert!((*ambient_r - 0.52).abs() < 0.0001);
            assert!((*ambient_g - 0.64).abs() < 0.0001);
            assert!((*ambient_b - 0.70).abs() < 0.0001);
            assert!((*ambient_ground_r - 0.22).abs() < 0.0001);
            assert!((*ambient_ground_g - 0.30).abs() < 0.0001);
            assert!((*ambient_ground_b - 0.30).abs() < 0.0001);
            assert!(lights.is_empty());
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_render_debug_command() {
    let commands = decode_all(&[0x28, 3]);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::SetRenderDebug3D { mode } => assert_eq!(*mode, 3),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_post_process_command() {
    let mut data = Vec::new();
    data.push(0x29);
    data.extend_from_slice(&0.72f32.to_le_bytes());
    data.extend_from_slice(&0.18f32.to_le_bytes());
    data.extend_from_slice(&0.06f32.to_le_bytes());
    data.extend_from_slice(&0.95f32.to_le_bytes());
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::SetPostProcess3D {
            bloom_threshold,
            bloom_strength,
            sharpen_strength,
            fxaa_strength,
        } => {
            assert!((*bloom_threshold - 0.72).abs() < 0.0001);
            assert!((*bloom_strength - 0.18).abs() < 0.0001);
            assert!((*sharpen_strength - 0.06).abs() < 0.0001);
            assert!((*fxaa_strength - 0.95).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_contact_ao_command() {
    let mut data = Vec::new();
    data.push(0x2A);
    data.extend_from_slice(&0.38f32.to_le_bytes());
    data.extend_from_slice(&2.75f32.to_le_bytes());
    data.extend_from_slice(&84.0f32.to_le_bytes());
    data.extend_from_slice(&0.22f32.to_le_bytes());
    data.extend_from_slice(&1.15f32.to_le_bytes());
    data.extend_from_slice(&0.018f32.to_le_bytes());
    data.extend_from_slice(&3u32.to_le_bytes());
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::SetContactAO3D {
            strength,
            radius,
            depth_scale,
            detail_strength,
            detail_radius,
            normal_bias,
            quality,
        } => {
            assert!((*strength - 0.38).abs() < 0.0001);
            assert!((*radius - 2.75).abs() < 0.0001);
            assert!((*depth_scale - 84.0).abs() < 0.0001);
            assert!((*detail_strength - 0.22).abs() < 0.0001);
            assert!((*detail_radius - 1.15).abs() < 0.0001);
            assert!((*normal_bias - 0.018).abs() < 0.0001);
            assert_eq!(*quality, 3);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_projected_decal_command() {
    let mut data = Vec::new();
    data.push(0x22);
    for value in [1.0f32, 2.0, 3.0, 0.75, 4.0, 5.0, 1.5, 0.2, 0.3, 0.4, 0.65] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::DrawProjectedDecal3D {
            position,
            yaw,
            width,
            length,
            depth,
            color,
        } => {
            assert!((position.x - 1.0).abs() < 0.0001);
            assert!((position.y - 2.0).abs() < 0.0001);
            assert!((position.z - 3.0).abs() < 0.0001);
            assert!((*yaw - 0.75).abs() < 0.0001);
            assert!((*width - 4.0).abs() < 0.0001);
            assert!((*length - 5.0).abs() < 0.0001);
            assert!((*depth - 1.5).abs() < 0.0001);
            assert!((color[3] - 0.65).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_projected_decal_atlas_commands() {
    let mut data = Vec::new();
    data.push(0x2B);
    data.extend_from_slice(&77u32.to_le_bytes());
    data.push(0x40);
    data.extend_from_slice(&88u32.to_le_bytes());
    data.push(0x41);
    data.extend_from_slice(&99u32.to_le_bytes());
    data.push(0x42);
    data.extend_from_slice(&100u32.to_le_bytes());
    data.push(0x2D);
    data.extend_from_slice(&18.0f32.to_le_bytes());
    data.extend_from_slice(&34.0f32.to_le_bytes());
    data.push(0x43);
    data.extend_from_slice(&0.35f32.to_le_bytes());
    data.extend_from_slice(&0.7f32.to_le_bytes());
    data.push(0x2E);
    data.extend_from_slice(&1u32.to_le_bytes());
    data.push(0x2F);
    data.extend_from_slice(&0.8f32.to_le_bytes());
    data.extend_from_slice(&0.38f32.to_le_bytes());
    data.extend_from_slice(&0.7f32.to_le_bytes());
    data.push(0x2C);
    for value in [
        1.0f32, 2.0, 3.0, 0.75, 4.0, 5.0, 1.5, 0.2, 0.3, 0.4, 0.65, 0.1, 0.2, 0.25, 0.5,
    ] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 9);
    match &commands[0] {
        DrawCommand::SetProjectedDecalAtlas3D { atlas_id } => assert_eq!(*atlas_id, 77),
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[1] {
        DrawCommand::SetProjectedDecalNormalAtlas3D { atlas_id } => {
            assert_eq!(*atlas_id, 88)
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[2] {
        DrawCommand::SetProjectedDecalRoughnessAtlas3D { atlas_id } => {
            assert_eq!(*atlas_id, 99)
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[3] {
        DrawCommand::SetProjectedDecalMaskAtlas3D { atlas_id } => {
            assert_eq!(*atlas_id, 100)
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[4] {
        DrawCommand::SetProjectedDecalDistanceFade3D { start, end } => {
            assert!((*start - 18.0).abs() < 0.0001);
            assert!((*end - 34.0).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[5] {
        DrawCommand::SetProjectedDecalAngleFade3D { start, end } => {
            assert!((*start - 0.35).abs() < 0.0001);
            assert!((*end - 0.7).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[6] {
        DrawCommand::SetProjectedDecalReceiverMask3D { mask } => assert_eq!(*mask, 1),
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[7] {
        DrawCommand::SetProjectedDecalSurfaceResponse3D {
            normal_strength,
            roughness,
            roughness_strength,
        } => {
            assert!((*normal_strength - 0.8).abs() < 0.0001);
            assert!((*roughness - 0.38).abs() < 0.0001);
            assert!((*roughness_strength - 0.7).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[8] {
        DrawCommand::DrawProjectedDecal3DUV {
            position,
            uv_rect,
            color,
            ..
        } => {
            assert!((position.x - 1.0).abs() < 0.0001);
            assert!((color[3] - 0.65).abs() < 0.0001);
            assert!((uv_rect[0] - 0.1).abs() < 0.0001);
            assert!((uv_rect[3] - 0.5).abs() < 0.0001);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_shadow_strength_command() {
    let mut data = Vec::new();
    data.push(0x27);
    data.push(1);
    data.extend_from_slice(&2048u32.to_le_bytes());
    data.extend_from_slice(&0.58f32.to_le_bytes());
    data.extend_from_slice(&1.75f32.to_le_bytes());
    data.extend_from_slice(&260.0f32.to_le_bytes());
    data.extend_from_slice(&70.0f32.to_le_bytes());
    data.extend_from_slice(&3u32.to_le_bytes());
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::SetShadow3D {
            enabled,
            resolution,
            strength,
            softness,
            distance,
            fade,
            quality,
        } => {
            assert!(*enabled);
            assert_eq!(*resolution, 2048);
            assert!((*strength - 0.58).abs() < 0.0001);
            assert!((*softness - 1.75).abs() < 0.0001);
            assert!((*distance - 260.0).abs() < 0.0001);
            assert!((*fade - 70.0).abs() < 0.0001);
            assert_eq!(*quality, 3);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_primitive_chunk_replace_command() {
    let mut data = Vec::new();
    data.push(0x38);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&9u32.to_le_bytes());
    data.extend_from_slice(&2u32.to_le_bytes());
    push_primitive_instance(&mut data, 11, 21, true);
    push_primitive_instance(&mut data, 12, 22, false);
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::Primitive3DReplaceChunk {
            scene_id,
            layer_id,
            chunk_id,
            instances,
        } => {
            assert_eq!(*scene_id, 7);
            assert_eq!(*layer_id, 8);
            assert_eq!(*chunk_id, 9);
            assert_eq!(instances.len(), 2);
            assert_eq!(instances[0].object_id, 11);
            assert_eq!(instances[0].model_id, 21);
            assert!(instances[0].visible);
            assert_eq!(instances[0].flags, 2);
            assert!((instances[0].lod_near - 12.0).abs() < 0.0001);
            assert!((instances[0].lod_far - 96.0).abs() < 0.0001);
            assert!((instances[0].wind_strength - 0.35).abs() < 0.0001);
            assert_eq!(instances[0].atlas_uv, [0.25, 0.5, 0.125, 0.25]);
            assert!(!instances[1].visible);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_primitive_material_table_and_chunk_refs() {
    let mut data = Vec::new();
    data.push(0x3A);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&33u32.to_le_bytes());
    push_default_material(&mut data);
    data.push(0x39);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&9u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    push_primitive_instance_ref(&mut data, 11, 21, 33, true);
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 2);
    match &commands[0] {
        DrawCommand::Primitive3DUpsertMaterials {
            scene_id,
            layer_id,
            materials,
        } => {
            assert_eq!(*scene_id, 7);
            assert_eq!(*layer_id, 8);
            assert_eq!(materials.len(), 1);
            assert_eq!(materials[0].material_id, 33);
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[1] {
        DrawCommand::Primitive3DReplaceChunkRefs {
            scene_id,
            layer_id,
            chunk_id,
            instances,
        } => {
            assert_eq!(*scene_id, 7);
            assert_eq!(*layer_id, 8);
            assert_eq!(*chunk_id, 9);
            assert_eq!(instances.len(), 1);
            assert_eq!(instances[0].material_id, 33);
            assert!(instances[0].visible);
            assert_eq!(instances[0].flags, 2);
            assert!((instances[0].wind_strength - 0.35).abs() < 0.0001);
            assert_eq!(instances[0].atlas_uv, [0.25, 0.5, 0.125, 0.25]);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_primitive_shape_material_tables_and_chunk_keys() {
    let mut data = Vec::new();
    data.push(0x3B);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&44u32.to_le_bytes());
    data.extend_from_slice(&21u32.to_le_bytes());
    data.push(0x3A);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&33u32.to_le_bytes());
    push_default_material(&mut data);
    data.push(0x3C);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&9u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    push_primitive_instance_key(&mut data, 11, 44, 33, true);
    data.push(0x3D);
    data.extend_from_slice(&7u32.to_le_bytes());
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&9u32.to_le_bytes());
    data.push(0);
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 4);
    match &commands[0] {
        DrawCommand::Primitive3DUpsertShapes {
            scene_id,
            layer_id,
            shapes,
        } => {
            assert_eq!(*scene_id, 7);
            assert_eq!(*layer_id, 8);
            assert_eq!(shapes.len(), 1);
            assert_eq!(shapes[0].shape_id, 44);
            assert_eq!(shapes[0].model_id, 21);
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[2] {
        DrawCommand::Primitive3DReplaceChunkKeys {
            scene_id,
            layer_id,
            chunk_id,
            instances,
        } => {
            assert_eq!(*scene_id, 7);
            assert_eq!(*layer_id, 8);
            assert_eq!(*chunk_id, 9);
            assert_eq!(instances.len(), 1);
            assert_eq!(instances[0].shape_id, 44);
            assert_eq!(instances[0].material_id, 33);
            assert_eq!(instances[0].tint, [0.0, 0.0, 0.0, 0.0]);
            assert!(instances[0].visible);
            assert_eq!(instances[0].flags, 2);
            assert!((instances[0].lod_far - 96.0).abs() < 0.0001);
            assert_eq!(instances[0].atlas_uv, [0.25, 0.5, 0.125, 0.25]);
        }
        other => panic!("unexpected command: {:?}", other),
    }
    match &commands[3] {
        DrawCommand::Primitive3DSetChunkVisible {
            scene_id,
            layer_id,
            chunk_id,
            visible,
        } => {
            assert_eq!(*scene_id, 7);
            assert_eq!(*layer_id, 8);
            assert_eq!(*chunk_id, 9);
            assert!(!*visible);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn decodes_extended_material_command() {
    let mut data = Vec::new();
    data.push(0x20);
    data.extend_from_slice(&7u32.to_le_bytes());
    for value in [1.0f32, 2.0, 3.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0.0f32, 0.0, 0.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in [1.0f32, 1.0, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&9u32.to_le_bytes());
    for value in [0.2f32, 0.3, 0.4, 1.0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&11u32.to_le_bytes());
    data.extend_from_slice(&12u32.to_le_bytes());
    data.extend_from_slice(&13u32.to_le_bytes());
    data.extend_from_slice(&14u32.to_le_bytes());
    data.extend_from_slice(&16u32.to_le_bytes());
    for value in [0.1f32, 0.2, 0.3, 1.1] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&0.72f32.to_le_bytes());
    data.extend_from_slice(&0.28f32.to_le_bytes());
    data.extend_from_slice(&0.66f32.to_le_bytes());
    data.extend_from_slice(&3.5f32.to_le_bytes());
    data.extend_from_slice(&15u32.to_le_bytes());
    data.extend_from_slice(&1.25f32.to_le_bytes());
    data.extend_from_slice(&0.45f32.to_le_bytes());
    data.extend_from_slice(&0.8f32.to_le_bytes());
    data.extend_from_slice(&0.6f32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&21u32.to_le_bytes());
    data.extend_from_slice(&22u32.to_le_bytes());
    let commands = decode_all(&data);
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        DrawCommand::DrawModel {
            model_id,
            material,
            animation_world_id,
            animation_target_id,
            ..
        } => {
            assert_eq!(*model_id, 7);
            assert_eq!(material.id, 9);
            assert_eq!(material.albedo_texture_id, 11);
            assert_eq!(material.normal_texture_id, 12);
            assert_eq!(material.metallic_roughness_texture_id, 13);
            assert_eq!(material.emissive_texture_id, 14);
            assert_eq!(material.mask_texture_id, 16);
            assert!((material.roughness - 0.72).abs() < 0.0001);
            assert!((material.metallic - 0.28).abs() < 0.0001);
            assert!((material.normal_scale - 0.66).abs() < 0.0001);
            assert!((material.uv_scale - 3.5).abs() < 0.0001);
            assert_eq!(material.toon_ramp_texture_id, 15);
            assert!((material.detail_strength - 1.25).abs() < 0.0001);
            assert!((material.macro_blend - 0.45).abs() < 0.0001);
            assert!((material.roughness_response - 0.8).abs() < 0.0001);
            assert!((material.toon_ramp_response - 0.6).abs() < 0.0001);
            assert_eq!(material.shading_mode, 1);
            assert_eq!(material.wrap_mode, 2);
            assert_eq!(material.filter_mode, 1);
            assert_eq!(*animation_world_id, 21);
            assert_eq!(*animation_target_id, 22);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}
