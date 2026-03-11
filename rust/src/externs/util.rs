use vo_ext::prelude::*;
use vo_runtime::builtins::error_helper::{write_error_to, write_nil_error};

use crate::renderer::Renderer;

use super::with_renderer;

pub(super) fn read_u16_le(data: &[u8], pos: &mut usize) -> u16 {
    let value = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
    *pos += 2;
    value
}

pub(super) fn read_u32_le(data: &[u8], pos: &mut usize) -> u32 {
    let value = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
    *pos += 4;
    value
}

pub(super) fn read_f64_le(data: &[u8], pos: &mut usize) -> f64 {
    let value = f64::from_le_bytes([
        data[*pos],
        data[*pos + 1],
        data[*pos + 2],
        data[*pos + 3],
        data[*pos + 4],
        data[*pos + 5],
        data[*pos + 6],
        data[*pos + 7],
    ]);
    *pos += 8;
    value
}

pub(super) fn ret_bytes(call: &mut ExternCallContext, slot: u16, data: &[u8]) {
    let slice_ref = call.alloc_bytes(data);
    call.ret_ref(slot, slice_ref);
}

pub(super) fn write_u32_handle_result(
    call: &mut ExternCallContext,
    value_slot: u16,
    error_slot: u16,
    result: Result<u32, String>,
) {
    match result {
        Ok(id) => {
            call.ret_u64(value_slot, id as u64);
            write_nil_error(call, error_slot);
        }
        Err(msg) => {
            call.ret_u64(value_slot, 0);
            write_error_to(call, error_slot, &msg);
        }
    }
}

pub(super) fn write_bytes_result(
    call: &mut ExternCallContext,
    value_slot: u16,
    error_slot: u16,
    result: Result<Vec<u8>, String>,
) {
    match result {
        Ok(data) => {
            ret_bytes(call, value_slot, &data);
            write_nil_error(call, error_slot);
        }
        Err(msg) => {
            ret_bytes(call, value_slot, &[]);
            write_error_to(call, error_slot, &msg);
        }
    }
}

pub(super) fn write_unit_result(
    call: &mut ExternCallContext,
    error_slot: u16,
    result: Result<(), String>,
) {
    match result {
        Ok(()) => write_nil_error(call, error_slot),
        Err(msg) => write_error_to(call, error_slot, &msg),
    }
}

pub(super) fn unwrap_or_panic<R>(result: Result<R, String>, context: &str) -> R {
    match result {
        Ok(value) => value,
        Err(msg) => panic!("{context}: {msg}"),
    }
}

pub(super) fn with_renderer_result<R>(
    f: impl FnOnce(&mut Renderer) -> Result<R, String>,
) -> Result<R, String> {
    match with_renderer(f) {
        Ok(result) => result,
        Err(msg) => Err(msg),
    }
}

pub(super) fn with_renderer_or_panic<R>(context: &str, f: impl FnOnce(&mut Renderer) -> R) -> R {
    unwrap_or_panic(with_renderer(f), context)
}
