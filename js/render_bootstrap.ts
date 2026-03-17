// Render island bootstrap: load wasm VM, wire transport, start render loop.
// Per design doc: JS is bootstrap only; render loop runs inside Vo VM.

import type { IslandChannel } from "./island_channel";

// VoVm interface from vo-web (dynamically loaded)
export interface VoVm {
  run(): string;
  runInit(): string;
  runScheduled(): string;
  pushIslandCommand(frame: Uint8Array): void;
  takeOutboundCommands(): Uint8Array[];
  takePendingHostEvents(): Array<{ token: string; delayMs: number; replay: boolean }>;
  wakeHostEvent(token: string): void;
  takeOutput(): string;
}

export interface VoWebModule {
  initVFS(): Promise<void>;
  VoVm: { withExterns(bytecode: Uint8Array): VoVm };
  preloadExtModule(path: string, bytes: Uint8Array, jsGlueUrl?: string): Promise<void>;
}

export interface RenderIslandConfig {
  bytecode: Uint8Array;
  voplayWasm: Uint8Array;
  // Optional wasm-bindgen JS glue bytes for voplay_island.wasm.
  // When provided, the module is loaded as a full bindgen module (DOM/WebGPU access).
  // When absent, falls back to standalone C-ABI mode.
  voplayWasmJsGlue?: Uint8Array | null;
  channel: IslandChannel;
  canvasId: string;
  debugLog?: (message: string) => void;
  onError?: (message: string) => void;
}

type VoplayDebugGlobal = typeof globalThis & {
  __voVoplayDebugLog?: (message: string) => void;
};

// RenderIsland: manages the render island VM lifecycle.
export class RenderIsland {
  private config: RenderIslandConfig;
  private vm: VoVm | null = null;
  private hostTimers = new Map<string, number>();
  private stopped = false;
  private recentConsoleErrors: string[] = [];
  private originalConsoleError: ((...args: unknown[]) => void) | null = null;

  constructor(config: RenderIslandConfig) {
    this.config = config;
  }

  async init(voWeb: VoWebModule): Promise<void> {
    const globalScope = globalThis as VoplayDebugGlobal;
    globalScope.__voVoplayDebugLog = (message: string) => this.debug(`voplay wasm ${message}`);
    this.installConsoleErrorCapture();
    await voWeb.initVFS();
    let jsGlueUrl = '';
    let blobUrlToRevoke = '';
    if (this.config.voplayWasmJsGlue && this.config.voplayWasmJsGlue.length > 0) {
      const jsText = new TextDecoder().decode(this.config.voplayWasmJsGlue);
      const blob = new Blob([jsText], { type: 'application/javascript' });
      blobUrlToRevoke = URL.createObjectURL(blob);
      jsGlueUrl = blobUrlToRevoke;
    }
    try {
      await voWeb.preloadExtModule("github.com/vo-lang/voplay", this.config.voplayWasm, jsGlueUrl);
    } finally {
      if (blobUrlToRevoke) URL.revokeObjectURL(blobUrlToRevoke);
    }
    this.vm = voWeb.VoVm.withExterns(this.config.bytecode);
    this.vm.runInit();
  }

  start(): void {
    if (!this.vm) throw new Error("VM not initialized");
    this.stopped = false;

    this.config.channel.onReceive((frame) => {
      try {
        if (this.stopped || !this.vm) return;
        this.vm.pushIslandCommand(frame);
        const outcome = this.vm.runScheduled();
        this.flush();
        this.scheduleHostEvents();
      } catch (error) {
        this.fail(`runScheduled inbound_bytes=${frame.byteLength}`, error);
      }
    });
  }

  stop(): void {
    this.stopped = true;
    for (const id of this.hostTimers.values()) {
      clearTimeout(id);
    }
    this.hostTimers.clear();
    this.config.channel.close();
    this.vm = null;
  }

  private flush(): void {
    if (!this.vm) return;
    const cmds = this.vm.takeOutboundCommands();
    for (const frame of cmds) {
      this.config.channel.send(frame);
    }
  }

  private scheduleHostEvents(): void {
    if (!this.vm || this.stopped) return;
    const events = this.vm.takePendingHostEvents();
    for (const ev of events) {
      if (this.hostTimers.has(ev.token)) continue;
      const id = window.setTimeout(() => {
        try {
          this.hostTimers.delete(ev.token);
          if (this.stopped || !this.vm) return;
          this.vm.wakeHostEvent(ev.token);
          const outcome = this.vm.runScheduled();
          this.flush();
          this.scheduleHostEvents();
        } catch (error) {
          this.fail(`wakeHostEvent token=${ev.token}`, error);
        }
      }, ev.delayMs);
      this.hostTimers.set(ev.token, id);
    }
  }

  private fail(context: string, error: unknown): void {
    let message = `${context}: ${this.describeError(error)}`;
    const captured = this.drainCapturedErrors();
    if (captured.length > 0) {
      message += `\nRust panic: ${captured.join('\n')}`;
    }
    this.debug(`render island fail: ${message}`);
    this.config.onError?.(message);
    this.stop();
  }

  private describeError(error: unknown): string {
    if (error instanceof Error) {
      const extraFields = error as unknown as Record<string, unknown>;
      const extra = Object.entries(extraFields)
        .map(([key, value]) => `${key}=${this.describeValue(value)}`)
        .join(", ");
      let message = `${error.name}: ${error.message}`;
      if (error.stack) {
        message += `\n${error.stack}`;
      }
      if (extra) {
        message += `\nextra: ${extra}`;
      }
      return message;
    }
    if (typeof error === "object" && error !== null) {
      return this.describeValue(error);
    }
    return String(error);
  }

  private describeValue(value: unknown): string {
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean" || value === null || value === undefined) {
      return String(value);
    }
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }

  private debug(message: string): void {
    this.config.debugLog?.(message);
  }

  private installConsoleErrorCapture(): void {
    if (this.originalConsoleError) return;
    this.originalConsoleError = console.error.bind(console);
    console.error = (...args: unknown[]) => {
      const msg = args.map(String).join(' ');
      this.recentConsoleErrors.push(msg);
      if (this.recentConsoleErrors.length > 8) this.recentConsoleErrors.shift();
      this.debug(`[console.error] ${msg}`);
      this.originalConsoleError!(...args);
    };
  }

  private drainCapturedErrors(): string[] {
    const errors = this.recentConsoleErrors.slice();
    this.recentConsoleErrors.length = 0;
    return errors;
  }
}
