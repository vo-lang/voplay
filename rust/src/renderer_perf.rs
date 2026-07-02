#[cfg(not(feature = "wasm"))]
use std::time::Instant;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use wasm_bindgen::prelude::*;

pub(crate) const PERF_PACKET_MAGIC: u8 = 0xf9;
pub(crate) const PERF_PACKET_VERSION: u8 = 1;
pub(crate) const PERF_PACKET_SCHEMA_VERSION: u32 = 1;
pub(crate) const PERF_PACKET_SOURCE_RENDERER: u32 = 2;
pub(crate) const RENDERER_PERF_PAYLOAD_VERSION: u32 = 4;

pub(crate) const RENDERER_DIAG_DISABLE_SHADOWS: u32 = 1 << 0;
pub(crate) const RENDERER_DIAG_DISABLE_POST_EFFECTS: u32 = 1 << 1;
pub(crate) const RENDERER_DIAG_DISABLE_BLOOM: u32 = 1 << 2;
pub(crate) const RENDERER_DIAG_DISABLE_SHARPEN: u32 = 1 << 3;
pub(crate) const RENDERER_DIAG_DISABLE_FXAA: u32 = 1 << 4;
pub(crate) const RENDERER_DIAG_DISABLE_CONTACT_AO: u32 = 1 << 5;
pub(crate) const RENDERER_DIAG_DISABLE_PRIMITIVES: u32 = 1 << 6;
pub(crate) const RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS: u32 = 1 << 7;
pub(crate) const RENDERER_DIAG_DISABLE_DECALS: u32 = 1 << 8;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = globalThis, js_name = "__voplayRendererPerfConfig")]
    fn js_renderer_perf_config() -> Result<String, JsValue>;
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct RendererPerfOverrides {
    flags: u32,
}

impl RendererPerfOverrides {
    pub(crate) fn current() -> Self {
        Self::from_config(&renderer_perf_config_string())
    }

    pub(crate) fn from_config(config: &str) -> Self {
        let mut flags = 0u32;
        for raw_token in
            config.split(|c: char| c == ',' || c == ';' || c == '&' || c.is_whitespace())
        {
            let token = normalize_renderer_perf_token(raw_token);
            match token.as_str() {
                "disableshadows" | "shadowoff" | "shadowsoff" | "noshadows" => {
                    flags |= RENDERER_DIAG_DISABLE_SHADOWS;
                }
                "disablepost" | "disableposteffects" | "postoff" | "posteffectsoff" => {
                    flags |= RENDERER_DIAG_DISABLE_POST_EFFECTS;
                }
                "disablebloom" | "bloomoff" | "nobloom" => {
                    flags |= RENDERER_DIAG_DISABLE_BLOOM;
                }
                "disablesharpen" | "sharpenoff" | "nosharpen" => {
                    flags |= RENDERER_DIAG_DISABLE_SHARPEN;
                }
                "disablefxaa" | "fxaaoff" | "nofxaa" => {
                    flags |= RENDERER_DIAG_DISABLE_FXAA;
                }
                "disablecontactao" | "contactaooff" | "noao" | "noambientocclusion" => {
                    flags |= RENDERER_DIAG_DISABLE_CONTACT_AO;
                }
                "disableprimitives" | "primitivesoff" | "noprimitives" => {
                    flags |= RENDERER_DIAG_DISABLE_PRIMITIVES;
                }
                "disableprimitiveshadows" | "primitiveshadowsoff" | "noprimitiveshadows" => {
                    flags |= RENDERER_DIAG_DISABLE_PRIMITIVE_SHADOWS;
                }
                "disabledecals" | "decalsoff" | "nodecals" => {
                    flags |= RENDERER_DIAG_DISABLE_DECALS;
                }
                _ => {}
            }
        }
        Self { flags }
    }

    pub(crate) fn flags(self) -> u32 {
        self.flags
    }

