// Render island bootstrap: load wasm VM, wire transport, start render loop.
// Per design doc: JS is bootstrap only; render loop runs inside Vo VM.
// RenderIsland: manages the render island VM lifecycle.
export class RenderIsland {
    constructor(config) {
        this.vm = null;
        this.hostTimers = new Map();
        this.stopped = false;
        this.recentConsoleErrors = [];
        this.originalConsoleError = null;
        this.config = config;
    }
    async init(voWeb) {
        const globalScope = globalThis;
        globalScope.__voVoplayDebugLog = (message) => this.debug(`voplay wasm ${message}`);
        this.installConsoleErrorCapture();
        await voWeb.initVFS();
        let jsGlueUrl = '';
        let blobUrlToRevoke = '';
        if (this.config.voplayWasmJsGlue && this.config.voplayWasmJsGlue.length > 0) {
            const jsText = new TextDecoder().decode(this.config.voplayWasmJsGlue);
            const blob = new Blob([jsText], { type: 'application/javascript' });
            blobUrlToRevoke = URL.createObjectURL(blob);
            jsGlueUrl = blobUrlToRevoke;
        }
        try {
            await voWeb.preloadExtModule("github.com/vo-lang/voplay", this.config.voplayWasm, jsGlueUrl);
        }
        finally {
            if (blobUrlToRevoke)
                URL.revokeObjectURL(blobUrlToRevoke);
        }
        this.vm = voWeb.VoVm.withExterns(this.config.bytecode);
        this.vm.runInit();
    }
    start() {
        if (!this.vm)
            throw new Error("VM not initialized");
        this.stopped = false;
        this.config.channel.onReceive((frame) => {
            try {
                if (this.stopped || !this.vm)
                    return;
                this.vm.pushIslandCommand(frame);
                const outcome = this.vm.runScheduled();
                this.flush();
                this.scheduleHostEvents();
            }
            catch (error) {
                this.fail(`runScheduled inbound_bytes=${frame.byteLength}`, error);
            }
        });
    }
    stop() {
        this.stopped = true;
        for (const id of this.hostTimers.values()) {
            clearTimeout(id);
        }
        this.hostTimers.clear();
        this.config.channel.close();
        this.vm = null;
    }
    flush() {
        if (!this.vm)
            return;
        const cmds = this.vm.takeOutboundCommands();
        for (const frame of cmds) {
            this.config.channel.send(frame);
        }
    }
    scheduleHostEvents() {
        if (!this.vm || this.stopped)
            return;
        const events = this.vm.takePendingHostEvents();
        for (const ev of events) {
            if (this.hostTimers.has(ev.token))
                continue;
            const id = window.setTimeout(() => {
                try {
                    this.hostTimers.delete(ev.token);
                    if (this.stopped || !this.vm)
                        return;
                    this.vm.wakeHostEvent(ev.token);
                    const outcome = this.vm.runScheduled();
                    this.flush();
                    this.scheduleHostEvents();
                }
                catch (error) {
                    this.fail(`wakeHostEvent token=${ev.token}`, error);
                }
            }, ev.delayMs);
            this.hostTimers.set(ev.token, id);
        }
    }
    fail(context, error) {
        let message = `${context}: ${this.describeError(error)}`;
        const captured = this.drainCapturedErrors();
        if (captured.length > 0) {
            message += `\nRust panic: ${captured.join('\n')}`;
        }
        this.debug(`render island fail: ${message}`);
        this.config.onError?.(message);
        this.stop();
    }
    describeError(error) {
        if (error instanceof Error) {
            const extraFields = error;
            const extra = Object.entries(extraFields)
                .map(([key, value]) => `${key}=${this.describeValue(value)}`)
                .join(", ");
            let message = `${error.name}: ${error.message}`;
            if (error.stack) {
                message += `\n${error.stack}`;
            }
            if (extra) {
                message += `\nextra: ${extra}`;
            }
            return message;
        }
        if (typeof error === "object" && error !== null) {
            return this.describeValue(error);
        }
        return String(error);
    }
    describeValue(value) {
        if (typeof value === "string")
            return value;
        if (typeof value === "number" || typeof value === "boolean" || value === null || value === undefined) {
            return String(value);
        }
        try {
            return JSON.stringify(value);
        }
        catch {
            return String(value);
        }
    }
    debug(message) {
        this.config.debugLog?.(message);
    }
    installConsoleErrorCapture() {
        if (this.originalConsoleError)
            return;
        this.originalConsoleError = console.error.bind(console);
        console.error = (...args) => {
            const msg = args.map(String).join(' ');
            this.recentConsoleErrors.push(msg);
            if (this.recentConsoleErrors.length > 8)
                this.recentConsoleErrors.shift();
            this.debug(`[console.error] ${msg}`);
            this.originalConsoleError(...args);
        };
    }
    drainCapturedErrors() {
        const errors = this.recentConsoleErrors.slice();
        this.recentConsoleErrors.length = 0;
        return errors;
    }
}
