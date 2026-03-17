export interface IslandChannel {
    init(): Promise<void>;
    send(frame: Uint8Array): void;
    onReceive(handler: (frame: Uint8Array) => void): void;
    close(): void;
}
export declare class TauriChannel implements IslandChannel {
    private unlisten;
    private handler;
    init(): Promise<void>;
    send(frame: Uint8Array): void;
    onReceive(handler: (frame: Uint8Array) => void): void;
    close(): void;
}
export declare class WorkerChannel implements IslandChannel {
    private handler;
    init(): Promise<void>;
    send(frame: Uint8Array): void;
    onReceive(handler: (frame: Uint8Array) => void): void;
    close(): void;
}
export declare class HostChannel implements IslandChannel {
    private worker;
    private handler;
    constructor(worker: Worker);
    init(): Promise<void>;
    send(frame: Uint8Array): void;
    onReceive(handler: (frame: Uint8Array) => void): void;
    close(): void;
}
