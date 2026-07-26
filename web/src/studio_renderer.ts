import { WebGpuCanvasAdapter } from "./webgpu_adapter.js";
import {
  decodeFrameSubmission,
  decodeSurfaceControl,
  encodeFrameOutcome,
  encodePlatformInput,
  surfaceKey,
  type BrowserFrameTerminal,
  type BrowserPlatformInputEvent,
  type BrowserSurfaceControl,
} from "./framework_lane.js";
import {
  BrowserGpuReadbackError,
  BrowserSurfaceHost,
  type BrowserSurfaceOwnerSnapshot,
  type BrowserSurfaceId,
} from "./platform_surface.js";
import {
  MessageKind,
  decodeFrameworkPacket,
  encodeFrameworkPacket,
} from "../../protocol/generated/voplay_protocol.js";
import {
  BrowserHapticsHost,
  type BrowserHapticsResult,
} from "./haptics.js";
import {
  BrowserGamepadInputSource,
  type BrowserGamepadEvent,
} from "./gamepad_input.js";

interface FrameworkLaneBinding {
  readonly session: { readonly index: number; readonly generation: number };
  readonly sessionEpoch: number;
  readonly channelEpoch: number;
}

interface StudioFrameworkLane {
  readonly binding: FrameworkLaneBinding;
  poll(): Promise<Uint8Array | null>;
  submit(payload: Uint8Array, requestId?: bigint): Promise<void>;
  close(): void;
}

interface FrameworkLaneCapability {
  open(role?: string): Promise<StudioFrameworkLane>;
}

interface AppSurfaceIdentity {
  readonly sessionId: number;
  readonly session: { readonly index: number; readonly generation: number };
  readonly sessionEpoch: bigint;
  readonly window: { readonly index: number; readonly generation: number };
  readonly view: { readonly index: number; readonly generation: number };
  readonly surface: { readonly index: number; readonly generation: number };
}

interface AppSurfaceLease {
  readonly element: HTMLDivElement | HTMLCanvasElement;
  release(): void;
}

interface AppSurfaceCapability {
  readonly sessionId: number;
  isInteractive(): boolean;
  resolve(surface: Readonly<{ index: number; generation: number }>): Promise<Readonly<{
    session: Readonly<{ index: number; generation: number }>;
    sessionEpoch: bigint;
    window: Readonly<{ index: number; generation: number }>;
    view: Readonly<{ index: number; generation: number }>;
    surface: Readonly<{ index: number; generation: number }>;
    kind: "game" | "ui" | "diagnostics";
    zOrder: number;
    inputPolicy: "observe" | "passthrough" | "interactive" | "exclusive";
  }>>;
  attach(descriptor: {
    readonly identity: AppSurfaceIdentity;
    readonly kind: "canvas";
    readonly layer: number;
    readonly input: "opaque" | "transparent" | "passthrough";
    readonly label: string;
  }): AppSurfaceLease;
  subscribeInput(sink: (event: BrowserPlatformInputEvent) => void): () => void;
  capturePointer(pointerId: number, identity: AppSurfaceIdentity): void;
  releasePointer(pointerId: number): void;
  focus(identity: AppSurfaceIdentity): void;
}

interface StudioRendererHost {
  readonly framework: Readonly<{ name: string; roles: readonly string[] }>;
  log(message: string): void;
  reportError(message: string): void;
  getCapability(name: "framework_lane"): FrameworkLaneCapability | null;
  getCapability(name: "app_surface"): AppSurfaceCapability | null;
}

interface SurfaceRecord {
  control: BrowserSurfaceControl;
  readonly id: BrowserSurfaceId;
  readonly lease: AppSurfaceLease;
}

const MAX_COMMAND_BYTES = 16 * 1024 * 1024;
const MAX_COMMANDS = 131_072;
const MAX_TEXTURES = 65_536;
const MAX_TEXTURE_BYTES = 512 * 1024 * 1024;
const MAX_PROFILE_ASSETS = 65_536;
const MAX_PROFILE_ASSET_BYTES = 512 * 1024 * 1024;
const MAX_PENDING_INPUT_RETURNS = 1024;
const MAX_COALESCED_INPUTS = 256;
const MAX_PENDING_READBACKS = 64;
const MAX_PENDING_READBACK_BYTES = 64 * 1024 * 1024;
const MAX_SINGLE_READBACK_BYTES = 16 * 1024 * 1024;
const MAX_RETAINED_FRAME_TRACES = 64;

