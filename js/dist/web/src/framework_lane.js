import { decodeFrameworkPacket, encodeFrameworkPacket, } from "../../protocol/generated/voplay_protocol.js";
const SURFACE_CONTROL_PAYLOAD_BYTES = 78;
const FRAME_PREFIX_BYTES = 92;
const PLATFORM_INPUT_PREFIX_BYTES = 80;
export const BROWSER_GAMEPAD_DEVICE_BASE = 0x8000_0000;
const BROWSER_POINTER_DEVICE_BASE = 16;
const textEncoder = new TextEncoder();
export function decodeSurfaceControl(packetBytes) {
    const packet = decodeFrameworkPacket(packetBytes);
    if (packet.header.kind !== 30 /* MessageKind.SurfaceControl */
        || packet.payload.byteLength !== SURFACE_CONTROL_PAYLOAD_BYTES) {
        throw new RangeError("invalid Voplay SurfaceControl packet");
    }
    const reader = new Reader(packet.payload);
    const action = surfaceAction(reader.u8());
    const session = reader.handle();
    const window = reader.handle();
    const view = reader.handle();
    const surface = reader.handle();
    const domain = reader.handle();
    const metrics = {
        width: reader.nonzeroU32(),
        height: reader.nonzeroU32(),
        scaleNumerator: reader.nonzeroU32(),
        scaleDenominator: reader.nonzeroU32(),
    };
    const renderEndpoint = reader.handle();
    const deviceGeneration = reader.nonzeroU64();
    const zOrder = reader.i32();
    const inputPolicy = reader.nonzeroU8();
    reader.finish();
    return {
        action,
        engine: packet.header.engine,
        session,
        window,
        view,
        surface,
        domain,
        metrics,
        renderEndpoint,
        deviceGeneration,
        zOrder,
        inputPolicy,
        channelEpoch: packet.header.channelEpoch,
        sequence: packet.header.sequence,
    };
}
export function decodeFrameSubmission(packetBytes) {
    const packet = decodeFrameworkPacket(packetBytes);
    if (packet.header.kind !== 20 /* MessageKind.FramePulse */ || packet.payload.byteLength < FRAME_PREFIX_BYTES) {
        throw new RangeError("invalid Voplay FramePulse packet");
    }
    const reader = new Reader(packet.payload);
    const session = reader.handle();
    const window = reader.handle();
    const view = reader.handle();
    const surface = reader.handle();
    const domain = reader.handle();
    const renderEndpoint = reader.handle();
    const deviceGeneration = reader.nonzeroU64();
    const pulseId = reader.nonzeroU64();
    const frameId = reader.nonzeroU64();
    const deadlineMicros = reader.u64();
    const graphSignature = reader.nonzeroU64();
    const commands = new Uint8Array(reader.bytes32());
    reader.finish();
    if (frameId !== packet.header.commitId
        || packet.header.baseRevision !== packet.header.newRevision) {
        throw new RangeError("Voplay FramePulse identity mismatch");
    }
    return {
        header: packet.header,
        session,
        window,
        view,
        deadlineMicros,
        frame: {
            surface: {
                engine: { session, engine: packet.header.engine },
                surface,
                domain,
            },
            pulseId,
            frameId,
            renderEndpoint,
            deviceGeneration,
            requiredRenderRevision: packet.header.newRevision,
            requiredControlRevision: packet.header.requiredControlRevision,
            graphSignature,
            commands,
        },
    };
}
export function encodeFrameOutcome(outcome) {
    const writer = new Writer(89);
    writer.u8(outcomeTag(outcome.terminal));
    writer.handle(outcome.session);
    writer.handle(outcome.frame.surface.surface);
    writer.handle(outcome.frame.surface.domain);
    writer.handle(outcome.frame.renderEndpoint);
    writer.u64(outcome.frame.deviceGeneration);
    writer.u64(outcome.frame.pulseId);
    writer.u64(outcome.frame.frameId);
    writer.u64(outcome.renderedRevision);
    writer.u64(outcome.observedControlRevision);
    writer.u64(outcome.fenceValue);
    writer.u64(outcome.completionMicros);
    return encodeFrameworkPacket({
        kind: 31 /* MessageKind.DeviceEvent */,
        engine: outcome.frame.surface.engine.engine,
        channelEpoch: outcome.channelEpoch,
        commitId: outcome.frame.frameId,
        baseRevision: outcome.renderedRevision,
        newRevision: outcome.renderedRevision,
        requiredControlRevision: outcome.observedControlRevision,
        sourceSimulationRevision: 0n,
        sequence: outcome.sequence,
    }, writer.finish());
}
export function encodePlatformInput(event, control, sequence) {
    if (!sameHandle(event.surface.session, control.session)
        || !sameHandle(event.surface.window, control.window)
        || !sameHandle(event.surface.view, control.view)
        || !sameHandle(event.surface.surface, control.surface)) {
        throw new RangeError("Voplay platform input Surface route mismatch");
    }
    const detail = encodePlatformInputDetail(event);
    const writer = new Writer(PLATFORM_INPUT_PREFIX_BYTES + detail.byteLength);
    writer.u8(1);
    writer.u8(platformInputTag(event.type));
    writer.u16(platformInputFlags(event));
    writer.handle(control.session);
    writer.handle(control.window);
    writer.handle(control.view);
    writer.handle(control.surface);
    writer.handle(control.domain);
    writer.u64(event.timestampMicros);
    writer.u64(event.sequence);
    writer.handle(platformInputDevice(event));
    writer.u32(platformInputCode(event));
    writer.i32(platformInputValue(event));
    writer.u32(detail.byteLength);
    writer.bytes(detail);
    return encodeFrameworkPacket({
        kind: 33 /* MessageKind.PlatformInput */,
        engine: control.engine,
        channelEpoch: control.channelEpoch,
        commitId: 0n,
        baseRevision: 0n,
        newRevision: 0n,
        requiredControlRevision: 0n,
        sourceSimulationRevision: 0n,
        sequence,
    }, writer.finish());
}
export function surfaceKey(id) {
    return [
        `${id.engine.session.index}:${id.engine.session.generation}`,
        `${id.engine.engine.index}:${id.engine.engine.generation}`,
        `${id.surface.index}:${id.surface.generation}`,
        `${id.domain.index}:${id.domain.generation}`,
    ].join("/");
}
function surfaceAction(tag) {
    switch (tag) {
        case 1:
            return "attach";
        case 2:
            return "resize";
        case 3:
            return "suspend";
        case 4:
            return "resume";
        case 5:
            return "detach";
        case 6:
            return "rebind";
        default:
            throw new RangeError("unknown Voplay SurfaceControl action");
    }
}
function outcomeTag(outcome) {
    switch (outcome) {
        case "presented":
            return 1;
        case "deadlineMissed":
            return 2;
        case "suspended":
            return 3;
        case "surfaceLost":
            return 4;
        case "deviceLost":
            return 5;
        case "rejectedBeforeSubmit":
            return 6;
        case "outcomeUnknown":
            return 7;
    }
}
function platformInputTag(type) {
    switch (type) {
        case "pointerDown": return 1;
        case "pointerMove": return 2;
        case "pointerUp": return 3;
        case "pointerCancel": return 4;
        case "wheel": return 5;
        case "keyDown": return 6;
        case "keyUp": return 7;
        case "text": return 8;
        case "compositionStart": return 9;
        case "compositionUpdate": return 10;
        case "compositionEnd": return 11;
        case "focus": return 12;
        case "gamepadConnect": return 13;
        case "gamepadDisconnect": return 14;
        case "gamepadButton": return 15;
        case "gamepadAxis": return 16;
        default: throw new RangeError("unknown Voplay platform input kind");
    }
}
function platformInputFlags(event) {
    return (event.alt ? 1 : 0)
        | (event.control ? 1 << 1 : 0)
        | (event.meta ? 1 << 2 : 0)
        | (event.shift ? 1 << 3 : 0)
        | (event.repeat ? 1 << 4 : 0)
        | (event.synthesized ? 1 << 5 : 0)
        | (event.composing ? 1 << 6 : 0)
        | (event.focused ? 1 << 7 : 0);
}
function platformInputDevice(event) {
    if (event.type.startsWith("gamepad")) {
        const index = event.gamepadIndex;
        const generation = event.gamepadGeneration;
        if (index === undefined
            || generation === undefined
            || index < 0
            || index >= 0x7fff_ffff
            || generation <= 0) {
            throw new RangeError("invalid browser gamepad identity");
        }
        return { index: BROWSER_GAMEPAD_DEVICE_BASE + index, generation };
    }
    const pointer = event.pointerId;
    if (pointer !== undefined
        && (!Number.isSafeInteger(pointer)
            || pointer < 0
            || pointer > BROWSER_GAMEPAD_DEVICE_BASE - BROWSER_POINTER_DEVICE_BASE - 1)) {
        throw new RangeError("invalid browser pointer identity");
    }
    let scalarDevice = 2;
    if (event.type.startsWith("key"))
        scalarDevice = 1;
    else if (event.type === "wheel")
        scalarDevice = 3;
    else if (event.type === "focus")
        scalarDevice = 4;
    return {
        index: pointer === undefined
            ? scalarDevice
            : BROWSER_POINTER_DEVICE_BASE + pointer,
        generation: 1,
    };
}
function platformInputCode(event) {
    if (event.type === "gamepadButton" || event.type === "gamepadAxis") {
        return (event.gamepadControl ?? -1) + 1;
    }
    if (event.type.startsWith("pointer"))
        return (event.button ?? -1) + 1;
    if (event.type === "wheel")
        return event.deltaMode ?? 0;
    if (event.type.startsWith("key"))
        return fnv1a32(event.physical ?? "");
    return 0;
}
function platformInputValue(event) {
    if (event.type === "gamepadButton" || event.type === "gamepadAxis") {
        return event.gamepadValueQ16 ?? 0;
    }
    if (event.type === "gamepadConnect")
        return 1;
    if (event.type === "gamepadDisconnect")
        return 0;
    if (event.type === "pointerDown")
        return 1;
    if (event.type === "pointerUp" || event.type === "pointerCancel")
        return 0;
    if (event.type === "pointerMove")
        return event.buttons ?? 0;
    if (event.type === "wheel")
        return event.deltaYMilli ?? 0;
    if (event.type === "keyDown")
        return 1;
    if (event.type === "keyUp")
        return 0;
    if (event.type === "focus")
        return event.focused ? 1 : 0;
    return 0;
}
function encodePlatformInputDetail(event) {
    switch (event.type) {
        case "pointerDown":
        case "pointerMove":
        case "pointerUp":
        case "pointerCancel": {
            const pointerType = textEncoder.encode(event.pointerType ?? "");
            const writer = new Writer(46 + pointerType.byteLength);
            writer.i32(event.xMilli ?? 0);
            writer.i32(event.yMilli ?? 0);
            writer.i32(event.localXMilli ?? 0);
            writer.i32(event.localYMilli ?? 0);
            writer.i32(event.movementXMilli ?? 0);
            writer.i32(event.movementYMilli ?? 0);
            writer.i32(event.button ?? -1);
            writer.u32(event.buttons ?? 0);
            writer.u32(event.pressureQ16 ?? 0);
            writer.i32(event.tiltX ?? 0);
            writer.i32(event.tiltY ?? 0);
            writer.u16(pointerType.byteLength);
            writer.bytes(pointerType);
            return writer.finish();
        }
        case "wheel": {
            const writer = new Writer(28);
            writer.i32(event.xMilli ?? 0);
            writer.i32(event.yMilli ?? 0);
            writer.i32(event.localXMilli ?? 0);
            writer.i32(event.localYMilli ?? 0);
            writer.i32(event.deltaXMilli ?? 0);
            writer.i32(event.deltaYMilli ?? 0);
            writer.i32(event.deltaZMilli ?? 0);
            return writer.finish();
        }
        case "keyDown":
        case "keyUp":
            return encodeStrings(event.physical ?? "", event.logical ?? "");
        case "text":
        case "compositionStart":
        case "compositionUpdate":
        case "compositionEnd":
            return encodeStrings(event.text ?? "", event.inputType ?? "");
        case "focus":
            return new Uint8Array(0);
        case "gamepadConnect": {
            const id = textEncoder.encode(event.gamepadId ?? "");
            const mapping = textEncoder.encode(event.gamepadMapping ?? "");
            if (id.byteLength > 0xffff || mapping.byteLength > 0xffff) {
                throw new RangeError("Voplay gamepad descriptor exceeds u16 length");
            }
            const writer = new Writer(4 + id.byteLength + mapping.byteLength);
            writer.u16(id.byteLength);
            writer.u16(mapping.byteLength);
            writer.bytes(id);
            writer.bytes(mapping);
            return writer.finish();
        }
        case "gamepadDisconnect":
        case "gamepadButton":
        case "gamepadAxis":
            return new Uint8Array();
        default:
            throw new RangeError("unknown Voplay platform input detail");
    }
}
function encodeStrings(first, second) {
    const firstBytes = textEncoder.encode(first);
    const secondBytes = textEncoder.encode(second);
    if (firstBytes.byteLength > 0xffff || secondBytes.byteLength > 0xffff) {
        throw new RangeError("Voplay platform input string exceeds u16 length");
    }
    const writer = new Writer(4 + firstBytes.byteLength + secondBytes.byteLength);
    writer.u16(firstBytes.byteLength);
    writer.bytes(firstBytes);
    writer.u16(secondBytes.byteLength);
    writer.bytes(secondBytes);
    return writer.finish();
}
function fnv1a32(value) {
    let hash = 0x811c9dc5;
    for (const byte of textEncoder.encode(value)) {
        hash ^= byte;
        hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    return hash;
}
function sameHandle(left, right) {
    return left.index === right.index && left.generation === right.generation;
}
class Reader {
    bytes;
    #offset = 0;
    constructor(bytes) {
        this.bytes = bytes;
    }
    u8() {
        return this.take(1)[0];
    }
    nonzeroU8() {
        const value = this.u8();
        if (value === 0)
            throw new RangeError("zero Voplay u8 identity");
        return value;
    }
    u32() {
        return this.view(4).getUint32(0, true);
    }
    nonzeroU32() {
        const value = this.u32();
        if (value === 0)
            throw new RangeError("zero Voplay u32 identity");
        return value;
    }
    i32() {
        return this.view(4).getInt32(0, true);
    }
    u64() {
        return this.view(8).getBigUint64(0, true);
    }
    nonzeroU64() {
        const value = this.u64();
        if (value === 0n)
            throw new RangeError("zero Voplay u64 identity");
        return value;
    }
    handle() {
        const value = { index: this.u32(), generation: this.u32() };
        if (value.index === 0xffffffff || value.generation === 0) {
            throw new RangeError("invalid Voplay handle");
        }
        return value;
    }
    bytes32() {
        return this.take(this.u32());
    }
    finish() {
        if (this.#offset !== this.bytes.byteLength) {
            throw new RangeError("trailing Voplay wire bytes");
        }
    }
    take(length) {
        const end = this.#offset + length;
        if (!Number.isSafeInteger(end) || end > this.bytes.byteLength) {
            throw new RangeError("truncated Voplay wire payload");
        }
        const value = this.bytes.subarray(this.#offset, end);
        this.#offset = end;
        return value;
    }
    view(length) {
        const bytes = this.take(length);
        return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    }
}
class Writer {
    #bytes;
    #offset = 0;
    constructor(length) {
        this.#bytes = new Uint8Array(length);
    }
    u8(value) {
        if (!Number.isInteger(value) || value < 0 || value > 0xff)
            throw new RangeError("invalid u8");
        this.require(1);
        this.#bytes[this.#offset] = value;
        this.#offset += 1;
    }
    u32(value) {
        if (!Number.isInteger(value) || value < 0 || value > 0xffff_ffff) {
            throw new RangeError("invalid u32");
        }
        this.require(4);
        new DataView(this.#bytes.buffer).setUint32(this.#offset, value, true);
        this.#offset += 4;
    }
    u16(value) {
        if (!Number.isInteger(value) || value < 0 || value > 0xffff)
            throw new RangeError("invalid u16");
        this.require(2);
        new DataView(this.#bytes.buffer).setUint16(this.#offset, value, true);
        this.#offset += 2;
    }
    i32(value) {
        if (!Number.isInteger(value) || value < -0x8000_0000 || value > 0x7fff_ffff) {
            throw new RangeError("invalid i32");
        }
        this.require(4);
        new DataView(this.#bytes.buffer).setInt32(this.#offset, value, true);
        this.#offset += 4;
    }
    u64(value) {
        if (value < 0n || value > 0xffffffffffffffffn)
            throw new RangeError("invalid u64");
        this.require(8);
        new DataView(this.#bytes.buffer).setBigUint64(this.#offset, value, true);
        this.#offset += 8;
    }
    handle(value) {
        this.u32(value.index);
        this.u32(value.generation);
    }
    bytes(value) {
        this.require(value.byteLength);
        this.#bytes.set(value, this.#offset);
        this.#offset += value.byteLength;
    }
    finish() {
        if (this.#offset !== this.#bytes.byteLength)
            throw new Error("incomplete Voplay wire payload");
        return this.#bytes;
    }
    require(length) {
        if (this.#offset + length > this.#bytes.byteLength)
            throw new RangeError("Voplay wire overflow");
    }
}
