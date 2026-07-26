import { WebGpuCanvasAdapter } from "./webgpu_adapter.js";
import { decodeFrameSubmission, decodeSurfaceControl, encodeFrameOutcome, encodePlatformInput, surfaceKey, } from "./framework_lane.js";
import { BrowserGpuReadbackError, BrowserSurfaceHost, } from "./platform_surface.js";
import { decodeFrameworkPacket, encodeFrameworkPacket, } from "../../protocol/generated/voplay_protocol.js";
import { BrowserHapticsHost, } from "./haptics.js";
import { BrowserGamepadInputSource, } from "./gamepad_input.js";
const MAX_COMMAND_BYTES = 16 * 1024 * 1024;
const MAX_COMMANDS = 131_072;
const MAX_TEXTURES = 65_536;
const MAX_TEXTURE_BYTES = 512 * 1024 * 1024;
const MAX_PROFILE_ASSETS = 65_536;
const MAX_PROFILE_ASSET_BYTES = 512 * 1024 * 1024;
const MAX_PENDING_INPUT_RETURNS = 1024;
const MAX_COALESCED_INPUTS = 256;
const MAX_PENDING_READBACKS = 64;
const MAX_PENDING_READBACK_BYTES = 64 * 1024 * 1024;
const MAX_SINGLE_READBACK_BYTES = 16 * 1024 * 1024;
const MAX_RETAINED_FRAME_TRACES = 64;
class VoplayStudioRenderer {
    #host = null;
    #lane = null;
    #surfaceCapability = null;
    #surfaceHost = null;
    #haptics = null;
    #gamepads = null;
    #surfaces = new Map();
    #unsubscribeInput = null;
    #coalescedInputs = new Map();
    #drainingCoalesced = new Set();
    #polling = false;
    #lastInboundSequence = 0n;
    #nextReturnSequence = 1n;
    #nextInputEventSequence = 1n;
    #pendingInputReturns = 0;
    #activeInputSurface = null;
    #gamepadRouteSurface = null;
    #textures = new Map();
    #textureRevisions = new Map();
    #textureBytes = 0;
    #profileAssets = new Map();
    #profileAssetRevisions = new Map();
    #profileAssetBytes = 0;
    #hostFrameFences = new Map();
    #pendingReadbacks = new Map();
    #pendingReadbackBytes = 0;
    #cancelledReadbacks = new Set();
    #frameTraces = new Map();
    #renderTargets = new Map();
    #renderFeatures = new Map();
    #engineStates = new Map();
    async init(host) {
        if (this.#host !== null)
            throw new Error("Voplay renderer already initialized");
        if (!host.framework.roles.includes("renderer")) {
            throw new Error("Voplay renderer requires the renderer role");
        }
        const laneCapability = host.getCapability("framework_lane");
        const surfaceCapability = host.getCapability("app_surface");
        if (laneCapability === null || surfaceCapability === null) {
            throw new Error("Voplay renderer requires framework_lane and app_surface");
        }
        const lane = await laneCapability.open("render");
        this.#host = host;
        this.#lane = lane;
        this.#surfaceCapability = surfaceCapability;
        this.#gamepads = new BrowserGamepadInputSource((event) => {
            this.#acceptGamepadInput(event);
        }, () => surfaceCapability.isInteractive());
        this.#haptics = new BrowserHapticsHost((result) => {
            void this.#submitHapticsResult(result).catch((error) => {
                this.#failPlatformInput(error);
            });
        }, (index) => this.#gamepads?.generation(index));
        this.#gamepads.start();
        this.#unsubscribeInput = surfaceCapability.subscribeInput((event) => {
            this.#acceptPlatformInput(event);
        });
        this.#polling = true;
        void this.#poll(host, lane);
        host.log(`Voplay browser renderer ready for ${host.framework.name}`);
    }
    async render(_container, bytes) {
        if (bytes.byteLength === 0)
            return;
        try {
            await this.#acceptPacket(bytes);
        }
        catch (error) {
            this.#host?.reportError(`Voplay render packet failed: ${errorMessage(error)}`);
            throw error;
        }
    }
    async acceptHostRenderCommand(bytes) {
        const reader = new HostRenderReader(bytes);
        reader.magic("VHR3");
        const routeEngine = reader.handle();
        reader.magic("VHR1");
        const action = reader.u8();
        switch (action) {
            case 1:
            case 2:
            case 3: {
                const id = reader.surface(routeEngine, this.#requireLane().binding.session);
                const metrics = reader.metrics();
                reader.finish();
                let record = this.#surfaces.get(surfaceKey(id));
                if (action === 1) {
                    if (record === undefined) {
                        record = await this.#attachHostRenderSurface(id, metrics);
                    }
                    else if (this.#surfaceHost !== null) {
                        this.#surfaceHost.resize(id, metrics);
                    }
                    record.control = { ...record.control, metrics };
                }
                else {
                    if (record === undefined) {
                        record = await this.#attachHostRenderSurface(id, metrics);
                    }
                    else if (this.#surfaceHost !== null) {
                        if (action === 3) {
                            await this.#surfaceHost.rebindDevice(id, metrics, this.#surfaceHost.ownerSnapshot().deviceGeneration);
                        }
                        else {
                            this.#surfaceHost.resize(id, metrics);
                        }
                    }
                    record.control = { ...record.control, metrics };
                }
                return null;
            }
            case 4: {
                const texture = reader.u64();
                const width = reader.u32();
                const height = reader.u32();
                const pixels = reader.blob();
                reader.finish();
                const expected = width * height * 4;
                if (texture <= 0n
                    || width <= 0
                    || height <= 0
                    || !Number.isSafeInteger(expected)
                    || pixels.byteLength !== expected) {
                    throw new Error("invalid Voplay host-render texture");
                }
                const rgba = new Uint8ClampedArray(pixels.byteLength);
                rgba.set(pixels);
                const source = await createImageBitmap(new ImageData(rgba, width, height));
                const textureKey = engineResourceKey(routeEngine, texture);
                const previous = this.#textures.get(textureKey);
                const nextBytes = this.#textureBytes
                    - (previous === undefined ? 0 : previous.source.width * previous.source.height * 4)
                    + pixels.byteLength;
                if ((previous === undefined && this.#textures.size >= MAX_TEXTURES)
                    || nextBytes > MAX_TEXTURE_BYTES) {
                    source.close();
                    throw new Error("Voplay host-render texture capacity exceeded");
                }
                try {
                    this.#surfaceHost?.registerTexture(texture, source, routeEngine);
                }
                catch (error) {
                    source.close();
                    throw error;
                }
                this.#textures.set(textureKey, {
                    engine: routeEngine,
                    texture,
                    revision: (previous?.revision ?? 0n) + 1n,
                    source,
                });
                this.#textureBytes = nextBytes;
                previous?.source.close();
                return null;
            }
            case 5: {
                const texture = reader.u64();
                reader.finish();
                const textureKey = engineResourceKey(routeEngine, texture);
                const previous = this.#textures.get(textureKey);
                if (previous !== undefined) {
                    this.#surfaceHost?.removeTexture(texture, routeEngine);
                    this.#textures.delete(textureKey);
                    this.#textureBytes -= previous.source.width * previous.source.height * 4;
                    previous.source.close();
                }
                return null;
            }
            case 6: {
                const kind = reader.u32();
                const asset = reader.u64();
                const revision = reader.u64();
                const assetBytes = reader.blob();
                reader.finish();
                this.#upsertHostProfileAsset(routeEngine, kind, asset, revision, assetBytes);
                return null;
            }
            case 7: {
                const kind = reader.u32();
                const asset = reader.u64();
                const revision = reader.u64();
                reader.finish();
                this.#removeHostProfileAsset(routeEngine, kind, asset, revision);
                return null;
            }
            case 8: {
                const frame = reader.frame(routeEngine, this.#requireLane().binding.session);
                reader.finish();
                const record = this.#surfaces.get(surfaceKey(frame.surface));
                if (record === undefined)
                    throw new Error("unknown Voplay host-render Surface");
                record.control = {
                    ...record.control,
                    renderEndpoint: frame.renderEndpoint,
                    deviceGeneration: frame.deviceGeneration,
                };
                const host = await this.#ensureHostRenderSurfaceHost(frame.deviceGeneration);
                const submission = host.submit(frame);
                this.#hostFrameFences.set(hostFrameKey(frame.surface, frame.frameId), submission.fenceValue);
                const traceKey = engineResourceKey(routeEngine, frame.frameId);
                this.#frameTraces.delete(traceKey);
                this.#frameTraces.set(traceKey, {
                    engine: routeEngine,
                    target: frame.surface.surface,
                    frameId: frame.frameId,
                    graphSignature: frame.graphSignature,
                    fence: submission.fenceValue,
                    width: record.control.metrics.width,
                    height: record.control.metrics.height,
                });
                while (this.#frameTraces.size > MAX_RETAINED_FRAME_TRACES) {
                    const oldest = this.#frameTraces.keys().next().value;
                    if (oldest === undefined)
                        break;
                    this.#frameTraces.delete(oldest);
                }
                return null;
            }
            case 9: {
                const ack = reader.frameAck(routeEngine, this.#requireLane().binding.session);
                const nowMicros = reader.u64();
                const deadlineMicros = reader.u64();
                reader.finish();
                const key = hostFrameKey(ack.surface, ack.frameId);
                const fence = this.#hostFrameFences.get(key);
                if (fence === undefined)
                    throw new Error("Voplay host-render frame fence disappeared");
                this.#requireSurfaceHost().present(ack.surface, ack.frameId, fence, nowMicros, deadlineMicros);
                this.#hostFrameFences.delete(key);
                return null;
            }
            case 10: {
                const id = reader.surface(routeEngine, this.#requireLane().binding.session);
                reader.finish();
                const key = surfaceKey(id);
                const record = this.#surfaces.get(key);
                if (record === undefined)
                    throw new Error("unknown Voplay host-render Surface");
                if (this.#gamepadRouteSurface === key)
                    this.#gamepads?.invalidate();
                this.#surfaceHost?.detach(id);
                record.lease.release();
                this.#surfaces.delete(key);
                if (this.#gamepadRouteSurface === key)
                    this.#gamepadRouteSurface = null;
                this.#removeSurfaceFrameTraces(id);
                return null;
            }
            case 11: {
                const request = reader.u64();
                const readback = reader.readbackRequest(routeEngine);
                reader.finish();
                if (request <= 0n) {
                    throw new Error("invalid Voplay host-render readback request identity");
                }
                const key = engineResourceKey(routeEngine, request);
                const bytesPerPixel = readback.format === 3 ? 8 : 4;
                const rowBytes = Math.ceil(readback.region.width * bytesPerPixel / 256) * 256;
                const readbackBytes = rowBytes * readback.region.height;
                if (this.#pendingReadbacks.has(key)
                    || this.#pendingReadbacks.size >= MAX_PENDING_READBACKS
                    || !Number.isSafeInteger(readbackBytes)
                    || readbackBytes <= 0
                    || readbackBytes > MAX_SINGLE_READBACK_BYTES
                    || this.#pendingReadbackBytes + readbackBytes > MAX_PENDING_READBACK_BYTES) {
                    return encodeHostRenderFailure(routeEngine, request, 1, false);
                }
                this.#pendingReadbacks.set(key, readbackBytes);
                this.#pendingReadbackBytes += readbackBytes;
                try {
                    const result = await this.#requireSurfaceHost().readRenderTarget(routeEngine, readback.target, readback.expectedRevision, readback.region, readback.format);
                    if (this.#cancelledReadbacks.delete(key))
                        return null;
                    return encodeHostRenderReadback(routeEngine, request, readback.target, result.targetRevision, result.rowBytes, result.bytes);
                }
                catch (error) {
                    if (this.#cancelledReadbacks.delete(key))
                        return null;
                    return encodeHostRenderFailure(routeEngine, request, error instanceof BrowserGpuReadbackError ? error.failure : 2, false);
                }
                finally {
                    this.#pendingReadbackBytes -= this.#pendingReadbacks.get(key) ?? 0;
                    this.#pendingReadbacks.delete(key);
                }
            }
            case 12: {
                const request = reader.u64();
                reader.finish();
                const key = engineResourceKey(routeEngine, request);
                if (this.#pendingReadbacks.has(key))
                    this.#cancelledReadbacks.add(key);
                return null;
            }
            case 13: {
                const request = reader.u64();
                const traceRequest = reader.frameTraceRequest();
                reader.finish();
                if (request <= 0n) {
                    throw new Error("invalid Voplay host-render frame trace identity");
                }
                const retained = this.#frameTraces.get(engineResourceKey(routeEngine, traceRequest.frameId));
                if (retained === undefined
                    || retained.graphSignature !== traceRequest.graphSignature) {
                    return encodeHostRenderFailure(routeEngine, request, 1, true);
                }
                const trace = encodeBrowserFrameTrace(retained);
                if (trace.byteLength > traceRequest.maxBytes) {
                    return encodeHostRenderFailure(routeEngine, request, 1, true);
                }
                return encodeHostRenderFrameTrace(routeEngine, request, retained.frameId, retained.graphSignature, trace);
            }
            case 14: {
                const request = reader.u64();
                reader.finish();
                if (request <= 0n) {
                    throw new Error("invalid Voplay host-render frame trace cancellation");
                }
                return null;
            }
            case 15: {
                const count = reader.u32();
                if (count > 4096)
                    throw new Error("Voplay host-render target capacity exceeded");
                const targets = [];
                const identities = new Set();
                for (let index = 0; index < count; index += 1) {
                    const engine = reader.handle();
                    const target = reader.handle();
                    const identity = `${target.index}:${target.generation}`;
                    if (!sameHandle(engine, routeEngine) || identities.has(identity)) {
                        throw new Error("invalid Voplay host-render target synchronization");
                    }
                    identities.add(identity);
                    targets.push(target);
                }
                reader.finish();
                if (targets.length === 0) {
                    this.#renderTargets.delete(handleKey(routeEngine));
                }
                else {
                    this.#renderTargets.set(handleKey(routeEngine), {
                        engine: routeEngine,
                        targets,
                    });
                }
                this.#surfaceHost?.synchronizeRenderTargets(routeEngine, targets);
                return null;
            }
            default:
                throw new Error(`unsupported Voplay host-render action ${action}`);
        }
    }
    stop() {
        this.#polling = false;
        this.#unsubscribeInput?.();
        this.#unsubscribeInput = null;
        this.#haptics?.close(false);
        this.#haptics = null;
        this.#gamepads?.close(false);
        this.#gamepads = null;
        this.#lane?.close();
        this.#lane = null;
        try {
            const abandoned = this.#surfaceHost?.close() ?? 0;
            if (abandoned > 0) {
                this.#host?.log(`Voplay browser renderer abandoned ${abandoned} pending frame(s) on close`);
            }
        }
        finally {
            this.#surfaceHost = null;
            for (const record of this.#surfaces.values())
                record.lease.release();
            this.#surfaces.clear();
            for (const texture of this.#textures.values())
                texture.source.close();
            this.#textures.clear();
            this.#textureRevisions.clear();
            this.#textureBytes = 0;
            this.#profileAssets.clear();
            this.#profileAssetRevisions.clear();
            this.#profileAssetBytes = 0;
            this.#hostFrameFences.clear();
            this.#pendingReadbacks.clear();
            this.#pendingReadbackBytes = 0;
            this.#cancelledReadbacks.clear();
            this.#frameTraces.clear();
            this.#renderTargets.clear();
            this.#renderFeatures.clear();
            this.#engineStates.clear();
            this.#coalescedInputs.clear();
            this.#drainingCoalesced.clear();
            this.#surfaceCapability = null;
            this.#host = null;
            this.#lastInboundSequence = 0n;
            this.#nextReturnSequence = 1n;
            this.#nextInputEventSequence = 1n;
            this.#pendingInputReturns = 0;
            this.#activeInputSurface = null;
            this.#gamepadRouteSurface = null;
        }
    }
    async #attachHostRenderSurface(id, metrics) {
        const capability = this.#requireSurfaceCapability();
        const lane = this.#requireLane();
        const route = await capability.resolve(id.surface);
        if (route.kind !== "game"
            || !sameHandle(route.session, lane.binding.session)
            || route.sessionEpoch !== BigInt(lane.binding.sessionEpoch)
            || !sameHandle(route.surface, id.surface)) {
            throw new Error("Voplay host-render Surface route does not match App Runtime authority");
        }
        const lease = capability.attach({
            identity: {
                sessionId: capability.sessionId,
                session: route.session,
                sessionEpoch: route.sessionEpoch,
                window: route.window,
                view: route.view,
                surface: route.surface,
            },
            kind: "canvas",
            layer: route.zOrder,
            input: hostInputPolicy(route.inputPolicy),
            label: `Voplay ${id.engine.engine.index}:${id.engine.engine.generation}`,
        });
        if (!(lease.element instanceof HTMLCanvasElement)) {
            lease.release();
            throw new Error("Voplay App Surface host returned a non-canvas element");
        }
        const control = {
            action: "attach",
            engine: id.engine.engine,
            session: id.engine.session,
            window: route.window,
            view: route.view,
            surface: id.surface,
            domain: id.domain,
            metrics,
            renderEndpoint: { index: 0, generation: 1 },
            deviceGeneration: 1n,
            zOrder: route.zOrder,
            inputPolicy: browserInputPolicyTag(route.inputPolicy),
            channelEpoch: BigInt(lane.binding.channelEpoch),
            sequence: 1n,
        };
        const record = { control, id, lease };
        try {
            this.#surfaceHost?.attach(id, lease.element, metrics);
        }
        catch (error) {
            lease.release();
            throw error;
        }
        this.#surfaces.set(surfaceKey(id), record);
        return record;
    }
    async #ensureHostRenderSurfaceHost(deviceGeneration) {
        if (this.#surfaceHost === null) {
            const adapter = await WebGpuCanvasAdapter.create({
                deviceGeneration,
                maxCommands: MAX_COMMANDS,
                maxCommandBytes: MAX_COMMAND_BYTES,
            });
            const host = new BrowserSurfaceHost(adapter, {
                maxSurfaces: 64,
                maxCommandBytes: MAX_COMMAND_BYTES,
            });
            const previousControls = new Map([...this.#surfaces.values()].map((record) => [record, record.control]));
            try {
                for (const resident of this.#textures.values()) {
                    host.registerTexture(resident.texture, resident.source, resident.engine);
                }
                for (const record of this.#surfaces.values()) {
                    host.attach(record.id, record.lease.element, record.control.metrics);
                    record.control = { ...record.control, deviceGeneration };
                }
                this.#synchronizeKnownRenderTargets(host);
            }
            catch (error) {
                for (const [record, control] of previousControls)
                    record.control = control;
                host.close();
                throw error;
            }
            this.#surfaceHost = host;
        }
        else {
            const currentGeneration = this.#surfaceHost.ownerSnapshot().deviceGeneration;
            if (currentGeneration > deviceGeneration) {
                throw new Error("stale Voplay host-render device generation");
            }
            if (currentGeneration < deviceGeneration
                || [...this.#surfaces.values()].some((record) => record.control.deviceGeneration !== deviceGeneration)) {
                for (const record of this.#surfaces.values()) {
                    await this.#surfaceHost.rebindDevice(record.id, record.control.metrics, deviceGeneration);
                    record.control = { ...record.control, deviceGeneration };
                }
            }
        }
        return this.#surfaceHost;
    }
    #synchronizeKnownRenderTargets(host) {
        for (const { engine, targets } of this.#renderTargets.values()) {
            host.synchronizeRenderTargets(engine, targets);
        }
    }
    #upsertHostProfileAsset(engine, kind, asset, revision, bytes) {
        const key = `${engineResourceKey(engine, asset)}/${kind}`;
        const previous = this.#profileAssets.get(key);
        const previousRevision = this.#profileAssetRevisions.get(key);
        const nextBytes = this.#profileAssetBytes - (previous?.bytes.byteLength ?? 0) + bytes.byteLength;
        if (kind < 2
            || asset <= 0n
            || revision <= (previousRevision ?? 0n)
            || bytes.byteLength === 0
            || (previous === undefined && this.#profileAssets.size >= MAX_PROFILE_ASSETS)
            || nextBytes > MAX_PROFILE_ASSET_BYTES) {
            throw new Error("invalid Voplay host-render profile asset");
        }
        this.#profileAssets.set(key, {
            engine,
            kind,
            asset,
            revision,
            bytes: bytes.slice(),
        });
        this.#profileAssetRevisions.set(key, revision);
        this.#profileAssetBytes = nextBytes;
    }
    #removeHostProfileAsset(engine, kind, asset, revision) {
        const key = `${engineResourceKey(engine, asset)}/${kind}`;
        const previousRevision = this.#profileAssetRevisions.get(key);
        if (kind < 2 || asset <= 0n || revision <= (previousRevision ?? 0n)) {
            throw new Error("stale Voplay host-render profile asset removal");
        }
        const previous = this.#profileAssets.get(key);
        if (previous !== undefined) {
            this.#profileAssets.delete(key);
            this.#profileAssetBytes -= previous.bytes.byteLength;
        }
        this.#profileAssetRevisions.set(key, revision);
    }
    quiesceForCapture() {
        return { stopped: 1, surfaces: this.#surfaces.size };
    }
    ownerSnapshot() {
        return {
            surfaces: this.#surfaceHost?.ownerSnapshot() ?? null,
            textureCount: this.#textures.size,
            textureBytes: this.#textureBytes,
            profileAssetCount: this.#profileAssets.size,
            profileAssetBytes: this.#profileAssetBytes,
            pendingReadbacks: this.#pendingReadbacks.size,
            pendingReadbackBytes: this.#pendingReadbackBytes,
            retainedFrameTraces: this.#frameTraces.size,
            renderTargets: [...this.#renderTargets.values()].reduce((total, group) => total + group.targets.length, 0),
            gamepads: this.#gamepads?.ownerSnapshot() ?? null,
            haptics: this.#haptics?.ownerSnapshot() ?? null,
            pendingInputReturns: this.#pendingInputReturns,
            coalescedInputs: this.#coalescedInputs.size,
        };
    }
    async #poll(host, lane) {
        while (this.#polling && this.#host === host && this.#lane === lane) {
            try {
                this.#haptics?.setEnabled(this.#requireSurfaceCapability().isInteractive());
                const packet = await lane.poll();
                if (!this.#polling || this.#host !== host || this.#lane !== lane)
                    return;
                if (packet === null) {
                    await delay(8);
                    continue;
                }
                try {
                    if (isRenderFeatureBootstrap(packet)) {
                        this.#acceptRenderFeatureBootstrap(packet);
                    }
                    else if (isHostRenderCommand(packet)) {
                        await this.acceptHostRenderCommand(packet);
                    }
                    else {
                        await this.#acceptPacket(packet);
                    }
                }
                catch (error) {
                    throw new Error(`${errorMessage(error)}; lane-bytes=${packet.byteLength}; `
                        + `prefix=${hexPrefix(packet, 16)}`);
                }
            }
            catch (error) {
                if (!this.#polling || this.#host !== host || this.#lane !== lane)
                    return;
                host.reportError(`Voplay framework lane failed: ${errorMessage(error)}`);
                this.#polling = false;
            }
        }
    }
    #acceptRenderFeatureBootstrap(bytes) {
        if (bytes.byteLength < 20 || !isRenderFeatureBootstrap(bytes)) {
            throw new Error("invalid Voplay RenderFeature bootstrap");
        }
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        const engine = {
            index: view.getUint32(8, true),
            generation: view.getUint32(12, true),
        };
        const count = view.getUint32(16, true);
        if (engine.index === 0xffff_ffff || engine.generation === 0 || count > 4096) {
            throw new Error("invalid Voplay RenderFeature bootstrap owner");
        }
        let offset = 20;
        const features = [];
        for (let index = 0; index < count; index += 1) {
            if (offset + 4 > bytes.byteLength) {
                throw new Error("truncated Voplay RenderFeature bootstrap");
            }
            const length = view.getUint32(offset, true);
            offset += 4;
            if (length === 0 || offset + length > bytes.byteLength) {
                throw new Error("invalid Voplay RenderFeature descriptor");
            }
            features.push(bytes.slice(offset, offset + length));
            offset += length;
        }
        if (offset !== bytes.byteLength) {
            throw new Error("Voplay RenderFeature bootstrap has trailing bytes");
        }
        const key = `${engine.index}:${engine.generation}`;
        if (this.#renderFeatures.has(key)) {
            throw new Error("duplicate Voplay RenderFeature bootstrap");
        }
        this.#renderFeatures.set(key, features);
    }
    async #acceptPacket(bytes) {
        const lane = this.#requireLane();
        const decoded = decodeFrameworkPacket(bytes);
        const header = decoded.header;
        if (header.channelEpoch !== BigInt(lane.binding.channelEpoch)) {
            throw new Error("Voplay packet channel epoch mismatch");
        }
        const lifecyclePacket = header.kind === 12 /* MessageKind.EngineStart */
            || header.kind === 14 /* MessageKind.EngineSuspend */
            || header.kind === 15 /* MessageKind.EngineResume */
            || header.kind === 16 /* MessageKind.EngineClose */
            || header.kind === 32 /* MessageKind.WorkerWake */;
        if (header.kind !== 37 /* MessageKind.RenderAssetData */
            && !lifecyclePacket
            && header.sequence <= this.#lastInboundSequence) {
            throw new Error("Voplay packet sequence regression");
        }
        if (header.kind !== 37 /* MessageKind.RenderAssetData */ && !lifecyclePacket) {
            this.#lastInboundSequence = header.sequence;
        }
        switch (header.kind) {
            case 12 /* MessageKind.EngineStart */:
            case 14 /* MessageKind.EngineSuspend */:
            case 15 /* MessageKind.EngineResume */:
            case 16 /* MessageKind.EngineClose */:
                await this.#applyEngineLifecycle(decoded);
                break;
            case 32 /* MessageKind.WorkerWake */:
                if (decoded.payload.byteLength !== 0) {
                    throw new Error("Voplay renderer wake payload must be empty");
                }
                break;
            case 30 /* MessageKind.SurfaceControl */:
                await this.#applySurfaceControl(decodeSurfaceControl(bytes));
                break;
            case 20 /* MessageKind.FramePulse */:
                await this.#renderFrame(bytes);
                break;
            case 37 /* MessageKind.RenderAssetData */:
                await this.#applyRenderAsset(bytes);
                break;
            case 34 /* MessageKind.HapticsCommand */:
                this.#requireHaptics().setEnabled(this.#requireSurfaceCapability().isInteractive());
                this.#requireHaptics().accept(decodeFrameworkPacket(bytes));
                break;
            default:
                throw new Error(`unsupported Voplay browser packet ${header.kind}`);
        }
    }
    async #applyEngineLifecycle(packet) {
        const { header, payload } = packet;
        if (payload.byteLength !== 0) {
            throw new Error("Voplay renderer lifecycle payload must be empty");
        }
        const key = `${header.engine.index}:${header.engine.generation}`;
        const current = this.#engineStates.get(key);
        switch (header.kind) {
            case 12 /* MessageKind.EngineStart */:
                if (current !== undefined) {
                    throw new Error("duplicate Voplay renderer EngineStart");
                }
                this.#engineStates.set(key, "running");
                await this.#replyLifecycle(packet, 13 /* MessageKind.EngineReady */);
                return;
            case 14 /* MessageKind.EngineSuspend */:
                if (current !== "running") {
                    throw new Error("invalid Voplay renderer EngineSuspend");
                }
                this.#engineStates.set(key, "suspended");
                return;
            case 15 /* MessageKind.EngineResume */:
                if (current !== "suspended") {
                    throw new Error("invalid Voplay renderer EngineResume");
                }
                this.#engineStates.set(key, "running");
                return;
            case 16 /* MessageKind.EngineClose */:
                if (current === undefined) {
                    throw new Error("invalid Voplay renderer EngineClose");
                }
                this.#engineStates.delete(key);
                this.#renderFeatures.delete(key);
                await this.#replyLifecycle(packet, 17 /* MessageKind.EngineClosed */);
                return;
            default:
                throw new Error("unsupported Voplay renderer lifecycle packet");
        }
    }
    async #replyLifecycle(packet, kind) {
        const lane = this.#requireLane();
        await lane.submit(encodeFrameworkPacket({
            ...packet.header,
            kind,
        }, new Uint8Array()), packet.header.sequence);
    }
    async #applySurfaceControl(control) {
        this.#validateSession(control.session);
        const id = surfaceId(control);
        const key = surfaceKey(id);
        switch (control.action) {
            case "attach": {
                const existing = this.#surfaces.get(key);
                if (existing !== undefined) {
                    if (!sameHandle(existing.control.session, control.session)
                        || !sameHandle(existing.control.window, control.window)
                        || !sameHandle(existing.control.view, control.view)
                        || !sameHandle(existing.control.surface, control.surface)) {
                        throw new Error("Voplay browser Surface attach changed its App route");
                    }
                    if (this.#surfaceHost !== null) {
                        if (this.#surfaceHost.ownerSnapshot().deviceGeneration
                            !== control.deviceGeneration) {
                            await this.#surfaceHost.rebindDevice(id, control.metrics, control.deviceGeneration);
                        }
                        else {
                            this.#surfaceHost.resize(id, control.metrics);
                        }
                    }
                    existing.control = control;
                    break;
                }
                const firstSurface = this.#surfaces.size === 0;
                const capability = this.#requireSurfaceCapability();
                const route = await capability.resolve(control.surface);
                if (route.kind !== "game"
                    || !sameHandle(route.session, control.session)
                    || route.sessionEpoch !== BigInt(this.#requireLane().binding.sessionEpoch)
                    || !sameHandle(route.window, control.window)
                    || !sameHandle(route.view, control.view)
                    || !sameHandle(route.surface, control.surface)
                    || route.zOrder !== control.zOrder
                    || hostInputPolicy(route.inputPolicy) !== inputPolicy(control.inputPolicy)) {
                    throw new Error("Voplay SurfaceControl route does not match App Runtime authority");
                }
                const lease = capability.attach({
                    identity: {
                        sessionId: capability.sessionId,
                        session: control.session,
                        sessionEpoch: BigInt(this.#requireLane().binding.sessionEpoch),
                        window: control.window,
                        view: control.view,
                        surface: control.surface,
                    },
                    kind: "canvas",
                    layer: control.zOrder,
                    input: inputPolicy(control.inputPolicy),
                    label: `Voplay ${control.engine.index}:${control.engine.generation}`,
                });
                if (!(lease.element instanceof HTMLCanvasElement)) {
                    lease.release();
                    throw new Error("Voplay App Surface host returned a non-canvas element");
                }
                let attached = false;
                try {
                    const surfaceHost = await this.#ensureHostRenderSurfaceHost(control.deviceGeneration);
                    surfaceHost.attach(id, lease.element, control.metrics);
                    attached = true;
                    this.#surfaces.set(key, { control, id, lease });
                    await this.#ensureHostRenderSurfaceHost(control.deviceGeneration);
                }
                catch (error) {
                    if (attached) {
                        try {
                            this.#surfaceHost?.detach(id);
                        }
                        catch {
                            this.#surfaceHost?.close();
                            this.#surfaceHost = null;
                        }
                    }
                    this.#surfaces.delete(key);
                    lease.release();
                    throw error;
                }
                if (firstSurface)
                    this.#gamepads?.invalidate();
                break;
            }
            case "resize": {
                const current = this.#surface(key);
                assertSameControlOwner(current.record.control, control);
                current.host.resize(id, control.metrics);
                current.record.control = control;
                break;
            }
            case "suspend": {
                const current = this.#surface(key);
                assertSameControlOwner(current.record.control, control);
                current.host.suspend(id);
                current.record.control = control;
                break;
            }
            case "resume": {
                const current = this.#surface(key);
                assertSameControlOwner(current.record.control, control);
                current.host.resume(id);
                current.record.control = control;
                break;
            }
            case "rebind": {
                const current = this.#surface(key);
                assertSameControlOwner(current.record.control, control);
                await current.host.rebindDevice(id, control.metrics, control.deviceGeneration);
                current.record.control = control;
                break;
            }
            case "detach": {
                const current = this.#surface(key);
                assertSameControlOwner(current.record.control, control);
                if (this.#gamepadRouteSurface === key)
                    this.#gamepads?.invalidate();
                current.host.detach(current.record.id);
                current.record.lease.release();
                this.#surfaces.delete(key);
                if (this.#gamepadRouteSurface === key)
                    this.#gamepadRouteSurface = null;
                this.#removeSurfaceFrameTraces(current.record.id);
                if (this.#activeInputSurface === key)
                    this.#activeInputSurface = null;
                break;
            }
        }
    }
    async #renderFrame(bytes) {
        const lane = this.#requireLane();
        const decoded = decodeFrameSubmission(bytes);
        this.#validateSession(decoded.session);
        const record = this.#surface(surfaceKey(decoded.frame.surface)).record;
        if (!sameHandle(decoded.window, record.control.window)
            || !sameHandle(decoded.view, record.control.view)
            || !sameHandle(decoded.frame.renderEndpoint, record.control.renderEndpoint)
            || decoded.frame.deviceGeneration !== record.control.deviceGeneration) {
            throw new Error("Voplay frame route does not match the current SurfaceControl");
        }
        let terminal;
        let fenceValue = 0n;
        try {
            const submission = this.#requireSurfaceHost().submit(decoded.frame);
            fenceValue = submission.fenceValue;
            terminal = this.#requireSurfaceHost().present(record.id, decoded.frame.frameId, submission.fenceValue, monotonicMicros(), decoded.deadlineMicros);
        }
        catch (error) {
            terminal = classifyFrameFailure(error, fenceValue !== 0n);
        }
        const sequence = this.#takeReturnSequence();
        const packet = encodeFrameOutcome({
            terminal,
            session: decoded.session,
            frame: decoded.frame,
            renderedRevision: decoded.frame.requiredRenderRevision,
            observedControlRevision: decoded.frame.requiredControlRevision,
            fenceValue,
            completionMicros: monotonicMicros(),
            channelEpoch: decoded.header.channelEpoch,
            sequence,
        });
        await lane.submit(packet, decoded.frame.frameId);
    }
    async #applyRenderAsset(bytes) {
        const packet = decodeFrameworkPacket(bytes);
        const payload = packet.payload;
        const magic = payload.byteLength >= 4
            ? String.fromCharCode(payload[0], payload[1], payload[2], payload[3])
            : "";
        if (magic === "VRT1") {
            await this.#applyTextureAsset(packet);
            return;
        }
        if (magic === "VRA1") {
            await this.#applyProfileAsset(packet);
            return;
        }
        throw new Error("unsupported Voplay browser render asset");
    }
    async #applyTextureAsset(packet) {
        const payload = packet.payload;
        const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
        const action = payload[4];
        const texture = takeAssetU64(view, 5);
        const revision = takeAssetU64(view, 13);
        const textureKey = engineResourceKey(packet.header.engine, texture);
        const current = this.#textures.get(textureKey);
        const currentRevision = this.#textureRevisions.get(textureKey);
        if (texture <= 0n
            || revision <= 0n
            || packet.header.commitId !== texture
            || packet.header.newRevision !== revision
            || (currentRevision !== undefined && currentRevision >= revision)) {
            throw new Error("stale Voplay browser texture asset");
        }
        if (action === 1) {
            if (payload.byteLength < 33)
                throw new Error("truncated Voplay browser texture asset");
            const width = view.getUint32(21, true);
            const height = view.getUint32(25, true);
            const byteLength = view.getUint32(29, true);
            const expected = width * height * 4;
            if (width === 0
                || height === 0
                || !Number.isSafeInteger(expected)
                || byteLength !== expected
                || payload.byteLength !== 33 + byteLength
                || (current === undefined && this.#textures.size >= MAX_TEXTURES)
                || (this.#textureBytes
                    - (current === undefined ? 0 : current.source.width * current.source.height * 4)
                    + byteLength > MAX_TEXTURE_BYTES)) {
                throw new Error("invalid Voplay browser texture dimensions");
            }
            const rgba = new Uint8ClampedArray(byteLength);
            rgba.set(payload.subarray(33));
            const source = await createImageBitmap(new ImageData(rgba, width, height));
            const previous = this.#textures.get(textureKey);
            try {
                this.#surfaceHost?.registerTexture(texture, source, packet.header.engine);
            }
            catch (error) {
                source.close();
                throw error;
            }
            this.#textures.set(textureKey, {
                engine: packet.header.engine,
                texture,
                revision,
                source,
            });
            this.#textureBytes = this.#textureBytes
                - (previous === undefined ? 0 : previous.source.width * previous.source.height * 4)
                + byteLength;
            previous?.source.close();
        }
        else if (action === 2 && payload.byteLength === 21) {
            const previous = this.#textures.get(textureKey);
            if (previous !== undefined) {
                this.#surfaceHost?.removeTexture(texture, packet.header.engine);
                this.#textures.delete(textureKey);
                this.#textureBytes -= previous.source.width * previous.source.height * 4;
                previous.source.close();
            }
        }
        else {
            throw new Error("unsupported Voplay browser texture action");
        }
        this.#textureRevisions.set(textureKey, revision);
        await this.#submitRenderAssetAck(packet, 1, texture, revision);
    }
    async #applyProfileAsset(packet) {
        const payload = packet.payload;
        if (payload.byteLength < 29)
            throw new Error("truncated Voplay browser profile asset");
        const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
        const action = payload[4];
        const kind = view.getUint32(5, true);
        const asset = view.getBigUint64(9, true);
        const revision = view.getBigUint64(17, true);
        const byteLength = view.getUint32(25, true);
        const key = `${engineResourceKey(packet.header.engine, asset)}/${kind}`;
        const current = this.#profileAssets.get(key);
        const currentRevision = this.#profileAssetRevisions.get(key);
        if (kind < 2
            || asset <= 0n
            || revision <= 0n
            || packet.header.commitId !== asset
            || packet.header.newRevision !== revision
            || (currentRevision !== undefined && currentRevision >= revision)) {
            throw new Error("stale Voplay browser profile asset");
        }
        if (action === 1 && byteLength > 0 && payload.byteLength === 29 + byteLength) {
            const nextBytes = this.#profileAssetBytes
                - (current?.bytes.byteLength ?? 0)
                + byteLength;
            if ((current === undefined && this.#profileAssets.size >= MAX_PROFILE_ASSETS)
                || nextBytes > MAX_PROFILE_ASSET_BYTES) {
                throw new Error("Voplay browser profile asset capacity exceeded");
            }
            this.#profileAssets.set(key, {
                engine: packet.header.engine,
                kind,
                asset,
                revision,
                bytes: payload.slice(29),
            });
            this.#profileAssetBytes = nextBytes;
        }
        else if (action === 2 && byteLength === 0 && payload.byteLength === 29) {
            if (current !== undefined) {
                this.#profileAssets.delete(key);
                this.#profileAssetBytes -= current.bytes.byteLength;
            }
        }
        else {
            throw new Error("unsupported Voplay browser profile asset action");
        }
        this.#profileAssetRevisions.set(key, revision);
        await this.#submitRenderAssetAck(packet, kind, asset, revision);
    }
    async #submitRenderAssetAck(packet, kind, asset, revision) {
        const ackPayload = new Uint8Array(4);
        new DataView(ackPayload.buffer).setUint32(0, kind, true);
        await this.#requireLane().submit(encodeFrameworkPacket({
            kind: 38 /* MessageKind.RenderAssetAck */,
            engine: packet.header.engine,
            channelEpoch: packet.header.channelEpoch,
            commitId: asset,
            baseRevision: 0n,
            newRevision: revision,
            requiredControlRevision: 0n,
            sourceSimulationRevision: packet.header.sourceSimulationRevision,
            sequence: packet.header.sequence,
        }, ackPayload), packet.header.sequence);
    }
    #acceptPlatformInput(event) {
        try {
            const record = this.#recordForInput(event);
            const capability = this.#requireSurfaceCapability();
            if (event.type === "pointerDown" && record.control.inputPolicy !== 3) {
                this.#activateInputSurface(surfaceKey(record.id));
                capability.focus(event.surface);
                if (event.pointerId !== undefined) {
                    capability.capturePointer(event.pointerId, event.surface);
                }
            }
            else if ((event.type === "pointerUp" || event.type === "pointerCancel")
                && event.pointerId !== undefined) {
                capability.releasePointer(event.pointerId);
            }
            else if (event.type === "focus" && event.focused) {
                this.#activateInputSurface(surfaceKey(record.id));
            }
            const key = coalescedInputKey(event);
            if (key === null) {
                if (this.#pendingInputReturns >= MAX_PENDING_INPUT_RETURNS) {
                    throw new Error("Voplay reliable platform input return capacity exceeded");
                }
                void this.#submitPlatformInput(event, record.control).catch((error) => {
                    this.#failPlatformInput(error);
                });
                return;
            }
            if (!this.#coalescedInputs.has(key)
                && this.#coalescedInputs.size >= MAX_COALESCED_INPUTS) {
                throw new Error("Voplay coalesced platform input capacity exceeded");
            }
            this.#coalescedInputs.set(key, event);
            if (!this.#drainingCoalesced.has(key)) {
                this.#drainingCoalesced.add(key);
                void this.#drainCoalescedInput(key).catch((error) => {
                    this.#failPlatformInput(error);
                });
            }
        }
        catch (error) {
            this.#failPlatformInput(error);
        }
    }
    async #drainCoalescedInput(key) {
        try {
            for (;;) {
                const event = this.#coalescedInputs.get(key);
                if (event === undefined)
                    return;
                this.#coalescedInputs.delete(key);
                const record = this.#recordForInput(event);
                await this.#submitPlatformInput(event, record.control);
            }
        }
        finally {
            this.#drainingCoalesced.delete(key);
            if (this.#coalescedInputs.has(key) && this.#polling) {
                this.#drainingCoalesced.add(key);
                void this.#drainCoalescedInput(key).catch((error) => {
                    this.#failPlatformInput(error);
                });
            }
        }
    }
    async #submitPlatformInput(event, control) {
        if (this.#pendingInputReturns >= MAX_PENDING_INPUT_RETURNS) {
            throw new Error("Voplay platform input return capacity exceeded");
        }
        const lane = this.#requireLane();
        const routedEvent = {
            ...event,
            sequence: this.#takeInputEventSequence(),
        };
        const packet = encodePlatformInput(routedEvent, control, this.#takeReturnSequence());
        this.#pendingInputReturns += 1;
        try {
            await lane.submit(packet);
        }
        finally {
            this.#pendingInputReturns -= 1;
        }
    }
    #acceptGamepadInput(event) {
        if (!this.#polling)
            return;
        if (event.type === "gamepadDisconnect") {
            this.#haptics?.disconnectGamepad(event.gamepadIndex, event.gamepadGeneration);
        }
        const record = (this.#activeInputSurface === null
            ? this.#gamepadRouteSurface === null
                ? undefined
                : this.#surfaces.get(this.#gamepadRouteSurface)
            : this.#surfaces.get(this.#activeInputSurface)) ?? this.#surfaces.values().next().value;
        if (record === undefined)
            return;
        this.#gamepadRouteSurface = surfaceKey(record.id);
        const lane = this.#requireLane();
        const capability = this.#requireSurfaceCapability();
        this.#acceptPlatformInput({
            ...event,
            sequence: 0n,
            surface: {
                sessionId: capability.sessionId,
                session: record.control.session,
                sessionEpoch: BigInt(lane.binding.sessionEpoch),
                window: record.control.window,
                view: record.control.view,
                surface: record.control.surface,
            },
        });
    }
    #recordForInput(event) {
        for (const record of this.#surfaces.values()) {
            if (sameHandle(record.control.session, event.surface.session)
                && sameHandle(record.control.window, event.surface.window)
                && sameHandle(record.control.view, event.surface.view)
                && sameHandle(record.control.surface, event.surface.surface)) {
                return record;
            }
        }
        throw new Error("Voplay platform input targets an unknown Surface");
    }
    #activateInputSurface(key) {
        if (this.#gamepadRouteSurface !== null && this.#gamepadRouteSurface !== key) {
            this.#gamepads?.invalidate();
        }
        this.#activeInputSurface = key;
    }
    #takeReturnSequence() {
        const sequence = this.#nextReturnSequence;
        if (sequence === 0xffffffffffffffffn) {
            throw new Error("Voplay framework return sequence exhausted");
        }
        this.#nextReturnSequence = sequence + 1n;
        return sequence;
    }
    #takeInputEventSequence() {
        const sequence = this.#nextInputEventSequence;
        if (sequence === 0xffffffffffffffffn) {
            throw new Error("Voplay platform input sequence exhausted");
        }
        this.#nextInputEventSequence = sequence + 1n;
        return sequence;
    }
    async #submitHapticsResult(result) {
        const payload = new Uint8Array(17);
        const view = new DataView(payload.buffer);
        view.setBigUint64(0, result.requestId, true);
        view.setUint32(8, result.device.index, true);
        view.setUint32(12, result.device.generation, true);
        view.setUint8(16, hapticsOutcomeTag(result.outcome));
        const header = result.commandHeader;
        const sequence = this.#takeReturnSequence();
        await this.#requireLane().submit(encodeFrameworkPacket({
            kind: 35 /* MessageKind.HapticsResult */,
            engine: header.engine,
            channelEpoch: header.channelEpoch,
            commitId: header.commitId,
            baseRevision: header.baseRevision,
            newRevision: header.newRevision,
            requiredControlRevision: header.requiredControlRevision,
            sourceSimulationRevision: header.sourceSimulationRevision,
            sequence,
        }, payload), result.requestId);
    }
    #failPlatformInput(error) {
        if (!this.#polling)
            return;
        this.#polling = false;
        this.#host?.reportError(`Voplay platform input lane failed: ${errorMessage(error)}`);
    }
    #surface(key) {
        const record = this.#surfaces.get(key);
        if (record === undefined)
            throw new Error("unknown Voplay browser Surface");
        return { record, host: this.#requireSurfaceHost() };
    }
    #validateSession(session) {
        const binding = this.#requireLane().binding.session;
        if (session.index !== binding.index || session.generation !== binding.generation) {
            throw new Error("Voplay packet App Session mismatch");
        }
    }
    #requireLane() {
        if (this.#lane === null)
            throw new Error("Voplay framework lane is closed");
        return this.#lane;
    }
    #requireSurfaceCapability() {
        if (this.#surfaceCapability === null)
            throw new Error("Voplay App Surface capability is closed");
        return this.#surfaceCapability;
    }
    #removeSurfaceFrameTraces(surface) {
        for (const [key, trace] of this.#frameTraces) {
            if (sameHandle(trace.engine, surface.engine.engine)
                && sameHandle(trace.target, surface.surface)) {
                this.#frameTraces.delete(key);
            }
        }
    }
    #requireSurfaceHost() {
        if (this.#surfaceHost === null)
            throw new Error("Voplay browser device is unavailable");
        return this.#surfaceHost;
    }
    #requireHaptics() {
        if (this.#haptics === null)
            throw new Error("Voplay browser haptics host is unavailable");
        return this.#haptics;
    }
}
function surfaceId(control) {
    return {
        engine: { session: control.session, engine: control.engine },
        surface: control.surface,
        domain: control.domain,
    };
}
function inputPolicy(value) {
    switch (value) {
        case 1:
            return "opaque";
        case 2:
            return "transparent";
        case 3:
            return "passthrough";
        default:
            throw new RangeError("invalid Voplay Surface input policy");
    }
}
function hostInputPolicy(value) {
    switch (value) {
        case "observe":
            return "transparent";
        case "passthrough":
            return "passthrough";
        case "interactive":
        case "exclusive":
            return "opaque";
    }
}
function browserInputPolicyTag(value) {
    switch (value) {
        case "interactive":
        case "exclusive":
            return 1;
        case "observe":
            return 2;
        case "passthrough":
            return 3;
    }
}
function assertSameControlOwner(previous, next) {
    if (!sameHandle(previous.engine, next.engine)
        || !sameHandle(previous.session, next.session)
        || !sameHandle(previous.window, next.window)
        || !sameHandle(previous.view, next.view)
        || !sameHandle(previous.surface, next.surface)
        || !sameHandle(previous.domain, next.domain)
        || previous.zOrder !== next.zOrder
        || previous.inputPolicy !== next.inputPolicy) {
        throw new Error("Voplay SurfaceControl owner route changed");
    }
}
function sameHandle(left, right) {
    return left.index === right.index && left.generation === right.generation;
}
function handleKey(handle) {
    return `${handle.index}:${handle.generation}`;
}
function engineResourceKey(engine, resource) {
    return `${handleKey(engine)}/${resource}`;
}
function coalescedInputKey(event) {
    if (event.type !== "pointerMove"
        && event.type !== "wheel"
        && event.type !== "gamepadAxis")
        return null;
    return [
        event.type,
        `${event.surface.surface.index}:${event.surface.surface.generation}`,
        event.type === "gamepadAxis"
            ? `${event.gamepadIndex ?? -1}:${event.gamepadGeneration ?? 0}:${event.gamepadControl ?? -1}`
            : event.pointerId ?? 0,
    ].join("/");
}
function classifyFrameFailure(error, submitted) {
    const message = errorMessage(error);
    if (message.includes("device") || message.includes("GPU generation"))
        return "deviceLost";
    if (message.includes("unknown surface") || message.includes("Surface"))
        return "surfaceLost";
    return submitted ? "outcomeUnknown" : "rejectedBeforeSubmit";
}
function monotonicMicros() {
    return BigInt(Math.max(0, Math.floor(performance.now() * 1000)));
}
function takeAssetU64(view, offset) {
    if (offset < 0 || offset + 8 > view.byteLength) {
        throw new Error("truncated Voplay browser texture identity");
    }
    return view.getBigUint64(offset, true);
}
class HostRenderReader {
    #bytes;
    #view;
    #offset = 0;
    constructor(bytes) {
        if (bytes.byteLength > MAX_COMMAND_BYTES) {
            throw new Error("Voplay host-render command exceeds browser limit");
        }
        this.#bytes = bytes;
        this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    }
    magic(expected) {
        const bytes = this.take(4);
        if (String.fromCharCode(...bytes) !== expected) {
            throw new Error(`invalid Voplay host-render ${expected} envelope`);
        }
    }
    u8() {
        return this.take(1)[0];
    }
    u32() {
        const offset = this.#reserve(4);
        return this.#view.getUint32(offset, true);
    }
    u64() {
        const offset = this.#reserve(8);
        return this.#view.getBigUint64(offset, true);
    }
    handle() {
        const handle = { index: this.u32(), generation: this.u32() };
        if (handle.generation === 0 || handle.index === 0xffff_ffff) {
            throw new Error("invalid Voplay host-render handle");
        }
        return handle;
    }
    surface(routeEngine, session) {
        const engine = this.handle();
        const surface = this.handle();
        const domainEngine = this.handle();
        const domain = this.handle();
        if (!sameHandle(engine, routeEngine) || !sameHandle(domainEngine, routeEngine)) {
            throw new Error("Voplay host-render Surface route mismatch");
        }
        return { engine: { session, engine }, surface, domain };
    }
    metrics() {
        const metrics = {
            width: this.u32(),
            height: this.u32(),
            scaleNumerator: this.u32(),
            scaleDenominator: this.u32(),
        };
        if (metrics.width <= 0
            || metrics.height <= 0
            || metrics.scaleNumerator <= 0
            || metrics.scaleDenominator <= 0) {
            throw new Error("invalid Voplay host-render Surface metrics");
        }
        return metrics;
    }
    blob() {
        return this.take(this.u32()).slice();
    }
    frame(routeEngine, session) {
        return {
            surface: this.surface(routeEngine, session),
            pulseId: this.u64(),
            frameId: this.u64(),
            deviceGeneration: this.u64(),
            requiredRenderRevision: this.u64(),
            requiredControlRevision: this.u64(),
            graphSignature: this.u64(),
            renderEndpoint: this.handle(),
            commands: this.blob(),
        };
    }
    frameAck(routeEngine, session) {
        return {
            surface: this.surface(routeEngine, session),
            pulseId: this.u64(),
            frameId: this.u64(),
            deviceGeneration: this.u64(),
            renderedRevision: this.u64(),
            observedControlRevision: this.u64(),
            renderEndpoint: this.handle(),
        };
    }
    readbackRequest(routeEngine) {
        const engine = this.handle();
        const target = this.handle();
        const expectedRevision = this.u64();
        const region = {
            x: this.u32(),
            y: this.u32(),
            width: this.u32(),
            height: this.u32(),
        };
        const format = this.u32();
        if (!sameHandle(engine, routeEngine)
            || expectedRevision <= 0n
            || region.width <= 0
            || region.height <= 0
            || format < 1
            || format > 4) {
            throw new Error("invalid Voplay host-render readback request");
        }
        return { target, expectedRevision, region, format };
    }
    frameTraceRequest() {
        const frameId = this.u64();
        const graphSignature = this.u64();
        const flags = this.u8();
        const maxBytes = this.u32();
        if (frameId <= 0n
            || graphSignature <= 0n
            || (flags & ~0x03) !== 0
            || maxBytes <= 0
            || maxBytes > MAX_COMMAND_BYTES) {
            throw new Error("invalid Voplay host-render frame trace request");
        }
        return {
            frameId,
            graphSignature,
            includeAttachments: (flags & 1) !== 0,
            includeShaderDiagnostics: (flags & 2) !== 0,
            maxBytes,
        };
    }
    take(length) {
        const offset = this.#reserve(length);
        return this.#bytes.subarray(offset, offset + length);
    }
    finish() {
        if (this.#offset !== this.#bytes.byteLength) {
            throw new Error("Voplay host-render command has trailing bytes");
        }
    }
    #reserve(length) {
        if (!Number.isSafeInteger(length) || length < 0) {
            throw new Error("invalid Voplay host-render field length");
        }
        const offset = this.#offset;
        const end = offset + length;
        if (!Number.isSafeInteger(end) || end > this.#bytes.byteLength) {
            throw new Error("truncated Voplay host-render command");
        }
        this.#offset = end;
        return offset;
    }
}
function hostFrameKey(surface, frameId) {
    return `${surfaceKey(surface)}:${frameId}`;
}
function encodeHostRenderFailure(engine, request, failure, frameTrace) {
    if (request <= 0n || failure < 1 || failure > 4) {
        throw new Error("invalid Voplay host-render failure");
    }
    const bytes = new Uint8Array(4 + 8 + 4 + 1 + 8 + 1);
    bytes.set([0x56, 0x48, 0x52, 0x34], 0);
    const view = new DataView(bytes.buffer);
    view.setUint32(4, engine.index, true);
    view.setUint32(8, engine.generation, true);
    bytes.set([0x56, 0x48, 0x52, 0x32], 12);
    bytes[16] = frameTrace ? 4 : 2;
    view.setBigUint64(17, request, true);
    bytes[25] = failure;
    return bytes;
}
function encodeHostRenderReadback(engine, request, target, targetRevision, rowBytes, payload) {
    if (request <= 0n
        || targetRevision <= 0n
        || !Number.isSafeInteger(rowBytes)
        || rowBytes <= 0
        || payload.byteLength === 0
        || payload.byteLength > MAX_COMMAND_BYTES) {
        throw new Error("invalid Voplay host-render readback result");
    }
    const bytes = new Uint8Array(57 + payload.byteLength);
    const view = new DataView(bytes.buffer);
    bytes.set([0x56, 0x48, 0x52, 0x34], 0);
    view.setUint32(4, engine.index, true);
    view.setUint32(8, engine.generation, true);
    bytes.set([0x56, 0x48, 0x52, 0x32], 12);
    bytes[16] = 1;
    view.setBigUint64(17, request, true);
    view.setUint32(25, engine.index, true);
    view.setUint32(29, engine.generation, true);
    view.setUint32(33, target.index, true);
    view.setUint32(37, target.generation, true);
    view.setBigUint64(41, targetRevision, true);
    view.setUint32(49, rowBytes, true);
    view.setUint32(53, payload.byteLength, true);
    bytes.set(payload, 57);
    return bytes;
}
function encodeBrowserFrameTrace(trace) {
    const label = new TextEncoder().encode("browser-portable");
    const bytes = new Uint8Array(128 + label.byteLength);
    const view = new DataView(bytes.buffer);
    bytes.set([0x56, 0x47, 0x54, 0x31, 1, 0, 0, 0], 0);
    view.setBigUint64(8, trace.frameId, true);
    view.setBigUint64(16, trace.graphSignature, true);
    view.setUint32(24, 1, true);
    view.setUint32(28, trace.engine.index, true);
    view.setUint32(32, trace.engine.generation, true);
    bytes.set([2, 0, 0, 0], 36);
    view.setUint32(40, trace.target.index, true);
    view.setUint32(44, trace.target.generation, true);
    view.setUint32(48, 0, true);
    view.setUint32(52, 0, true);
    view.setUint32(56, trace.width, true);
    view.setUint32(60, trace.height, true);
    view.setBigUint64(64, trace.fence, true);
    view.setUint32(72, 1, true);
    view.setUint32(76, 0, true);
    view.setUint32(80, 0, true);
    view.setUint32(84, 1, true);
    bytes.set([1, 0, 0, 0], 88);
    view.setBigUint64(92, 0n, true);
    view.setBigUint64(100, 0n, true);
    view.setBigUint64(108, trace.graphSignature, true);
    view.setUint32(116, 1, true);
    view.setUint32(120, 0, true);
    view.setUint32(124, label.byteLength, true);
    bytes.set(label, 128);
    return bytes;
}
function encodeHostRenderFrameTrace(engine, request, frameId, graphSignature, trace) {
    if (request <= 0n
        || frameId <= 0n
        || graphSignature <= 0n
        || trace.byteLength === 0
        || trace.byteLength > MAX_COMMAND_BYTES) {
        throw new Error("invalid Voplay host-render frame trace result");
    }
    const bytes = new Uint8Array(45 + trace.byteLength);
    const view = new DataView(bytes.buffer);
    bytes.set([0x56, 0x48, 0x52, 0x34], 0);
    view.setUint32(4, engine.index, true);
    view.setUint32(8, engine.generation, true);
    bytes.set([0x56, 0x48, 0x52, 0x32], 12);
    bytes[16] = 3;
    view.setBigUint64(17, request, true);
    view.setBigUint64(25, frameId, true);
    view.setBigUint64(33, graphSignature, true);
    view.setUint32(41, trace.byteLength, true);
    bytes.set(trace, 45);
    return bytes;
}
function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}
function isHostRenderCommand(bytes) {
    return bytes.byteLength >= 4
        && bytes[0] === 0x56
        && bytes[1] === 0x48
        && bytes[2] === 0x52
        && (bytes[3] === 0x31 || bytes[3] === 0x33);
}
function isRenderFeatureBootstrap(bytes) {
    return bytes.byteLength >= 8
        && bytes[0] === 0x56
        && bytes[1] === 0x46
        && bytes[2] === 0x52
        && bytes[3] === 0x42
        && bytes[4] === 0x32
        && bytes[5] === 0
        && bytes[6] === 0
        && bytes[7] === 0;
}
function hexPrefix(bytes, limit) {
    return Array.from(bytes.subarray(0, limit))
        .map((value) => value.toString(16).padStart(2, "0"))
        .join("");
}
function hapticsOutcomeTag(outcome) {
    switch (outcome) {
        case "succeeded": return 1;
        case "unsupported": return 2;
        case "cancelled": return 3;
        case "deviceLost": return 4;
        case "failed": return 5;
    }
}
function delay(milliseconds) {
    return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
export default new VoplayStudioRenderer();
