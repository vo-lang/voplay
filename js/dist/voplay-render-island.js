// voplay-render-island.ts
// Implements the RendererModule interface for Studio's framework-neutral render island.
// Studio loads this file dynamically from the VFS snapshot and calls init(host).
import { bootstrapWebView, stopWebView } from "./bootstrap_webview";
// Relative paths within the framework's VFS snapshot for the island WASM.
// These match wasm-pack output names for the `wasm-island` feature build.
const WASM_BG_PATH = "wasm/voplay_island_bg.wasm";
const WASM_JS_PATH = "wasm/voplay_island.js";
// Canvas id used by voplay's render worker.
const CANVAS_ID = "canvas";
export async function init(host) {
    const channel = await host.createIslandChannel();
    await channel.init();
    const wasmBytes = host.getVfsBytes(WASM_BG_PATH);
    if (!wasmBytes) {
        throw new Error(`[voplay] WASM binary not found in VFS snapshot: ${WASM_BG_PATH}`);
    }
    const jsGlueBytes = host.getVfsBytes(WASM_JS_PATH);
    if (!jsGlueBytes) {
        throw new Error(`[voplay] WASM JS glue not found in VFS snapshot: ${WASM_JS_PATH}`);
    }
    await bootstrapWebView({
        canvasId: CANVAS_ID,
        bytecode: host.moduleBytes,
        voplayWasm: wasmBytes,
        voplayWasmJsGlue: jsGlueBytes,
    }, host.voWeb, channel, host.debugLog, host.reportError);
}
export function render(_container, _bytes) {
    // voplay renders directly to the WebGPU canvas via the render island VM.
    // Render bytes from the logic island are dispatched through the island channel,
    // not delivered here.
}
export function stop() {
    stopWebView();
}
export default { init, render, stop };
