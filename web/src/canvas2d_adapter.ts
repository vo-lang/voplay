import type {
  BrowserGpuAdapter,
  BrowserPlatformSubmission,
  Handle,
  SurfaceMetrics,
} from "./platform_surface.js";

interface CanvasRecord {
  readonly backbuffer: HTMLCanvasElement;
  pendingFence?: bigint;
}

export interface Canvas2dAdapterConfig {
  readonly deviceGeneration: bigint;
  readonly maxCommands: number;
  readonly maxCommandBytes: number;
}

const FRAME_MAGIC = [0x56, 0x46, 0x43, 0x31] as const;

export class Canvas2dGpuAdapter implements BrowserGpuAdapter {
  #deviceGeneration: bigint;
  readonly #maxCommands: number;
  readonly #maxCommandBytes: number;
  readonly #records = new Map<HTMLCanvasElement, CanvasRecord>();
  readonly #textures = new Map<string, CanvasImageSource>();
  #nextFence = 1n;
  #closed = false;

  constructor(config: Canvas2dAdapterConfig) {
    if (
      config.deviceGeneration <= 0n
      || !Number.isSafeInteger(config.maxCommands)
      || config.maxCommands <= 0
      || !Number.isSafeInteger(config.maxCommandBytes)
      || config.maxCommandBytes < 8
    ) {
      throw new RangeError("invalid Canvas2D adapter config");
    }
    this.#deviceGeneration = config.deviceGeneration;
    this.#maxCommands = config.maxCommands;
    this.#maxCommandBytes = config.maxCommandBytes;
  }

  get deviceGeneration(): bigint {
    return this.#deviceGeneration;
  }

  rebindDevice(deviceGeneration: bigint): void {
    this.#assertOpen();
    if (deviceGeneration <= this.#deviceGeneration) {
      throw new RangeError("stale Canvas2D device generation");
    }
    this.#deviceGeneration = deviceGeneration;
    this.#nextFence = 1n;
    for (const record of this.#records.values()) delete record.pendingFence;
  }

  registerTexture(texture: bigint, source: CanvasImageSource, engine?: Handle): void {
    this.#assertOpen();
    if (texture <= 0n) throw new RangeError("invalid Canvas2D texture");
    if (engine !== undefined) validateHandle(engine);
    this.#textures.set(textureKey(engine === undefined ? "global" : handleScope(engine), texture), source);
  }

  removeTexture(texture: bigint, engine?: Handle): boolean {
    this.#assertOpen();
    if (engine !== undefined) validateHandle(engine);
    return this.#textures.delete(
      textureKey(engine === undefined ? "global" : handleScope(engine), texture),
    );
  }