    pub(crate) fn has(self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

fn normalize_renderer_perf_token(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    for ch in token.chars() {
        if ch != '_' && ch != '-' && ch != '=' {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
fn renderer_perf_config_string() -> String {
    js_renderer_perf_config().unwrap_or_default()
}

#[cfg(not(all(feature = "wasm", target_arch = "wasm32")))]
fn renderer_perf_config_string() -> String {
    String::new()
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct RendererPerfStats {
    pub(crate) frame_id: u32,
    pub(crate) display_tick: u32,
    pub(crate) submit_frame_ms: f64,
    pub(crate) surface_acquire_ms: f64,
    pub(crate) decode_ms: f64,
    pub(crate) scene_update_ms: f64,
    pub(crate) depth_pass_ms: f64,
    pub(crate) shadow_pass_ms: f64,
    pub(crate) main_pass_ms: f64,
    pub(crate) main_pass_setup_ms: f64,
    pub(crate) main_skybox_ms: f64,
    pub(crate) main_model_ms: f64,
    pub(crate) main_primitive_ms: f64,
    pub(crate) main_pass_close_ms: f64,
    pub(crate) post_pass_ms: f64,
    pub(crate) overlay_pass_ms: f64,
    pub(crate) queue_submit_cpu_ms: f64,
    pub(crate) present_cpu_ms: f64,
    pub(crate) draw_calls: u32,
    pub(crate) model_draws: u32,
    pub(crate) skinned_draws: u32,
    pub(crate) primitive_draws: u32,
    pub(crate) sprite_draws: u32,
    pub(crate) text_draws: u32,
    pub(crate) instances: u32,
    pub(crate) triangles: u32,
    pub(crate) upload_bytes: u32,
    pub(crate) bind_group_creates: u32,
    pub(crate) buffer_creates: u32,
    pub(crate) texture_uploads: u32,
    pub(crate) resident_chunk_rebuilds: u32,
    pub(crate) shadow_cascades: u32,
    pub(crate) primitive_chunks: u32,
    pub(crate) post_effects: u32,
    pub(crate) retained_scene_upserts: u32,
    pub(crate) retained_scene_removals: u32,
    pub(crate) visible_objects: u32,
    pub(crate) culled_objects: u32,
    pub(crate) diagnostic_flags: u32,
    pub(crate) graph_pass_count: u32,
    pub(crate) graph_resource_count: u32,
    pub(crate) graph_target_count: u32,
    pub(crate) graph_ready_target_count: u32,
}

#[cfg(not(feature = "wasm"))]
pub(crate) type PerfInstant = Instant;
#[cfg(feature = "wasm")]
pub(crate) type PerfInstant = f64;

#[cfg(not(feature = "wasm"))]
pub(crate) fn perf_now() -> PerfInstant {
    Instant::now()
}

#[cfg(feature = "wasm")]
pub(crate) fn perf_now() -> PerfInstant {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now())
        .unwrap_or_else(js_sys::Date::now)
}

#[cfg(not(feature = "wasm"))]
pub(crate) fn elapsed_ms(start: PerfInstant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[cfg(feature = "wasm")]
pub(crate) fn elapsed_ms(start: PerfInstant) -> f64 {
    (perf_now() - start).max(0.0)
}

pub(crate) fn elapsed_ms_opt(start: Option<PerfInstant>) -> f64 {
    start.map(elapsed_ms).unwrap_or(0.0)
}

pub(crate) fn saturating_u32(value: usize) -> u32 {
    value.min(u32::MAX as usize) as u32
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn encode_renderer_perf_payload(stats: &RendererPerfStats) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 16 * 8 + 25 * 4);
    push_u32(&mut out, RENDERER_PERF_PAYLOAD_VERSION);
    push_f64(&mut out, stats.submit_frame_ms);
    push_f64(&mut out, stats.surface_acquire_ms);
    push_f64(&mut out, stats.decode_ms);
    push_f64(&mut out, stats.scene_update_ms);
    push_f64(&mut out, stats.depth_pass_ms);
    push_f64(&mut out, stats.shadow_pass_ms);
    push_f64(&mut out, stats.main_pass_ms);
    push_f64(&mut out, stats.main_pass_setup_ms);
    push_f64(&mut out, stats.main_skybox_ms);
    push_f64(&mut out, stats.main_model_ms);
    push_f64(&mut out, stats.main_primitive_ms);
    push_f64(&mut out, stats.main_pass_close_ms);
    push_f64(&mut out, stats.post_pass_ms);
    push_f64(&mut out, stats.overlay_pass_ms);
    push_f64(&mut out, stats.queue_submit_cpu_ms);
    push_f64(&mut out, stats.present_cpu_ms);
    push_u32(&mut out, stats.draw_calls);
    push_u32(&mut out, stats.model_draws);
    push_u32(&mut out, stats.skinned_draws);
    push_u32(&mut out, stats.primitive_draws);
    push_u32(&mut out, stats.sprite_draws);
    push_u32(&mut out, stats.text_draws);
    push_u32(&mut out, stats.instances);
    push_u32(&mut out, stats.triangles);
    push_u32(&mut out, stats.upload_bytes);
    push_u32(&mut out, stats.bind_group_creates);
    push_u32(&mut out, stats.buffer_creates);
    push_u32(&mut out, stats.texture_uploads);
    push_u32(&mut out, stats.resident_chunk_rebuilds);
    push_u32(&mut out, stats.shadow_cascades);
    push_u32(&mut out, stats.primitive_chunks);
    push_u32(&mut out, stats.post_effects);
    push_u32(&mut out, stats.retained_scene_upserts);
    push_u32(&mut out, stats.retained_scene_removals);
    push_u32(&mut out, stats.visible_objects);
    push_u32(&mut out, stats.culled_objects);
    push_u32(&mut out, stats.diagnostic_flags);
    push_u32(&mut out, stats.graph_pass_count);
    push_u32(&mut out, stats.graph_resource_count);
    push_u32(&mut out, stats.graph_target_count);
    push_u32(&mut out, stats.graph_ready_target_count);
    out
}

pub(crate) fn encode_renderer_perf_packet(stats: &RendererPerfStats) -> Vec<u8> {
    let payload = encode_renderer_perf_payload(stats);
    let mut out = Vec::with_capacity(50 + payload.len());
    out.push(PERF_PACKET_MAGIC);
    out.push(PERF_PACKET_VERSION);
    push_u32(&mut out, PERF_PACKET_SCHEMA_VERSION);
    push_u32(&mut out, stats.frame_id);
    push_u32(&mut out, stats.display_tick);
    push_u32(&mut out, PERF_PACKET_SOURCE_RENDERER);
    push_u32(&mut out, payload.len() as u32);
    push_f64(&mut out, stats.submit_frame_ms);
    push_f64(&mut out, 0.0);
    push_u32(&mut out, 0);
    push_u32(&mut out, stats.upload_bytes);
    push_u32(&mut out, 1);
    out.extend_from_slice(&payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_perf_override_parser_accepts_budget_tokens() {
        let overrides = RendererPerfOverrides::from_config(
            "disable-shadows, disable_post_effects no-fxaa nodecals",
        );
        assert!(overrides.has(RENDERER_DIAG_DISABLE_SHADOWS));
        assert!(overrides.has(RENDERER_DIAG_DISABLE_POST_EFFECTS));
        assert!(overrides.has(RENDERER_DIAG_DISABLE_FXAA));
        assert!(overrides.has(RENDERER_DIAG_DISABLE_DECALS));
        assert!(!overrides.has(RENDERER_DIAG_DISABLE_PRIMITIVES));
    }

    #[test]
    fn renderer_perf_packet_keeps_schema_header() {
        let stats = RendererPerfStats {
            frame_id: 7,
            display_tick: 9,
            submit_frame_ms: 1.5,
            upload_bytes: 44,
            ..RendererPerfStats::default()
        };
        let packet = encode_renderer_perf_packet(&stats);
        assert_eq!(packet[0], PERF_PACKET_MAGIC);
        assert_eq!(packet[1], PERF_PACKET_VERSION);
        assert_eq!(
            u32::from_le_bytes(packet[2..6].try_into().unwrap()),
            PERF_PACKET_SCHEMA_VERSION
        );
        assert_eq!(u32::from_le_bytes(packet[6..10].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(packet[10..14].try_into().unwrap()), 9);
        assert_eq!(
            u32::from_le_bytes(packet[14..18].try_into().unwrap()),
            PERF_PACKET_SOURCE_RENDERER
        );
    }
}
