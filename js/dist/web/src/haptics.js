import { BROWSER_GAMEPAD_DEVICE_BASE } from "./framework_lane.js";
const MAX_PENDING_HAPTICS = 256;
const MAX_RUMBLE_DURATION_MILLIS = 60_000;
export class BrowserHapticsHost {
    #pending = new Map();
    #emit;
    #generation;
    #onDisconnect;
    #closed = false;
    #enabled = true;
    constructor(emit, generation) {
        this.#emit = emit;
        this.#generation = generation ?? null;
        this.#onDisconnect = (event) => {
            const gamepad = event.gamepad;
            this.disconnectGamepad(gamepad.index);
        };
        window.addEventListener("gamepaddisconnected", this.#onDisconnect);
    }
    accept(packet) {
        if (this.#closed)
            throw new Error("browser haptics host is closed");
        if (packet.header.kind !== 34 /* MessageKind.HapticsCommand */) {
            throw new Error("expected Voplay haptics command");
        }
        const view = new DataView(packet.payload.buffer, packet.payload.byteOffset, packet.payload.byteLength);
        if (packet.payload.byteLength < 1)
            throw new Error("truncated Voplay haptics command");
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
    setEnabled(enabled) {
        if (this.#closed || this.#enabled === enabled)
            return;
        this.#enabled = enabled;
        if (enabled)
            return;
        for (const pending of [...this.#pending.values()]) {
            const reset = pending.actuator?.reset?.();
            if (reset)
                void reset.catch(() => undefined);
            this.#finish(pending, "cancelled");
        }
    }
    ownerSnapshot() {
        return {
            closed: this.#closed,
            enabled: this.#enabled,
            pending: this.#pending.size,
        };
    }
    disconnectGamepad(index, generation) {
        if (this.#closed)
            return;
        for (const pending of [...this.#pending.values()]) {
            if (browserGamepadIndex(pending.device) === index
                && (generation === undefined || pending.device.generation === generation)) {
                this.#finish(pending, "deviceLost");
            }
        }
    }
    close(emitResults = true) {
        if (this.#closed)
            return;
        this.#closed = true;
        window.removeEventListener("gamepaddisconnected", this.#onDisconnect);
        for (const pending of [...this.#pending.values()]) {
            const reset = pending.actuator?.reset?.();
            if (reset)
                void reset.catch(() => undefined);
            if (emitResults)
                this.#finish(pending, "cancelled");
            else
                this.#discard(pending);
        }
    }
    #start(header, payload) {
        if (payload.byteLength !== 40)
            throw new Error("invalid Voplay rumble command");
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
        if (requestId === 0n
            || device.index === 0xffff_ffff
            || device.generation === 0
            || duration === 0
            || duration > MAX_RUMBLE_DURATION_MILLIS
            || (strong === 0 && weak === 0)
            || deadline === 0n
            || this.#pending.has(pendingKey(header.engine, requestId))
            || this.#pending.size >= MAX_PENDING_HAPTICS) {
            throw new Error("invalid Voplay rumble identity");
        }
        const pending = {
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
            if (reset)
                void reset.catch(() => undefined);
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
        if (this.#generation !== null
            && this.#generation(gamepadIndex) !== device.generation) {
            this.#finish(pending, "deviceLost");
            return;
        }
        const gamepad = navigator.getGamepads().find((candidate) => candidate !== null && candidate.index === gamepadIndex);
        if (gamepad === undefined || gamepad === null) {
            this.#finish(pending, "deviceLost");
            return;
        }
        const actuator = gamepad.vibrationActuator
            ?? gamepad.hapticActuators?.[0];
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
        }).then((result) => this.#finish(pending, result === "complete"
            ? "succeeded"
            : result === "preempted" ? "cancelled" : "failed"), () => this.#finish(pending, "failed"));
    }
    #cancel(header, payload) {
        if (payload.byteLength !== 16)
            throw new Error("invalid Voplay rumble cancellation");
        const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
        const requestId = view.getBigUint64(0, true);
        const device = {
            index: view.getUint32(8, true),
            generation: view.getUint32(12, true),
        };
        const pending = this.#pending.get(pendingKey(header.engine, requestId));
        if (pending === undefined || !sameHandle(pending.device, device))
            return;
        const gamepadIndex = browserGamepadIndex(device);
        const gamepad = navigator.getGamepads().find((candidate) => (gamepadIndex !== null
            && candidate !== null
            && candidate.index === gamepadIndex));
        const actuator = gamepad === undefined || gamepad === null
            ? undefined
            : gamepad.vibrationActuator
                ?? gamepad.hapticActuators?.[0];
        void actuator?.reset?.();
        this.#finish(pending, "cancelled");
    }
    #finish(pending, outcome) {
        if (pending.settled)
            return;
        pending.settled = true;
        if (pending.deadlineTimer !== undefined)
            clearTimeout(pending.deadlineTimer);
        this.#pending.delete(pendingKey(pending.header.engine, pending.requestId));
        this.#emit({
            commandHeader: pending.header,
            requestId: pending.requestId,
            device: pending.device,
            outcome,
        });
    }
    #discard(pending) {
        if (pending.settled)
            return;
        pending.settled = true;
        if (pending.deadlineTimer !== undefined)
            clearTimeout(pending.deadlineTimer);
        this.#pending.delete(pendingKey(pending.header.engine, pending.requestId));
    }
}
function nowMillis() {
    return BigInt(Math.max(0, Math.round(performance.now())));
}
function sameHandle(left, right) {
    return left.index === right.index && left.generation === right.generation;
}
function browserGamepadIndex(device) {
    return device.index < BROWSER_GAMEPAD_DEVICE_BASE
        ? null
        : device.index - BROWSER_GAMEPAD_DEVICE_BASE;
}
function pendingKey(engine, requestId) {
    return `${engine.index}:${engine.generation}/${requestId}`;
}
