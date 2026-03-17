import type { IslandChannel } from "./island_channel";
import type { VoWebModule } from "./render_bootstrap";
interface StudioGuiHost {
    getCanvas(): HTMLCanvasElement | null;
    sendEvent(handlerId: number, payload: string): Promise<Uint8Array>;
    createIslandChannel(): Promise<IslandChannel>;
    debugLog(message: string): void;
    reportError(message: string): void;
    voWeb: VoWebModule;
    moduleBytes: Uint8Array;
    getVfsBytes(path: string): Uint8Array | null;
}
export declare function init(host: StudioGuiHost): Promise<void>;
export declare function render(_container: HTMLElement, _bytes: Uint8Array): void;
export declare function stop(): void;
declare const _default: {
    init: typeof init;
    render: typeof render;
    stop: typeof stop;
};
export default _default;
