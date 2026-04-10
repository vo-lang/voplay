import type { IslandChannel } from "./island_channel";
import { RenderIsland, type VoWebModule } from "./render_bootstrap";
export interface WebViewBootstrapConfig {
    canvasId: string;
    bytecode: Uint8Array;
    voplayWasm: Uint8Array;
    voplayWasmJsGlue?: Uint8Array | null;
}
export declare function bootstrapWebView(config: WebViewBootstrapConfig, voWeb: VoWebModule, channel: IslandChannel, debugLog?: (message: string) => void, onError?: (message: string) => void): Promise<RenderIsland>;
export declare function stopWebView(island?: RenderIsland): void;
export declare function installInputHandlers(canvasId: string): () => void;
