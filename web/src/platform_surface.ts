export interface Handle {
  readonly index: number;
  readonly generation: number;
}

export interface EngineId {
  readonly session: Handle;
  readonly engine: Handle;
}

export interface BrowserSurfaceId {
  readonly engine: EngineId;
  readonly surface: Handle;
  readonly domain: Handle;
}

export interface SurfaceMetrics {
  readonly width: number;
  readonly height: number;
  readonly scaleNumerator: number;
  readonly scaleDenominator: number;
}

export interface BrowserFrameSubmission {
  readonly surface: BrowserSurfaceId;
  readonly pulseId: bigint;
  readonly frameId: bigint;
  readonly renderEndpoint: Handle;
  readonly deviceGeneration: bigint;
  readonly requiredRenderRevision: bigint;
  readonly requiredControlRevision: bigint;
  readonly graphSignature: bigint;
  readonly commands: Uint8Array;
}

export interface BrowserPlatformSubmission {
  readonly fenceValue: bigint;
  readonly deviceGeneration: bigint;
}

export interface BrowserRenderTargetReadback {
  readonly targetRevision: bigint;
  readonly rowBytes: number;
  readonly bytes: Uint8Array;
}

export class BrowserGpuReadbackError extends Error {
  readonly failure: 1 | 2 | 3 | 4;

  constructor(failure: 1 | 2 | 3 | 4, message: string) {
    super(message);
    this.name = "BrowserGpuReadbackError";
    this.failure = failure;
  }
}

export type BrowserPresentOutcome =
  | "presented"
  | "deadlineMissed"
  | "suspended"
  | "surfaceLost"
  | "deviceLost";

