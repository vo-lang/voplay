//! Audio externs (voplay root package).
//!
//! voplay only provides `audioLoadFile` — reads bytes via file_io then
//! delegates to vogui's audio engine. All other audio externs are
//! provided by vogui.

use vo_ext::prelude::*;

use super::util::write_u32_handle_result;
use crate::file_io;
use vogui::audio;

#[vo_fn("voplay", "audioLoadFile")]
pub fn audio_load_file(call: &mut ExternCallContext) -> ExternResult {
    let path = call.arg_str(0).to_string();
    let result = file_io::read_bytes(&path)
        .map_err(|e| format!("audio load error: {e}"))
        .and_then(|data| audio::with_global_audio_result(|engine| engine.load_bytes(data)));
    write_u32_handle_result(call, 0, 1, result);
    ExternResult::Ok
}
