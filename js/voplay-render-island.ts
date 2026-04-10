// voplay-render-island.ts
// Implements the RendererModule interface for Studio's framework-neutral render island.
// Studio loads this file dynamically from the VFS snapshot and calls init(host).

import { bootstrapWebView, stopWebView } from "./bootstrap_webview";
import type { IslandChannel } from "./island_channel";
import type { RenderIsland, VoWebModule } from "./render_bootstrap";

// ── RendererModule contract (mirrors studio/src/lib/gui/render_island.ts) ─────

interface StudioGuiHost {
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

class RenderIslandWidgetInstance {
  private readonly canvas: HTMLCanvasElement;
  private readonly canvasId: string;
  private readonly resizeObserver: ResizeObserver;
  private island: RenderIsland | null = null;
  private channel: IslandChannel | null = null;
  private destroyed = false;
  private mounted = false;

  constructor(
    private readonly container: HTMLElement,
    props: Record<string, unknown>,
    private readonly onEvent: (payload: string) => void,
    private readonly services: HostServices,
  ) {
    this.canvasId = makeCanvasId(props);
    this.services.debugLog(`[voplay] widget.create canvasId=${this.canvasId}`);
    this.canvas = document.createElement("canvas");
    this.canvas.id = this.canvasId;
    this.canvas.tabIndex = 0;
    this.canvas.style.width = "100%";
    this.canvas.style.height = "100%";
    this.canvas.style.display = "block";
    this.container.style.display = "block";
    this.container.style.overflow = "hidden";
    this.container.appendChild(this.canvas);
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
    this.services.debugLog(`[voplay] widget.destroy canvasId=${this.canvasId}`);
    this.resizeObserver.disconnect();
    if (this.island) {
      stopWebView(this.island);
      this.island = null;
      this.channel = null;
    } else if (this.channel) {
      this.channel.close();
      this.channel = null;
    }
    this.canvas.remove();
    activeInstances.delete(this);
  }

  private async start(): Promise<void> {
    this.services.debugLog(`[voplay] island.start.begin canvasId=${this.canvasId}`);
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
      this.services.debugLog(`[voplay] island.channel.create.begin canvasId=${this.canvasId}`);
      channel = await this.services.createChannel();
      this.services.debugLog(`[voplay] island.channel.create.ready canvasId=${this.canvasId}`);
      if (this.destroyed) {
        channel.close();
        return;
      }
      this.services.debugLog(`[voplay] island.channel.init.begin canvasId=${this.canvasId}`);
      await channel.init();
      this.services.debugLog(`[voplay] island.channel.init.ready canvasId=${this.canvasId}`);
      if (this.destroyed) {
        channel.close();
        return;
      }
      this.services.debugLog(`[voplay] island.voweb.begin canvasId=${this.canvasId}`);
      const voWeb = await this.services.getVoWeb();
      this.services.debugLog(`[voplay] island.voweb.ready canvasId=${this.canvasId}`);
      this.services.debugLog(`[voplay] island.bootstrap.begin canvasId=${this.canvasId}`);
      const island = await bootstrapWebView(
        {
          canvasId: this.canvasId,
          bytecode: this.services.moduleBytes,
          voplayWasm: wasmBytes,
          voplayWasmJsGlue: jsGlueBytes,
        },
        voWeb,
        channel,
        this.services.debugLog,
        this.services.reportError,
      );
      this.services.debugLog(`[voplay] island.bootstrap.ready canvasId=${this.canvasId}`);
      if (this.destroyed) {
        stopWebView(island);
        return;
      }
      this.channel = channel;
      this.island = island;
      this.mounted = true;
      this.services.debugLog(`[voplay] island.start.ready canvasId=${this.canvasId}`);
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
      this.services.debugLog(`[voplay] widget.event.skip type=${type} canvasId=${this.canvasId} width=${width} height=${height}`);
      return;
    }
    this.services.debugLog(`[voplay] widget.event type=${type} canvasId=${this.canvasId} width=${width} height=${height}`);
    this.onEvent(JSON.stringify({ type, width, height, canvasId: this.canvasId }));
  }
}

export async function init(host: StudioGuiHost): Promise<void> {
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