class VoplayStudioRenderer {
  #host: StudioRendererHost | null = null;
  #lane: StudioFrameworkLane | null = null;
  #surfaceCapability: AppSurfaceCapability | null = null;
  #surfaceHost: BrowserSurfaceHost | null = null;
  #haptics: BrowserHapticsHost | null = null;
  #gamepads: BrowserGamepadInputSource | null = null;
  #surfaces = new Map<string, SurfaceRecord>();
  #unsubscribeInput: (() => void) | null = null;
  #coalescedInputs = new Map<string, BrowserPlatformInputEvent>();
  #drainingCoalesced = new Set<string>();
  #polling = false;
  #lastInboundSequence = 0n;
  #nextReturnSequence = 1n;
  #nextInputEventSequence = 1n;
  #pendingInputReturns = 0;
  #activeInputSurface: string | null = null;
  #gamepadRouteSurface: string | null = null;
  #textures = new Map<string, {
    readonly engine: { readonly index: number; readonly generation: number };
    readonly texture: bigint;
    readonly revision: bigint;
    readonly source: ImageBitmap;
  }>();
  #textureRevisions = new Map<string, bigint>();
  #textureBytes = 0;
  #profileAssets = new Map<string, {
    readonly engine: { readonly index: number; readonly generation: number };
    readonly kind: number;
    readonly asset: bigint;
    readonly revision: bigint;
    readonly bytes: Uint8Array;
  }>();
  #profileAssetRevisions = new Map<string, bigint>();
  #profileAssetBytes = 0;
  #hostFrameFences = new Map<string, bigint>();
  #pendingReadbacks = new Map<string, number>();
  #pendingReadbackBytes = 0;
  #cancelledReadbacks = new Set<string>();
  #frameTraces = new Map<string, Readonly<{
    readonly engine: { readonly index: number; readonly generation: number };
    readonly target: { readonly index: number; readonly generation: number };
    readonly frameId: bigint;
    readonly graphSignature: bigint;
    readonly fence: bigint;
    readonly width: number;
    readonly height: number;
  }>>();
  #renderTargets = new Map<string, Readonly<{
    readonly engine: { readonly index: number; readonly generation: number };
    readonly targets: readonly {
      readonly index: number;
      readonly generation: number;
    }[];
  }>>();

  async init(host: StudioRendererHost): Promise<void> {
    if (this.#host !== null) throw new Error("Voplay renderer already initialized");
    if (!host.framework.roles.includes("renderer")) {
      throw new Error("Voplay renderer requires the renderer role");
    }
    const laneCapability = host.getCapability("framework_lane");
    const surfaceCapability = host.getCapability("app_surface");
    if (laneCapability === null || surfaceCapability === null) {
      throw new Error("Voplay renderer requires framework_lane and app_surface");
    }
    const lane = await laneCapability.open("render");
    this.#host = host;
    this.#lane = lane;
    this.#surfaceCapability = surfaceCapability;
    this.#gamepads = new BrowserGamepadInputSource((event) => {
      this.#acceptGamepadInput(event);
    }, () => surfaceCapability.isInteractive());
    this.#haptics = new BrowserHapticsHost((result) => {
      void this.#submitHapticsResult(result).catch((error) => {
        this.#failPlatformInput(error);
      });
    }, (index) => this.#gamepads?.generation(index));
    this.#gamepads.start();
    this.#unsubscribeInput = surfaceCapability.subscribeInput((event) => {
      this.#acceptPlatformInput(event);
    });
    this.#polling = true;
    void this.#poll(host, lane);
    host.log(`Voplay browser renderer ready for ${host.framework.name}`);
  }

  async render(_container: HTMLElement, bytes: Uint8Array): Promise<void> {
    if (bytes.byteLength === 0) return;
    try {
      await this.#acceptPacket(bytes);
    } catch (error) {
      this.#host?.reportError(`Voplay render packet failed: ${errorMessage(error)}`);
      throw error;
    }
  }

  async acceptHostRenderCommand(bytes: Uint8Array): Promise<Uint8Array | null> {
    const reader = new HostRenderReader(bytes);
    reader.magic("VHR3");
    const routeEngine = reader.handle();
    reader.magic("VHR1");
    const action = reader.u8();
    switch (action) {
      case 1:
      case 2:
      case 3: {
        const id = reader.surface(routeEngine, this.#requireLane().binding.session);
        const metrics = reader.metrics();
        reader.finish();
        let record = this.#surfaces.get(surfaceKey(id));
        if (action === 1) {
          if (record === undefined) {
            record = await this.#attachHostRenderSurface(id, metrics);
          } else if (this.#surfaceHost !== null) {
            this.#surfaceHost.resize(id, metrics);
          }
          record.control = { ...record.control, metrics };
        } else {
          if (record === undefined) {
            record = await this.#attachHostRenderSurface(id, metrics);
          } else if (this.#surfaceHost !== null) {
            if (action === 3) {
              await this.#surfaceHost.rebindDevice(
                id,
                metrics,
                this.#surfaceHost.ownerSnapshot().deviceGeneration,
              );
            } else {
              this.#surfaceHost.resize(id, metrics);
            }
          }
          record.control = { ...record.control, metrics };
        }
        return null;
      }
      case 4: {
        const texture = reader.u64();
        const width = reader.u32();
        const height = reader.u32();
        const pixels = reader.blob();
        reader.finish();
        const expected = width * height * 4;
        if (
          texture <= 0n
          || width <= 0
          || height <= 0
          || !Number.isSafeInteger(expected)
          || pixels.byteLength !== expected
        ) {
          throw new Error("invalid Voplay host-render texture");
        }
        const rgba = new Uint8ClampedArray(pixels.byteLength);
        rgba.set(pixels);
        const source = await createImageBitmap(new ImageData(rgba, width, height));
        const textureKey = engineResourceKey(routeEngine, texture);
        const previous = this.#textures.get(textureKey);
        const nextBytes = this.#textureBytes
          - (previous === undefined ? 0 : previous.source.width * previous.source.height * 4)
          + pixels.byteLength;
        if (
          (previous === undefined && this.#textures.size >= MAX_TEXTURES)
          || nextBytes > MAX_TEXTURE_BYTES
        ) {
          source.close();
          throw new Error("Voplay host-render texture capacity exceeded");
        }
        try {
          this.#surfaceHost?.registerTexture(texture, source, routeEngine);
        } catch (error) {
          source.close();
          throw error;
        }
        this.#textures.set(textureKey, {
          engine: routeEngine,
          texture,
          revision: (previous?.revision ?? 0n) + 1n,
          source,
        });
        this.#textureBytes = nextBytes;
        previous?.source.close();
        return null;
      }
      case 5: {
        const texture = reader.u64();
        reader.finish();
        const textureKey = engineResourceKey(routeEngine, texture);
        const previous = this.#textures.get(textureKey);
        if (previous !== undefined) {
          this.#surfaceHost?.removeTexture(texture, routeEngine);
          this.#textures.delete(textureKey);
          this.#textureBytes -= previous.source.width * previous.source.height * 4;
          previous.source.close();
        }
        return null;
      }
      case 6: {
        const kind = reader.u32();
        const asset = reader.u64();
        const revision = reader.u64();
        const assetBytes = reader.blob();
        reader.finish();
        this.#upsertHostProfileAsset(routeEngine, kind, asset, revision, assetBytes);
        return null;
      }
      case 7: {
        const kind = reader.u32();
        const asset = reader.u64();
        const revision = reader.u64();
        reader.finish();
        this.#removeHostProfileAsset(routeEngine, kind, asset, revision);
        return null;
      }
      case 8: {
        const frame = reader.frame(routeEngine, this.#requireLane().binding.session);
        reader.finish();
        const record = this.#surfaces.get(surfaceKey(frame.surface));
        if (record === undefined) throw new Error("unknown Voplay host-render Surface");
        record.control = {
          ...record.control,
          renderEndpoint: frame.renderEndpoint,
          deviceGeneration: frame.deviceGeneration,
        };
        const host = await this.#ensureHostRenderSurfaceHost(frame.deviceGeneration);
        const submission = host.submit(frame);
        this.#hostFrameFences.set(hostFrameKey(frame.surface, frame.frameId), submission.fenceValue);
        const traceKey = engineResourceKey(routeEngine, frame.frameId);
        this.#frameTraces.delete(traceKey);
        this.#frameTraces.set(traceKey, {
          engine: routeEngine,
          target: frame.surface.surface,
          frameId: frame.frameId,
          graphSignature: frame.graphSignature,
          fence: submission.fenceValue,
          width: record.control.metrics.width,
          height: record.control.metrics.height,
        });
        while (this.#frameTraces.size > MAX_RETAINED_FRAME_TRACES) {
          const oldest = this.#frameTraces.keys().next().value as string | undefined;
          if (oldest === undefined) break;
          this.#frameTraces.delete(oldest);
        }
        return null;
      }
      case 9: {
        const ack = reader.frameAck(routeEngine, this.#requireLane().binding.session);
        const nowMicros = reader.u64();
        const deadlineMicros = reader.u64();
        reader.finish();
        const key = hostFrameKey(ack.surface, ack.frameId);
        const fence = this.#hostFrameFences.get(key);
        if (fence === undefined) throw new Error("Voplay host-render frame fence disappeared");
        this.#requireSurfaceHost().present(
          ack.surface,
          ack.frameId,
          fence,
          nowMicros,
          deadlineMicros,
        );
        this.#hostFrameFences.delete(key);
        return null;
      }
      case 10: {
        const id = reader.surface(routeEngine, this.#requireLane().binding.session);
        reader.finish();
        const key = surfaceKey(id);
        const record = this.#surfaces.get(key);
        if (record === undefined) throw new Error("unknown Voplay host-render Surface");
        if (this.#gamepadRouteSurface === key) this.#gamepads?.invalidate();
        this.#surfaceHost?.detach(id);
        record.lease.release();
        this.#surfaces.delete(key);
        if (this.#gamepadRouteSurface === key) this.#gamepadRouteSurface = null;
        this.#removeSurfaceFrameTraces(id);
        return null;
      }
      case 11: {
        const request = reader.u64();
        const readback = reader.readbackRequest(routeEngine);
        reader.finish();
        if (request <= 0n) {
          throw new Error("invalid Voplay host-render readback request identity");
        }
        const key = engineResourceKey(routeEngine, request);
        const bytesPerPixel = readback.format === 3 ? 8 : 4;
        const rowBytes = Math.ceil(
          readback.region.width * bytesPerPixel / 256,
        ) * 256;
        const readbackBytes = rowBytes * readback.region.height;
        if (
          this.#pendingReadbacks.has(key)
          || this.#pendingReadbacks.size >= MAX_PENDING_READBACKS
          || !Number.isSafeInteger(readbackBytes)
          || readbackBytes <= 0
          || readbackBytes > MAX_SINGLE_READBACK_BYTES
          || this.#pendingReadbackBytes + readbackBytes > MAX_PENDING_READBACK_BYTES
        ) {
          return encodeHostRenderFailure(routeEngine, request, 1, false);
        }
        this.#pendingReadbacks.set(key, readbackBytes);
        this.#pendingReadbackBytes += readbackBytes;
        try {
          const result = await this.#requireSurfaceHost().readRenderTarget(
            routeEngine,
            readback.target,
            readback.expectedRevision,
            readback.region,
            readback.format,
          );
          if (this.#cancelledReadbacks.delete(key)) return null;
          return encodeHostRenderReadback(
            routeEngine,
            request,
            readback.target,
            result.targetRevision,
            result.rowBytes,
            result.bytes,
          );
        } catch (error) {
          if (this.#cancelledReadbacks.delete(key)) return null;
          return encodeHostRenderFailure(
            routeEngine,
            request,
            error instanceof BrowserGpuReadbackError ? error.failure : 2,
            false,
          );
        } finally {
          this.#pendingReadbackBytes -= this.#pendingReadbacks.get(key) ?? 0;
          this.#pendingReadbacks.delete(key);
        }
      }
      case 12: {
        const request = reader.u64();
        reader.finish();
        const key = engineResourceKey(routeEngine, request);
        if (this.#pendingReadbacks.has(key)) this.#cancelledReadbacks.add(key);
        return null;
      }
      case 13: {
        const request = reader.u64();
        const traceRequest = reader.frameTraceRequest();
        reader.finish();
        if (request <= 0n) {
          throw new Error("invalid Voplay host-render frame trace identity");
        }
        const retained = this.#frameTraces.get(
          engineResourceKey(routeEngine, traceRequest.frameId),
        );
        if (
          retained === undefined
          || retained.graphSignature !== traceRequest.graphSignature
        ) {
          return encodeHostRenderFailure(routeEngine, request, 1, true);
        }
        const trace = encodeBrowserFrameTrace(retained);
        if (trace.byteLength > traceRequest.maxBytes) {
          return encodeHostRenderFailure(routeEngine, request, 1, true);
        }
        return encodeHostRenderFrameTrace(
          routeEngine,
          request,
          retained.frameId,
          retained.graphSignature,
          trace,
        );
      }
      case 14: {
        const request = reader.u64();
        reader.finish();
        if (request <= 0n) {
          throw new Error("invalid Voplay host-render frame trace cancellation");
        }
        return null;
      }
      case 15: {
        const count = reader.u32();
        if (count > 4096) throw new Error("Voplay host-render target capacity exceeded");
        const targets: { readonly index: number; readonly generation: number }[] = [];
        const identities = new Set<string>();
        for (let index = 0; index < count; index += 1) {
          const engine = reader.handle();
          const target = reader.handle();
          const identity = `${target.index}:${target.generation}`;
          if (!sameHandle(engine, routeEngine) || identities.has(identity)) {
            throw new Error("invalid Voplay host-render target synchronization");
          }
          identities.add(identity);
          targets.push(target);
        }
        reader.finish();
        if (targets.length === 0) {
          this.#renderTargets.delete(handleKey(routeEngine));
        } else {
          this.#renderTargets.set(handleKey(routeEngine), {
            engine: routeEngine,
            targets,
          });
        }
        this.#surfaceHost?.synchronizeRenderTargets(routeEngine, targets);
        return null;
      }
      default:
        throw new Error(`unsupported Voplay host-render action ${action}`);
    }
  }

  stop(): void {
    this.#polling = false;
    this.#unsubscribeInput?.();
    this.#unsubscribeInput = null;
    this.#haptics?.close(false);
    this.#haptics = null;
    this.#gamepads?.close(false);
    this.#gamepads = null;
    this.#lane?.close();
    this.#lane = null;
    try {
      const abandoned = this.#surfaceHost?.close() ?? 0;
      if (abandoned > 0) {
        this.#host?.log(`Voplay browser renderer abandoned ${abandoned} pending frame(s) on close`);
      }
    } finally {
      this.#surfaceHost = null;
      for (const record of this.#surfaces.values()) record.lease.release();
      this.#surfaces.clear();
      for (const texture of this.#textures.values()) texture.source.close();
      this.#textures.clear();
      this.#textureRevisions.clear();
      this.#textureBytes = 0;
      this.#profileAssets.clear();
      this.#profileAssetRevisions.clear();
      this.#profileAssetBytes = 0;
      this.#hostFrameFences.clear();
      this.#pendingReadbacks.clear();
      this.#pendingReadbackBytes = 0;
      this.#cancelledReadbacks.clear();
      this.#frameTraces.clear();
      this.#renderTargets.clear();
      this.#coalescedInputs.clear();
      this.#drainingCoalesced.clear();
      this.#surfaceCapability = null;
      this.#host = null;
      this.#lastInboundSequence = 0n;
      this.#nextReturnSequence = 1n;
      this.#nextInputEventSequence = 1n;
      this.#pendingInputReturns = 0;
      this.#activeInputSurface = null;
      this.#gamepadRouteSurface = null;
    }
  }

  async #attachHostRenderSurface(id: BrowserSurfaceId, metrics: {
    readonly width: number;
    readonly height: number;
    readonly scaleNumerator: number;
    readonly scaleDenominator: number;
  }): Promise<SurfaceRecord> {
    const capability = this.#requireSurfaceCapability();
    const lane = this.#requireLane();
    const route = await capability.resolve(id.surface);
    if (
      route.kind !== "game"
      || !sameHandle(route.session, lane.binding.session)
      || route.sessionEpoch !== BigInt(lane.binding.sessionEpoch)
      || !sameHandle(route.surface, id.surface)
    ) {
      throw new Error("Voplay host-render Surface route does not match App Runtime authority");
    }
    const lease = capability.attach({
      identity: {
        sessionId: capability.sessionId,
        session: route.session,
        sessionEpoch: route.sessionEpoch,
        window: route.window,
        view: route.view,
        surface: route.surface,
      },
      kind: "canvas",
      layer: route.zOrder,
      input: hostInputPolicy(route.inputPolicy),
      label: `Voplay ${id.engine.engine.index}:${id.engine.engine.generation}`,
    });
    if (!(lease.element instanceof HTMLCanvasElement)) {
      lease.release();
      throw new Error("Voplay App Surface host returned a non-canvas element");
    }
    const control: BrowserSurfaceControl = {
      action: "attach",
      engine: id.engine.engine,
      session: id.engine.session,
      window: route.window,
      view: route.view,
      surface: id.surface,
      domain: id.domain,
      metrics,
      renderEndpoint: { index: 0, generation: 1 },
      deviceGeneration: 1n,
      zOrder: route.zOrder,
      inputPolicy: browserInputPolicyTag(route.inputPolicy),
      channelEpoch: BigInt(lane.binding.channelEpoch),
      sequence: 1n,
    };
    const record = { control, id, lease };
    try {
      this.#surfaceHost?.attach(id, lease.element, metrics);
    } catch (error) {
      lease.release();
      throw error;
    }
    this.#surfaces.set(surfaceKey(id), record);
    return record;
  }

  async #ensureHostRenderSurfaceHost(deviceGeneration: bigint): Promise<BrowserSurfaceHost> {
    if (this.#surfaceHost === null) {
      const adapter = await WebGpuCanvasAdapter.create({
        deviceGeneration,
        maxCommands: MAX_COMMANDS,
        maxCommandBytes: MAX_COMMAND_BYTES,
      });
      const host = new BrowserSurfaceHost(adapter, {
        maxSurfaces: 64,
        maxCommandBytes: MAX_COMMAND_BYTES,
      });
      const previousControls = new Map(
        [...this.#surfaces.values()].map((record) => [record, record.control] as const),
      );
      try {
        for (const resident of this.#textures.values()) {
          host.registerTexture(resident.texture, resident.source, resident.engine);
        }
        for (const record of this.#surfaces.values()) {
          host.attach(record.id, record.lease.element as HTMLCanvasElement, record.control.metrics);
          record.control = { ...record.control, deviceGeneration };
        }
        this.#synchronizeKnownRenderTargets(host);
      } catch (error) {
        for (const [record, control] of previousControls) record.control = control;
        host.close();
        throw error;
      }
      this.#surfaceHost = host;
    } else {
      const currentGeneration = this.#surfaceHost.ownerSnapshot().deviceGeneration;
      if (currentGeneration > deviceGeneration) {
        throw new Error("stale Voplay host-render device generation");
      }
      if (
        currentGeneration < deviceGeneration
        || [...this.#surfaces.values()].some(
          (record) => record.control.deviceGeneration !== deviceGeneration,
        )
      ) {
        for (const record of this.#surfaces.values()) {
          await this.#surfaceHost.rebindDevice(
            record.id,
            record.control.metrics,
            deviceGeneration,
          );
          record.control = { ...record.control, deviceGeneration };
        }
      }
    }
    return this.#surfaceHost;
  }

  #synchronizeKnownRenderTargets(host: BrowserSurfaceHost): void {
    for (const { engine, targets } of this.#renderTargets.values()) {
      host.synchronizeRenderTargets(engine, targets);
    }
  }

  #upsertHostProfileAsset(
    engine: { readonly index: number; readonly generation: number },
    kind: number,
    asset: bigint,
    revision: bigint,
    bytes: Uint8Array,
  ): void {
    const key = `${engineResourceKey(engine, asset)}/${kind}`;
    const previous = this.#profileAssets.get(key);
    const previousRevision = this.#profileAssetRevisions.get(key);
    const nextBytes = this.#profileAssetBytes - (previous?.bytes.byteLength ?? 0) + bytes.byteLength;
    if (
      kind < 2
      || asset <= 0n
      || revision <= (previousRevision ?? 0n)
      || bytes.byteLength === 0
      || (previous === undefined && this.#profileAssets.size >= MAX_PROFILE_ASSETS)
      || nextBytes > MAX_PROFILE_ASSET_BYTES
    ) {
      throw new Error("invalid Voplay host-render profile asset");
    }
    this.#profileAssets.set(key, {
      engine,
      kind,
      asset,
      revision,
      bytes: bytes.slice(),
    });
    this.#profileAssetRevisions.set(key, revision);
    this.#profileAssetBytes = nextBytes;
  }

  #removeHostProfileAsset(
    engine: { readonly index: number; readonly generation: number },
    kind: number,
    asset: bigint,
    revision: bigint,
  ): void {
    const key = `${engineResourceKey(engine, asset)}/${kind}`;
    const previousRevision = this.#profileAssetRevisions.get(key);
    if (kind < 2 || asset <= 0n || revision <= (previousRevision ?? 0n)) {
      throw new Error("stale Voplay host-render profile asset removal");
    }
    const previous = this.#profileAssets.get(key);
    if (previous !== undefined) {
      this.#profileAssets.delete(key);
      this.#profileAssetBytes -= previous.bytes.byteLength;
    }
    this.#profileAssetRevisions.set(key, revision);
  }

  quiesceForCapture(): { stopped: number; surfaces: number } {
    return { stopped: 1, surfaces: this.#surfaces.size };
  }

  ownerSnapshot(): {
    readonly surfaces: BrowserSurfaceOwnerSnapshot | null;
    readonly textureCount: number;
    readonly textureBytes: number;
    readonly profileAssetCount: number;
    readonly profileAssetBytes: number;
    readonly pendingReadbacks: number;
    readonly pendingReadbackBytes: number;
    readonly retainedFrameTraces: number;
    readonly renderTargets: number;
    readonly gamepads: ReturnType<BrowserGamepadInputSource["ownerSnapshot"]> | null;
    readonly haptics: ReturnType<BrowserHapticsHost["ownerSnapshot"]> | null;
    readonly pendingInputReturns: number;
    readonly coalescedInputs: number;
  } {
    return {
      surfaces: this.#surfaceHost?.ownerSnapshot() ?? null,
      textureCount: this.#textures.size,
      textureBytes: this.#textureBytes,
      profileAssetCount: this.#profileAssets.size,
      profileAssetBytes: this.#profileAssetBytes,
      pendingReadbacks: this.#pendingReadbacks.size,
      pendingReadbackBytes: this.#pendingReadbackBytes,
      retainedFrameTraces: this.#frameTraces.size,
      renderTargets: [...this.#renderTargets.values()].reduce(
        (total, group) => total + group.targets.length,
        0,
      ),
      gamepads: this.#gamepads?.ownerSnapshot() ?? null,
      haptics: this.#haptics?.ownerSnapshot() ?? null,
      pendingInputReturns: this.#pendingInputReturns,
      coalescedInputs: this.#coalescedInputs.size,
    };
  }

  async #poll(host: StudioRendererHost, lane: StudioFrameworkLane): Promise<void> {
    while (this.#polling && this.#host === host && this.#lane === lane) {
      try {
        this.#haptics?.setEnabled(this.#requireSurfaceCapability().isInteractive());
        const packet = await lane.poll();
        if (!this.#polling || this.#host !== host || this.#lane !== lane) return;
        if (packet === null) {
          await delay(8);
          continue;
        }
        await this.#acceptPacket(packet);
      } catch (error) {
        if (!this.#polling || this.#host !== host || this.#lane !== lane) return;
        host.reportError(`Voplay framework lane failed: ${errorMessage(error)}`);
        this.#polling = false;
      }
    }
  }

  async #acceptPacket(bytes: Uint8Array): Promise<void> {
    const lane = this.#requireLane();
    const header = decodeFrameworkPacket(bytes).header;
    if (header.channelEpoch !== BigInt(lane.binding.channelEpoch)) {
      throw new Error("Voplay packet channel epoch mismatch");
    }
    if (
      header.kind !== MessageKind.RenderAssetData
      && header.sequence <= this.#lastInboundSequence
    ) {
      throw new Error("Voplay packet sequence regression");
    }
    if (header.kind !== MessageKind.RenderAssetData) {
      this.#lastInboundSequence = header.sequence;
    }
    switch (header.kind) {
      case MessageKind.SurfaceControl:
        await this.#applySurfaceControl(decodeSurfaceControl(bytes));
        break;
      case MessageKind.FramePulse:
        await this.#renderFrame(bytes);
        break;
      case MessageKind.RenderAssetData:
        await this.#applyRenderAsset(bytes);
        break;
      case MessageKind.HapticsCommand:
        this.#requireHaptics().setEnabled(this.#requireSurfaceCapability().isInteractive());
        this.#requireHaptics().accept(decodeFrameworkPacket(bytes));
        break;
      default:
        throw new Error(`unsupported Voplay browser packet ${header.kind}`);
    }
  }

  async #applySurfaceControl(control: BrowserSurfaceControl): Promise<void> {
    this.#validateSession(control.session);
    const id = surfaceId(control);
    const key = surfaceKey(id);
    switch (control.action) {
      case "attach": {
        const existing = this.#surfaces.get(key);
        if (existing !== undefined) {
          if (
            !sameHandle(existing.control.session, control.session)
            || !sameHandle(existing.control.window, control.window)
            || !sameHandle(existing.control.view, control.view)
            || !sameHandle(existing.control.surface, control.surface)
          ) {
            throw new Error("Voplay browser Surface attach changed its App route");
          }
          if (this.#surfaceHost !== null) {
            if (
              this.#surfaceHost.ownerSnapshot().deviceGeneration
              !== control.deviceGeneration
            ) {
              await this.#surfaceHost.rebindDevice(
                id,
                control.metrics,
                control.deviceGeneration,
              );
            } else {
              this.#surfaceHost.resize(id, control.metrics);
            }
          }
          existing.control = control;
          break;
        }
        const firstSurface = this.#surfaces.size === 0;
        const capability = this.#requireSurfaceCapability();
        const route = await capability.resolve(control.surface);
        if (
          route.kind !== "game"
          || !sameHandle(route.session, control.session)
          || route.sessionEpoch !== BigInt(this.#requireLane().binding.sessionEpoch)
          || !sameHandle(route.window, control.window)
          || !sameHandle(route.view, control.view)
          || !sameHandle(route.surface, control.surface)
          || route.zOrder !== control.zOrder
          || hostInputPolicy(route.inputPolicy) !== inputPolicy(control.inputPolicy)
        ) {
          throw new Error("Voplay SurfaceControl route does not match App Runtime authority");
        }
        const lease = capability.attach({
          identity: {
            sessionId: capability.sessionId,
            session: control.session,
            sessionEpoch: BigInt(this.#requireLane().binding.sessionEpoch),
            window: control.window,
            view: control.view,
            surface: control.surface,
          },
          kind: "canvas",
          layer: control.zOrder,
          input: inputPolicy(control.inputPolicy),
          label: `Voplay ${control.engine.index}:${control.engine.generation}`,
        });
        if (!(lease.element instanceof HTMLCanvasElement)) {
          lease.release();
          throw new Error("Voplay App Surface host returned a non-canvas element");
        }
        let attached = false;
        try {
          const surfaceHost =
            await this.#ensureHostRenderSurfaceHost(control.deviceGeneration);
          surfaceHost.attach(id, lease.element, control.metrics);
          attached = true;
          this.#surfaces.set(key, { control, id, lease });
          await this.#ensureHostRenderSurfaceHost(control.deviceGeneration);
        } catch (error) {
          if (attached) {
            try {
              this.#surfaceHost?.detach(id);
            } catch {
              this.#surfaceHost?.close();
              this.#surfaceHost = null;
            }
          }
          this.#surfaces.delete(key);
          lease.release();
          throw error;
        }
        if (firstSurface) this.#gamepads?.invalidate();
        break;
      }
      case "resize": {
        const current = this.#surface(key);
        assertSameControlOwner(current.record.control, control);
        current.host.resize(id, control.metrics);
        current.record.control = control;
        break;
      }
      case "suspend": {
        const current = this.#surface(key);
        assertSameControlOwner(current.record.control, control);
        current.host.suspend(id);
        current.record.control = control;
        break;
      }
      case "resume": {
        const current = this.#surface(key);
        assertSameControlOwner(current.record.control, control);
        current.host.resume(id);
        current.record.control = control;
        break;
      }
      case "rebind": {
        const current = this.#surface(key);
        assertSameControlOwner(current.record.control, control);
        await current.host.rebindDevice(id, control.metrics, control.deviceGeneration);
        current.record.control = control;
        break;
      }
      case "detach": {
        const current = this.#surface(key);
        assertSameControlOwner(current.record.control, control);
        if (this.#gamepadRouteSurface === key) this.#gamepads?.invalidate();
        current.host.detach(current.record.id);
        current.record.lease.release();
        this.#surfaces.delete(key);
        if (this.#gamepadRouteSurface === key) this.#gamepadRouteSurface = null;
        this.#removeSurfaceFrameTraces(current.record.id);
        if (this.#activeInputSurface === key) this.#activeInputSurface = null;
        break;
      }
    }
  }

  async #renderFrame(bytes: Uint8Array): Promise<void> {
    const lane = this.#requireLane();
    const decoded = decodeFrameSubmission(bytes);
    this.#validateSession(decoded.session);
    const record = this.#surface(surfaceKey(decoded.frame.surface)).record;
    if (
      !sameHandle(decoded.window, record.control.window)
      || !sameHandle(decoded.view, record.control.view)
      || !sameHandle(decoded.frame.renderEndpoint, record.control.renderEndpoint)
      || decoded.frame.deviceGeneration !== record.control.deviceGeneration
    ) {
      throw new Error("Voplay frame route does not match the current SurfaceControl");
    }
    let terminal: BrowserFrameTerminal;
    let fenceValue = 0n;
    try {
      const submission = this.#requireSurfaceHost().submit(decoded.frame);
      fenceValue = submission.fenceValue;
      terminal = this.#requireSurfaceHost().present(
        record.id,
        decoded.frame.frameId,
        submission.fenceValue,
        monotonicMicros(),
        decoded.deadlineMicros,
      );
    } catch (error) {
      terminal = classifyFrameFailure(error, fenceValue !== 0n);
    }
    const sequence = this.#takeReturnSequence();
    const packet = encodeFrameOutcome({
      terminal,
      session: decoded.session,
      frame: decoded.frame,
      renderedRevision: decoded.frame.requiredRenderRevision,
      observedControlRevision: decoded.frame.requiredControlRevision,
      fenceValue,
      completionMicros: monotonicMicros(),
      channelEpoch: decoded.header.channelEpoch,
      sequence,
    });
    await lane.submit(packet, decoded.frame.frameId);
  }

  async #applyRenderAsset(bytes: Uint8Array): Promise<void> {
    const packet = decodeFrameworkPacket(bytes);
    const payload = packet.payload;
    const magic = payload.byteLength >= 4
      ? String.fromCharCode(payload[0]!, payload[1]!, payload[2]!, payload[3]!)
      : "";
    if (magic === "VRT1") {
      await this.#applyTextureAsset(packet);
      return;
    }
    if (magic === "VRA1") {
      await this.#applyProfileAsset(packet);
      return;
    }
    throw new Error("unsupported Voplay browser render asset");
  }

  async #applyTextureAsset(packet: ReturnType<typeof decodeFrameworkPacket>): Promise<void> {
    const payload = packet.payload;
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const action = payload[4]!;
    const texture = takeAssetU64(view, 5);
    const revision = takeAssetU64(view, 13);
    const textureKey = engineResourceKey(packet.header.engine, texture);
    const current = this.#textures.get(textureKey);
    const currentRevision = this.#textureRevisions.get(textureKey);
    if (
      texture <= 0n
      || revision <= 0n
      || packet.header.commitId !== texture
      || packet.header.newRevision !== revision
      || (currentRevision !== undefined && currentRevision >= revision)
    ) {
      throw new Error("stale Voplay browser texture asset");
    }
    if (action === 1) {
      if (payload.byteLength < 33) throw new Error("truncated Voplay browser texture asset");
      const width = view.getUint32(21, true);
      const height = view.getUint32(25, true);
      const byteLength = view.getUint32(29, true);
      const expected = width * height * 4;
      if (
        width === 0
        || height === 0
        || !Number.isSafeInteger(expected)
        || byteLength !== expected
        || payload.byteLength !== 33 + byteLength
        || (current === undefined && this.#textures.size >= MAX_TEXTURES)
        || (
          this.#textureBytes
            - (current === undefined ? 0 : current.source.width * current.source.height * 4)
            + byteLength > MAX_TEXTURE_BYTES
        )
      ) {
        throw new Error("invalid Voplay browser texture dimensions");
      }
      const rgba = new Uint8ClampedArray(byteLength);
      rgba.set(payload.subarray(33));
      const source = await createImageBitmap(new ImageData(rgba, width, height));
      const previous = this.#textures.get(textureKey);
      try {
        this.#surfaceHost?.registerTexture(texture, source, packet.header.engine);
      } catch (error) {
        source.close();
        throw error;
      }
      this.#textures.set(textureKey, {
        engine: packet.header.engine,
        texture,
        revision,
        source,
      });
      this.#textureBytes = this.#textureBytes
        - (previous === undefined ? 0 : previous.source.width * previous.source.height * 4)
        + byteLength;
      previous?.source.close();
    } else if (action === 2 && payload.byteLength === 21) {
      const previous = this.#textures.get(textureKey);
      if (previous !== undefined) {
        this.#surfaceHost?.removeTexture(texture, packet.header.engine);
        this.#textures.delete(textureKey);
        this.#textureBytes -= previous.source.width * previous.source.height * 4;
        previous.source.close();
      }
    } else {
      throw new Error("unsupported Voplay browser texture action");
    }
    this.#textureRevisions.set(textureKey, revision);
    await this.#submitRenderAssetAck(packet, 1, texture, revision);
  }

  async #applyProfileAsset(packet: ReturnType<typeof decodeFrameworkPacket>): Promise<void> {
    const payload = packet.payload;
    if (payload.byteLength < 29) throw new Error("truncated Voplay browser profile asset");
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const action = payload[4]!;
    const kind = view.getUint32(5, true);
    const asset = view.getBigUint64(9, true);
    const revision = view.getBigUint64(17, true);
    const byteLength = view.getUint32(25, true);
    const key = `${engineResourceKey(packet.header.engine, asset)}/${kind}`;
    const current = this.#profileAssets.get(key);
    const currentRevision = this.#profileAssetRevisions.get(key);
    if (
      kind < 2
      || asset <= 0n
      || revision <= 0n
      || packet.header.commitId !== asset
      || packet.header.newRevision !== revision
      || (currentRevision !== undefined && currentRevision >= revision)
    ) {
      throw new Error("stale Voplay browser profile asset");
    }
    if (action === 1 && byteLength > 0 && payload.byteLength === 29 + byteLength) {
      const nextBytes = this.#profileAssetBytes
        - (current?.bytes.byteLength ?? 0)
        + byteLength;
      if (
        (current === undefined && this.#profileAssets.size >= MAX_PROFILE_ASSETS)
        || nextBytes > MAX_PROFILE_ASSET_BYTES
      ) {
        throw new Error("Voplay browser profile asset capacity exceeded");
      }
      this.#profileAssets.set(key, {
        engine: packet.header.engine,
        kind,
        asset,
        revision,
        bytes: payload.slice(29),
      });
      this.#profileAssetBytes = nextBytes;
    } else if (action === 2 && byteLength === 0 && payload.byteLength === 29) {
      if (current !== undefined) {
        this.#profileAssets.delete(key);
        this.#profileAssetBytes -= current.bytes.byteLength;
      }
    } else {
      throw new Error("unsupported Voplay browser profile asset action");
    }
    this.#profileAssetRevisions.set(key, revision);
    await this.#submitRenderAssetAck(packet, kind, asset, revision);
  }

  async #submitRenderAssetAck(
    packet: ReturnType<typeof decodeFrameworkPacket>,
    kind: number,
    asset: bigint,
    revision: bigint,
  ): Promise<void> {
    const ackPayload = new Uint8Array(4);
    new DataView(ackPayload.buffer).setUint32(0, kind, true);
    await this.#requireLane().submit(encodeFrameworkPacket({
      kind: MessageKind.RenderAssetAck,
      engine: packet.header.engine,
      channelEpoch: packet.header.channelEpoch,
      commitId: asset,
      baseRevision: 0n,
      newRevision: revision,
      requiredControlRevision: 0n,
      sourceSimulationRevision: packet.header.sourceSimulationRevision,
      sequence: packet.header.sequence,
    }, ackPayload), packet.header.sequence);
  }

  #acceptPlatformInput(event: BrowserPlatformInputEvent): void {
    try {
      const record = this.#recordForInput(event);
      const capability = this.#requireSurfaceCapability();
      if (event.type === "pointerDown" && record.control.inputPolicy !== 3) {
        this.#activateInputSurface(surfaceKey(record.id));
        capability.focus(event.surface);
        if (event.pointerId !== undefined) {
          capability.capturePointer(event.pointerId, event.surface);
        }
      } else if (
        (event.type === "pointerUp" || event.type === "pointerCancel")
        && event.pointerId !== undefined
      ) {
        capability.releasePointer(event.pointerId);
      } else if (event.type === "focus" && event.focused) {
        this.#activateInputSurface(surfaceKey(record.id));
      }
      const key = coalescedInputKey(event);
      if (key === null) {
        if (this.#pendingInputReturns >= MAX_PENDING_INPUT_RETURNS) {
          throw new Error("Voplay reliable platform input return capacity exceeded");
        }
        void this.#submitPlatformInput(event, record.control).catch((error) => {
          this.#failPlatformInput(error);
        });
        return;
      }
      if (
        !this.#coalescedInputs.has(key)
        && this.#coalescedInputs.size >= MAX_COALESCED_INPUTS
      ) {
        throw new Error("Voplay coalesced platform input capacity exceeded");
      }
      this.#coalescedInputs.set(key, event);
      if (!this.#drainingCoalesced.has(key)) {
        this.#drainingCoalesced.add(key);
        void this.#drainCoalescedInput(key).catch((error) => {
          this.#failPlatformInput(error);
        });
      }
    } catch (error) {
      this.#failPlatformInput(error);
    }
  }

  async #drainCoalescedInput(key: string): Promise<void> {
    try {
      for (;;) {
        const event = this.#coalescedInputs.get(key);
        if (event === undefined) return;
        this.#coalescedInputs.delete(key);
        const record = this.#recordForInput(event);
        await this.#submitPlatformInput(event, record.control);
      }
    } finally {
      this.#drainingCoalesced.delete(key);
      if (this.#coalescedInputs.has(key) && this.#polling) {
        this.#drainingCoalesced.add(key);
        void this.#drainCoalescedInput(key).catch((error) => {
          this.#failPlatformInput(error);
        });
      }
    }
  }

  async #submitPlatformInput(
    event: BrowserPlatformInputEvent,
    control: BrowserSurfaceControl,
  ): Promise<void> {
    if (this.#pendingInputReturns >= MAX_PENDING_INPUT_RETURNS) {
      throw new Error("Voplay platform input return capacity exceeded");
    }
    const lane = this.#requireLane();
    const routedEvent: BrowserPlatformInputEvent = {
      ...event,
      sequence: this.#takeInputEventSequence(),
    };
    const packet = encodePlatformInput(routedEvent, control, this.#takeReturnSequence());
    this.#pendingInputReturns += 1;
    try {
      await lane.submit(packet);
    } finally {
      this.#pendingInputReturns -= 1;
    }
  }

  #acceptGamepadInput(event: BrowserGamepadEvent): void {
    if (!this.#polling) return;
    if (event.type === "gamepadDisconnect") {
      this.#haptics?.disconnectGamepad(event.gamepadIndex, event.gamepadGeneration);
    }
    const record = (
      this.#activeInputSurface === null
        ? this.#gamepadRouteSurface === null
          ? undefined
          : this.#surfaces.get(this.#gamepadRouteSurface)
        : this.#surfaces.get(this.#activeInputSurface)
    ) ?? this.#surfaces.values().next().value as SurfaceRecord | undefined;
    if (record === undefined) return;
    this.#gamepadRouteSurface = surfaceKey(record.id);
    const lane = this.#requireLane();
    const capability = this.#requireSurfaceCapability();
    this.#acceptPlatformInput({
      ...event,
      sequence: 0n,
      surface: {
        sessionId: capability.sessionId,
        session: record.control.session,
        sessionEpoch: BigInt(lane.binding.sessionEpoch),
        window: record.control.window,
        view: record.control.view,
        surface: record.control.surface,
      },
    });
  }

  #recordForInput(event: BrowserPlatformInputEvent): SurfaceRecord {
    for (const record of this.#surfaces.values()) {
      if (
        sameHandle(record.control.session, event.surface.session)
        && sameHandle(record.control.window, event.surface.window)
        && sameHandle(record.control.view, event.surface.view)
        && sameHandle(record.control.surface, event.surface.surface)
      ) {
        return record;
      }
    }
    throw new Error("Voplay platform input targets an unknown Surface");
  }

  #activateInputSurface(key: string): void {
    if (this.#gamepadRouteSurface !== null && this.#gamepadRouteSurface !== key) {
      this.#gamepads?.invalidate();
    }
    this.#activeInputSurface = key;
  }

  #takeReturnSequence(): bigint {
    const sequence = this.#nextReturnSequence;
    if (sequence === 0xffff_ffff_ffff_ffffn) {
      throw new Error("Voplay framework return sequence exhausted");
    }
    this.#nextReturnSequence = sequence + 1n;
    return sequence;
  }

  #takeInputEventSequence(): bigint {
    const sequence = this.#nextInputEventSequence;
    if (sequence === 0xffff_ffff_ffff_ffffn) {
      throw new Error("Voplay platform input sequence exhausted");
    }
    this.#nextInputEventSequence = sequence + 1n;
    return sequence;
  }

  async #submitHapticsResult(result: BrowserHapticsResult): Promise<void> {
    const payload = new Uint8Array(17);
    const view = new DataView(payload.buffer);
    view.setBigUint64(0, result.requestId, true);
    view.setUint32(8, result.device.index, true);
    view.setUint32(12, result.device.generation, true);
    view.setUint8(16, hapticsOutcomeTag(result.outcome));
    const header = result.commandHeader;
    const sequence = this.#takeReturnSequence();
    await this.#requireLane().submit(encodeFrameworkPacket({
      kind: MessageKind.HapticsResult,
      engine: header.engine,
      channelEpoch: header.channelEpoch,
      commitId: header.commitId,
      baseRevision: header.baseRevision,
      newRevision: header.newRevision,
      requiredControlRevision: header.requiredControlRevision,
      sourceSimulationRevision: header.sourceSimulationRevision,
      sequence,
    }, payload), result.requestId);
  }

  #failPlatformInput(error: unknown): void {
    if (!this.#polling) return;
    this.#polling = false;
    this.#host?.reportError(`Voplay platform input lane failed: ${errorMessage(error)}`);
  }

  #surface(key: string): { record: SurfaceRecord; host: BrowserSurfaceHost } {
    const record = this.#surfaces.get(key);
    if (record === undefined) throw new Error("unknown Voplay browser Surface");
    return { record, host: this.#requireSurfaceHost() };
  }

  #validateSession(session: { index: number; generation: number }): void {
    const binding = this.#requireLane().binding.session;
    if (session.index !== binding.index || session.generation !== binding.generation) {
      throw new Error("Voplay packet App Session mismatch");
    }
  }

  #requireLane(): StudioFrameworkLane {
    if (this.#lane === null) throw new Error("Voplay framework lane is closed");
    return this.#lane;
  }

  #requireSurfaceCapability(): AppSurfaceCapability {
    if (this.#surfaceCapability === null) throw new Error("Voplay App Surface capability is closed");
    return this.#surfaceCapability;
  }

  #removeSurfaceFrameTraces(surface: BrowserSurfaceId): void {
    for (const [key, trace] of this.#frameTraces) {
      if (
        sameHandle(trace.engine, surface.engine.engine)
        && sameHandle(trace.target, surface.surface)
      ) {
        this.#frameTraces.delete(key);
      }
    }
  }

  #requireSurfaceHost(): BrowserSurfaceHost {
    if (this.#surfaceHost === null) throw new Error("Voplay browser device is unavailable");
    return this.#surfaceHost;
  }

  #requireHaptics(): BrowserHapticsHost {
    if (this.#haptics === null) throw new Error("Voplay browser haptics host is unavailable");
    return this.#haptics;
  }
}

