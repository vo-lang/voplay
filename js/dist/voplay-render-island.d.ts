import type { IslandChannel } from "./island_channel";
import type { VoWebModule } from "./render_bootstrap";
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
    create(container: HTMLElement, props: Record<string, unknown>, onEvent: (payload: string) => void): {
        update(props: Record<string, unknown>): void;
        destroy(): void;
    };
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
export declare function init(host: RendererHost): Promise<void>;
export declare function render(_container: HTMLElement, _bytes: Uint8Array): void;
export declare function stop(): void;
declare const _default: {
    init: typeof init;
    render: typeof render;
    stop: typeof stop;
};
export default _default;
