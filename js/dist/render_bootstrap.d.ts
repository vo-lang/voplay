import type { IslandChannel } from "./island_channel";
export interface VoVm {
    run(): string;
    runInit(): string;
    runScheduled(): string;
    pushIslandCommand(frame: Uint8Array): void;
    takeOutboundCommands(): Uint8Array[];
    takePendingHostEvents(): Array<{
        token: string;
        delayMs: number;
        replay: boolean;
    }>;
    wakeHostEvent(token: string): void;
    takeOutput(): string;
}
export interface VoWebModule {
    initVFS(): Promise<void>;
    VoVm: {
        withExterns(bytecode: Uint8Array): VoVm;
    };
    preloadExtModule(path: string, bytes: Uint8Array, jsGlueUrl?: string): Promise<void>;
}
export interface RenderIslandConfig {
    bytecode: Uint8Array;
    voplayWasm: Uint8Array;
    voplayWasmJsGlue?: Uint8Array | null;
    channel: IslandChannel;
    canvasId: string;
    debugLog?: (message: string) => void;
    onError?: (message: string) => void;
}
export declare class RenderIsland {
    private config;
    private vm;
    private hostTimers;
    private stopped;
    private recentConsoleErrors;
    private originalConsoleError;
    constructor(config: RenderIslandConfig);
    init(voWeb: VoWebModule): Promise<void>;
    start(): void;
    stop(): void;
    private flush;
    private scheduleHostEvents;
    private fail;
    private describeError;
    private describeValue;
    private debug;
    private installConsoleErrorCapture;
    private drainCapturedErrors;
}