function surfaceId(control: BrowserSurfaceControl): BrowserSurfaceId {
  return {
    engine: { session: control.session, engine: control.engine },
    surface: control.surface,
    domain: control.domain,
  };
}

function inputPolicy(value: number): "opaque" | "transparent" | "passthrough" {
  switch (value) {
    case 1:
      return "opaque";
    case 2:
      return "transparent";
    case 3:
      return "passthrough";
    default:
      throw new RangeError("invalid Voplay Surface input policy");
  }
}

function hostInputPolicy(
  value: "observe" | "passthrough" | "interactive" | "exclusive",
): "opaque" | "transparent" | "passthrough" {
  switch (value) {
    case "observe":
      return "transparent";
    case "passthrough":
      return "passthrough";
    case "interactive":
    case "exclusive":
      return "opaque";
  }
}

function browserInputPolicyTag(
  value: "observe" | "passthrough" | "interactive" | "exclusive",
): number {
  switch (value) {
    case "interactive":
    case "exclusive":
      return 1;
    case "observe":
      return 2;
    case "passthrough":
      return 3;
  }
}

function assertSameControlOwner(
  previous: BrowserSurfaceControl,
  next: BrowserSurfaceControl,
): void {
  if (
    !sameHandle(previous.engine, next.engine)
    || !sameHandle(previous.session, next.session)
    || !sameHandle(previous.window, next.window)
    || !sameHandle(previous.view, next.view)
    || !sameHandle(previous.surface, next.surface)
    || !sameHandle(previous.domain, next.domain)
    || previous.zOrder !== next.zOrder
    || previous.inputPolicy !== next.inputPolicy
  ) {
    throw new Error("Voplay SurfaceControl owner route changed");
  }
}