export interface BrowserGpuAdapter {
  readonly deviceGeneration: bigint;
  rebindDevice(deviceGeneration: bigint): void | Promise<void>;
  synchronizeRenderTargets?(
    engine: Handle,
    targets: readonly Handle[],
  ): void;
  attach(canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void;
  resize(canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void;
  submit(
    canvas: HTMLCanvasElement,
    commands: Uint8Array,
    graphSignature: bigint,
    engine: Handle,
    renderRevision: bigint,
  ): BrowserPlatformSubmission;
  readRenderTarget?(
    engine: Handle,
    target: Handle,
    expectedRevision: bigint,
    region: Readonly<{ x: number; y: number; width: number; height: number }>,
    format: number,
  ): Promise<BrowserRenderTargetReadback>;
  present(canvas: HTMLCanvasElement, fenceValue: bigint): void;
  detach(canvas: HTMLCanvasElement): void;
  abandon?(canvas: HTMLCanvasElement): void;
  registerTexture?(
    texture: bigint,
    source: CanvasImageSource,
    engine?: Handle,
  ): void;
  removeTexture?(texture: bigint, engine?: Handle): boolean;
  close?(): void;
}

export interface BrowserSurfaceHostConfig {
  readonly maxSurfaces: number;
  readonly maxCommandBytes: number;
}

export interface BrowserSurfaceHostMetrics {
  readonly liveSurfaces: number;
  readonly peakSurfaces: number;
  readonly pendingFrames: number;
  readonly peakPendingFrames: number;
  readonly submittedFrames: bigint;
  readonly submittedCommandBytes: bigint;
  readonly presentedFrames: bigint;
  readonly deadlineMisses: bigint;
  readonly deviceLosses: bigint;
  readonly deviceRebinds: bigint;
  readonly abandonedFrames: bigint;
}

export interface BrowserSurfaceOwnerSnapshot {
  readonly deviceGeneration: bigint;
  readonly metrics: BrowserSurfaceHostMetrics;
  readonly surfaces: readonly BrowserSurfaceId[];
}

interface SurfaceRecord {
  readonly id: BrowserSurfaceId;
  readonly canvas: HTMLCanvasElement;
  metrics: SurfaceMetrics;
  pending?: {
    readonly frameId: bigint;
    readonly submission: BrowserPlatformSubmission;
  };
  suspended: boolean;
}

export class BrowserSurfaceHost {
  readonly #adapter: BrowserGpuAdapter;
  readonly #config: BrowserSurfaceHostConfig;
  readonly #surfaces = new Map<string, SurfaceRecord>();
  #closed = false;
  #peakSurfaces = 0;
  #peakPendingFrames = 0;
  #submittedFrames = 0n;
  #submittedCommandBytes = 0n;
  #presentedFrames = 0n;
  #deadlineMisses = 0n;
  #deviceLosses = 0n;
  #deviceRebinds = 0n;
  #abandonedFrames = 0n;
  #deviceRebind: Promise<void> | null = null;

  constructor(adapter: BrowserGpuAdapter, config: BrowserSurfaceHostConfig) {
    validateConfig(config);
    if (adapter.deviceGeneration <= 0n) throw new Error("invalid browser GPU generation");
    this.#adapter = adapter;
    this.#config = config;
  }

  attach(id: BrowserSurfaceId, canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void {
    this.#assertOpen();
    validateSurfaceId(id);
    validateMetrics(metrics);
    const key = surfaceKey(id);
    if (this.#surfaces.has(key)) throw new Error("surface already attached");
    if (this.#surfaces.size >= this.#config.maxSurfaces) throw new Error("surface capacity");
    this.#adapter.attach(canvas, metrics);
    try {
      applyCanvasMetrics(canvas, metrics);
    } catch (error) {
      try {
        this.#adapter.detach(canvas);
      } catch {
        this.#adapter.abandon?.(canvas);
      }
      throw error;
    }
    this.#surfaces.set(key, { id, canvas, metrics, suspended: false });
    this.#peakSurfaces = Math.max(this.#peakSurfaces, this.#surfaces.size);
  }

  registerTexture(
    texture: bigint,
    source: CanvasImageSource,
    engine?: Handle,
  ): void {
    this.#assertOpen();
    const register = this.#adapter.registerTexture;
    if (register === undefined) throw new Error("browser GPU texture upload unsupported");
    register.call(this.#adapter, texture, source, engine);
  }

  removeTexture(texture: bigint, engine?: Handle): boolean {
    this.#assertOpen();
    const remove = this.#adapter.removeTexture;
    if (remove === undefined) throw new Error("browser GPU texture removal unsupported");
    return remove.call(this.#adapter, texture, engine);
  }

  synchronizeRenderTargets(engine: Handle, targets: readonly Handle[]): void {
    this.#assertOpen();
    const synchronize = this.#adapter.synchronizeRenderTargets;
    if (synchronize === undefined) {
      throw new Error("browser GPU render target synchronization unsupported");
    }
    synchronize.call(this.#adapter, engine, targets);
  }

  readRenderTarget(
    engine: Handle,
    target: Handle,
    expectedRevision: bigint,
    region: Readonly<{ x: number; y: number; width: number; height: number }>,
    format: number,
  ): Promise<BrowserRenderTargetReadback> {
    this.#assertOpen();
    const read = this.#adapter.readRenderTarget;
    if (read === undefined) {
      return Promise.reject(new BrowserGpuReadbackError(
        1,
        "browser GPU readback unsupported",
      ));
    }
    return read.call(
      this.#adapter,
      engine,
      target,
      expectedRevision,
      region,
      format,
    );
  }

  resize(id: BrowserSurfaceId, metrics: SurfaceMetrics): void {
    validateMetrics(metrics);
    const record = this.#surface(id);
    if (record.pending !== undefined) throw new Error("frame pending");
    this.#adapter.resize(record.canvas, metrics);
    applyCanvasMetrics(record.canvas, metrics);
    record.metrics = metrics;
  }

  suspend(id: BrowserSurfaceId): void {
    const record = this.#surface(id);
    if (record.suspended) throw new Error("surface already suspended");
    record.suspended = true;
  }

  resume(id: BrowserSurfaceId): void {
    const record = this.#surface(id);
    if (!record.suspended) throw new Error("surface is active");
    record.suspended = false;
  }

  submit(frame: BrowserFrameSubmission): BrowserPlatformSubmission {
    const record = this.#surface(frame.surface);
    if (record.suspended || record.pending !== undefined) throw new Error("surface unavailable");
    if (
      frame.pulseId <= 0n ||
      frame.frameId <= 0n ||
      frame.deviceGeneration !== this.#adapter.deviceGeneration ||
      frame.commands.byteLength > this.#config.maxCommandBytes
    ) {
      throw new Error("invalid frame submission");
    }
    const submission = this.#adapter.submit(
      record.canvas,
      copyBytes(frame.commands),
      frame.graphSignature,
      frame.surface.engine.engine,
      frame.requiredRenderRevision,
    );
    if (submission.deviceGeneration !== frame.deviceGeneration || submission.fenceValue <= 0n) {
      throw new Error("stale GPU submission");
    }
    record.pending = { frameId: frame.frameId, submission };
    this.#submittedFrames += 1n;
    this.#submittedCommandBytes += BigInt(frame.commands.byteLength);
    this.#peakPendingFrames = Math.max(this.#peakPendingFrames, this.#pendingFrameCount());
    return submission;
  }

  present(
    id: BrowserSurfaceId,
    frameId: bigint,
    fenceValue: bigint,
    nowMicros: bigint,
    deadlineMicros: bigint,
  ): BrowserPresentOutcome {
    const record = this.#surface(id);
    const pending = record.pending;
    if (
      pending === undefined ||
      pending.frameId !== frameId ||
      pending.submission.fenceValue !== fenceValue
    ) {
      throw new Error("frame ACK mismatch");
    }
    if (pending.submission.deviceGeneration !== this.#adapter.deviceGeneration) {
      this.#deviceLosses += 1n;
      return "deviceLost";
    }
    this.#adapter.present(record.canvas, fenceValue);
    delete record.pending;
    this.#presentedFrames += 1n;
    if (nowMicros > deadlineMicros) {
      this.#deadlineMisses += 1n;
      return "deadlineMissed";
    }
    return "presented";
  }

  rebindDevice(
    id: BrowserSurfaceId,
    metrics: SurfaceMetrics,
    deviceGeneration: bigint,
  ): void | Promise<void> {
    if (this.#deviceRebind !== null) {
      return this.#deviceRebind.then(() => this.rebindDevice(
        id,
        metrics,
        deviceGeneration,
      ));
    }
    if (deviceGeneration < this.#adapter.deviceGeneration) {
      throw new Error("stale browser GPU generation");
    }
    validateMetrics(metrics);
    const record = this.#surface(id);
    if (deviceGeneration > this.#adapter.deviceGeneration) {
      const rebound = this.#adapter.rebindDevice(deviceGeneration);
      if (rebound instanceof Promise) {
        let task: Promise<void>;
        task = rebound.then(
          () => {
            if (this.#deviceRebind === task) this.#deviceRebind = null;
            this.#deviceRebinds += 1n;
          },
          (error) => {
            if (this.#deviceRebind === task) this.#deviceRebind = null;
            throw error;
          },
        );
        this.#deviceRebind = task;
        return task.then(() => {
          this.#adapter.resize(record.canvas, metrics);
          applyCanvasMetrics(record.canvas, metrics);
          record.metrics = metrics;
          delete record.pending;
        });
      }
      this.#deviceRebinds += 1n;
    }
    this.#adapter.resize(record.canvas, metrics);
    applyCanvasMetrics(record.canvas, metrics);
    record.metrics = metrics;
    delete record.pending;
  }

  detach(id: BrowserSurfaceId): void {
    const key = surfaceKey(id);
    const record = this.#surface(id);
    if (record.pending !== undefined) throw new Error("frame outcome unknown");
    this.#adapter.detach(record.canvas);
    this.#surfaces.delete(key);
  }

  close(): number {
    if (this.#closed) return 0;
    let abandoned = 0;
    for (const record of this.#surfaces.values()) {
      try {
        this.#adapter.detach(record.canvas);
      } catch {
        abandoned += 1;
        this.#abandonedFrames += 1n;
        this.#adapter.abandon?.(record.canvas);
      }
    }
    this.#surfaces.clear();
    this.#adapter.close?.();
    this.#closed = true;
    return abandoned;
  }

  metrics(): BrowserSurfaceHostMetrics {
    return {
      liveSurfaces: this.#surfaces.size,
      peakSurfaces: this.#peakSurfaces,
      pendingFrames: this.#pendingFrameCount(),
      peakPendingFrames: this.#peakPendingFrames,
      submittedFrames: this.#submittedFrames,
      submittedCommandBytes: this.#submittedCommandBytes,
      presentedFrames: this.#presentedFrames,
      deadlineMisses: this.#deadlineMisses,
      deviceLosses: this.#deviceLosses,
      deviceRebinds: this.#deviceRebinds,
      abandonedFrames: this.#abandonedFrames,
    };
  }

  ownerSnapshot(): BrowserSurfaceOwnerSnapshot {
    return {
      deviceGeneration: this.#adapter.deviceGeneration,
      metrics: this.metrics(),
      surfaces: [...this.#surfaces.values()].map((record) => record.id),
    };
  }

  #pendingFrameCount(): number {
    let pending = 0;
    for (const record of this.#surfaces.values()) {
      if (record.pending !== undefined) pending += 1;
    }
    return pending;
  }

  #surface(id: BrowserSurfaceId): SurfaceRecord {
    this.#assertOpen();
    validateSurfaceId(id);
    const record = this.#surfaces.get(surfaceKey(id));
    if (record === undefined) throw new Error("unknown surface");
    return record;
  }

  #assertOpen(): void {
    if (this.#closed) throw new Error("browser surface host closed");
  }
}

function applyCanvasMetrics(canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void {
  canvas.width = metrics.width;
  canvas.height = metrics.height;
  canvas.style.width = `${(metrics.width * metrics.scaleDenominator) / metrics.scaleNumerator}px`;
  canvas.style.height = `${(metrics.height * metrics.scaleDenominator) / metrics.scaleNumerator}px`;
}

function validateConfig(config: BrowserSurfaceHostConfig): void {
  if (
    !Number.isSafeInteger(config.maxSurfaces) ||
    config.maxSurfaces <= 0 ||
    !Number.isSafeInteger(config.maxCommandBytes) ||
    config.maxCommandBytes <= 0
  ) {
    throw new Error("invalid browser surface config");
  }
}

function validateSurfaceId(id: BrowserSurfaceId): void {
  validateHandle(id.engine.session);
  validateHandle(id.engine.engine);
  validateHandle(id.surface);
  validateHandle(id.domain);
}

function validateMetrics(metrics: SurfaceMetrics): void {
  if (
    !Number.isSafeInteger(metrics.width) ||
    metrics.width <= 0 ||
    !Number.isSafeInteger(metrics.height) ||
    metrics.height <= 0 ||
    !Number.isSafeInteger(metrics.scaleNumerator) ||
    metrics.scaleNumerator <= 0 ||
    !Number.isSafeInteger(metrics.scaleDenominator) ||
    metrics.scaleDenominator <= 0
  ) {
    throw new Error("invalid surface metrics");
  }
}

function validateHandle(handle: Handle): void {
  if (
    !Number.isSafeInteger(handle.index) ||
    handle.index < 0 ||
    handle.index >= 0xffff_ffff ||
    !Number.isSafeInteger(handle.generation) ||
    handle.generation <= 0 ||
    handle.generation > 0xffff_ffff
  ) {
    throw new Error("invalid handle");
  }
}

function surfaceKey(id: BrowserSurfaceId): string {
  return `${id.engine.session.index}:${id.engine.session.generation}/${id.engine.engine.index}:${id.engine.engine.generation}/${id.surface.index}:${id.surface.generation}/${id.domain.index}:${id.domain.generation}`;
}

function copyBytes(bytes: Uint8Array): Uint8Array {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy;
}
