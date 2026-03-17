// WebView bootstrap adapter for Studio.
// Provides canvas from DOM and a host-provided transport.

import type { IslandChannel } from "./island_channel";
import { RenderIsland, type VoWebModule, type RenderIslandConfig } from "./render_bootstrap";

export interface WebViewBootstrapConfig {
  canvasId: string;
  bytecode: Uint8Array;
  voplayWasm: Uint8Array;
  // Optional wasm-bindgen JS glue bytes for voplay_island.wasm (DOM/WebGPU access).
  voplayWasmJsGlue?: Uint8Array | null;
}

let currentIsland: RenderIsland | null = null;

export async function bootstrapWebView(
  config: WebViewBootstrapConfig,
  voWeb: VoWebModule,
  channel: IslandChannel,
  debugLog?: (message: string) => void,
  onError?: (message: string) => void,
): Promise<RenderIsland> {
  // Stop any existing island
  currentIsland?.stop();

  // Create and start render island
  const island = new RenderIsland({
    bytecode: config.bytecode,
    voplayWasm: config.voplayWasm,
    voplayWasmJsGlue: config.voplayWasmJsGlue,
    channel,
    canvasId: config.canvasId,
    debugLog,
    onError,
  });

  await island.init(voWeb);
  island.start();

  currentIsland = island;
  return island;
}

export function stopWebView(): void {
  currentIsland?.stop();
  currentIsland = null;
}

export function installInputHandlers(canvasId: string): () => void {
  const canvas = document.getElementById(canvasId) as HTMLCanvasElement | null;
  if (!canvas) return () => {};

  // Input handlers will be managed by voplay externs inside the VM.
  // This function is a placeholder for any additional setup needed.
  return () => {};
}