function sameHandle(
  left: { readonly index: number; readonly generation: number },
  right: { readonly index: number; readonly generation: number },
): boolean {
  return left.index === right.index && left.generation === right.generation;
}

function handleKey(handle: { readonly index: number; readonly generation: number }): string {
  return `${handle.index}:${handle.generation}`;
}

function engineResourceKey(
  engine: { readonly index: number; readonly generation: number },
  resource: bigint,
): string {
  return `${handleKey(engine)}/${resource}`;
}

function coalescedInputKey(event: BrowserPlatformInputEvent): string | null {
  if (
    event.type !== "pointerMove"
    && event.type !== "wheel"
    && event.type !== "gamepadAxis"
  ) return null;
  return [
    event.type,
    `${event.surface.surface.index}:${event.surface.surface.generation}`,
    event.type === "gamepadAxis"
      ? `${event.gamepadIndex ?? -1}:${event.gamepadGeneration ?? 0}:${event.gamepadControl ?? -1}`
      : event.pointerId ?? 0,
  ].join("/");
}

function classifyFrameFailure(error: unknown, submitted: boolean): BrowserFrameTerminal {
  const message = errorMessage(error);
  if (message.includes("device") || message.includes("GPU generation")) return "deviceLost";
  if (message.includes("unknown surface") || message.includes("Surface")) return "surfaceLost";
  return submitted ? "outcomeUnknown" : "rejectedBeforeSubmit";
}

