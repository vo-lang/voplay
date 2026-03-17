// Island transport channels for render island ↔ native/host communication.
// Per design doc: TauriChannel (Studio), WorkerChannel (Playground worker side),
// HostChannel (Playground main thread side).
// TauriChannel: WebView ↔ Native via invoke + event listener.
export class TauriChannel {
    constructor() {
        this.unlisten = null;
        this.handler = null;
    }
    async init() {
        const { listen } = await import("@tauri-apps/api/event");
        this.unlisten = await listen("island_data", (ev) => {
            this.handler?.(new Uint8Array(ev.payload));
        });
    }
    send(frame) {
        import("@tauri-apps/api/core").then(({ invoke }) => {
            invoke("__island_transport_push", { data: Array.from(frame) });
        });
    }
    onReceive(handler) {
        this.handler = handler;
    }
    close() {
        this.unlisten?.();
        this.unlisten = null;
        this.handler = null;
    }
}
// WorkerChannel: Worker side, communicates with main thread via postMessage.
export class WorkerChannel {
    constructor() {
        this.handler = null;
    }
    async init() {
        self.onmessage = (ev) => {
            if (ev.data instanceof ArrayBuffer) {
                this.handler?.(new Uint8Array(ev.data));
            }
        };
    }
    send(frame) {
        const buf = frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength);
        self
            .postMessage(buf, { transfer: [buf] });
    }
    onReceive(handler) {
        this.handler = handler;
    }
    close() {
        self.onmessage = null;
        this.handler = null;
    }
}
// HostChannel: Main thread side, communicates with a Worker.
export class HostChannel {
    constructor(worker) {
        this.handler = null;
        this.worker = worker;
    }
    async init() {
        this.worker.onmessage = (ev) => {
            if (ev.data instanceof ArrayBuffer) {
                this.handler?.(new Uint8Array(ev.data));
            }
        };
    }
    send(frame) {
        const buf = frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength);
        this.worker.postMessage(buf, [buf]);
    }
    onReceive(handler) {
        this.handler = handler;
    }
    close() {
        this.worker.onmessage = null;
        this.handler = null;
    }
}
