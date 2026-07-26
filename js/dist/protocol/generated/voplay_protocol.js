// @generated from framework schema. DO NOT EDIT.
export const SCHEMA_ID = "voplay.engine";
export const PROTOCOL_MAJOR = 1;
export const PROTOCOL_MINOR = 1;
export const SCHEMA_IDENTITY = [206, 245, 237, 231, 88, 77, 170, 113, 29, 84, 60, 189, 166, 204, 202, 241];
export const MAJOR_COMPAT_FINGERPRINT = [144, 249, 88, 53, 204, 13, 79, 236, 201, 174, 144, 69, 245, 109, 57, 183, 29, 57, 151, 61, 115, 177, 74, 89, 244, 251, 123, 68, 212, 103, 28, 188];
export const EXACT_SCHEMA_FINGERPRINT = [254, 141, 190, 190, 145, 134, 236, 92, 49, 20, 233, 54, 105, 125, 113, 77, 59, 191, 22, 106, 121, 226, 94, 198, 251, 202, 144, 105, 197, 44, 191, 161];
export const MAX_EVENT_PAYLOAD_BYTES = 262144;
export const MAX_PACKET_BYTES = 67108864;
export const MAX_SNAPSHOT_OBJECTS = 1000000;
export const MAX_STAGED_EVENT_BYTES = 4194304;
export const MAX_STAGED_EVENTS = 4096;
export const MAX_TRANSACTION_OPS = 131072;
export const HEADER_BYTES = 80;
export function messageKindFromWire(value) {
    switch (value) {
        case 1: return 1 /* MessageKind.RenderStateTransaction */;
        case 2: return 2 /* MessageKind.RenderStateAck */;
        case 3: return 3 /* MessageKind.RenderStateSnapshot */;
        case 4: return 4 /* MessageKind.RenderEvent */;
        case 5: return 5 /* MessageKind.RenderEventResult */;
        case 6: return 6 /* MessageKind.RenderControlTransaction */;
        case 7: return 7 /* MessageKind.RenderControlAck */;
        case 8: return 8 /* MessageKind.AudioControlTransaction */;
        case 9: return 9 /* MessageKind.AudioEvent */;
        case 10: return 10 /* MessageKind.AudioEventResult */;
        case 11: return 11 /* MessageKind.AudioControlAck */;
        case 12: return 12 /* MessageKind.EngineStart */;
        case 13: return 13 /* MessageKind.EngineReady */;
        case 14: return 14 /* MessageKind.EngineSuspend */;
        case 15: return 15 /* MessageKind.EngineResume */;
        case 16: return 16 /* MessageKind.EngineClose */;
        case 17: return 17 /* MessageKind.EngineClosed */;
        case 18: return 18 /* MessageKind.TickInput */;
        case 19: return 19 /* MessageKind.TickResult */;
        case 20: return 20 /* MessageKind.FramePulse */;
        case 21: return 21 /* MessageKind.AssetRequest */;
        case 22: return 22 /* MessageKind.AssetCompletion */;
        case 23: return 23 /* MessageKind.AssetControl */;
        case 24: return 24 /* MessageKind.PhysicsBatch */;
        case 25: return 25 /* MessageKind.PhysicsResult */;
        case 26: return 26 /* MessageKind.Diagnostics */;
        case 27: return 27 /* MessageKind.InspectionRequest */;
        case 28: return 28 /* MessageKind.InspectionResponse */;
        case 29: return 29 /* MessageKind.RenderTransient */;
        case 30: return 30 /* MessageKind.SurfaceControl */;
        case 31: return 31 /* MessageKind.DeviceEvent */;
        case 32: return 32 /* MessageKind.WorkerWake */;
        case 33: return 33 /* MessageKind.PlatformInput */;
        case 34: return 34 /* MessageKind.HapticsCommand */;
        case 35: return 35 /* MessageKind.HapticsResult */;
        case 36: return 36 /* MessageKind.AudioAssetData */;
        case 37: return 37 /* MessageKind.RenderAssetData */;
        case 38: return 38 /* MessageKind.RenderAssetAck */;
        case 39: return 39 /* MessageKind.AudioAssetAck */;
        case 40: return 40 /* MessageKind.RenderReadbackRequest */;
        case 41: return 41 /* MessageKind.RenderReadbackResult */;
        case 42: return 42 /* MessageKind.ControlLeaseRequest */;
        case 43: return 43 /* MessageKind.ControlLeaseGranted */;
        case 44: return 44 /* MessageKind.ControlCommit */;
        case 45: return 45 /* MessageKind.ControlCommitted */;
        case 46: return 46 /* MessageKind.ControlRealizationResult */;
        case 47: return 47 /* MessageKind.ControlObserved */;
        case 48: return 48 /* MessageKind.ControlObservedAck */;
        case 49: return 49 /* MessageKind.ControlStateAdopt */;
        case 50: return 50 /* MessageKind.ControlStateAdopted */;
        default: return null;
    }
}
function requireFrameworkMessageKind(value) {
    const kind = messageKindFromWire(value);
    if (kind === null)
        throw new RangeError("unknown framework message kind");
    return kind;
}
export function decodeFrameworkPacket(input) {
    if (!(input instanceof Uint8Array))
        throw new TypeError("framework packet must be Uint8Array");
    if (input.byteLength < HEADER_BYTES)
        throw new RangeError("truncated framework packet header");
    if (input.byteLength > MAX_PACKET_BYTES)
        throw new RangeError("framework packet exceeds packet limit");
    const view = new DataView(input.buffer, input.byteOffset, HEADER_BYTES);
    const header = {
        kind: requireFrameworkMessageKind(view.getUint16(0, true)),
        engine: readFrameworkHandle(view, 4),
        channelEpoch: view.getBigUint64(12, true),
        commitId: view.getBigUint64(20, true),
        baseRevision: view.getBigUint64(28, true),
        newRevision: view.getBigUint64(36, true),
        requiredControlRevision: view.getBigUint64(44, true),
        sourceSimulationRevision: view.getBigUint64(52, true),
        sequence: view.getBigUint64(60, true),
        payloadLen: view.getUint32(76, true),
    };
    if (header.payloadLen > MAX_PACKET_BYTES - HEADER_BYTES
        || input.byteLength !== HEADER_BYTES + header.payloadLen) {
        throw new RangeError("framework packet payload length mismatch");
    }
    return { header, payload: input.subarray(HEADER_BYTES) };
}
export function encodeFrameworkPacket(header, payload) {
    if (!(payload instanceof Uint8Array))
        throw new TypeError("framework packet payload must be Uint8Array");
    if (payload.byteLength > MAX_PACKET_BYTES - HEADER_BYTES)
        throw new RangeError("framework packet payload exceeds limit");
    const output = new Uint8Array(HEADER_BYTES + payload.byteLength);
    const view = new DataView(output.buffer);
    if (messageKindFromWire(header.kind) === null)
        throw new RangeError("unknown framework message kind");
    view.setUint16(0, header.kind, true);
    writeFrameworkHandle(view, 4, header.engine);
    validateFrameworkU64(header.channelEpoch, "channel_epoch");
    view.setBigUint64(12, header.channelEpoch, true);
    validateFrameworkU64(header.commitId, "commit_id");
    view.setBigUint64(20, header.commitId, true);
    validateFrameworkU64(header.baseRevision, "base_revision");
    view.setBigUint64(28, header.baseRevision, true);
    validateFrameworkU64(header.newRevision, "new_revision");
    view.setBigUint64(36, header.newRevision, true);
    validateFrameworkU64(header.requiredControlRevision, "required_control_revision");
    view.setBigUint64(44, header.requiredControlRevision, true);
    validateFrameworkU64(header.sourceSimulationRevision, "source_simulation_revision");
    view.setBigUint64(52, header.sourceSimulationRevision, true);
    validateFrameworkU64(header.sequence, "sequence");
    view.setBigUint64(60, header.sequence, true);
    view.setUint32(76, payload.byteLength, true);
    output.set(payload, HEADER_BYTES);
    return output;
}
function readFrameworkHandle(view, offset) {
    const handle = { index: view.getUint32(offset, true), generation: view.getUint32(offset + 4, true) };
    validateFrameworkHandle(handle);
    return handle;
}
function writeFrameworkHandle(view, offset, handle) {
    validateFrameworkHandle(handle);
    view.setUint32(offset, handle.index, true);
    view.setUint32(offset + 4, handle.generation, true);
}
function validateFrameworkHandle(handle) {
    if (!Number.isInteger(handle.index) || handle.index < 0 || handle.index >= 0xffffffff
        || !Number.isInteger(handle.generation) || handle.generation < 1 || handle.generation > 0xffffffff) {
        throw new RangeError("invalid framework packet handle");
    }
}
function validateFrameworkU16(value, label) {
    if (!Number.isInteger(value) || value < 0 || value > 0xffff)
        throw new RangeError(`invalid ${label}`);
}
function validateFrameworkU32(value, label) {
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff)
        throw new RangeError(`invalid ${label}`);
}
function validateFrameworkU64(value, label) {
    if (typeof value !== "bigint" || value < 0n || value > 0xffffffffffffffffn)
        throw new RangeError(`invalid ${label}`);
}