function monotonicMicros(): bigint {
  return BigInt(Math.max(0, Math.floor(performance.now() * 1000)));
}

function takeAssetU64(view: DataView, offset: number): bigint {
  if (offset < 0 || offset + 8 > view.byteLength) {
    throw new Error("truncated Voplay browser texture identity");
  }
  return view.getBigUint64(offset, true);
}

class HostRenderReader {
  readonly #bytes: Uint8Array;
  readonly #view: DataView;
  #offset = 0;

  constructor(bytes: Uint8Array) {
    if (bytes.byteLength > MAX_COMMAND_BYTES) {
      throw new Error("Voplay host-render command exceeds browser limit");
    }
    this.#bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  magic(expected: string): void {
    const bytes = this.take(4);
    if (String.fromCharCode(...bytes) !== expected) {
      throw new Error(`invalid Voplay host-render ${expected} envelope`);
    }
  }

  u8(): number {
    return this.take(1)[0]!;
  }

  u32(): number {
    const offset = this.#reserve(4);
    return this.#view.getUint32(offset, true);
  }

  u64(): bigint {
    const offset = this.#reserve(8);
    return this.#view.getBigUint64(offset, true);
  }

  handle(): { readonly index: number; readonly generation: number } {
    const handle = { index: this.u32(), generation: this.u32() };
    if (handle.generation === 0 || handle.index === 0xffff_ffff) {
      throw new Error("invalid Voplay host-render handle");
    }
    return handle;
  }

