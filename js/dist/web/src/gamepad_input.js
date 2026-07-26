const MAX_BROWSER_GAMEPADS = 64;
const MAX_GAMEPAD_CONTROLS = 256;
export class BrowserGamepadInputSource {
    #emit;
    #enabled;
    #known = new Map();
    #generations = new Map();
    #animationFrame = null;
    #closed = false;
    constructor(emit, enabled = () => true) {
        this.#emit = emit;
        this.#enabled = enabled;
    }
    start() {
        if (this.#closed || this.#animationFrame !== null)
            return;
        this.#animationFrame = requestAnimationFrame(() => this.#poll());
    }
    invalidate() {
        this.#disconnectKnown(nowMicros());
    }
    generation(index) {
        return this.#known.get(index)?.generation;
    }
    ownerSnapshot() {
        return {
            closed: this.#closed,
            polling: this.#animationFrame !== null,
            connected: this.#known.size,
            generations: this.#generations.size,
        };
    }
    close(emitDisconnects = true) {
        if (this.#closed)
            return;
        this.#closed = true;
        if (this.#animationFrame !== null) {
            cancelAnimationFrame(this.#animationFrame);
            this.#animationFrame = null;
        }
        if (emitDisconnects) {
            const timestampMicros = nowMicros();
            for (const [index, snapshot] of this.#known) {
                this.#emitReleases(index, snapshot, timestampMicros);
                this.#emit({
                    type: "gamepadDisconnect",
                    timestampMicros,
                    gamepadIndex: index,
                    gamepadGeneration: snapshot.generation,
                    synthesized: true,
                });
            }
        }
        this.#known.clear();
    }
    #poll() {
        this.#animationFrame = null;
        if (this.#closed)
            return;
        const timestampMicros = nowMicros();
        if (!this.#enabled()) {
            this.#disconnectKnown(timestampMicros);
            this.#animationFrame = requestAnimationFrame(() => this.#poll());
            return;
        }
        const seen = new Set();
        for (const gamepad of navigator.getGamepads()) {
            if (gamepad === null)
                continue;
            if (!Number.isSafeInteger(gamepad.index)
                || gamepad.index < 0
                || gamepad.index >= 0x7fff_ffff
                || (!this.#known.has(gamepad.index) && seen.size >= MAX_BROWSER_GAMEPADS)) {
                continue;
            }
            seen.add(gamepad.index);
            const previous = this.#known.get(gamepad.index);
            if (previous !== undefined
                && (previous.id !== gamepad.id || previous.mapping !== gamepad.mapping)) {
                this.#emitReleases(gamepad.index, previous, timestampMicros);
                this.#emit({
                    type: "gamepadDisconnect",
                    timestampMicros,
                    gamepadIndex: gamepad.index,
                    gamepadGeneration: previous.generation,
                    synthesized: true,
                });
                this.#known.delete(gamepad.index);
            }
            const current = this.#snapshot(gamepad);
            const activePrevious = this.#known.get(gamepad.index);
            if (activePrevious === undefined) {
                this.#known.set(gamepad.index, current);
                this.#emit({
                    type: "gamepadConnect",
                    timestampMicros,
                    gamepadIndex: gamepad.index,
                    gamepadGeneration: current.generation,
                    gamepadId: current.id,
                    gamepadMapping: current.mapping,
                });
                this.#emitChanges(gamepad.index, undefined, current, timestampMicros);
            }
            else {
                this.#known.set(gamepad.index, current);
                this.#emitChanges(gamepad.index, activePrevious, current, timestampMicros);
            }
        }
        for (const [index, snapshot] of [...this.#known]) {
            if (seen.has(index))
                continue;
            this.#emitReleases(index, snapshot, timestampMicros);
            this.#emit({
                type: "gamepadDisconnect",
                timestampMicros,
                gamepadIndex: index,
                gamepadGeneration: snapshot.generation,
                synthesized: true,
            });
            this.#known.delete(index);
        }
        this.#animationFrame = requestAnimationFrame(() => this.#poll());
    }
    #disconnectKnown(timestampMicros) {
        for (const [index, snapshot] of this.#known) {
            this.#emitReleases(index, snapshot, timestampMicros);
            this.#emit({
                type: "gamepadDisconnect",
                timestampMicros,
                gamepadIndex: index,
                gamepadGeneration: snapshot.generation,
                synthesized: true,
            });
        }
        this.#known.clear();
    }
    #snapshot(gamepad) {
        const previous = this.#known.get(gamepad.index);
        let generation = previous?.generation;
        if (generation === undefined) {
            generation = (this.#generations.get(gamepad.index) ?? 0) + 1;
            if (generation > 0xffff_ffff) {
                throw new Error("browser gamepad generation exhausted");
            }
            this.#generations.set(gamepad.index, generation);
        }
        return {
            generation,
            id: gamepad.id,
            mapping: gamepad.mapping,
            buttons: gamepad.buttons
                .slice(0, MAX_GAMEPAD_CONTROLS)
                .map((button) => quantizeButton(button.value)),
            axes: gamepad.axes.slice(0, MAX_GAMEPAD_CONTROLS).map(quantizeAxis),
        };
    }
    #emitChanges(index, previous, current, timestampMicros, synthesized = false) {
        const buttonCount = Math.max(current.buttons.length, previous?.buttons.length ?? 0);
        for (let control = 0; control < buttonCount; control += 1) {
            const value = current.buttons[control] ?? 0;
            if (value !== (previous?.buttons[control] ?? 0)) {
                this.#emit({
                    type: "gamepadButton",
                    timestampMicros,
                    gamepadIndex: index,
                    gamepadGeneration: current.generation,
                    gamepadControl: control,
                    gamepadValueQ16: value,
                    ...(synthesized ? { synthesized: true } : {}),
                });
            }
        }
        const axisCount = Math.max(current.axes.length, previous?.axes.length ?? 0);
        for (let control = 0; control < axisCount; control += 1) {
            const value = current.axes[control] ?? 0;
            if (value !== (previous?.axes[control] ?? 0)) {
                this.#emit({
                    type: "gamepadAxis",
                    timestampMicros,
                    gamepadIndex: index,
                    gamepadGeneration: current.generation,
                    gamepadControl: control,
                    gamepadValueQ16: value,
                    ...(synthesized ? { synthesized: true } : {}),
                });
            }
        }
    }
    #emitReleases(index, snapshot, timestampMicros) {
        this.#emitChanges(index, snapshot, {
            ...snapshot,
            buttons: snapshot.buttons.map(() => 0),
            axes: snapshot.axes.map(() => 0),
        }, timestampMicros, true);
    }
}
function quantizeButton(value) {
    if (!Number.isFinite(value))
        return 0;
    return Math.round(Math.min(1, Math.max(0, value)) * 0xffff);
}
function quantizeAxis(value) {
    if (!Number.isFinite(value))
        return 0;
    return Math.round(Math.min(1, Math.max(-1, value)) * 0x7fff);
}
function nowMicros() {
    return BigInt(Math.max(0, Math.round(performance.now() * 1_000)));
}
