// voplay-render-island.ts
// Implements the RendererModule interface for Studio's framework-neutral render island.
// Studio loads this file dynamically from the VFS snapshot and calls init(host).

import { bootstrapWebView, stopWebView } from "./bootstrap_webview";
import type { IslandChannel } from "./island_channel";
import type { RenderIsland, VoWebModule } from "./render_bootstrap";

// ── RendererModule contract (mirrors studio/src/lib/gui/render_island.ts) ─────

interface RendererHost {
  moduleBytes: Uint8Array;
  log(message: string): void;
  reportError(message: string): void;
  getCapability(name: "widget"): WidgetCapability | null;
  getCapability(name: "island_transport"): IslandTransportCapability | null;
  getCapability(name: "vo_web"): VoWebCapability | null;
  getCapability(name: "vfs"): VfsCapability | null;
}

interface WidgetFactory {
  create(
    container: HTMLElement,
    props: Record<string, unknown>,
    onEvent: (payload: string) => void,
  ): { update(props: Record<string, unknown>): void; destroy(): void };
}

interface WidgetCapability {
  register(name: string, factory: WidgetFactory): void;
}

interface IslandTransportCapability {
  createChannel(): Promise<IslandChannel>;
}

interface VoWebCapability {
  getVoWeb(): Promise<VoWebModule>;
}

interface VfsCapability {
  getBytes(path: string): Uint8Array | null;
}

type HostServices = {
  moduleBytes: Uint8Array;
  createChannel(): Promise<IslandChannel>;
  getVoWeb(): Promise<VoWebModule>;
  getVfsBytes(path: string): Uint8Array | null;
  debugLog(message: string): void;
  reportError(message: string): void;
};

type VoplayDebugGlobal = typeof globalThis & {
  __voplayDebugStatus?: (message: string) => void;
};

// Relative paths within the framework's VFS snapshot for the island WASM.
// These match wasm-pack output names for the `wasm-island` feature build.
const WASM_BG_PATH = "wasm/voplay_island_bg.wasm";
const WASM_JS_PATH = "wasm/voplay_island.js";

const activeInstances = new Set<RenderIslandWidgetInstance>();
let widgetSequence = 0;
let hostServices: HostServices | null = null;

function describeError(error: unknown): string {
  if (error instanceof Error) {
    return `${error.name}: ${error.message}`;
  }
  return String(error);
}

function requireCapability<T>(value: T | null, name: string): T {
  if (value == null) {
    throw new Error(`[voplay] missing renderer host capability: ${name}`);
  }
  return value;
}

function makeCanvasId(props: Record<string, unknown>): string {
  const prefix = typeof props.canvasId === "string" && props.canvasId.trim().length > 0
    ? props.canvasId.trim()
    : "voplay-canvas";
  widgetSequence += 1;
  return `${prefix}-${widgetSequence}`;
}

function shouldShowDebugOverlay(): boolean {
  try {
    const params = new URLSearchParams(window.location.search);
    return params.has("rendererDebug")
      || params.has("debug")
      || window.localStorage.getItem("voplay.rendererDebug") === "1";
  } catch {
    return false;
  }
}

class RenderIslandWidgetInstance {
  private readonly canvas: HTMLCanvasElement;
  private readonly canvasId: string;
  private readonly resizeObserver: ResizeObserver;
  private readonly debugOverlay: HTMLDivElement | null = null;
  private readonly debugStatusCallback: ((message: string) => void) | null = null;
  private island: RenderIsland | null = null;
  private channel: IslandChannel | null = null;
  private readonly debugMessages: string[] = [];
  private destroyed = false;
  private mounted = false;

  constructor(
    private readonly container: HTMLElement,
    props: Record<string, unknown>,
    private readonly onEvent: (payload: string) => void,
    private readonly services: HostServices,
  ) {
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
      this.debugStatusCallback = (message: string) => this.setDebugStatus(message);
      (globalThis as VoplayDebugGlobal).__voplayDebugStatus = this.debugStatusCallback;
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

  update(_props: Record<string, unknown>): void {}

  destroy(): void {
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
    } else if (this.channel) {
      this.channel.close();
      this.channel = null;
    }
    const debugGlobal = globalThis as VoplayDebugGlobal;
    if (this.debugStatusCallback && debugGlobal.__voplayDebugStatus === this.debugStatusCallback) {
      delete debugGlobal.__voplayDebugStatus;
    }
    this.debugOverlay?.remove();
    this.canvas.remove();
    activeInstances.delete(this);
  }

  private async start(): Promise<void> {
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

    let channel: IslandChannel | null = null;
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
      const island = await bootstrapWebView(
        {
          canvasId: this.canvasId,
          bytecode: this.services.moduleBytes,
          voplayWasm: wasmBytes,
          voplayWasmJsGlue: jsGlueBytes,
        },
        voWeb,
        channel,
        (message) => this.debug(message),
        this.services.reportError,
      );
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
    } catch (error) {
      channel?.close();
      this.services.reportError(`[voplay] ${describeError(error)}`);
    }
  }

  private emitWidgetEvent(type: "mount" | "resize"): void {
    const width = Math.round(this.container.clientWidth);
    const height = Math.round(this.container.clientHeight);
    if (width <= 0 || height <= 0) {
      this.debug(`[voplay] widget.event.skip type=${type} canvasId=${this.canvasId} width=${width} height=${height}`);
      return;
    }
    this.debug(`[voplay] widget.event type=${type} canvasId=${this.canvasId} width=${width} height=${height}`);
    this.onEvent(JSON.stringify({ type, width, height, canvasId: this.canvasId }));
  }

  private debug(message: string): void {
    this.services.debugLog(message);
    this.setDebugStatus(message);
  }

  private setDebugStatus(message: string): void {
    if (this.debugOverlay) {
      this.debugMessages.push(message);
      while (this.debugMessages.length > 6) {
        this.debugMessages.shift();
      }
      this.debugOverlay.textContent = this.debugMessages.join("\n");
    }
  }
}

export async function init(host: RendererHost): Promise<void> {
  const widget = requireCapability(host.getCapability("widget"), "widget");
  const islandTransport = requireCapability(host.getCapability("island_transport"), "island_transport");
  const voWeb = requireCapability(host.getCapability("vo_web"), "vo_web");
  const vfs = requireCapability(host.getCapability("vfs"), "vfs");
  hostServices = {
    moduleBytes: host.moduleBytes,
    createChannel: () => islandTransport.createChannel(),
    getVoWeb: () => voWeb.getVoWeb(),
    getVfsBytes: (path: string) => vfs.getBytes(path),
    debugLog: (message: string) => host.log(message),
    reportError: (message: string) => host.reportError(message),
  };
  host.log('[voplay] init');
  widget.register("voplay-render-island", {
    create(container: HTMLElement, props: Record<string, unknown>, onEvent: (payload: string) => void) {
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

export function render(_container: HTMLElement, _bytes: Uint8Array): void {
  // voplay renders directly to the WebGPU canvas via the render island VM.
  // Render bytes from the logic island are dispatched through the island channel,
  // not delivered here.
}

export function stop(): void {
  hostServices?.debugLog('[voplay] stop');
  for (const instance of Array.from(activeInstances)) {
    instance.destroy();
  }
  activeInstances.clear();
  hostServices = null;
  stopWebView();
}

export default { init, render, stop };