  surface(
    routeEngine: { readonly index: number; readonly generation: number },
    session: { readonly index: number; readonly generation: number },
  ): BrowserSurfaceId {
    const engine = this.handle();
    const surface = this.handle();
    const domainEngine = this.handle();
    const domain = this.handle();
    if (!sameHandle(engine, routeEngine) || !sameHandle(domainEngine, routeEngine)) {
      throw new Error("Voplay host-render Surface route mismatch");
    }
    return { engine: { session, engine }, surface, domain };
  }

  metrics(): {
    readonly width: number;
    readonly height: number;
    readonly scaleNumerator: number;
    readonly scaleDenominator: number;
  } {
    const metrics = {
      width: this.u32(),
      height: this.u32(),
      scaleNumerator: this.u32(),
      scaleDenominator: this.u32(),
    };
    if (
      metrics.width <= 0
      || metrics.height <= 0
      || metrics.scaleNumerator <= 0
      || metrics.scaleDenominator <= 0
    ) {
      throw new Error("invalid Voplay host-render Surface metrics");
    }
    return metrics;
  }

  blob(): Uint8Array {
    return this.take(this.u32()).slice();
  }

  frame(
    routeEngine: { readonly index: number; readonly generation: number },
    session: { readonly index: number; readonly generation: number },
  ): {
    readonly surface: BrowserSurfaceId;
    readonly pulseId: bigint;
    readonly frameId: bigint;
    readonly deviceGeneration: bigint;
    readonly requiredRenderRevision: bigint;
    readonly requiredControlRevision: bigint;
    readonly graphSignature: bigint;
    readonly renderEndpoint: { readonly index: number; readonly generation: number };
    readonly commands: Uint8Array;
  } {
    return {
      surface: this.surface(routeEngine, session),
      pulseId: this.u64(),
      frameId: this.u64(),
      deviceGeneration: this.u64(),
      requiredRenderRevision: this.u64(),
      requiredControlRevision: this.u64(),
      graphSignature: this.u64(),
      renderEndpoint: this.handle(),
      commands: this.blob(),
    };
  }

