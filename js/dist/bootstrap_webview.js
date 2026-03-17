// WebView bootstrap adapter for Studio.
// Provides canvas from DOM and a host-provided transport.
import { RenderIsland } from "./render_bootstrap";
let currentIsland = null;
export async function bootstrapWebView(config, voWeb, channel, debugLog, onError) {
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
export function stopWebView() {
    currentIsland?.stop();
    currentIsland = null;
}
export function installInputHandlers(canvasId) {
    const canvas = document.getElementById(canvasId);
    if (!canvas)
        return () => { };
    // Input handlers will be managed by voplay externs inside the VM.
    // This function is a placeholder for any additional setup needed.
    return () => { };
}
