// voplay-render-island.ts
// Implements the RendererModule interface for Studio's framework-neutral render island.
// Studio loads this file dynamically from the VFS snapshot and calls init(host).
import { bootstrapWebView, stopWebView } from "./bootstrap_webview";
// Relative paths within the framework's VFS snapshot for the island WASM.
// These match wasm-pack output names for the `wasm-island` feature build.
const WASM_BG_PATH = "wasm/voplay_island_bg.wasm";
const WASM_JS_PATH = "wasm/voplay_island.js";
const DEFAULT_MAX_RENDER_CANVAS_PIXELS = 2250000;
const activeInstances = new Set();
let widgetSequence = 0;
let hostServices = null;
function describeError(error) {
    if (error instanceof Error) {
        return `${error.name}: ${error.message}`;
    }
    return String(error);
}
function requireCapability(value, name) {
    if (value == null) {
        throw new Error(`[voplay] missing renderer host capability: ${name}`);
    }
    return value;
}
function makeCanvasId(props) {
    const prefix = typeof props.canvasId === "string" && props.canvasId.trim().length > 0
        ? props.canvasId.trim()
        : "voplay-canvas";
    widgetSequence += 1;
    return `${prefix}-${widgetSequence}`;
}
function shouldShowDebugOverlay() {
    try {
        const params = new URLSearchParams(window.location.search);
        return params.has("rendererDebug")
            || params.has("debug")
            || window.localStorage.getItem("voplay.rendererDebug") === "1";
    }
    catch {
        return false;
    }
}
function readLocationParam(name) {
    try {
        const searchValue = new URLSearchParams(window.location.search).get(name);
        if (searchValue !== null) {
            return searchValue;
        }
        const hash = window.location.hash || "";
        const queryOffset = hash.indexOf("?");
        if (queryOffset >= 0) {
            return new URLSearchParams(hash.slice(queryOffset + 1)).get(name);
        }
    }
    catch {
        return null;
    }
    return null;
}
function readPositiveNumberSetting(queryName, storageName) {
    let raw = readLocationParam(queryName);
    if (raw === null) {
        try {
            raw = window.localStorage.getItem(storageName);
        }
        catch {
            raw = null;
        }
    }
    if (raw == null || raw.trim() === "") {
        return null;
    }
    const value = Number(raw);
    if (!Number.isFinite(value) || value <= 0) {
        return null;
    }
    return value;
}
function resolveRenderDevicePixelRatio(width, height, nativeDevicePixelRatio) {
    const native = Number.isFinite(nativeDevicePixelRatio) && nativeDevicePixelRatio > 0
        ? nativeDevicePixelRatio
        : 1;
    const explicit = readPositiveNumberSetting("voplayRenderPixelRatio", "voplay.render.pixelRatio");
    let ratio = explicit ?? native;
    const cap = readPositiveNumberSetting("voplayRenderPixelRatioCap", "voplay.render.pixelRatioCap");
    if (cap !== null) {
        ratio = Math.min(ratio, cap);
    }
    const maxPixels = readPositiveNumberSetting("voplayMaxCanvasPixels", "voplay.render.maxCanvasPixels")
        ?? DEFAULT_MAX_RENDER_CANVAS_PIXELS;
    if (maxPixels > 0 && width > 0 && height > 0) {
        ratio = Math.min(ratio, Math.sqrt(maxPixels / (width * height)));
    }
    ratio = Math.min(ratio, native);
    const minRatio = native >= 1 ? 1 : native;
    return Math.max(minRatio, ratio);
}
class RenderIslandWidgetInstance {
    constructor(container, props, onEvent, services) {
        this.container = container;
        this.onEvent = onEvent;
        this.services = services;
        this.debugOverlay = null;
        this.debugStatusCallback = null;
        this.island = null;
        this.channel = null;
        this.debugMessages = [];
        this.destroyed = false;
        this.mounted = false;
        this.canvasId = makeCanvasId(props);
        this.debug(`[voplay] widget.create canvasId=${this.canvasId}`);
        this.canvas = document.createElement("canvas");
        this.canvas.id = this.canvasId;
        this.canvas.tabIndex = 0;
        this.canvas.style.width = "100%";
        this.canvas.style.height = "100%";
        this.canvas.style.display = "block";
        this.container.style.display = "block";
        this.container.style.overflow = "hidden";
        if (shouldShowDebugOverlay()) {
            this.container.style.position = this.container.style.position || "relative";
            this.debugOverlay = document.createElement("div");
            this.debugOverlay.style.position = "absolute";
            this.debugOverlay.style.left = "8px";
            this.debugOverlay.style.bottom = "8px";
            this.debugOverlay.style.maxWidth = "min(92%, 760px)";
            this.debugOverlay.style.padding = "6px 8px";
            this.debugOverlay.style.borderRadius = "4px";
            this.debugOverlay.style.background = "rgba(7, 10, 18, 0.72)";
            this.debugOverlay.style.color = "#d7e3ff";
            this.debugOverlay.style.font = "11px ui-monospace, SFMono-Regular, Menlo, monospace";
            this.debugOverlay.style.lineHeight = "1.35";
            this.debugOverlay.style.pointerEvents = "none";
            this.debugOverlay.style.zIndex = "20";
            this.debugOverlay.textContent = "voplay: creating render island";
            this.debugStatusCallback = (message) => this.setDebugStatus(message);
            globalThis.__voplayDebugStatus = this.debugStatusCallback;
        }
        this.container.appendChild(this.canvas);
        if (this.debugOverlay) {
            this.container.appendChild(this.debugOverlay);
        }
        this.resizeObserver = new ResizeObserver(() => {
            if (!this.mounted) {
                return;
            }
            this.emitWidgetEvent("resize");
        });
        this.resizeObserver.observe(this.container);
        void this.start();
    }
    update(_props) { }
    destroy() {
        if (this.destroyed) {
            return;
        }
        this.destroyed = true;
        this.debug(`[voplay] widget.destroy canvasId=${this.canvasId}`);
        this.resizeObserver.disconnect();
        if (this.island) {
            stopWebView(this.island);
            this.island = null;
            this.channel = null;
        }
        else if (this.channel) {
            this.channel.close();
            this.channel = null;
        }
        const debugGlobal = globalThis;
        if (this.debugStatusCallback && debugGlobal.__voplayDebugStatus === this.debugStatusCallback) {
            delete debugGlobal.__voplayDebugStatus;
        }
        this.debugOverlay?.remove();
        this.canvas.remove();
        activeInstances.delete(this);
    }
    async start() {
        this.debug(`[voplay] island.start.begin canvasId=${this.canvasId}`);
        const wasmBytes = this.services.getVfsBytes(WASM_BG_PATH);
        if (!wasmBytes) {
            this.services.reportError(`[voplay] WASM binary not found in VFS snapshot: ${WASM_BG_PATH}`);
            return;
        }
        const jsGlueBytes = this.services.getVfsBytes(WASM_JS_PATH);
        if (!jsGlueBytes) {
            this.services.reportError(`[voplay] WASM JS glue not found in VFS snapshot: ${WASM_JS_PATH}`);
            return;
        }
        let channel = null;
        try {
            this.debug(`[voplay] island.channel.create.begin canvasId=${this.canvasId}`);
            channel = await this.services.createChannel();
            this.debug(`[voplay] island.channel.create.ready canvasId=${this.canvasId}`);
            if (this.destroyed) {
                channel.close();
                return;
            }
            this.debug(`[voplay] island.channel.init.begin canvasId=${this.canvasId}`);
            await channel.init();
            this.debug(`[voplay] island.channel.init.ready canvasId=${this.canvasId}`);
            if (this.destroyed) {
                channel.close();
                return;
            }
            this.debug(`[voplay] island.voweb.begin canvasId=${this.canvasId}`);
            const voWeb = await this.services.getVoWeb();
            this.debug(`[voplay] island.voweb.ready canvasId=${this.canvasId}`);
            this.debug(`[voplay] island.bootstrap.begin canvasId=${this.canvasId}`);
            this.publishRenderMetrics();
            const island = await bootstrapWebView({
                canvasId: this.canvasId,
                bytecode: this.services.moduleBytes,
                voplayWasm: wasmBytes,
                voplayWasmJsGlue: jsGlueBytes,
            }, voWeb, channel, (message) => this.debug(message), this.services.reportError);
            this.debug(`[voplay] island.bootstrap.ready canvasId=${this.canvasId}`);
            if (this.destroyed) {
                stopWebView(island);
                return;
            }
            this.channel = channel;
            this.island = island;
            this.mounted = true;
            this.debug(`[voplay] island.start.ready canvasId=${this.canvasId}`);
            this.emitWidgetEvent("mount");
            requestAnimationFrame(() => {
                if (!this.destroyed) {
                    this.canvas.focus();
                }
            });
        }
        catch (error) {
            channel?.close();
            this.services.reportError(`[voplay] ${describeError(error)}`);
        }
    }
    emitWidgetEvent(type) {
        const metrics = this.publishRenderMetrics();
        if (!metrics) {
            return;
        }
        this.debug(`[voplay] widget.event type=${type} canvasId=${this.canvasId} size=${metrics.width}x${metrics.height} pixels=${metrics.pixelWidth}x${metrics.pixelHeight} dpr=${metrics.devicePixelRatio} nativeDpr=${metrics.nativeDevicePixelRatio}`);
        this.onEvent(JSON.stringify({ type, width: metrics.width, height: metrics.height, pixelWidth: metrics.pixelWidth, pixelHeight: metrics.pixelHeight, devicePixelRatio: metrics.devicePixelRatio, nativeDevicePixelRatio: metrics.nativeDevicePixelRatio, canvasId: this.canvasId }));
    }
    publishRenderMetrics() {
        const width = Math.round(this.container.clientWidth);
        const height = Math.round(this.container.clientHeight);
        if (width <= 0 || height <= 0) {
            this.debug(`[voplay] widget.metrics.skip canvasId=${this.canvasId} width=${width} height=${height}`);
            return null;
        }
        const nativeDevicePixelRatio = window.devicePixelRatio || 1;
        const devicePixelRatio = resolveRenderDevicePixelRatio(width, height, nativeDevicePixelRatio);
        const pixelWidth = Math.max(1, Math.round(width * devicePixelRatio));
        const pixelHeight = Math.max(1, Math.round(height * devicePixelRatio));
        const renderGlobal = globalThis;
        renderGlobal.__voplayNativeDevicePixelRatio = nativeDevicePixelRatio;
        renderGlobal.__voplayRenderDevicePixelRatio = devicePixelRatio;
        return { width, height, nativeDevicePixelRatio, devicePixelRatio, pixelWidth, pixelHeight };
    }
    debug(message) {
        this.services.debugLog(message);
        this.setDebugStatus(message);
    }
    setDebugStatus(message) {
        if (this.debugOverlay) {
            this.debugMessages.push(message);
            while (this.debugMessages.length > 6) {
                this.debugMessages.shift();
            }
            this.debugOverlay.textContent = this.debugMessages.join("\n");
        }
    }
}
export async function init(host) {
    const widget = requireCapability(host.getCapability("widget"), "widget");
    const islandTransport = requireCapability(host.getCapability("island_transport"), "island_transport");
    const voWeb = requireCapability(host.getCapability("vo_web"), "vo_web");
    const vfs = requireCapability(host.getCapability("vfs"), "vfs");
    hostServices = {
        moduleBytes: host.moduleBytes,
        createChannel: () => islandTransport.createChannel(),
        getVoWeb: () => voWeb.getVoWeb(),
        getVfsBytes: (path) => vfs.getBytes(path),
        debugLog: (message) => host.log(message),
        reportError: (message) => host.reportError(message),
    };
    host.log('[voplay] init');
    widget.register("voplay-render-island", {
        create(container, props, onEvent) {
            const services = hostServices;
            if (!services) {
                throw new Error("[voplay] widget provider used before init completed");
            }
            const instance = new RenderIslandWidgetInstance(container, props, onEvent, services);
            activeInstances.add(instance);
            return instance;
        },
    });
}
export function render(_container, _bytes) {
    // voplay renders directly to the WebGPU canvas via the render island VM.
    // Render bytes from the logic island are dispatched through the island channel,
    // not delivered here.
}
export function stop() {
    hostServices?.debugLog('[voplay] stop');
    for (const instance of Array.from(activeInstances)) {
        instance.destroy();
    }
    activeInstances.clear();
    hostServices = null;
    stopWebView();
}
export default { init, render, stop };