  frameAck(
    routeEngine: { readonly index: number; readonly generation: number },
    session: { readonly index: number; readonly generation: number },
  ): {
    readonly surface: BrowserSurfaceId;
    readonly pulseId: bigint;
    readonly frameId: bigint;
    readonly deviceGeneration: bigint;
    readonly renderedRevision: bigint;
    readonly observedControlRevision: bigint;
    readonly renderEndpoint: { readonly index: number; readonly generation: number };
  } {
    return {
      surface: this.surface(routeEngine, session),
      pulseId: this.u64(),
      frameId: this.u64(),
      deviceGeneration: this.u64(),
      renderedRevision: this.u64(),
      observedControlRevision: this.u64(),
      renderEndpoint: this.handle(),
    };
  }

  readbackRequest(
    routeEngine: { readonly index: number; readonly generation: number },
  ): {
    readonly target: { readonly index: number; readonly generation: number };
    readonly expectedRevision: bigint;
    readonly region: Readonly<{ x: number; y: number; width: number; height: number }>;
    readonly format: number;
  } {
    const engine = this.handle();
    const target = this.handle();
    const expectedRevision = this.u64();
    const region = {
      x: this.u32(),
      y: this.u32(),
      width: this.u32(),
      height: this.u32(),
    };
    const format = this.u32();
    if (
      !sameHandle(engine, routeEngine)
      || expectedRevision <= 0n
      || region.width <= 0
      || region.height <= 0
      || format < 1
      || format > 4
    ) {
      throw new Error("invalid Voplay host-render readback request");
    }
    return { target, expectedRevision, region, format };
  }

