// WebView bootstrap adapter for Studio.
// Provides canvas from DOM and a host-provided transport.
import { RenderIsland } from "./render_bootstrap";
const activeIslands = new Set();
export async function bootstrapWebView(config, voWeb, channel, debugLog, onError) {
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
    activeIslands.add(island);
    return island;
}
export function stopWebView(island) {
    if (island) {
        island.stop();
        activeIslands.delete(island);
        return;
    }
    for (const activeIsland of activeIslands) {
        activeIsland.stop();
    }
    activeIslands.clear();
}
export function installInputHandlers(canvasId) {
    const canvas = document.getElementById(canvasId);
    if (!canvas)
        return () => { };
    // Input handlers will be managed by voplay externs inside the VM.
    // This function is a placeholder for any additional setup needed.
    return () => { };
}
