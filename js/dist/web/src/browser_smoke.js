import { decodeFrameworkPacket, encodeFrameworkPacket, } from "../../protocol/generated/voplay_protocol.js";
import { VoplayBrowserAssetProvider } from "./asset_provider.js";
import { VoplayBrowserAudioProvider } from "./audio_provider.js";
import { Canvas2dGpuAdapter } from "./canvas2d_adapter.js";
import { BROWSER_GAMEPAD_DEVICE_BASE } from "./framework_lane.js";
import { BrowserHapticsHost } from "./haptics.js";
import { BrowserSurfaceHost, } from "./platform_surface.js";
import { WebGpuCanvasAdapter } from "./webgpu_adapter.js";
const cases = [];
const results = requireElement("results", HTMLPreElement);
const canvas = requireElement("surface", HTMLCanvasElement);
const peerCanvas = requireElement("peer-surface", HTMLCanvasElement);
async function main() {
    await runCase("WebGPU two-session three-frame present and isolation", () => smokeWebGpu(canvas, peerCanvas));
    await runCase("surface lifecycle and Canvas2D pixels", () => smokeSurface(document.createElement("canvas")));
    await runCase("browser haptics terminal outcomes", smokeHaptics);
    await runCase("browser provider stop and restart", smokeProviderRestart);
    await runCase("browser asset provider lifecycle", smokeAssetProviderLifecycle);
    await runCase("browser audio user-gesture activation", smokeAudioGesture);
    const report = {
        complete: true,
        passed: cases.every((entry) => entry.passed),
        cases,
    };
    window.__voplayBrowserSmoke = report;
    results.textContent = JSON.stringify(report, null, 2);
    document.documentElement.dataset.smoke = report.passed ? "passed" : "failed";
}
async function runCase(name, test) {
    try {
        await withTimeout(test(), 15_000, name);
        cases.push({ name, passed: true, detail: "ok" });
    }
    catch (error) {
        cases.push({
            name,
            passed: false,
            detail: error instanceof Error ? `${error.name}: ${error.message}` : String(error),
        });
    }
    results.textContent = JSON.stringify({
        complete: false,
        passed: cases.every((entry) => entry.passed),
        cases,
    }, null, 2);
}
async function withTimeout(operation, timeoutMilliseconds, label) {
    let timeout = 0;
    try {
        return await Promise.race([
            Promise.resolve(operation),
            new Promise((_, reject) => {
                timeout = window.setTimeout(() => reject(new Error(`${label} timed out after ${timeoutMilliseconds}ms`)), timeoutMilliseconds);
            }),
        ]);
    }
    finally {
        window.clearTimeout(timeout);
    }
}
async function smokeWebGpu(target, peerTarget) {
    const adapter = await WebGpuCanvasAdapter.create({
        deviceGeneration: 1n,
        maxCommands: 8,
        maxCommandBytes: 1024,
    });
    const host = new BrowserSurfaceHost(adapter, {
        maxSurfaces: 2,
        maxCommandBytes: 1024,
    });
    const surface = smokeSurfaceId(1);
    const peerSurface = smokeSurfaceId(11);
    const metrics = {
        width: 64,
        height: 48,
        scaleNumerator: 2,
        scaleDenominator: 1,
    };
    try {
        host.attach(surface, target, metrics);
        host.attach(peerSurface, peerTarget, metrics);
        for (let pulse = 1n; pulse <= 3n; pulse += 1n) {
            const submission = host.submit(frame(surface, pulse, pulse, 1n, portableFrame([1, Number(12n * pulse), 18, 28, 255], rectangleCommand(8, 8, 24, 16, [40, 220, 90, 255]))));
            assert(host.present(surface, pulse, submission.fenceValue, 90n, 100n) === "presented", `WebGPU pulse ${pulse} presented`);
            const peerSubmission = host.submit(frame(peerSurface, pulse, pulse, 1n, portableFrame([1, 8, Number(16n * pulse), 32, 255], rectangleCommand(12, 10, 20, 12, [230, 140, 30, 255]))));
            assert(host.present(peerSurface, pulse, peerSubmission.fenceValue, 90n, 100n) === "presented", `peer WebGPU pulse ${pulse} presented`);
        }
        host.suspend(surface);
        host.resize(peerSurface, { ...metrics, width: 72 });
        const peerWhilePrimarySuspended = host.submit(frame(peerSurface, 4n, 4n, 1n, portableFrame([1, 6, 12, 18, 255])));
        assert(host.present(peerSurface, 4n, peerWhilePrimarySuspended.fenceValue, 90n, 100n) === "presented", "peer session presents while primary is suspended");
        assert(peerTarget.width === 72, "peer resize remains isolated");
        assert(target.width === 64, "primary dimensions remain isolated");
        host.resume(surface);
        const interrupted = host.submit(frame(surface, 4n, 4n, 1n, portableFrame([1, 60, 10, 20, 255])));
        await adapter.triggerControlledDeviceLoss();
        expectThrow(() => host.submit(frame(peerSurface, 5n, 5n, 1n, portableFrame([1, 0, 0, 0, 255]))), "WebGPU device lost");
        await adapter.rebindDevice(2n);
        assert(host.present(surface, 4n, interrupted.fenceValue, 90n, 100n) === "deviceLost", "pending old-generation frame receives deviceLost terminal");
        await host.rebindDevice(surface, metrics, 2n);
        await host.rebindDevice(peerSurface, { ...metrics, width: 72 }, 2n);
        const recovered = host.submit(frame(surface, 5n, 5n, 2n, portableFrame([1, 4, 8, 12, 255])));
        assert(host.present(surface, 5n, recovered.fenceValue, 90n, 100n) === "presented", "replacement WebGPU device presents");
        host.detach(surface);
        host.detach(peerSurface);
        assert(host.close() === 0, "clean WebGPU surface close");
    }
    finally {
        adapter.close();
    }
}
function smokeSurface(target) {
    const adapter = new Canvas2dGpuAdapter({
        deviceGeneration: 1n,
        maxCommands: 8,
        maxCommandBytes: 1024,
    });
    const host = new BrowserSurfaceHost(adapter, {
        maxSurfaces: 1,
        maxCommandBytes: 1024,
    });
    const surface = smokeSurfaceId();
    const metrics = {
        width: 64,
        height: 48,
        scaleNumerator: 2,
        scaleDenominator: 1,
    };
    host.attach(surface, target, metrics);
    assert(target.width === 64 && target.height === 48, "canvas backing dimensions");
    assert(target.style.width === "32px" && target.style.height === "24px", "canvas logical size");
    const first = host.submit(frame(surface, 1n, 1n, 1n, portableFrame([1, 12, 18, 28, 255], rectangleCommand(8, 8, 24, 16, [40, 220, 90, 255]))));
    assert(first.fenceValue === 1n, "first fence identity");
    assert(host.present(surface, 1n, first.fenceValue, 90n, 100n) === "presented", "presented");
    const context = target.getContext("2d");
    assert(context !== null, "Canvas2D context");
    assertPixel(context.getImageData(10, 10, 1, 1).data, [40, 220, 90, 255], "rectangle pixel");
    assertPixel(context.getImageData(2, 2, 1, 1).data, [12, 18, 28, 255], "clear pixel");
    const second = host.submit(frame(surface, 2n, 2n, 1n, portableFrame([1, 70, 80, 90, 255])));
    assert(host.present(surface, 2n, second.fenceValue, 101n, 100n) === "deadlineMissed", "deadline terminal");
    host.suspend(surface);
    expectThrow(() => host.submit(frame(surface, 3n, 3n, 1n, portableFrame([1, 0, 0, 0, 255]))), "surface unavailable");
    host.resume(surface);
    expectThrow(() => host.submit(frame(surface, 3n, 3n, 2n, portableFrame([1, 0, 0, 0, 255]))), "invalid frame submission");
    const lost = host.submit(frame(surface, 3n, 3n, 1n, portableFrame([1, 120, 20, 30, 255])));
    adapter.rebindDevice(2n);
    assert(host.present(surface, 3n, lost.fenceValue, 100n, 100n) === "deviceLost", "device-loss terminal");
    host.rebindDevice(surface, { ...metrics, width: 80 }, 2n);
    assert(adapter.deviceGeneration === 2n && Number(target.width) === 80, "device rebind");
    const recovered = host.submit(frame(surface, 4n, 4n, 2n, portableFrame([1, 3, 6, 9, 255])));
    assert(recovered.fenceValue === 1n, "rebound fence restarts");
    assert(host.present(surface, 4n, recovered.fenceValue, 100n, 100n) === "presented", "rebound present");
    expectThrow(() => adapter.submit(target, new Uint8Array([...portableFrame([1, 0, 0, 0, 255]), 0]), 1n, surface.engine.engine, 1n), "portable frame trailing bytes");
    host.detach(surface);
    assert(host.close() === 0, "clean surface close");
    expectThrow(() => host.resize(surface, metrics), "browser surface host closed");
    const abandonedAdapter = new Canvas2dGpuAdapter({
        deviceGeneration: 1n,
        maxCommands: 8,
        maxCommandBytes: 1024,
    });
    const abandonedHost = new BrowserSurfaceHost(abandonedAdapter, {
        maxSurfaces: 1,
        maxCommandBytes: 1024,
    });
    abandonedHost.attach(surface, target, metrics);
    abandonedHost.submit(frame(surface, 1n, 1n, 1n, portableFrame([1, 9, 8, 7, 255])));
    assert(abandonedHost.close() === 1, "pending frame is explicitly abandoned");
    assert(abandonedHost.close() === 0, "abandon close is idempotent");
    expectThrow(() => abandonedAdapter.detach(target), "Canvas2D adapter closed");
}
async function smokeHaptics() {
    const original = Object.getOwnPropertyDescriptor(Navigator.prototype, "getGamepads");
    const outcomes = [];
    let gamepads = [];
    Object.defineProperty(Navigator.prototype, "getGamepads", {
        configurable: true,
        value: () => gamepads,
    });
    try {
        const host = new BrowserHapticsHost((result) => outcomes.push(result.outcome), () => 7);
        gamepads = [gamepad(0)];
        host.accept(rumblePacket(1n, 7));
        assert(outcomes.shift() === "unsupported", "unsupported terminal");
        gamepads = [gamepad(0, { playEffect: async () => "complete" })];
        host.accept(rumblePacket(2n, 7));
        await nextMicrotask();
        assert(outcomes.shift() === "succeeded", "success terminal");
        let resetCount = 0;
        let releasePending;
        gamepads = [gamepad(0, {
                playEffect: () => new Promise((resolve) => {
                    releasePending = () => resolve("complete");
                }),
                reset: async () => {
                    resetCount += 1;
                    return "complete";
                },
            })];
        host.accept(rumblePacket(3n, 7));
        host.accept(cancelPacket(3n, 7));
        assert(outcomes.shift() === "cancelled" && resetCount === 1, "cancel terminal");
        releasePending?.();
        await nextMicrotask();
        assert(outcomes.length === 0, "cancel settles exactly once");
        let releaseDisconnected;
        gamepads = [gamepad(0, {
                playEffect: () => new Promise((resolve) => {
                    releaseDisconnected = () => resolve("complete");
                }),
            })];
        host.accept(rumblePacket(4n, 7));
        const disconnect = new Event("gamepaddisconnected");
        Object.defineProperty(disconnect, "gamepad", { value: gamepads[0] });
        window.dispatchEvent(disconnect);
        assert(outcomes.shift() === "deviceLost", "disconnect terminal");
        releaseDisconnected?.();
        await nextMicrotask();
        assert(outcomes.length === 0, "disconnect settles exactly once");
        host.accept(rumblePacket(5n, 8));
        assert(outcomes.shift() === "deviceLost", "generation loss terminal");
        gamepads = [gamepad(0, { playEffect: async () => Promise.reject(new Error("actuator lost")) })];
        host.accept(rumblePacket(6n, 7));
        await nextMicrotask();
        await nextMicrotask();
        assert(outcomes.shift() === "failed", "actuator failure terminal");
        host.close();
        expectThrow(() => host.accept(rumblePacket(7n, 7)), "browser haptics host is closed");
    }
    finally {
        if (original === undefined) {
            delete Navigator.prototype.getGamepads;
        }
        else {
            Object.defineProperty(Navigator.prototype, "getGamepads", original);
        }
    }
}
async function smokeProviderRestart() {
    await restartProvider(new VoplayBrowserAssetProvider(), "game-asset");
    await restartProvider(new VoplayBrowserAudioProvider(), "game-audio");
}
async function smokeAssetProviderLifecycle() {
    const provider = new VoplayBrowserAssetProvider();
    const lane = new MemoryLane(31);
    const errors = [];
    const binds = [];
    const releases = [];
    const host = {
        framework: { name: "browser-asset-smoke", providerRoles: ["game-asset"] },
        log: () => { },
        reportError: (message) => errors.push(message),
        getCapability: (name) => {
            if (name === "framework_lane")
                return { open: async () => lane };
            if (name === "asset_buffer") {
                return {
                    bind: async (asset, artifact) => binds.push({ asset, artifact: artifact.slice() }),
                    read: async () => new ArrayBuffer(0),
                    release: (asset) => releases.push(asset),
                };
            }
            return null;
        },
    };
    pushProviderPacket(lane, 12 /* MessageKind.EngineStart */, 1n, new Uint8Array());
    await provider.init(host);
    assert((await takeProviderReplies(lane, errors, 1))[0].header.kind === 13 /* MessageKind.EngineReady */, "asset EngineReady");
    const assetId = Uint8Array.from({ length: 16 }, (_, index) => index + 1);
    const artifact1 = Uint8Array.from({ length: 16 }, (_, index) => 0x20 + index);
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 2n, concatSmokeBytes(Uint8Array.of(1), assetRegistration(assetId, 1n, artifact1, true)));
    const registered = (await takeProviderReplies(lane, errors, 1))[0];
    assert(registered.payload[0] === 5, "asset registered");
    const assetRef = smokeHandle(registered.payload, 5);
    const artifact2 = Uint8Array.from({ length: 16 }, (_, index) => 0x40 + index);
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 3n, concatSmokeBytes(Uint8Array.of(2), assetRegistration(assetId, 2n, artifact2, false)));
    const reloaded = (await takeProviderReplies(lane, errors, 1))[0];
    assert(reloaded.payload[0] === 5, "asset hot reload");
    assert(sameSmokeHandle(assetRef, smokeHandle(reloaded.payload, 5)), "hot reload preserves AssetRef");
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 4n, Uint8Array.of(3, 1));
    const scopeReply = (await takeProviderReplies(lane, errors, 1))[0];
    assert(scopeReply.payload[0] === 1, "asset scope opened");
    const scope = smokeHandle(scopeReply.payload, 1);
    const request = new Uint8Array(24);
    writeSmokeHandle(request, 0, scope);
    writeSmokeHandle(request, 8, assetRef);
    new DataView(request.buffer).setBigUint64(16, 1000000n, true);
    pushProviderPacket(lane, 21 /* MessageKind.AssetRequest */, 5n, request);
    const requestReplies = await takeProviderReplies(lane, errors, 2);
    const accepted = requestReplies.find((packet) => packet.payload[0] === 2);
    const work = requestReplies.find((packet) => packet.payload[0] === 3);
    assert(accepted !== undefined && work !== undefined, "asset request admission and work");
    const ticket = smokeHandle(accepted.payload, 1);
    assert(sameSmokeHandle(assetRef, smokeHandle(work.payload, 9)), "asset work identity");
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 6n, concatSmokeBytes(Uint8Array.of(9), work.payload.subarray(1)));
    const terminal = (await takeProviderReplies(lane, errors, 1))[0];
    assert(terminal.payload[0] === 4
        && sameSmokeHandle(ticket, smokeHandle(terminal.payload, 1))
        && terminal.payload[17] === 1, "asset terminal success");
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 7n, concatSmokeBytes(Uint8Array.of(5), encodeSmokeHandle(scope)));
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 8n, concatSmokeBytes(Uint8Array.of(6), encodeSmokeHandle(ticket)));
    pushProviderPacket(lane, 23 /* MessageKind.AssetControl */, 9n, concatSmokeBytes(Uint8Array.of(7), encodeSmokeHandle(scope)));
    const releaseReplies = await takeProviderReplies(lane, errors, 2);
    assert(releaseReplies.every((packet) => packet.payload[0] === 6), "asset release acknowledgements");
    pushProviderPacket(lane, 16 /* MessageKind.EngineClose */, 10n, new Uint8Array());
    assert((await takeProviderReplies(lane, errors, 1))[0].header.kind === 17 /* MessageKind.EngineClosed */, "asset EngineClosed");
    provider.stop();
    assert(binds.length === 2
        && sameSmokeHandle(binds[0].asset, binds[1].asset)
        && releases.length === 1
        && sameSmokeHandle(releases[0], assetRef), "asset buffer bind/release ownership");
}
async function smokeAudioGesture() {
    const provider = new VoplayBrowserAudioProvider();
    const lane = new MemoryLane(21);
    const logs = [];
    const errors = [];
    let inputSink = null;
    lane.inbound.push(encodeFrameworkPacket(providerHeader(12 /* MessageKind.EngineStart */, lane.binding.channelEpoch), new Uint8Array()));
    const host = {
        framework: { name: "browser-audio-smoke", providerRoles: ["game-audio"] },
        log: (message) => logs.push(message),
        reportError: (message) => errors.push(message),
        getCapability: (name) => {
            if (name === "framework_lane")
                return { open: async () => lane };
            if (name === "asset_buffer")
                return { read: async () => new ArrayBuffer(0) };
            if (name === "app_surface") {
                return {
                    isInteractive: () => true,
                    subscribeInput: (sink) => {
                        inputSink = sink;
                        return () => {
                            inputSink = null;
                        };
                    },
                };
            }
            return null;
        },
    };
    await provider.init(host);
    await waitFor(() => lane.outbound.length === 1 || errors.length > 0);
    assert(decodeFrameworkPacket(lane.outbound[0]).header.kind === 13 /* MessageKind.EngineReady */, "audio gesture EngineReady");
    const button = requireElement("audio-gesture", HTMLButtonElement);
    await new Promise((resolve, reject) => {
        const timeout = window.setTimeout(() => reject(new Error("audio gesture timeout")), 10_000);
        button.addEventListener("click", () => {
            inputSink?.({ type: "pointerDown" });
            void waitFor(() => logs.some((message) => message.includes("audio device active"))
                || errors.length > 0).then(() => {
                window.clearTimeout(timeout);
                if (errors.length > 0) {
                    reject(new Error(errors.join(", ")));
                }
                else {
                    resolve();
                }
            }, reject);
        }, { once: true });
        button.disabled = false;
        button.dataset.smokeReady = "true";
    });
    provider.stop();
}
async function restartProvider(provider, role) {
    for (let cycle = 0; cycle < 2; cycle += 1) {
        const lane = new MemoryLane(11 + cycle);
        const errors = [];
        lane.inbound.push(encodeFrameworkPacket(providerHeader(12 /* MessageKind.EngineStart */, lane.binding.channelEpoch), new Uint8Array()));
        const host = {
            framework: { name: "browser-smoke", providerRoles: [role] },
            log: () => { },
            reportError: (message) => errors.push(message),
            getCapability: (name) => {
                if (name === "framework_lane") {
                    return { open: async () => lane };
                }
                if (name === "asset_buffer") {
                    return {
                        bind: async () => { },
                        read: async () => new ArrayBuffer(0),
                        release: () => { },
                    };
                }
                if (name === "app_surface") {
                    return {
                        isInteractive: () => true,
                        subscribeInput: () => () => { },
                    };
                }
                return null;
            },
        };
        await provider.init(host);
        await waitFor(() => lane.outbound.length === 1 || errors.length > 0);
        assert(errors.length === 0, `${role} restart error: ${errors.join(", ")}`);
        const reply = decodeFrameworkPacket(lane.outbound[0]);
        assert(reply.header.kind === 13 /* MessageKind.EngineReady */, `${role} EngineReady`);
        provider.stop();
        assert(lane.closed, `${role} lane close`);
    }
}
class MemoryLane {
    binding;
    inbound = [];
    outbound = [];
    closed = false;
    constructor(channelEpoch) {
        this.binding = {
            channelEpoch,
            caller: { endpointIndex: 0, endpointGeneration: 1 },
        };
    }
    async poll() {
        return this.inbound.shift() ?? null;
    }
    async submit(payload) {
        this.outbound.push(payload.slice());
    }
    close() {
        this.closed = true;
    }
}
function providerHeader(kind, channelEpoch) {
    return {
        kind,
        engine: { index: 9, generation: 1 },
        channelEpoch: BigInt(channelEpoch),
        commitId: 0n,
        baseRevision: 0n,
        newRevision: 0n,
        requiredControlRevision: 0n,
        sourceSimulationRevision: 0n,
        sequence: 1n,
        payloadLen: 0,
    };
}
function pushProviderPacket(lane, kind, sequence, payload) {
    lane.inbound.push(encodeFrameworkPacket({
        ...providerHeader(kind, lane.binding.channelEpoch),
        sequence,
    }, payload));
}
async function takeProviderReplies(lane, errors, count) {
    await waitFor(() => lane.outbound.length >= count || errors.length > 0);
    assert(errors.length === 0, `provider error: ${errors.join(", ")}`);
    return lane.outbound.splice(0, count).map(decodeFrameworkPacket);
}
function assetRegistration(assetId, sourceRevision, artifactId, counted) {
    const bytes = new Uint8Array((counted ? 4 : 0) + 52);
    const offset = counted ? 4 : 0;
    const view = new DataView(bytes.buffer);
    if (counted)
        view.setUint32(0, 1, true);
    bytes.set(assetId, offset);
    view.setBigUint64(offset + 16, 1n, true);
    view.setBigUint64(offset + 24, sourceRevision, true);
    bytes.set(artifactId, offset + 32);
    view.setUint32(offset + 48, 0, true);
    return bytes;
}
function smokeHandle(bytes, offset) {
    const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 8);
    return { index: view.getUint32(0, true), generation: view.getUint32(4, true) };
}
function encodeSmokeHandle(handle) {
    const bytes = new Uint8Array(8);
    writeSmokeHandle(bytes, 0, handle);
    return bytes;
}
function writeSmokeHandle(bytes, offset, handle) {
    const view = new DataView(bytes.buffer, bytes.byteOffset + offset, 8);
    view.setUint32(0, handle.index, true);
    view.setUint32(4, handle.generation, true);
}
function sameSmokeHandle(left, right) {
    return left.index === right.index && left.generation === right.generation;
}
function concatSmokeBytes(...parts) {
    const output = new Uint8Array(parts.reduce((sum, part) => sum + part.byteLength, 0));
    let offset = 0;
    for (const part of parts) {
        output.set(part, offset);
        offset += part.byteLength;
    }
    return output;
}
function frame(surface, pulseId, frameId, deviceGeneration, commands) {
    return {
        surface,
        pulseId,
        frameId,
        renderEndpoint: { index: 5, generation: 1 },
        deviceGeneration,
        requiredRenderRevision: 1n,
        requiredControlRevision: 1n,
        graphSignature: 1n,
        commands,
    };
}
function smokeSurfaceId(sessionIndex = 1) {
    return {
        engine: {
            session: { index: sessionIndex, generation: 1 },
            engine: { index: 2, generation: 1 },
        },
        surface: { index: 3, generation: 1 },
        domain: { index: 4, generation: 1 },
    };
}
function portableFrame(...commands) {
    const size = 8 + commands.reduce((total, command) => total + command.length, 0);
    const bytes = new Uint8Array(size);
    bytes.set([0x56, 0x46, 0x43, 0x31]);
    new DataView(bytes.buffer).setUint32(4, commands.length, true);
    let offset = 8;
    for (const command of commands) {
        bytes.set(command, offset);
        offset += command.length;
    }
    return bytes;
}
function rectangleCommand(x, y, width, height, color) {
    const bytes = new Uint8Array(21);
    const view = new DataView(bytes.buffer);
    view.setUint8(0, 2);
    view.setUint32(1, x, true);
    view.setUint32(5, y, true);
    view.setUint32(9, width, true);
    view.setUint32(13, height, true);
    bytes.set(color, 17);
    return [...bytes];
}
function rumblePacket(requestId, generation) {
    const payload = new Uint8Array(41);
    const view = new DataView(payload.buffer);
    view.setUint8(0, 1);
    view.setBigUint64(1, requestId, true);
    view.setUint32(17, BROWSER_GAMEPAD_DEVICE_BASE, true);
    view.setUint32(21, generation, true);
    view.setUint32(25, 25, true);
    view.setUint16(29, 0xffff, true);
    view.setUint16(31, 0x7fff, true);
    view.setBigUint64(33, 1000000n, true);
    return { header: header(payload.byteLength), payload };
}
function cancelPacket(requestId, generation) {
    const payload = new Uint8Array(17);
    const view = new DataView(payload.buffer);
    view.setUint8(0, 2);
    view.setBigUint64(1, requestId, true);
    view.setUint32(9, BROWSER_GAMEPAD_DEVICE_BASE, true);
    view.setUint32(13, generation, true);
    return { header: header(payload.byteLength), payload };
}
function header(payloadLen) {
    return {
        kind: 34 /* MessageKind.HapticsCommand */,
        engine: { index: 2, generation: 1 },
        channelEpoch: 1n,
        commitId: 1n,
        baseRevision: 0n,
        newRevision: 0n,
        requiredControlRevision: 1n,
        sourceSimulationRevision: 1n,
        sequence: 1n,
        payloadLen,
    };
}
function gamepad(index, actuator) {
    return {
        index,
        connected: true,
        id: "Voplay smoke gamepad",
        mapping: "standard",
        timestamp: performance.now(),
        axes: [],
        buttons: [],
        vibrationActuator: actuator,
    };
}
function assertPixel(actual, expected, label) {
    assert(expected.every((value, index) => actual[index] === value), label);
}
function assert(condition, message) {
    if (!condition)
        throw new Error(message);
}
function expectThrow(action, expectedMessage) {
    try {
        action();
    }
    catch (error) {
        assert(error instanceof Error && error.message.includes(expectedMessage), expectedMessage);
        return;
    }
    throw new Error(`expected error: ${expectedMessage}`);
}
function nextMicrotask() {
    return new Promise((resolve) => queueMicrotask(resolve));
}
async function waitFor(predicate) {
    const deadline = performance.now() + 1000;
    while (!predicate()) {
        if (performance.now() >= deadline)
            throw new Error("browser smoke timeout");
        await new Promise((resolve) => window.setTimeout(resolve, 8));
    }
}
function requireElement(id, constructor) {
    const element = document.getElementById(id);
    if (!(element instanceof constructor))
        throw new Error(`missing #${id}`);
    return element;
}
await main();