  frameTraceRequest(): {
    readonly frameId: bigint;
    readonly graphSignature: bigint;
    readonly includeAttachments: boolean;
    readonly includeShaderDiagnostics: boolean;
    readonly maxBytes: number;
  } {
    const frameId = this.u64();
    const graphSignature = this.u64();
    const flags = this.u8();
    const maxBytes = this.u32();
    if (
      frameId <= 0n
      || graphSignature <= 0n
      || (flags & ~0x03) !== 0
      || maxBytes <= 0
      || maxBytes > MAX_COMMAND_BYTES
    ) {
      throw new Error("invalid Voplay host-render frame trace request");
    }
    return {
      frameId,
      graphSignature,
      includeAttachments: (flags & 1) !== 0,
      includeShaderDiagnostics: (flags & 2) !== 0,
      maxBytes,
    };
  }

  take(length: number): Uint8Array {
    const offset = this.#reserve(length);
    return this.#bytes.subarray(offset, offset + length);
  }

  finish(): void {
    if (this.#offset !== this.#bytes.byteLength) {
      throw new Error("Voplay host-render command has trailing bytes");
    }
  }

  #reserve(length: number): number {
    if (!Number.isSafeInteger(length) || length < 0) {
      throw new Error("invalid Voplay host-render field length");
    }
    const offset = this.#offset;
    const end = offset + length;
    if (!Number.isSafeInteger(end) || end > this.#bytes.byteLength) {
      throw new Error("truncated Voplay host-render command");
    }
    this.#offset = end;
    return offset;
  }
}

function hostFrameKey(surface: BrowserSurfaceId, frameId: bigint): string {
  return `${surfaceKey(surface)}:${frameId}`;
}

function encodeHostRenderFailure(
  engine: { readonly index: number; readonly generation: number },
  request: bigint,
  failure: number,
  frameTrace: boolean,
): Uint8Array {
  if (request <= 0n || failure < 1 || failure > 4) {
    throw new Error("invalid Voplay host-render failure");
  }
  const bytes = new Uint8Array(4 + 8 + 4 + 1 + 8 + 1);
  bytes.set([0x56, 0x48, 0x52, 0x34], 0);
  const view = new DataView(bytes.buffer);
  view.setUint32(4, engine.index, true);
  view.setUint32(8, engine.generation, true);
  bytes.set([0x56, 0x48, 0x52, 0x32], 12);
  bytes[16] = frameTrace ? 4 : 2;
  view.setBigUint64(17, request, true);
  bytes[25] = failure;
  return bytes;
}

function encodeHostRenderReadback(
  engine: { readonly index: number; readonly generation: number },
  request: bigint,
  target: { readonly index: number; readonly generation: number },
  targetRevision: bigint,
  rowBytes: number,
  payload: Uint8Array,
): Uint8Array {
  if (
    request <= 0n
    || targetRevision <= 0n
    || !Number.isSafeInteger(rowBytes)
    || rowBytes <= 0
    || payload.byteLength === 0
    || payload.byteLength > MAX_COMMAND_BYTES
  ) {
    throw new Error("invalid Voplay host-render readback result");
  }
  const bytes = new Uint8Array(57 + payload.byteLength);
  const view = new DataView(bytes.buffer);
  bytes.set([0x56, 0x48, 0x52, 0x34], 0);
  view.setUint32(4, engine.index, true);
  view.setUint32(8, engine.generation, true);
  bytes.set([0x56, 0x48, 0x52, 0x32], 12);
  bytes[16] = 1;
  view.setBigUint64(17, request, true);
  view.setUint32(25, engine.index, true);
  view.setUint32(29, engine.generation, true);
  view.setUint32(33, target.index, true);
  view.setUint32(37, target.generation, true);
  view.setBigUint64(41, targetRevision, true);
  view.setUint32(49, rowBytes, true);
  view.setUint32(53, payload.byteLength, true);
  bytes.set(payload, 57);
  return bytes;
}

function encodeBrowserFrameTrace(trace: Readonly<{
  readonly engine: { readonly index: number; readonly generation: number };
  readonly target: { readonly index: number; readonly generation: number };
  readonly frameId: bigint;
  readonly graphSignature: bigint;
  readonly fence: bigint;
  readonly width: number;
  readonly height: number;
}>): Uint8Array {
  const label = new TextEncoder().encode("browser-portable");
  const bytes = new Uint8Array(128 + label.byteLength);
  const view = new DataView(bytes.buffer);
  bytes.set([0x56, 0x47, 0x54, 0x31, 1, 0, 0, 0], 0);
  view.setBigUint64(8, trace.frameId, true);
  view.setBigUint64(16, trace.graphSignature, true);
  view.setUint32(24, 1, true);
  view.setUint32(28, trace.engine.index, true);
  view.setUint32(32, trace.engine.generation, true);
  bytes.set([2, 0, 0, 0], 36);
  view.setUint32(40, trace.target.index, true);
  view.setUint32(44, trace.target.generation, true);
  view.setUint32(48, 0, true);
  view.setUint32(52, 0, true);
  view.setUint32(56, trace.width, true);
  view.setUint32(60, trace.height, true);
  view.setBigUint64(64, trace.fence, true);
  view.setUint32(72, 1, true);
  view.setUint32(76, 0, true);
  view.setUint32(80, 0, true);
  view.setUint32(84, 1, true);
  bytes.set([1, 0, 0, 0], 88);
  view.setBigUint64(92, 0n, true);
  view.setBigUint64(100, 0n, true);
  view.setBigUint64(108, trace.graphSignature, true);
  view.setUint32(116, 1, true);
  view.setUint32(120, 0, true);
  view.setUint32(124, label.byteLength, true);
  bytes.set(label, 128);
  return bytes;
}

function encodeHostRenderFrameTrace(
  engine: { readonly index: number; readonly generation: number },
  request: bigint,
  frameId: bigint,
  graphSignature: bigint,
  trace: Uint8Array,
): Uint8Array {
  if (
    request <= 0n
    || frameId <= 0n
    || graphSignature <= 0n
    || trace.byteLength === 0
    || trace.byteLength > MAX_COMMAND_BYTES
  ) {
    throw new Error("invalid Voplay host-render frame trace result");
  }
  const bytes = new Uint8Array(45 + trace.byteLength);
  const view = new DataView(bytes.buffer);
  bytes.set([0x56, 0x48, 0x52, 0x34], 0);
  view.setUint32(4, engine.index, true);
  view.setUint32(8, engine.generation, true);
  bytes.set([0x56, 0x48, 0x52, 0x32], 12);
  bytes[16] = 3;
  view.setBigUint64(17, request, true);
  view.setBigUint64(25, frameId, true);
  view.setBigUint64(33, graphSignature, true);
  view.setUint32(41, trace.byteLength, true);
  bytes.set(trace, 45);
  return bytes;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function hapticsOutcomeTag(outcome: BrowserHapticsResult["outcome"]): number {
  switch (outcome) {
    case "succeeded": return 1;
    case "unsupported": return 2;
    case "cancelled": return 3;
    case "deviceLost": return 4;
    case "failed": return 5;
  }
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export default new VoplayStudioRenderer();
