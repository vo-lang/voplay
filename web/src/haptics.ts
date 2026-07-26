import type {
  FrameworkPacket,
  FrameworkPacketHeader,
  GenerationalHandle,
} from "../../protocol/generated/voplay_protocol.js";
import { MessageKind } from "../../protocol/generated/voplay_protocol.js";
import { BROWSER_GAMEPAD_DEVICE_BASE } from "./framework_lane.js";

export type BrowserHapticsOutcome =
  | "succeeded"
  | "unsupported"
  | "cancelled"
  | "deviceLost"
  | "failed";

const MAX_PENDING_HAPTICS = 256;
const MAX_RUMBLE_DURATION_MILLIS = 60_000;

export interface BrowserHapticsResult {
  readonly commandHeader: FrameworkPacketHeader;
  readonly requestId: bigint;
  readonly device: GenerationalHandle;
  readonly outcome: BrowserHapticsOutcome;
}

interface PendingRumble {
  readonly header: FrameworkPacketHeader;
  readonly requestId: bigint;
  readonly device: GenerationalHandle;
  actuator?: DualRumbleActuator;
  deadlineTimer?: ReturnType<typeof setTimeout>;
  settled: boolean;
}

interface DualRumbleActuator {
  playEffect(
    type: "dual-rumble",
    parameters: {
      duration: number;
      startDelay: number;
      strongMagnitude: number;
      weakMagnitude: number;
    },
  ): Promise<string>;
  reset?(): Promise<string>;
}

export class BrowserHapticsHost {
  readonly #pending = new Map<string, PendingRumble>();
  readonly #emit: (result: BrowserHapticsResult) => void;
  readonly #generation: ((index: number) => number | undefined) | null;
  readonly #onDisconnect: (event: Event) => void;
  #closed = false;
  #enabled = true;

  constructor(
    emit: (result: BrowserHapticsResult) => void,
    generation?: (index: number) => number | undefined,
  ) {
    this.#emit = emit;
    this.#generation = generation ?? null;
    this.#onDisconnect = (event) => {
      const gamepad = (event as GamepadEvent).gamepad;
      this.disconnectGamepad(gamepad.index);
    };
    window.addEventListener("gamepaddisconnected", this.#onDisconnect);
  }

