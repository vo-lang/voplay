import { portableFrameCommands } from "./canvas2d_adapter.js";
import { BrowserGpuReadbackError, } from "./platform_surface.js";
const RENDER_ATTACHMENT = 0x10;
const TEXTURE_BINDING_COPY_DST = 0x04 | 0x02;
const VERTEX_COPY_DST = 0x20 | 0x08;
const OFFSCREEN_USAGE = 0x10 | 0x04 | 0x02 | 0x01;
const MAX_TARGETS = 2048;
const MAX_TARGET_BYTES = 512 * 1024 * 1024;
const MAX_READBACK_BYTES = 16 * 1024 * 1024;
const MAX_PENDING_READBACK_BYTES = 64 * 1024 * 1024;
const MAP_READ_COPY_DST = 0x01 | 0x08;
export class WebGpuCanvasAdapter {
    #gpu;
    #adapter;
    #device;
    #format;
    #records = new Map();
    #textures = new Map();
    #offscreenTargets = new Map();
    #authorizedTargets = new Map();
    #pipelineCache = new Map();
    #maxCommands;
    #maxCommandBytes;
    #solidPipeline;
    #texturePipeline;
    #sampler;
    #deviceGeneration;
    #nextFence = 1n;
    #lost = false;
    #closed = false;
    #replacement = null;
    #pendingReadbackBytes = 0;
    static async create(config) {
        validateConfig(config);
        const gpu = navigator.gpu;
        if (gpu === undefined)
            throw new Error("WebGPU capability unavailable");
        const adapter = await gpu.requestAdapter({ powerPreference: "high-performance" });
        if (adapter === null)
            throw new Error("WebGPU adapter unavailable");
        return new WebGpuCanvasAdapter(gpu, adapter, await adapter.requestDevice(), gpu.getPreferredCanvasFormat(), config);
    }
    constructor(gpu, adapter, device, format, config) {
        this.#gpu = gpu;
        this.#adapter = adapter;
        this.#device = device;
        this.#format = format;
        this.#deviceGeneration = config.deviceGeneration;
        this.#maxCommands = config.maxCommands;
        this.#maxCommandBytes = config.maxCommandBytes;
        [this.#solidPipeline, this.#texturePipeline, this.#sampler] = createPipelines(device, format);
        this.#pipelineCache.set(`${format}:1:0`, [this.#solidPipeline, this.#texturePipeline]);
        this.#observeDevice(device);
    }
    get deviceGeneration() {
        return this.#deviceGeneration;
    }
    /**
     * Deterministically destroys the owned GPUDevice for platform certification
     * and recovery fault injection. Recovery still requires a higher generation
     * through `rebindDevice`.
     */
    async triggerControlledDeviceLoss() {
        this.#assertReady();
        const device = this.#device;
        device.destroy();
        await device.lost;
        await Promise.resolve();
        if (!this.#lost || device !== this.#device || this.#replacement === null) {
            throw new Error("WebGPU controlled device loss was not observed");
        }
    }
    async rebindDevice(deviceGeneration) {
        this.#assertOpen();
        if (deviceGeneration <= this.#deviceGeneration)
            throw new RangeError("stale WebGPU generation");
        if (this.#pendingReadbackBytes > 0 && !this.#lost) {
            throw new Error("WebGPU readback pending during device rebind");
        }
        if (this.#lost) {
            const replacement = await this.#requireReplacement();
            this.#assertOpen();
            this.#adapter = replacement.adapter;
            this.#device = replacement.device;
            this.#replacement = null;
            try {
                [this.#solidPipeline, this.#texturePipeline, this.#sampler] =
                    createPipelines(replacement.device, this.#format);
                this.#pipelineCache.clear();
                this.#pipelineCache.set(`${this.#format}:1:0`, [this.#solidPipeline, this.#texturePipeline]);
                for (const record of this.#records.values())
                    this.#configure(record);
                for (const resident of this.#textures.values())
                    this.#uploadResident(resident);
                this.#lost = false;
                this.#observeDevice(replacement.device);
            }
            catch (error) {
                replacement.device.destroy();
                this.#lost = true;
                this.#setReplacement(this.#startReplacement(replacement.device));
                throw error;
            }
        }
        this.#deviceGeneration = deviceGeneration;
        this.#nextFence = 1n;
        for (const record of this.#records.values()) {
            for (const buffer of record.pendingVertices ?? [])
                buffer.destroy();
            record.externalDepth?.gpu.destroy();
            record.externalReadback?.gpu.destroy();
            delete record.externalDepth;
            delete record.externalReadback;
            delete record.pendingVertices;
            delete record.pendingFence;
        }
        for (const target of this.#offscreenTargets.values())
            destroyOffscreenTarget(target);
        this.#offscreenTargets.clear();
    }
    attach(canvas, metrics) {
        this.#assertReady();
        if (this.#records.has(canvas))
            throw new Error("WebGPU surface already attached");
        const record = {
            context: webGpuContext(canvas),
            metrics,
            targetIdentities: new Set(),
        };
        resizeCanvas(canvas, metrics);
        try {
            this.#configure(record);
            this.#records.set(canvas, record);
        }
        catch (error) {
            record.context.unconfigure();
            throw error;
        }
    }
    resize(canvas, metrics) {
        this.#assertReady();
        const record = this.#record(canvas);
        if (record.pendingFence !== undefined)
            throw new Error("WebGPU frame pending");
        record.externalDepth?.gpu.destroy();
        record.externalReadback?.gpu.destroy();
        delete record.externalDepth;
        delete record.externalReadback;
        resizeCanvas(canvas, metrics);
        record.metrics = metrics;
        this.#configure(record);
    }
    synchronizeRenderTargets(engine, targets) {
        validateHandle(engine);
        if (targets.length > MAX_TARGETS) {
            throw new Error("WebGPU render target capacity exceeded");
        }
        const identities = new Set();
        for (const target of targets) {
            validateHandle(target);
            const identity = handleIdentity(target);
            if (identities.has(identity)) {
                throw new Error("duplicate WebGPU render target");
            }
            identities.add(identity);
        }
        const scope = handleScope(engine);
        const previous = this.#authorizedTargets.get(scope) ?? new Set();
        const removed = new Set([...previous].filter((identity) => !identities.has(identity)));
        if (removed.size > 0
            && [...this.#records.values()].some((record) => record.pendingFence !== undefined
                && [...removed].some((identity) => record.targetIdentities.has(scopedTargetKey(scope, identity))))) {
            throw new Error("WebGPU render target retirement still has a pending frame");
        }
        if (identities.size === 0)
            this.#authorizedTargets.delete(scope);
        else
            this.#authorizedTargets.set(scope, identities);
        const prefix = `${scope}/`;
        for (const record of this.#records.values()) {
            for (const key of record.targetIdentities) {
                if (key.startsWith(prefix) && !identities.has(targetIdentityFromKey(key))) {
                    record.targetIdentities.delete(key);
                }
            }
            if (record.externalTarget?.scope === scope
                && record.externalTarget.identity !== 0n
                && !identities.has(record.externalTarget.identity)) {
                record.externalDepth?.gpu.destroy();
                record.externalReadback?.gpu.destroy();
                delete record.externalDepth;
                delete record.externalReadback;
                delete record.externalTarget;
            }
        }
        for (const [key, target] of this.#offscreenTargets) {
            if (!key.startsWith(prefix) || identities.has(targetIdentityFromKey(key)))
                continue;
            destroyOffscreenTarget(target);
            this.#offscreenTargets.delete(key);
        }
    }
    async readRenderTarget(engine, target, expectedRevision, region, format) {
        this.#assertOpen();
        if (this.#lost)
            throw new BrowserGpuReadbackError(4, "WebGPU device lost");
        const readbackDevice = this.#device;
        validateHandle(engine);
        validateHandle(target);
        const scope = handleScope(engine);
        const identity = handleIdentity(target);
        const resident = this.#offscreenTargets.get(scopedTargetKey(scope, identity));
        const externalRecord = resident === undefined
            ? [...this.#records.values()].find((record) => record.externalReadback?.scope === scope
                && record.externalReadback.identity === identity)
            : undefined;
        const external = externalRecord?.externalReadback;
        const width = resident?.width ?? external?.width ?? 0;
        const height = resident?.height ?? external?.height ?? 0;
        const usage = resident?.usage ?? external?.usage ?? 0;
        const revision = resident?.revision ?? external?.revision ?? 0n;
        const colorFormat = resident?.colorFormat ?? external?.colorFormat ?? 0;
        const depthFormat = resident?.depthFormat ?? external?.depthFormat ?? 0;
        if ((resident === undefined && external === undefined)
            || expectedRevision <= 0n
            || revision !== expectedRevision
            || (usage & (1 << 8)) === 0
            || !validReadbackRegion(region, width, height)) {
            throw new BrowserGpuReadbackError(1, "invalid WebGPU render target readback");
        }
        const bytesPerPixel = format === 1 || format === 2 || format === 4
            ? 4
            : format === 3
                ? 8
                : 0;
        if (bytesPerPixel === 0
            || format === 4 && depthFormat !== 4
            || format !== 4 && ![1, 2].includes(colorFormat)) {
            throw new BrowserGpuReadbackError(1, "unsupported WebGPU render target readback format");
        }
        const sourceBytesPerPixel = format === 4
            ? 4
            : colorFormat === 2
                ? 8
                : 4;
        const sourceRowBytes = Math.ceil(region.width * sourceBytesPerPixel / 256) * 256;
        const outputRowBytes = Math.ceil(region.width * bytesPerPixel / 256) * 256;
        const stagingBytes = sourceRowBytes * region.height;
        const outputBytes = outputRowBytes * region.height;
        if (!Number.isSafeInteger(stagingBytes)
            || !Number.isSafeInteger(outputBytes)
            || outputBytes <= 0
            || stagingBytes > MAX_READBACK_BYTES
            || outputBytes > MAX_READBACK_BYTES) {
            throw new BrowserGpuReadbackError(1, "WebGPU render target readback capacity exceeded");
        }
        const reservationBytes = Math.max(stagingBytes, outputBytes);
        if (this.#pendingReadbackBytes + reservationBytes > MAX_PENDING_READBACK_BYTES) {
            throw new BrowserGpuReadbackError(1, "WebGPU pending readback byte capacity exceeded");
        }
        this.#pendingReadbackBytes += reservationBytes;
        let staging;
        try {
            staging = readbackDevice.createBuffer({
                label: "voplay-render-target-readback",
                size: stagingBytes,
                usage: MAP_READ_COPY_DST,
            });
            const encoder = readbackDevice.createCommandEncoder({
                label: "voplay-render-target-readback-copy",
            });
            encoder.copyTextureToBuffer({
                texture: format === 4
                    ? resident?.depth ?? externalRecord?.externalDepth?.gpu
                    : resident?.gpu ?? external.gpu,
                ...(format === 4 ? { aspect: "depth-only" } : {}),
                origin: { x: region.x, y: region.y, z: 0 },
            }, {
                buffer: staging,
                bytesPerRow: sourceRowBytes,
                rowsPerImage: region.height,
            }, {
                width: region.width,
                height: region.height,
                depthOrArrayLayers: 1,
            });
            readbackDevice.queue.submit([encoder.finish()]);
            await staging.mapAsync(1);
            const mapped = new Uint8Array(staging.getMappedRange());
            const bytes = convertReadbackBytes(mapped, sourceRowBytes, region.width, region.height, format === 4
                ? 4
                : external === undefined
                    ? colorFormat
                    : this.#format.startsWith("bgra")
                        ? 5
                        : 1, format, outputRowBytes);
            staging.unmap();
            return { targetRevision: revision, rowBytes: outputRowBytes, bytes };
        }
        catch (error) {
            if (error instanceof BrowserGpuReadbackError)
                throw error;
            const deviceLost = this.#lost || readbackDevice !== this.#device;
            throw new BrowserGpuReadbackError(deviceLost ? 4 : 2, deviceLost ? "WebGPU device lost during readback" : "WebGPU readback outcome unknown");
        }
        finally {
            staging?.destroy();
            this.#pendingReadbackBytes -= reservationBytes;
        }
    }
    submit(canvas, commands, graphSignature, engine, renderRevision) {
        this.#assertReady();
        validateHandle(engine);
        const record = this.#record(canvas);
        if (record.pendingFence !== undefined
            || graphSignature <= 0n
            || renderRevision <= 0n
            || commands.byteLength > this.#maxCommandBytes) {
            throw new Error("invalid WebGPU submission");
        }
        const portable = portableFrameCommands(commands);
        const bundled = decodeTargetBundle(portable, MAX_TARGETS, this.#maxCommandBytes);
        const scope = handleScope(engine);
        if (bundled !== null
            && (!this.#authorizedTargets.has(scope)
                || bundled.some((target) => !this.#authorizedTargets.get(scope).has(target.identity)))) {
            throw new Error("WebGPU frame references an unauthorized render target");
        }
        const targets = bundled ?? [{
                identity: 0n,
                width: record.metrics.width,
                height: record.metrics.height,
                external: true,
                colorFormat: 1,
                depthFormat: 0,
                sampleCount: 1,
                usage: 1 << 3,
                commands: portable,
            }];
        if (targets.filter((target) => target.external).length !== 1) {
            throw new Error("WebGPU frame requires one external target");
        }
        if (this.#nextFence > 0xffffffffffffffffn) {
            throw new RangeError("WebGPU fence exhausted");
        }
        const encoder = this.#device.createCommandEncoder({ label: "voplay-portable-frame" });
        const externalTexture = record.context.getCurrentTexture();
        const externalView = externalTexture.createView();
        const vertexBuffers = [];
        const renderedTargets = new Set();
        const renderedResidents = [];
        const renderedExternalResidents = [];
        const sampledTargets = new Map();
        const frameTextures = new Map();
        for (const [key, texture] of this.#textures) {
            const identity = textureIdentityForScope(key, "global");
            if (identity !== null)
                frameTextures.set(identity, texture);
        }
        for (const [key, texture] of this.#textures) {
            const identity = textureIdentityForScope(key, scope);
            if (identity !== null)
                frameTextures.set(identity, texture);
        }
        const prefix = `${scope}/`;
        for (const [key, target] of this.#offscreenTargets) {
            if (key.startsWith(prefix))
                sampledTargets.set(targetIdentityFromKey(key), target);
        }
        try {
            for (const target of targets) {
                const metrics = {
                    width: target.width,
                    height: target.height,
                    scaleNumerator: 1,
                    scaleDenominator: 1,
                };
                let view = externalView;
                let resolveView;
                let depthView;
                let pipelines = [this.#solidPipeline, this.#texturePipeline];
                if (target.external) {
                    if (target.width !== record.metrics.width || target.height !== record.metrics.height) {
                        throw new Error("WebGPU external target dimensions mismatch");
                    }
                    if (record.externalTarget !== undefined
                        && (record.externalTarget.scope !== scope
                            || record.externalTarget.identity !== target.identity)) {
                        record.externalDepth?.gpu.destroy();
                        record.externalReadback?.gpu.destroy();
                        delete record.externalDepth;
                        delete record.externalReadback;
                    }
                    record.externalTarget = { scope, identity: target.identity };
                    depthView = this.#externalDepthView(record, target);
                    pipelines = this.#pipelines(this.#format, 1, target.depthFormat);
                }
                else {
                    const offscreen = this.#offscreenTarget(scope, target);
                    sampledTargets.set(target.identity, offscreen);
                    depthView = offscreenDepthView(offscreen);
                    view = (offscreen.multisampled ?? offscreen.gpu).createView();
                    resolveView = offscreen.multisampled === undefined
                        ? undefined
                        : offscreen.gpu.createView();
                    pipelines = this.#pipelines(target.colorFormat === 2 ? "rgba16float" : "rgba8unorm-srgb", target.sampleCount, target.depthFormat);
                }
                if (!target.external)
                    renderedResidents.push(sampledTargets.get(target.identity));
                vertexBuffers.push(this.#encodePortableTarget(encoder, view, target.commands, metrics, renderedTargets, sampledTargets, frameTextures, depthView, resolveView, pipelines));
                if (target.external) {
                    if ((target.usage & (1 << 8)) !== 0) {
                        const readback = this.#externalReadbackTarget(record, scope, target);
                        encoder.copyTextureToTexture({ texture: externalTexture }, { texture: readback.gpu }, {
                            width: target.width,
                            height: target.height,
                            depthOrArrayLayers: 1,
                        });
                        renderedExternalResidents.push(readback);
                    }
                    else {
                        record.externalReadback?.gpu.destroy();
                        delete record.externalReadback;
                    }
                }
                renderedTargets.add(target.identity);
            }
            this.#device.queue.submit([encoder.finish()]);
            for (const resident of renderedResidents)
                resident.revision = renderRevision;
            for (const resident of renderedExternalResidents)
                resident.revision = renderRevision;
        }
        catch (error) {
            for (const buffer of vertexBuffers)
                buffer.destroy();
            this.#pruneOffscreenTargets();
            throw error;
        }
        record.targetIdentities = new Set(targets
            .filter((target) => target.identity !== 0n)
            .map((target) => scopedTargetKey(scope, target.identity)));
        this.#pruneOffscreenTargets();
        const fenceValue = this.#nextFence++;
        record.pendingFence = fenceValue;
        record.pendingVertices = vertexBuffers;
        return { fenceValue, deviceGeneration: this.#deviceGeneration };
    }
    present(canvas, fenceValue) {
        this.#assertReady();
        const record = this.#record(canvas);
        if (record.pendingFence !== fenceValue)
            throw new Error("WebGPU fence mismatch");
        for (const buffer of record.pendingVertices ?? [])
            buffer.destroy();
        delete record.pendingVertices;
        delete record.pendingFence;
    }
    detach(canvas) {
        const record = this.#record(canvas);
        if (record.pendingFence !== undefined)
            throw new Error("WebGPU frame outcome unknown");
        record.externalDepth?.gpu.destroy();
        record.externalReadback?.gpu.destroy();
        record.context.unconfigure();
        this.#records.delete(canvas);
        this.#pruneOffscreenTargets();
    }
    registerTexture(texture, source, engine) {
        this.#assertReady();
        if (texture <= 0n)
            throw new RangeError("invalid WebGPU texture");
        if (engine !== undefined)
            validateHandle(engine);
        const key = scopedTextureKey(engine === undefined ? "global" : handleScope(engine), texture);
        const [width, height] = imageDimensions(source);
        const previous = this.#textures.get(key);
        const gpu = this.#device.createTexture({
            label: "voplay-rgba-texture",
            size: { width, height, depthOrArrayLayers: 1 },
            format: "rgba8unorm",
            usage: TEXTURE_BINDING_COPY_DST,
        });
        try {
            this.#device.queue.copyExternalImageToTexture({ source }, { texture: gpu }, { width, height });
            const resident = {
                source,
                width,
                height,
                gpu,
                bindGroup: this.#device.createBindGroup({
                    layout: this.#texturePipeline.getBindGroupLayout(0),
                    entries: [
                        { binding: 0, resource: this.#sampler },
                        { binding: 1, resource: gpu.createView() },
                    ],
                }),
            };
            this.#textures.set(key, resident);
            previous?.gpu.destroy();
        }
        catch (error) {
            gpu.destroy();
            throw error;
        }
    }
    removeTexture(texture, engine) {
        this.#assertOpen();
        if (engine !== undefined)
            validateHandle(engine);
        const key = scopedTextureKey(engine === undefined ? "global" : handleScope(engine), texture);
        const resident = this.#textures.get(key);
        if (resident === undefined)
            return false;
        resident.gpu.destroy();
        this.#textures.delete(key);
        return true;
    }
    abandon(canvas) {
        const record = this.#record(canvas);
        for (const buffer of record.pendingVertices ?? [])
            buffer.destroy();
        record.externalDepth?.gpu.destroy();
        record.externalReadback?.gpu.destroy();
        record.context.unconfigure();
        this.#records.delete(canvas);
        this.#pruneOffscreenTargets();
    }
    close() {
        if (this.#closed)
            return;
        for (const [canvas, record] of this.#records) {
            for (const buffer of record.pendingVertices ?? [])
                buffer.destroy();
            record.externalDepth?.gpu.destroy();
            record.externalReadback?.gpu.destroy();
            record.context.unconfigure();
            this.#records.delete(canvas);
        }
        for (const resident of this.#textures.values())
            resident.gpu.destroy();
        this.#textures.clear();
        for (const target of this.#offscreenTargets.values())
            destroyOffscreenTarget(target);
        this.#offscreenTargets.clear();
        this.#authorizedTargets.clear();
        this.#device.destroy();
        void this.#replacement?.then((replacement) => replacement.device.destroy(), () => undefined);
        this.#replacement = null;
        this.#closed = true;
    }
    #uploadResident(resident) {
        const previous = resident.gpu;
        const gpu = this.#device.createTexture({
            label: "voplay-rgba-texture",
            size: { width: resident.width, height: resident.height, depthOrArrayLayers: 1 },
            format: "rgba8unorm",
            usage: TEXTURE_BINDING_COPY_DST,
        });
        try {
            this.#device.queue.copyExternalImageToTexture({ source: resident.source }, { texture: gpu }, { width: resident.width, height: resident.height });
            const bindGroup = this.#device.createBindGroup({
                layout: this.#texturePipeline.getBindGroupLayout(0),
                entries: [
                    { binding: 0, resource: this.#sampler },
                    { binding: 1, resource: gpu.createView() },
                ],
            });
            resident.gpu = gpu;
            resident.bindGroup = bindGroup;
            previous.destroy();
        }
        catch (error) {
            gpu.destroy();
            throw error;
        }
    }
    #offscreenTarget(scope, target) {
        const key = scopedTargetKey(scope, target.identity);
        const previous = this.#offscreenTargets.get(key);
        if (previous?.width === target.width
            && previous.height === target.height
            && previous.usage === target.usage
            && previous.colorFormat === target.colorFormat
            && previous.depthFormat === target.depthFormat
            && previous.sampleCount === target.sampleCount)
            return previous;
        if (previous === undefined && this.#offscreenTargets.size >= MAX_TARGETS) {
            throw new Error("WebGPU offscreen target capacity exceeded");
        }
        const bytesPerPixel = target.colorFormat === 2 ? 8 : 4;
        const colorBytes = target.width
            * target.height
            * bytesPerPixel
            * (1 + (target.sampleCount > 1 ? target.sampleCount : 0));
        const targetBytes = colorBytes + (target.depthFormat === 0
            ? 0
            : target.width * target.height * 4 * target.sampleCount);
        const nextBytes = [...this.#offscreenTargets.entries()].reduce((total, [candidate, resident]) => total
            + (candidate === key ? 0 : offscreenTargetBytes(resident)), targetBytes);
        if (!Number.isSafeInteger(nextBytes) || nextBytes > MAX_TARGET_BYTES) {
            throw new Error("WebGPU offscreen target byte capacity exceeded");
        }
        const format = target.colorFormat === 2 ? "rgba16float" : "rgba8unorm-srgb";
        const gpu = this.#device.createTexture({
            label: "voplay-offscreen-render-target",
            size: { width: target.width, height: target.height, depthOrArrayLayers: 1 },
            format,
            usage: OFFSCREEN_USAGE,
        });
        let multisampled;
        let depth;
        try {
            multisampled = target.sampleCount > 1
                ? this.#device.createTexture({
                    label: "voplay-offscreen-msaa-target",
                    size: { width: target.width, height: target.height, depthOrArrayLayers: 1 },
                    format,
                    sampleCount: target.sampleCount,
                    usage: RENDER_ATTACHMENT,
                })
                : undefined;
            depth = target.depthFormat === 0
                ? undefined
                : this.#device.createTexture({
                    label: "voplay-offscreen-depth-target",
                    size: { width: target.width, height: target.height, depthOrArrayLayers: 1 },
                    format: target.depthFormat === 3 ? "depth24plus" : "depth32float",
                    sampleCount: target.sampleCount,
                    usage: RENDER_ATTACHMENT
                        | (target.depthFormat === 4 && (target.usage & (1 << 8)) !== 0 ? 0x01 : 0),
                });
            const resident = {
                width: target.width,
                height: target.height,
                gpu,
                ...(multisampled === undefined ? {} : { multisampled }),
                ...(depth === undefined ? {} : { depth }),
                usage: target.usage,
                colorFormat: target.colorFormat,
                depthFormat: target.depthFormat,
                sampleCount: target.sampleCount,
                revision: 0n,
            };
            this.#offscreenTargets.set(key, resident);
            if (previous !== undefined)
                destroyOffscreenTarget(previous);
            return resident;
        }
        catch (error) {
            depth?.destroy();
            multisampled?.destroy();
            gpu.destroy();
            throw error;
        }
    }
    #externalDepthView(record, target) {
        if (target.depthFormat === 0) {
            record.externalDepth?.gpu.destroy();
            delete record.externalDepth;
            return undefined;
        }
        const readback = (target.usage & (1 << 8)) !== 0;
        if (record.externalDepth?.format === target.depthFormat
            && record.externalDepth.readback === readback) {
            return record.externalDepth.gpu.createView();
        }
        const gpu = this.#device.createTexture({
            label: "voplay-external-depth-target",
            size: {
                width: record.metrics.width,
                height: record.metrics.height,
                depthOrArrayLayers: 1,
            },
            format: target.depthFormat === 3 ? "depth24plus" : "depth32float",
            usage: RENDER_ATTACHMENT
                | (target.depthFormat === 4 && readback ? 0x01 : 0),
        });
        const previous = record.externalDepth;
        record.externalDepth = { format: target.depthFormat, readback, gpu };
        previous?.gpu.destroy();
        return gpu.createView();
    }
    #externalReadbackTarget(record, scope, target) {
        const previous = record.externalReadback;
        if (previous?.scope === scope
            && previous.identity === target.identity
            && previous.width === target.width
            && previous.height === target.height
            && previous.usage === target.usage
            && previous.colorFormat === target.colorFormat
            && previous.depthFormat === target.depthFormat) {
            return previous;
        }
        const byteLength = target.width * target.height * 4;
        if (!Number.isSafeInteger(byteLength) || byteLength <= 0 || byteLength > MAX_TARGET_BYTES) {
            throw new Error("WebGPU external readback target byte capacity exceeded");
        }
        const gpu = this.#device.createTexture({
            label: "voplay-external-readback-target",
            size: {
                width: target.width,
                height: target.height,
                depthOrArrayLayers: 1,
            },
            format: this.#format,
            usage: 0x01 | 0x02,
        });
        const resident = {
            scope,
            identity: target.identity,
            width: target.width,
            height: target.height,
            usage: target.usage,
            colorFormat: target.colorFormat,
            depthFormat: target.depthFormat,
            gpu,
            revision: 0n,
        };
        record.externalReadback = resident;
        previous?.gpu.destroy();
        return resident;
    }
    #pruneOffscreenTargets() {
        const live = new Set();
        for (const record of this.#records.values()) {
            for (const identity of record.targetIdentities)
                live.add(identity);
        }
        for (const [key, target] of this.#offscreenTargets) {
            if (live.has(key))
                continue;
            destroyOffscreenTarget(target);
            this.#offscreenTargets.delete(key);
        }
    }
    #encodePortableTarget(encoder, target, commands, metrics, renderedTargets, sampledTargets, textures, depthTarget, resolveTarget, pipelines) {
        const frame = decodePortableFrame(commands, metrics, this.#maxCommands, textures, sampledTargets, renderedTargets);
        const vertexBuffer = this.#device.createBuffer({
            label: "voplay-portable-frame-vertices",
            size: Math.max(32, frame.vertices.byteLength),
            usage: VERTEX_COPY_DST,
        });
        if (frame.vertices.byteLength > 0) {
            this.#device.queue.writeBuffer(vertexBuffer, 0, frame.vertices);
        }
        try {
            for (const segment of frame.passes) {
                const pass = encoder.beginRenderPass({
                    colorAttachments: [{
                            view: target,
                            clearValue: colorValue(segment.clear ?? [0, 0, 0, 0]),
                            loadOp: segment.clear === null ? "load" : "clear",
                            storeOp: "store",
                            ...(resolveTarget === undefined ? {} : { resolveTarget }),
                        }],
                    ...(depthTarget === undefined ? {} : {
                        depthStencilAttachment: {
                            view: depthTarget,
                            depthClearValue: 1,
                            depthLoadOp: "clear",
                            depthStoreOp: "store",
                        },
                    }),
                });
                pass.setVertexBuffer(0, vertexBuffer);
                for (const draw of segment.draws) {
                    if (draw.kind === "solid") {
                        pass.setPipeline(pipelines[0]);
                    }
                    else if (draw.kind === "texture") {
                        const texture = textures.get(draw.texture);
                        if (texture === undefined)
                            throw new Error("unknown WebGPU texture");
                        pass.setPipeline(pipelines[1]);
                        pass.setBindGroup(0, this.#device.createBindGroup({
                            layout: pipelines[1].getBindGroupLayout(0),
                            entries: [
                                { binding: 0, resource: this.#sampler },
                                { binding: 1, resource: texture.gpu.createView() },
                            ],
                        }));
                    }
                    else {
                        const sampled = sampledTargets.get(draw.target);
                        if (sampled === undefined)
                            throw new Error("unknown WebGPU sampled target");
                        pass.setPipeline(pipelines[1]);
                        pass.setBindGroup(0, this.#device.createBindGroup({
                            layout: pipelines[1].getBindGroupLayout(0),
                            entries: [
                                { binding: 0, resource: this.#sampler },
                                { binding: 1, resource: sampled.gpu.createView() },
                            ],
                        }));
                    }
                    pass.draw(draw.count, 1, draw.first);
                }
                pass.end();
            }
            return vertexBuffer;
        }
        catch (error) {
            vertexBuffer.destroy();
            throw error;
        }
    }
    #configure(record) {
        record.context.configure({
            device: this.#device,
            format: this.#format,
            alphaMode: "premultiplied",
            usage: RENDER_ATTACHMENT | 0x01,
        });
    }
    #pipelines(format, sampleCount, depthFormat) {
        const key = `${format}:${sampleCount}:${depthFormat}`;
        const existing = this.#pipelineCache.get(key);
        if (existing !== undefined)
            return existing;
        const [solid, textured] = createPipelines(this.#device, format, sampleCount, depthFormat);
        const pipelines = [solid, textured];
        this.#pipelineCache.set(key, pipelines);
        return pipelines;
    }
    #observeDevice(device) {
        void device.lost.then(() => {
            if (this.#closed || device !== this.#device)
                return;
            this.#lost = true;
            this.#setReplacement(this.#startReplacement(device));
        });
    }
    async #requireReplacement() {
        if (this.#replacement === null)
            throw new Error("WebGPU replacement unavailable");
        try {
            return await this.#replacement;
        }
        catch (error) {
            if (this.#closed || !this.#lost)
                throw error;
            const retry = this.#startReplacement(this.#device);
            this.#setReplacement(retry);
            return retry;
        }
    }
    #startReplacement(lostDevice) {
        return this.#gpu
            .requestAdapter({ powerPreference: "high-performance" })
            .then(async (adapter) => {
            if (adapter === null)
                throw new Error("WebGPU replacement adapter unavailable");
            const replacement = await adapter.requestDevice();
            if (this.#closed || lostDevice !== this.#device)
                replacement.destroy();
            return { adapter, device: replacement };
        });
    }
    #setReplacement(replacement) {
        this.#replacement = replacement;
        void replacement.catch(() => undefined);
    }
    #record(canvas) {
        this.#assertOpen();
        const record = this.#records.get(canvas);
        if (record === undefined)
            throw new Error("unknown WebGPU surface");
        return record;
    }
    #assertReady() {
        this.#assertOpen();
        if (this.#lost)
            throw new Error("WebGPU device lost");
    }
    #assertOpen() {
        if (this.#closed)
            throw new Error("WebGPU adapter closed");
    }
}
function createPipelines(device, format, sampleCount = 1, depthFormat = 0) {
    const module = device.createShaderModule({ label: "voplay-portable-shader", code: SHADER });
    const vertex = {
        module,
        entryPoint: "vertex_main",
        buffers: [{
                arrayStride: 32,
                attributes: [
                    { shaderLocation: 0, offset: 0, format: "float32x2" },
                    { shaderLocation: 1, offset: 8, format: "float32x2" },
                    { shaderLocation: 2, offset: 16, format: "float32x4" },
                ],
            }],
    };
    const blend = {
        color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha", operation: "add" },
        alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
    };
    const primitive = { topology: "triangle-list", cullMode: "none" };
    const depthStencil = depthFormat === 0
        ? {}
        : {
            depthStencil: {
                format: depthFormat === 3 ? "depth24plus" : "depth32float",
                depthWriteEnabled: true,
                depthCompare: "less-equal",
            },
        };
    const solid = device.createRenderPipeline({
        label: "voplay-portable-solid",
        layout: "auto",
        vertex,
        fragment: { module, entryPoint: "solid_main", targets: [{ format, blend }] },
        primitive,
        multisample: { count: sampleCount },
        ...depthStencil,
    });
    const textured = device.createRenderPipeline({
        label: "voplay-portable-textured",
        layout: "auto",
        vertex,
        fragment: { module, entryPoint: "texture_main", targets: [{ format, blend }] },
        primitive,
        multisample: { count: sampleCount },
        ...depthStencil,
    });
    return [solid, textured, device.createSampler({
            magFilter: "linear",
            minFilter: "linear",
            addressModeU: "clamp-to-edge",
            addressModeV: "clamp-to-edge",
        })];
}
function destroyOffscreenTarget(target) {
    target.depth?.destroy();
    target.multisampled?.destroy();
    target.gpu.destroy();
}
function offscreenTargetBytes(target) {
    const bytesPerPixel = target.colorFormat === 2 ? 8 : 4;
    const colorBytes = target.width
        * target.height
        * bytesPerPixel
        * (1 + (target.sampleCount > 1 ? target.sampleCount : 0));
    const depthBytes = target.depth === undefined
        ? 0
        : target.width * target.height * 4 * target.sampleCount;
    return colorBytes + depthBytes;
}
function offscreenDepthView(target) {
    return target.depth?.createView();
}
function convertReadbackBytes(source, sourceRowBytes, width, height, sourceFormat, outputFormat, outputRowBytes) {
    const output = new Uint8Array(outputRowBytes * height);
    const sourceView = new DataView(source.buffer, source.byteOffset, source.byteLength);
    const outputView = new DataView(output.buffer);
    for (let row = 0; row < height; row += 1) {
        for (let column = 0; column < width; column += 1) {
            const sourceOffset = row * sourceRowBytes + column * (sourceFormat === 2 ? 8 : 4);
            const outputOffset = row * outputRowBytes + column * (outputFormat === 3 ? 8 : 4);
            if (sourceFormat === 4) {
                outputView.setUint32(outputOffset, sourceView.getUint32(sourceOffset, true), true);
                continue;
            }
            if (sourceFormat === 2 && outputFormat === 3) {
                output.set(source.subarray(sourceOffset, sourceOffset + 8), outputOffset);
                continue;
            }
            const channels = sourceFormat === 2
                ? [
                    halfToFloat(sourceView.getUint16(sourceOffset, true)),
                    halfToFloat(sourceView.getUint16(sourceOffset + 2, true)),
                    halfToFloat(sourceView.getUint16(sourceOffset + 4, true)),
                    halfToFloat(sourceView.getUint16(sourceOffset + 6, true)),
                ]
                : sourceFormat === 5
                    ? [
                        source[sourceOffset + 2] / 255,
                        source[sourceOffset + 1] / 255,
                        source[sourceOffset] / 255,
                        source[sourceOffset + 3] / 255,
                    ]
                    : [
                        source[sourceOffset] / 255,
                        source[sourceOffset + 1] / 255,
                        source[sourceOffset + 2] / 255,
                        source[sourceOffset + 3] / 255,
                    ];
            if (outputFormat === 3) {
                for (let channel = 0; channel < 4; channel += 1) {
                    outputView.setUint16(outputOffset + channel * 2, floatToHalf(channels[channel]), true);
                }
                continue;
            }
            const red = normalizedByte(channels[0]);
            const blue = normalizedByte(channels[2]);
            output[outputOffset] = outputFormat === 2 ? blue : red;
            output[outputOffset + 1] = normalizedByte(channels[1]);
            output[outputOffset + 2] = outputFormat === 2 ? red : blue;
            output[outputOffset + 3] = normalizedByte(channels[3]);
        }
    }
    return output;
}
function normalizedByte(value) {
    if (Number.isNaN(value))
        return 0;
    return Math.round(Math.min(1, Math.max(0, value)) * 255);
}
function halfToFloat(bits) {
    const sign = (bits & 0x8000) === 0 ? 1 : -1;
    const exponent = (bits >>> 10) & 0x1f;
    const fraction = bits & 0x03ff;
    if (exponent === 0) {
        return fraction === 0 ? sign * 0 : sign * 2 ** -14 * (fraction / 1024);
    }
    if (exponent === 0x1f)
        return fraction === 0 ? sign * Infinity : Number.NaN;
    return sign * 2 ** (exponent - 15) * (1 + fraction / 1024);
}
function floatToHalf(value) {
    if (Number.isNaN(value))
        return 0x7e00;
    const bits = new Uint32Array(new Float32Array([value]).buffer)[0];
    const sign = (bits >>> 16) & 0x8000;
    let exponent = ((bits >>> 23) & 0xff) - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if (exponent <= 0) {
        if (exponent < -10)
            return sign;
        mantissa = (mantissa | 0x80_0000) >>> (1 - exponent);
        return sign | ((mantissa + 0x1000) >>> 13);
    }
    if (exponent >= 0x1f) {
        return sign | (mantissa === 0 ? 0x7c00 : 0x7e00);
    }
    mantissa += 0x1000;
    if ((mantissa & 0x80_0000) !== 0) {
        mantissa = 0;
        exponent += 1;
        if (exponent >= 0x1f)
            return sign | 0x7c00;
    }
    return sign | (exponent << 10) | (mantissa >>> 13);
}
function decodeTargetBundle(bytes, maxTargets, maxBytes) {
    if (bytes.byteLength < 4
        || bytes[0] !== 0x56
        || bytes[1] !== 0x54
        || bytes[2] !== 0x42
        || (bytes[3] !== 0x31 && bytes[3] !== 0x32)) {
        return null;
    }
    if (bytes.byteLength < 8 || bytes.byteLength > maxBytes) {
        throw new RangeError("invalid WebGPU target bundle");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const version = bytes[3] === 0x32 ? 2 : 1;
    const count = view.getUint32(4, true);
    if (count === 0 || count > maxTargets) {
        throw new RangeError("WebGPU target bundle capacity exceeded");
    }
    const targets = [];
    const identities = new Set();
    let offset = 8;
    for (let index = 0; index < count; index += 1) {
        requireBytes(bytes, offset, version === 2 ? 28 : 21);
        const handleIndex = view.getUint32(offset, true);
        const generation = view.getUint32(offset + 4, true);
        const width = view.getUint32(offset + 8, true);
        const height = view.getUint32(offset + 12, true);
        const externalTag = bytes[offset + 16];
        const colorFormat = version === 2 ? bytes[offset + 17] : 1;
        const depthFormat = version === 2 ? bytes[offset + 18] : 0;
        const sampleCount = version === 2 ? bytes[offset + 19] : 1;
        const usage = version === 2 ? view.getUint32(offset + 20, true) : (1 << 3);
        const commandBytes = view.getUint32(offset + (version === 2 ? 24 : 17), true);
        const identity = BigInt(handleIndex) | (BigInt(generation) << 32n);
        offset += version === 2 ? 28 : 21;
        requireBytes(bytes, offset, commandBytes);
        if (handleIndex === 0xffff_ffff
            || generation === 0
            || width === 0
            || height === 0
            || (externalTag !== 0 && externalTag !== 1)
            || (colorFormat !== 1 && colorFormat !== 2)
            || ![0, 3, 4].includes(depthFormat)
            || ![1, 2, 4, 8].includes(sampleCount)
            || (depthFormat !== 0 && sampleCount !== 1)
            || (usage & ~0x1ff) !== 0
            || (usage & (1 << 3)) === 0
            || ((usage & (1 << 4)) !== 0) !== (depthFormat !== 0)
            || (externalTag === 1
                && (colorFormat !== 1 || sampleCount !== 1))
            || identities.has(identity)) {
            throw new RangeError("invalid WebGPU target descriptor");
        }
        identities.add(identity);
        targets.push({
            identity,
            width,
            height,
            external: externalTag === 1,
            colorFormat,
            depthFormat,
            sampleCount,
            usage,
            commands: bytes.subarray(offset, offset + commandBytes),
        });
        offset += commandBytes;
    }
    if (offset !== bytes.byteLength)
        throw new RangeError("WebGPU target bundle trailing bytes");
    return targets;
}
function decodePortableFrame(bytes, metrics, maxCommands, textures, sampledTargets, renderedTargets) {
    if (bytes.byteLength < 8
        || bytes[0] !== 0x56
        || bytes[1] !== 0x46
        || bytes[2] !== 0x43
        || bytes[3] !== 0x31) {
        throw new RangeError("invalid WebGPU portable frame");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const count = view.getUint32(4, true);
    if (count > maxCommands)
        throw new RangeError("WebGPU portable command capacity");
    const vertices = [];
    const passes = [];
    let current = { clear: [0, 0, 0, 0], draws: [] };
    passes.push(current);
    let offset = 8;
    for (let index = 0; index < count; index += 1) {
        requireBytes(bytes, offset, 1);
        const tag = bytes[offset++];
        if (tag === 1) {
            requireBytes(bytes, offset, 4);
            current = { clear: takeColor(bytes, offset), draws: [] };
            passes.push(current);
            offset += 4;
            continue;
        }
        const first = vertices.length / 8;
        if (tag === 2) {
            requireBytes(bytes, offset, 20);
            pushRect(vertices, metrics, view.getUint32(offset, true), view.getUint32(offset + 4, true), view.getUint32(offset + 8, true), view.getUint32(offset + 12, true), [0, 0, 1, 1], takeColor(bytes, offset + 16));
            offset += 20;
            current.draws.push({ kind: "solid", first, count: 6 });
            continue;
        }
        if (tag === 3) {
            requireBytes(bytes, offset, 44);
            const texture = view.getBigUint64(offset, true);
            const resident = textures.get(texture);
            if (resident === undefined)
                throw new RangeError("unknown WebGPU portable texture");
            const sourceX = view.getUint32(offset + 24, true);
            const sourceY = view.getUint32(offset + 28, true);
            const sourceWidth = view.getUint32(offset + 32, true);
            const sourceHeight = view.getUint32(offset + 36, true);
            pushRect(vertices, metrics, view.getUint32(offset + 8, true), view.getUint32(offset + 12, true), view.getUint32(offset + 16, true), view.getUint32(offset + 20, true), [
                sourceX / resident.width,
                sourceY / resident.height,
                (sourceX + sourceWidth) / resident.width,
                (sourceY + sourceHeight) / resident.height,
            ], takeColor(bytes, offset + 40));
            offset += 44;
            current.draws.push({ kind: "texture", first, count: 6, texture });
            continue;
        }
        if (tag === 4) {
            requireBytes(bytes, offset, 28);
            const color = takeColor(bytes, offset + 24);
            for (let point = 0; point < 3; point += 1) {
                pushVertex(vertices, metrics, view.getInt32(offset + point * 8, true), view.getInt32(offset + point * 8 + 4, true), 0, 0, color);
            }
            offset += 28;
            current.draws.push({ kind: "solid", first, count: 3 });
            continue;
        }
        if (tag === 5) {
            requireBytes(bytes, offset, 44);
            const target = view.getBigUint64(offset, true);
            const sampled = sampledTargets.get(target);
            if (sampled === undefined
                || (sampled.usage & (1 << 0)) === 0
                || !renderedTargets.has(target))
                throw new RangeError("unknown or unordered WebGPU sampled target");
            const sourceX = view.getUint32(offset + 24, true);
            const sourceY = view.getUint32(offset + 28, true);
            const sourceWidth = view.getUint32(offset + 32, true);
            const sourceHeight = view.getUint32(offset + 36, true);
            if (sourceWidth === 0
                || sourceHeight === 0
                || sourceX + sourceWidth > sampled.width
                || sourceY + sourceHeight > sampled.height) {
                throw new RangeError("invalid WebGPU sampled target region");
            }
            pushRect(vertices, metrics, view.getUint32(offset + 8, true), view.getUint32(offset + 12, true), view.getUint32(offset + 16, true), view.getUint32(offset + 20, true), [
                sourceX / sampled.width,
                sourceY / sampled.height,
                (sourceX + sourceWidth) / sampled.width,
                (sourceY + sourceHeight) / sampled.height,
            ], takeColor(bytes, offset + 40));
            offset += 44;
            current.draws.push({ kind: "target", first, count: 6, target });
            continue;
        }
        throw new RangeError(`unsupported WebGPU portable command ${tag}`);
    }
    if (offset !== bytes.byteLength)
        throw new RangeError("WebGPU portable trailing bytes");
    return { vertices: new Float32Array(vertices), passes };
}
function pushRect(vertices, metrics, x, y, width, height, uv, color) {
    const x1 = x + width;
    const y1 = y + height;
    pushVertex(vertices, metrics, x, y, uv[0], uv[1], color);
    pushVertex(vertices, metrics, x1, y, uv[2], uv[1], color);
    pushVertex(vertices, metrics, x, y1, uv[0], uv[3], color);
    pushVertex(vertices, metrics, x, y1, uv[0], uv[3], color);
    pushVertex(vertices, metrics, x1, y, uv[2], uv[1], color);
    pushVertex(vertices, metrics, x1, y1, uv[2], uv[3], color);
}
function pushVertex(vertices, metrics, x, y, u, v, color) {
    vertices.push(x / metrics.width * 2 - 1, 1 - y / metrics.height * 2, u, v, color[0] / 255, color[1] / 255, color[2] / 255, color[3] / 255);
}
function takeColor(bytes, offset) {
    return [bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]];
}
function colorValue(color) {
    return { r: color[0] / 255, g: color[1] / 255, b: color[2] / 255, a: color[3] / 255 };
}
function imageDimensions(source) {
    const dimensions = source;
    const width = dimensions.width ?? dimensions.naturalWidth ?? dimensions.videoWidth ?? 0;
    const height = dimensions.height ?? dimensions.naturalHeight ?? dimensions.videoHeight ?? 0;
    if (width <= 0 || height <= 0)
        throw new RangeError("invalid WebGPU texture dimensions");
    return [width, height];
}
function requireBytes(bytes, offset, length) {
    if (!Number.isSafeInteger(offset + length) || offset + length > bytes.byteLength) {
        throw new RangeError("truncated WebGPU portable command");
    }
}
function webGpuContext(canvas) {
    const context = canvas.getContext("webgpu");
    if (context === null)
        throw new Error("WebGPU canvas context unavailable");
    return context;
}
function resizeCanvas(canvas, metrics) {
    canvas.width = metrics.width;
    canvas.height = metrics.height;
}
function validateHandle(handle) {
    if (!Number.isSafeInteger(handle.index)
        || handle.index < 0
        || handle.index >= 0xffff_ffff
        || !Number.isSafeInteger(handle.generation)
        || handle.generation <= 0
        || handle.generation > 0xffff_ffff) {
        throw new RangeError("invalid WebGPU handle");
    }
}
function handleIdentity(handle) {
    return BigInt(handle.index) | (BigInt(handle.generation) << 32n);
}
function handleScope(handle) {
    return `${handle.index}:${handle.generation}`;
}
function scopedTargetKey(scope, target) {
    return `${scope}/${target}`;
}
function scopedTextureKey(scope, texture) {
    return `${scope}/${texture}`;
}
function textureIdentityForScope(key, scope) {
    const prefix = `${scope}/`;
    const globalPrefix = "global/";
    if (key.startsWith(prefix))
        return BigInt(key.slice(prefix.length));
    if (key.startsWith(globalPrefix))
        return BigInt(key.slice(globalPrefix.length));
    return null;
}
function targetIdentityFromKey(key) {
    const separator = key.lastIndexOf("/");
    if (separator < 0)
        throw new Error("invalid scoped WebGPU target identity");
    return BigInt(key.slice(separator + 1));
}
function validReadbackRegion(region, targetWidth, targetHeight) {
    return [
        region.x,
        region.y,
        region.width,
        region.height,
    ].every(Number.isSafeInteger)
        && region.x >= 0
        && region.y >= 0
        && region.width > 0
        && region.height > 0
        && region.x + region.width <= targetWidth
        && region.y + region.height <= targetHeight;
}
function validateConfig(config) {
    if (config.deviceGeneration <= 0n
        || !Number.isSafeInteger(config.maxCommands)
        || config.maxCommands <= 0
        || !Number.isSafeInteger(config.maxCommandBytes)
        || config.maxCommandBytes < 8) {
        throw new RangeError("invalid WebGPU adapter config");
    }
}
const SHADER = `
struct VertexOut {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
  @location(1) color: vec4f,
}

@vertex
fn vertex_main(
  @location(0) position: vec2f,
  @location(1) uv: vec2f,
  @location(2) color: vec4f,
) -> VertexOut {
  var out: VertexOut;
  out.position = vec4f(position, 0.0, 1.0);
  out.uv = uv;
  out.color = color;
  return out;
}

@fragment
fn solid_main(in: VertexOut) -> @location(0) vec4f {
  return in.color;
}

@group(0) @binding(0) var image_sampler: sampler;
@group(0) @binding(1) var image_texture: texture_2d<f32>;

@fragment
fn texture_main(in: VertexOut) -> @location(0) vec4f {
  return textureSample(image_texture, image_sampler, in.uv) * in.color;
}
`;