  attach(canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void {
    this.#assertOpen();
    if (this.#records.has(canvas)) throw new Error("Canvas2D surface already attached");
    const backbuffer = document.createElement("canvas");
    resizeCanvas(backbuffer, metrics);
    resizeCanvas(canvas, metrics);
    this.#records.set(canvas, { backbuffer });
  }

  resize(canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void {
    this.#assertOpen();
    const record = this.#record(canvas);
    if (record.pendingFence !== undefined) throw new Error("Canvas2D frame pending");
    resizeCanvas(record.backbuffer, metrics);
    resizeCanvas(canvas, metrics);
  }

  submit(
    canvas: HTMLCanvasElement,
    commands: Uint8Array,
    graphSignature: bigint,
    engine: Handle,
    renderRevision: bigint,
  ): BrowserPlatformSubmission {
    this.#assertOpen();
    const record = this.#record(canvas);
    if (record.pendingFence !== undefined) throw new Error("Canvas2D frame pending");
    validateHandle(engine);
    if (
      graphSignature <= 0n
      || renderRevision <= 0n
      || commands.byteLength > this.#maxCommandBytes
    ) {
      throw new RangeError("invalid Canvas2D submission");
    }
    const textures = new Map<bigint, CanvasImageSource>();
    for (const [key, source] of this.#textures) {
      const identity = textureIdentity(key, "global");
      if (identity !== null) textures.set(identity, source);
    }
    const scope = handleScope(engine);
    for (const [key, source] of this.#textures) {
      const identity = textureIdentity(key, scope);
      if (identity !== null) textures.set(identity, source);
    }
    rasterPortableFrame(
      record.backbuffer,
      portableFrameCommands(commands),
      this.#maxCommands,
      textures,
    );
    if (this.#nextFence > 0xffff_ffff_ffff_ffffn) {
      throw new RangeError("Canvas2D fence exhausted");
    }
    const fenceValue = this.#nextFence;
    this.#nextFence += 1n;
    record.pendingFence = fenceValue;
    return { fenceValue, deviceGeneration: this.deviceGeneration };
  }

  present(canvas: HTMLCanvasElement, fenceValue: bigint): void {
    this.#assertOpen();
    const record = this.#record(canvas);
    if (record.pendingFence !== fenceValue) throw new Error("Canvas2D fence mismatch");
    const context = canvas.getContext("2d", { alpha: true });
    if (context === null) throw new Error("Canvas2D context unavailable");
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.drawImage(record.backbuffer, 0, 0);
    delete record.pendingFence;
  }

  detach(canvas: HTMLCanvasElement): void {
    this.#assertOpen();
    const record = this.#record(canvas);
    if (record.pendingFence !== undefined) throw new Error("Canvas2D frame outcome unknown");
    const context = canvas.getContext("2d");
    context?.clearRect(0, 0, canvas.width, canvas.height);
    this.#records.delete(canvas);
  }

  abandon(canvas: HTMLCanvasElement): void {
    this.#assertOpen();
    const record = this.#record(canvas);
    const context = canvas.getContext("2d");
    context?.clearRect(0, 0, canvas.width, canvas.height);
    delete record.pendingFence;
    this.#records.delete(canvas);
  }

  close(): void {
    if (this.#closed) return;
    for (const canvas of [...this.#records.keys()]) this.abandon(canvas);
    this.#textures.clear();
    this.#closed = true;
  }

  #record(canvas: HTMLCanvasElement): CanvasRecord {
    const record = this.#records.get(canvas);
    if (record === undefined) throw new Error("unknown Canvas2D surface");
    return record;
  }

  #assertOpen(): void {
    if (this.#closed) throw new Error("Canvas2D adapter closed");
  }
}

export function portableFrameCommands(bytes: Uint8Array): Uint8Array {
  if (
    bytes.byteLength < 4
    || bytes[0] !== 0x56
    || bytes[1] !== 0x33
    || bytes[2] !== 0x46
    || bytes[3] !== 0x31
  ) {
    return bytes;
  }
  if (
    bytes.byteLength < 16
    || bytes[4]! < 1
    || bytes[4]! > 5
    || bytes[5] !== 1
    || bytes[6] !== 0
    || bytes[7] !== 0
  ) {
    throw new RangeError("invalid Voplay 3D scene frame");
  }
  const headerBytes = bytes[4]! >= 2 ? 20 : 16;
  if (bytes.byteLength < headerBytes) throw new RangeError("truncated Voplay 3D scene frame");
  const fallbackBytes = new DataView(
    bytes.buffer,
    bytes.byteOffset,
    bytes.byteLength,
  ).getUint32(8, true);
  const end = headerBytes + fallbackBytes;
  if (
    fallbackBytes === 0
    || !Number.isSafeInteger(end)
    || end > bytes.byteLength
  ) {
    throw new RangeError("invalid Voplay 3D portable fallback");
  }
  return bytes.subarray(headerBytes, end);
}

export function rasterPortableFrame(
  target: HTMLCanvasElement,
  bytes: Uint8Array,
  maxCommands: number,
  textures: ReadonlyMap<bigint, CanvasImageSource> = new Map(),
): void {
  if (!(bytes instanceof Uint8Array) || bytes.byteLength < 8) {
    throw new RangeError("truncated portable frame");
  }
  for (let index = 0; index < FRAME_MAGIC.length; index += 1) {
    if (bytes[index] !== FRAME_MAGIC[index]) throw new RangeError("invalid portable frame magic");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const count = view.getUint32(4, true);
  if (count > maxCommands) throw new RangeError("portable frame command capacity");
  const context = target.getContext("2d", { alpha: true });
  if (context === null) throw new Error("Canvas2D backbuffer unavailable");
  let offset = 8;
  for (let index = 0; index < count; index += 1) {
    const tag = takeU8(bytes, offset);
    offset += 1;
    switch (tag) {
      case 1: {
        const color = takeColor(bytes, offset);
        offset += 4;
        context.save();
        context.globalCompositeOperation = "copy";
        context.fillStyle = rgba(color);
        context.fillRect(0, 0, target.width, target.height);
        context.restore();
        break;
      }
      case 2: {
        requireBytes(bytes, offset, 20);
        const x = view.getUint32(offset, true);
        const y = view.getUint32(offset + 4, true);
        const width = view.getUint32(offset + 8, true);
        const height = view.getUint32(offset + 12, true);
        const color = takeColor(bytes, offset + 16);
        offset += 20;
        const clippedWidth = Math.min(width, Math.max(0, target.width - Math.min(x, target.width)));
        const clippedHeight = Math.min(height, Math.max(0, target.height - Math.min(y, target.height)));
        if (clippedWidth > 0 && clippedHeight > 0) {
          context.fillStyle = rgba(color);
          context.fillRect(Math.min(x, target.width), Math.min(y, target.height), clippedWidth, clippedHeight);
        }
        break;
      }
      case 3: {
        requireBytes(bytes, offset, 44);
        const texture = view.getBigUint64(offset, true);
        const x = view.getUint32(offset + 8, true);
        const y = view.getUint32(offset + 12, true);
        const width = view.getUint32(offset + 16, true);
        const height = view.getUint32(offset + 20, true);
        const sourceX = view.getUint32(offset + 24, true);
        const sourceY = view.getUint32(offset + 28, true);
        const sourceWidth = view.getUint32(offset + 32, true);
        const sourceHeight = view.getUint32(offset + 36, true);
        const tint = takeColor(bytes, offset + 40);
        offset += 44;
        const source = textures.get(texture);
        if (
          texture <= 0n
          || source === undefined
          || width === 0
          || height === 0
          || sourceWidth === 0
          || sourceHeight === 0
        ) {
          throw new RangeError("invalid Canvas2D textured rectangle");
        }
        drawTintedImage(
          context,
          source,
          sourceX,
          sourceY,
          sourceWidth,
          sourceHeight,
          x,
          y,
          width,
          height,
          tint,
        );
        break;
      }
      default:
        throw new RangeError(`unsupported portable frame command ${tag}`);
    }
  }
  if (offset !== bytes.byteLength) throw new RangeError("portable frame trailing bytes");
}

function drawTintedImage(
  context: CanvasRenderingContext2D,
  source: CanvasImageSource,
  sourceX: number,
  sourceY: number,
  sourceWidth: number,
  sourceHeight: number,
  x: number,
  y: number,
  width: number,
  height: number,
  tint: readonly [number, number, number, number],
): void {
  if (tint[0] === 255 && tint[1] === 255 && tint[2] === 255 && tint[3] === 255) {
    context.drawImage(
      source,
      sourceX,
      sourceY,
      sourceWidth,
      sourceHeight,
      x,
      y,
      width,
      height,
    );
    return;
  }
  const scratch = document.createElement("canvas");
  scratch.width = width;
  scratch.height = height;
  const scratchContext = scratch.getContext("2d", { alpha: true });
  if (scratchContext === null) throw new Error("Canvas2D sprite scratch unavailable");
  scratchContext.drawImage(
    source,
    sourceX,
    sourceY,
    sourceWidth,
    sourceHeight,
    0,
    0,
    width,
    height,
  );
  scratchContext.globalCompositeOperation = "multiply";
  scratchContext.fillStyle = `rgb(${tint[0]}, ${tint[1]}, ${tint[2]})`;
  scratchContext.fillRect(0, 0, width, height);
  scratchContext.globalCompositeOperation = "destination-in";
  scratchContext.globalAlpha = tint[3] / 255;
  scratchContext.drawImage(
    source,
    sourceX,
    sourceY,
    sourceWidth,
    sourceHeight,
    0,
    0,
    width,
    height,
  );
  context.drawImage(scratch, x, y);
}

function resizeCanvas(canvas: HTMLCanvasElement, metrics: SurfaceMetrics): void {
  canvas.width = metrics.width;
  canvas.height = metrics.height;
}

function validateHandle(handle: Handle): void {
  if (
    !Number.isSafeInteger(handle.index)
    || handle.index < 0
    || handle.index >= 0xffff_ffff
    || !Number.isSafeInteger(handle.generation)
    || handle.generation <= 0
    || handle.generation > 0xffff_ffff
  ) {
    throw new RangeError("invalid Canvas2D handle");
  }
}

function handleScope(handle: Handle): string {
  return `${handle.index}:${handle.generation}`;
}

function textureKey(scope: string, texture: bigint): string {
  return `${scope}/${texture}`;
}

function textureIdentity(key: string, scope: string): bigint | null {
  const prefix = `${scope}/`;
  if (!key.startsWith(prefix)) return null;
  const identity = BigInt(key.slice(prefix.length));
  return identity > 0n ? identity : null;
}

function takeU8(bytes: Uint8Array, offset: number): number {
  requireBytes(bytes, offset, 1);
  return bytes[offset]!;
}

function takeColor(bytes: Uint8Array, offset: number): readonly [number, number, number, number] {
  requireBytes(bytes, offset, 4);
  return [bytes[offset]!, bytes[offset + 1]!, bytes[offset + 2]!, bytes[offset + 3]!];
}

function requireBytes(bytes: Uint8Array, offset: number, length: number): void {
  const end = offset + length;
  if (!Number.isSafeInteger(end) || end > bytes.byteLength) {
    throw new RangeError("truncated portable frame command");
  }
}

function rgba(color: readonly [number, number, number, number]): string {
  return `rgba(${color[0]}, ${color[1]}, ${color[2]}, ${color[3] / 255})`;
}