  accept(packet: FrameworkPacket): void {
    if (this.#closed) throw new Error("browser haptics host is closed");
    if (packet.header.kind !== MessageKind.HapticsCommand) {
      throw new Error("expected Voplay haptics command");
    }
    const view = new DataView(
      packet.payload.buffer,
      packet.payload.byteOffset,
      packet.payload.byteLength,
    );
    if (packet.payload.byteLength < 1) throw new Error("truncated Voplay haptics command");
    switch (view.getUint8(0)) {
      case 1:
        this.#start(packet.header, packet.payload.subarray(1));
        return;
      case 2:
        this.#cancel(packet.header, packet.payload.subarray(1));
        return;
      default:
        throw new Error("unknown Voplay haptics command");
    }
  }

  setEnabled(enabled: boolean): void {
    if (this.#closed || this.#enabled === enabled) return;
    this.#enabled = enabled;
    if (enabled) return;
    for (const pending of [...this.#pending.values()]) {
      const reset = pending.actuator?.reset?.();
      if (reset) void reset.catch(() => undefined);
      this.#finish(pending, "cancelled");
    }
  }

  ownerSnapshot(): Readonly<{
    closed: boolean;
    enabled: boolean;
    pending: number;
  }> {
    return {
      closed: this.#closed,
      enabled: this.#enabled,
      pending: this.#pending.size,
    };
  }

  disconnectGamepad(index: number, generation?: number): void {
    if (this.#closed) return;
    for (const pending of [...this.#pending.values()]) {
      if (
        browserGamepadIndex(pending.device) === index
        && (generation === undefined || pending.device.generation === generation)
      ) {
        this.#finish(pending, "deviceLost");
      }
    }
  }

  close(emitResults = true): void {
    if (this.#closed) return;
    this.#closed = true;
    window.removeEventListener("gamepaddisconnected", this.#onDisconnect);
    for (const pending of [...this.#pending.values()]) {
      const reset = pending.actuator?.reset?.();
      if (reset) void reset.catch(() => undefined);
      if (emitResults) this.#finish(pending, "cancelled");
      else this.#discard(pending);
    }
  }

  #start(header: FrameworkPacketHeader, payload: Uint8Array): void {
    if (payload.byteLength !== 40) throw new Error("invalid Voplay rumble command");
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const requestId = view.getBigUint64(0, true);
    const device = {
      index: view.getUint32(16, true),
      generation: view.getUint32(20, true),
    };
    const duration = view.getUint32(24, true);
    const strong = view.getUint16(28, true);
    const weak = view.getUint16(30, true);
    const deadline = view.getBigUint64(32, true);
    if (
      requestId === 0n
      || device.index === 0xffff_ffff
      || device.generation === 0
      || duration === 0
      || duration > MAX_RUMBLE_DURATION_MILLIS
      || (strong === 0 && weak === 0)
      || deadline === 0n
      || this.#pending.has(pendingKey(header.engine, requestId))
      || this.#pending.size >= MAX_PENDING_HAPTICS
    ) {
      throw new Error("invalid Voplay rumble identity");
    }
    const pending: PendingRumble = {
      header,
      requestId,
      device,
      settled: false,
    };
    this.#pending.set(pendingKey(header.engine, requestId), pending);
    const now = nowMillis();
    if (deadline <= now) {
      this.#finish(pending, "cancelled");
      return;
    }
    const deadlineDelay = Number(deadline - now);
    pending.deadlineTimer = setTimeout(() => {
      const reset = pending.actuator?.reset?.();
      if (reset) void reset.catch(() => undefined);
      this.#finish(pending, "cancelled");
    }, Math.min(0x7fff_ffff, deadlineDelay));
    if (!this.#enabled) {
      this.#finish(pending, "cancelled");
      return;
    }
    const gamepadIndex = browserGamepadIndex(device);
    if (gamepadIndex === null) {
      this.#finish(pending, "deviceLost");
      return;
    }
    if (
      this.#generation !== null
      && this.#generation(gamepadIndex) !== device.generation
    ) {
      this.#finish(pending, "deviceLost");
      return;
    }
    const gamepad = navigator.getGamepads().find(
      (candidate) => candidate !== null && candidate.index === gamepadIndex,
    );
    if (gamepad === undefined || gamepad === null) {
      this.#finish(pending, "deviceLost");
      return;
    }
    const actuator = (
      gamepad as Gamepad & {
        vibrationActuator?: DualRumbleActuator;
        hapticActuators?: readonly DualRumbleActuator[];
      }
    ).vibrationActuator
      ?? (
        gamepad as Gamepad & {
          hapticActuators?: readonly DualRumbleActuator[];
        }
      ).hapticActuators?.[0];
    if (actuator === undefined) {
      this.#finish(pending, "unsupported");
      return;
    }
    pending.actuator = actuator;
    void actuator.playEffect("dual-rumble", {
      duration,
      startDelay: 0,
      strongMagnitude: strong / 0xffff,
      weakMagnitude: weak / 0xffff,
    }).then(
      (result) => this.#finish(
        pending,
        result === "complete"
          ? "succeeded"
          : result === "preempted" ? "cancelled" : "failed",
      ),
      () => this.#finish(pending, "failed"),
    );
  }

  #cancel(header: FrameworkPacketHeader, payload: Uint8Array): void {
    if (payload.byteLength !== 16) throw new Error("invalid Voplay rumble cancellation");
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const requestId = view.getBigUint64(0, true);
    const device = {
      index: view.getUint32(8, true),
      generation: view.getUint32(12, true),
    };
    const pending = this.#pending.get(pendingKey(header.engine, requestId));
    if (pending === undefined || !sameHandle(pending.device, device)) return;
    const gamepadIndex = browserGamepadIndex(device);
    const gamepad = navigator.getGamepads().find(
      (candidate) => (
        gamepadIndex !== null
        && candidate !== null
        && candidate.index === gamepadIndex
      ),
    );
    const actuator = gamepad === undefined || gamepad === null
      ? undefined
      : (
          gamepad as Gamepad & {
            vibrationActuator?: DualRumbleActuator;
            hapticActuators?: readonly DualRumbleActuator[];
          }
        ).vibrationActuator
        ?? (
          gamepad as Gamepad & {
            hapticActuators?: readonly DualRumbleActuator[];
          }
        ).hapticActuators?.[0];
    void actuator?.reset?.();
    this.#finish(pending, "cancelled");
  }

  #finish(pending: PendingRumble, outcome: BrowserHapticsOutcome): void {
    if (pending.settled) return;
    pending.settled = true;
    if (pending.deadlineTimer !== undefined) clearTimeout(pending.deadlineTimer);
    this.#pending.delete(pendingKey(pending.header.engine, pending.requestId));
    this.#emit({
      commandHeader: pending.header,
      requestId: pending.requestId,
      device: pending.device,
      outcome,
    });
  }

  #discard(pending: PendingRumble): void {
    if (pending.settled) return;
    pending.settled = true;
    if (pending.deadlineTimer !== undefined) clearTimeout(pending.deadlineTimer);
    this.#pending.delete(pendingKey(pending.header.engine, pending.requestId));
  }
}

function nowMillis(): bigint {
  return BigInt(Math.max(0, Math.round(performance.now())));
}

function sameHandle(left: GenerationalHandle, right: GenerationalHandle): boolean {
  return left.index === right.index && left.generation === right.generation;
}

function browserGamepadIndex(device: GenerationalHandle): number | null {
  return device.index < BROWSER_GAMEPAD_DEVICE_BASE
    ? null
    : device.index - BROWSER_GAMEPAD_DEVICE_BASE;
}

function pendingKey(engine: GenerationalHandle, requestId: bigint): string {
  return `${engine.index}:${engine.generation}/${requestId}`;
}
