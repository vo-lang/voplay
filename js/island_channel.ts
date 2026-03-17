// Island transport channels for render island ↔ native/host communication.
// Per design doc: TauriChannel (Studio), WorkerChannel (Playground worker side),
// HostChannel (Playground main thread side).

export interface IslandChannel {
  init(): Promise<void>;
  send(frame: Uint8Array): void;
  onReceive(handler: (frame: Uint8Array) => void): void;
  close(): void;
}

// TauriChannel: WebView ↔ Native via invoke + event listener.
export class TauriChannel implements IslandChannel {
  private unlisten: (() => void) | null = null;
  private handler: ((frame: Uint8Array) => void) | null = null;

  async init(): Promise<void> {
    const { listen } = await import("@tauri-apps/api/event");
    this.unlisten = await listen<number[]>("island_data", (ev) => {
      this.handler?.(new Uint8Array(ev.payload));
    });
  }

  send(frame: Uint8Array): void {
    import("@tauri-apps/api/core").then(({ invoke }) => {
      invoke("__island_transport_push", { data: Array.from(frame) });
    });
  }

  onReceive(handler: (frame: Uint8Array) => void): void {
    this.handler = handler;
  }

  close(): void {
    this.unlisten?.();
    this.unlisten = null;
    this.handler = null;
  }
}

// WorkerChannel: Worker side, communicates with main thread via postMessage.
export class WorkerChannel implements IslandChannel {
  private handler: ((frame: Uint8Array) => void) | null = null;

  async init(): Promise<void> {
    self.onmessage = (ev: MessageEvent) => {
      if (ev.data instanceof ArrayBuffer) {
        this.handler?.(new Uint8Array(ev.data));
      }
    };
  }

  send(frame: Uint8Array): void {
    const buf = frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength);
    (self as unknown as { postMessage(msg: unknown, opts: { transfer: Transferable[] }): void })
      .postMessage(buf, { transfer: [buf] });
  }

  onReceive(handler: (frame: Uint8Array) => void): void {
    this.handler = handler;
  }

  close(): void {
    self.onmessage = null;
    this.handler = null;
  }
}

// HostChannel: Main thread side, communicates with a Worker.
export class HostChannel implements IslandChannel {
  private worker: Worker;
  private handler: ((frame: Uint8Array) => void) | null = null;

  constructor(worker: Worker) {
    this.worker = worker;
  }

  async init(): Promise<void> {
    this.worker.onmessage = (ev: MessageEvent) => {
      if (ev.data instanceof ArrayBuffer) {
        this.handler?.(new Uint8Array(ev.data));
      }
    };
  }

  send(frame: Uint8Array): void {
    const buf = frame.buffer.slice(frame.byteOffset, frame.byteOffset + frame.byteLength);
    this.worker.postMessage(buf, [buf]);
  }

  onReceive(handler: (frame: Uint8Array) => void): void {
    this.handler = handler;
  }

  close(): void {
    this.worker.onmessage = null;
    this.handler = null;
  }
}
