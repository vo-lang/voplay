// @generated from framework schema. DO NOT EDIT.
export const SCHEMA_ID = "voplay.engine" as const;
export const PROTOCOL_MAJOR = 1;
export const PROTOCOL_MINOR = 1;
export const SCHEMA_IDENTITY = [206, 245, 237, 231, 88, 77, 170, 113, 29, 84, 60, 189, 166, 204, 202, 241] as const;
export const MAJOR_COMPAT_FINGERPRINT = [144, 249, 88, 53, 204, 13, 79, 236, 201, 174, 144, 69, 245, 109, 57, 183, 29, 57, 151, 61, 115, 177, 74, 89, 244, 251, 123, 68, 212, 103, 28, 188] as const;
export const EXACT_SCHEMA_FINGERPRINT = [254, 141, 190, 190, 145, 134, 236, 92, 49, 20, 233, 54, 105, 125, 113, 77, 59, 191, 22, 106, 121, 226, 94, 198, 251, 202, 144, 105, 197, 44, 191, 161] as const;
export const MAX_EVENT_PAYLOAD_BYTES = 262144;
export const MAX_PACKET_BYTES = 67108864;
export const MAX_SNAPSHOT_OBJECTS = 1000000;
export const MAX_STAGED_EVENT_BYTES = 4194304;
export const MAX_STAGED_EVENTS = 4096;
export const MAX_TRANSACTION_OPS = 131072;
export const enum MessageKind {
  RenderStateTransaction = 1,
  RenderStateAck = 2,
  RenderStateSnapshot = 3,
  RenderEvent = 4,
  RenderEventResult = 5,
  RenderControlTransaction = 6,
  RenderControlAck = 7,
  AudioControlTransaction = 8,
  AudioEvent = 9,
  AudioEventResult = 10,
  AudioControlAck = 11,
  EngineStart = 12,
  EngineReady = 13,
  EngineSuspend = 14,
  EngineResume = 15,
  EngineClose = 16,
  EngineClosed = 17,
  TickInput = 18,
  TickResult = 19,
  FramePulse = 20,
  AssetRequest = 21,
  AssetCompletion = 22,
  AssetControl = 23,
  PhysicsBatch = 24,
  PhysicsResult = 25,
  Diagnostics = 26,
  InspectionRequest = 27,
  InspectionResponse = 28,
  RenderTransient = 29,
  SurfaceControl = 30,
  DeviceEvent = 31,
  WorkerWake = 32,
  PlatformInput = 33,
  HapticsCommand = 34,
  HapticsResult = 35,
  AudioAssetData = 36,
  RenderAssetData = 37,
  RenderAssetAck = 38,
  AudioAssetAck = 39,
  RenderReadbackRequest = 40,
  RenderReadbackResult = 41,
  ControlLeaseRequest = 42,
  ControlLeaseGranted = 43,
  ControlCommit = 44,
  ControlCommitted = 45,
  ControlRealizationResult = 46,
  ControlObserved = 47,
  ControlObservedAck = 48,
  ControlStateAdopt = 49,
  ControlStateAdopted = 50,
}
export const HEADER_BYTES = 80;
export interface GenerationalHandle { readonly index: number; readonly generation: number; }
export interface FrameworkPacketHeader {
  readonly kind: MessageKind;
  readonly engine: GenerationalHandle;
  readonly channelEpoch: bigint;
  readonly commitId: bigint;
  readonly baseRevision: bigint;
  readonly newRevision: bigint;
  readonly requiredControlRevision: bigint;
  readonly sourceSimulationRevision: bigint;
  readonly sequence: bigint;
  readonly payloadLen: number;
}
export interface FrameworkPacket { readonly header: FrameworkPacketHeader; readonly payload: Uint8Array; }
export function messageKindFromWire(value: number): MessageKind | null {
switch (value) {
    case 1: return MessageKind.RenderStateTransaction;
    case 2: return MessageKind.RenderStateAck;
    case 3: return MessageKind.RenderStateSnapshot;
    case 4: return MessageKind.RenderEvent;
    case 5: return MessageKind.RenderEventResult;
    case 6: return MessageKind.RenderControlTransaction;
    case 7: return MessageKind.RenderControlAck;
    case 8: return MessageKind.AudioControlTransaction;
    case 9: return MessageKind.AudioEvent;
    case 10: return MessageKind.AudioEventResult;
    case 11: return MessageKind.AudioControlAck;
    case 12: return MessageKind.EngineStart;
    case 13: return MessageKind.EngineReady;
    case 14: return MessageKind.EngineSuspend;
    case 15: return MessageKind.EngineResume;
    case 16: return MessageKind.EngineClose;
    case 17: return MessageKind.EngineClosed;
    case 18: return MessageKind.TickInput;
    case 19: return MessageKind.TickResult;
    case 20: return MessageKind.FramePulse;
    case 21: return MessageKind.AssetRequest;
    case 22: return MessageKind.AssetCompletion;
    case 23: return MessageKind.AssetControl;
    case 24: return MessageKind.PhysicsBatch;
    case 25: return MessageKind.PhysicsResult;
    case 26: return MessageKind.Diagnostics;
    case 27: return MessageKind.InspectionRequest;
    case 28: return MessageKind.InspectionResponse;
    case 29: return MessageKind.RenderTransient;
    case 30: return MessageKind.SurfaceControl;
    case 31: return MessageKind.DeviceEvent;
    case 32: return MessageKind.WorkerWake;
    case 33: return MessageKind.PlatformInput;
    case 34: return MessageKind.HapticsCommand;
    case 35: return MessageKind.HapticsResult;
    case 36: return MessageKind.AudioAssetData;
    case 37: return MessageKind.RenderAssetData;
    case 38: return MessageKind.RenderAssetAck;
    case 39: return MessageKind.AudioAssetAck;
    case 40: return MessageKind.RenderReadbackRequest;
    case 41: return MessageKind.RenderReadbackResult;
    case 42: return MessageKind.ControlLeaseRequest;
    case 43: return MessageKind.ControlLeaseGranted;
    case 44: return MessageKind.ControlCommit;
    case 45: return MessageKind.ControlCommitted;
    case 46: return MessageKind.ControlRealizationResult;
    case 47: return MessageKind.ControlObserved;
    case 48: return MessageKind.ControlObservedAck;
    case 49: return MessageKind.ControlStateAdopt;
    case 50: return MessageKind.ControlStateAdopted;
    default: return null;
}
}
function requireFrameworkMessageKind(value: number): MessageKind {
const kind = messageKindFromWire(value);
if (kind === null) throw new RangeError("unknown framework message kind");
return kind;
}
export function decodeFrameworkPacket(input: Uint8Array): FrameworkPacket {
if (!(input instanceof Uint8Array)) throw new TypeError("framework packet must be Uint8Array");
if (input.byteLength < HEADER_BYTES) throw new RangeError("truncated framework packet header");
if (input.byteLength > MAX_PACKET_BYTES) throw new RangeError("framework packet exceeds packet limit");
const view = new DataView(input.buffer, input.byteOffset, HEADER_BYTES);
const header: FrameworkPacketHeader = {
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
export function encodeFrameworkPacket(
header: Omit<FrameworkPacketHeader, "payloadLen">,
payload: Uint8Array,
): Uint8Array {
if (!(payload instanceof Uint8Array)) throw new TypeError("framework packet payload must be Uint8Array");
if (payload.byteLength > MAX_PACKET_BYTES - HEADER_BYTES) throw new RangeError("framework packet payload exceeds limit");
const output = new Uint8Array(HEADER_BYTES + payload.byteLength);
const view = new DataView(output.buffer);
  if (messageKindFromWire(header.kind) === null) throw new RangeError("unknown framework message kind");
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
function readFrameworkHandle(view: DataView, offset: number): GenerationalHandle {
const handle = { index: view.getUint32(offset, true), generation: view.getUint32(offset + 4, true) };
validateFrameworkHandle(handle);
return handle;
}
function writeFrameworkHandle(view: DataView, offset: number, handle: GenerationalHandle): void {
validateFrameworkHandle(handle);
view.setUint32(offset, handle.index, true);
view.setUint32(offset + 4, handle.generation, true);
}
function validateFrameworkHandle(handle: GenerationalHandle): void {
if (!Number.isInteger(handle.index) || handle.index < 0 || handle.index >= 0xffffffff
|| !Number.isInteger(handle.generation) || handle.generation < 1 || handle.generation > 0xffffffff) {
throw new RangeError("invalid framework packet handle");
}
}
function validateFrameworkU16(value: number, label: string): void {
if (!Number.isInteger(value) || value < 0 || value > 0xffff) throw new RangeError(`invalid ${label}`);
}
function validateFrameworkU32(value: number, label: string): void {
if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw new RangeError(`invalid ${label}`);
}
function validateFrameworkU64(value: bigint, label: string): void {
if (typeof value !== "bigint" || value < 0n || value > 0xffffffffffffffffn) throw new RangeError(`invalid ${label}`);
}
