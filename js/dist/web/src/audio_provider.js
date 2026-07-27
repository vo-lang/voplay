import { decodeFrameworkPacket, encodeFrameworkPacket, } from "../../protocol/generated/voplay_protocol.js";
const AUDIO_CONTROL_MAGIC = [0x56, 0x41, 0x43, 0x53];
const AUDIO_CONTROL_PREFIX_BYTES = 20;
const AUDIO_CONTROL_ENTRY_PREFIX_BYTES = 48;
const MAX_AUDIO_CONTROL_ENTRIES = 4096;
const MAX_AUDIO_DESCRIPTOR_BYTES = 4 * 1024 * 1024;
const MAX_AUDIO_BUSES = 64;
const MAX_PERSISTENT_SOURCES = 1024;
const MAX_AUDIO_DUCKING_RULES = 128;
const AUDIO_DROPPED_OUTCOME = 2;
const AUDIO_LOCKED_OUTCOME = 5;
const AUDIO_FAILED_OUTCOME = 7;
const AUDIO_DEADLINE_EXCEEDED_OUTCOME = 6;
const AUDIO_CONTROL_UNAVAILABLE_OUTCOME = 4;
const MAX_AUDIO_ASSETS = 65_536;
const MAX_AUDIO_ASSET_BYTES = 512 * 1024 * 1024;
const MAX_DECODED_AUDIO_BYTES = 512 * 1024 * 1024;
const MAX_AUDIO_VOICES = 1024;
const MAX_AUDIO_OBSERVATION_HISTORY = 256;
const MAX_U64 = 0xffffffffffffffffn;
export class VoplayBrowserAudioProvider {
    #host = null;
    #lane = null;
    #polling = false;
    #state = "created";
    #engine = null;
    #endpointGeneration = { index: 0, generation: 1 };
    #lastSequence = 0n;
    #controlRevision = 0n;
    #nowMillis = 0n;
    #audioFrameOffset = 0n;
    #lastControl = null;
    #lastObservedProgress = "";
    #publishedObservations = new Set();
    #realizationFailures = new Set();
    #observedAudioEventSequence = 0n;
    #deviceState = "ready-locked";
    #audioContext = null;
    #assetBuffers = null;
    #control = {
        buses: [], sources: [], retiringBuses: [], retiringSources: [], entries: [],
    };
    #busNodes = new Map();
    #persistentNodes = new Map();
    #persistentStarts = new Map();
    #recoveryOffsetsMillis = new Map();
    #completedPersistent = new Set();
    #nextPersistentStart = 1n;
    #oneShotNodes = new Map();
    #nextOneShot = 1n;
    #decodedAssets = new Map();
    #decodedAudioBytes = 0;
    #decodedUseClock = 1n;
    #audioAssets = new Map();
    #audioAssetRevisions = new Map();
    #audioAssetTombstones = new Set();
    #audioAssetBytes = 0;
    #unsubscribeInput = null;
    #activationAttempts = 0n;
    #deviceLosses = 0n;
    #deviceRecoveries = 0n;
    #oneShotsStarted = 0n;
    #oneShotsFinished = 0n;
    #isInteractive = () => true;
    #interactive = true;
    async init(host) {
        if (this.#host !== null)
            throw new Error("Voplay audio provider already initialized");
        if (this.#state === "closed") {
            this.#state = "created";
            this.#deviceState = "ready-locked";
        }
        if (!host.framework.providerRoles.includes("game-audio")) {
            throw new Error("Voplay audio provider requires the game-audio role");
        }
        const lanes = host.getCapability("framework_lane");
        if (lanes === null)
            throw new Error("Voplay audio provider requires framework_lane");
        const assetBuffers = host.getCapability("asset_buffer");
        const lane = await lanes.open("audio");
        this.#endpointGeneration = {
            index: lane.binding.caller.endpointIndex,
            generation: lane.binding.caller.endpointGeneration,
        };
        if (this.#endpointGeneration.index === 0xffffffff
            || this.#endpointGeneration.generation === 0) {
            throw new Error("Voplay audio lane has an invalid endpoint generation");
        }
        this.#host = host;
        this.#lane = lane;
        this.#assetBuffers = assetBuffers;
        const appSurface = host.getCapability("app_surface");
        this.#isInteractive = () => appSurface?.isInteractive() ?? true;
        this.#interactive = this.#isInteractive();
        this.#unsubscribeInput = appSurface?.subscribeInput((event) => {
            if ((this.#deviceState === "ready-locked" || this.#deviceState === "suspended")
                && event.synthesized !== true
                && (event.type === "pointerDown" || event.type === "keyDown")) {
                void this.#activateFromGesture();
            }
        }) ?? null;
        this.#polling = true;
        setTimeout(() => {
            if (this.#polling && this.#host === host && this.#lane === lane) {
                void this.#poll(host, lane);
            }
        }, 0);
        host.log(`Voplay browser audio provider ready-locked for ${host.framework.name}`);
    }
    stop() {
        this.#polling = false;
        this.#unsubscribeInput?.();
        this.#unsubscribeInput = null;
        this.#lane?.close();
        this.#lane = null;
        this.#host = null;
        this.#state = "closed";
        this.#engine = null;
        this.#lastSequence = 0n;
        this.#controlRevision = 0n;
        this.#nowMillis = 0n;
        this.#audioFrameOffset = 0n;
        this.#lastControl = null;
        this.#lastObservedProgress = "";
        this.#publishedObservations.clear();
        this.#realizationFailures.clear();
        this.#recoveryOffsetsMillis.clear();
        this.#completedPersistent.clear();
        this.#observedAudioEventSequence = 0n;
        this.#deviceState = "lost";
        if (this.#audioContext !== null) {
            this.#audioContext.onstatechange = null;
            void this.#audioContext.close();
        }
        this.#audioContext = null;
        this.#assetBuffers = null;
        this.#control = {
            buses: [], sources: [], retiringBuses: [], retiringSources: [], entries: [],
        };
        this.#disconnectGraph();
        this.#clearDecodedAssets();
        this.#audioAssets.clear();
        this.#audioAssetRevisions.clear();
        this.#audioAssetTombstones.clear();
        this.#audioAssetBytes = 0;
        this.#isInteractive = () => true;
        this.#interactive = true;
    }
    quiesceForCapture() {
        return { stopped: 1, state: this.#state };
    }
    ownerSnapshot() {
        return {
            state: this.#state,
            deviceState: this.#deviceState,
            controlRevision: this.#controlRevision,
            buses: this.#busNodes.size,
            persistentSources: this.#persistentNodes.size,
            completedPersistentSources: this.#completedPersistent.size,
            oneShots: this.#oneShotNodes.size,
            decodedAssets: this.#decodedAssets.size,
            decodedAudioBytes: this.#decodedAudioBytes,
            audioAssets: this.#audioAssets.size,
            audioAssetBytes: this.#audioAssetBytes,
            activationAttempts: this.#activationAttempts,
            deviceLosses: this.#deviceLosses,
            deviceRecoveries: this.#deviceRecoveries,
            oneShotsStarted: this.#oneShotsStarted,
            oneShotsFinished: this.#oneShotsFinished,
            observationHistory: this.#publishedObservations.size,
        };
    }
    async #poll(host, lane) {
        while (this.#polling && this.#host === host && this.#lane === lane) {
            try {
                this.#syncInteractiveOutput();
                const bytes = await lane.poll();
                if (!this.#polling || this.#host !== host || this.#lane !== lane)
                    return;
                if (bytes === null) {
                    await delay(8);
                    continue;
                }
                await this.#dispatch(decodeFrameworkPacket(bytes));
                await delay(0);
            }
            catch (error) {
                if (!this.#polling || this.#host !== host || this.#lane !== lane)
                    return;
                this.#polling = false;
                host.reportError(`Voplay audio provider failed: ${errorMessage(error)}`);
            }
        }
    }
    async #dispatch(packet) {
        const { header, payload } = packet;
        this.#validateEnvelope(packet);
        switch (header.kind) {
            case 12 /* MessageKind.EngineStart */:
                if (this.#state !== "created" || payload.byteLength !== 0) {
                    throw new Error("invalid Voplay audio EngineStart");
                }
                this.#engine = header.engine;
                this.#state = "running";
                await this.#reply(packet, 13 /* MessageKind.EngineReady */, new Uint8Array(), {
                    newRevision: 0n,
                });
                return;
            case 8 /* MessageKind.AudioControlTransaction */:
                this.#requireRunning(header.engine);
                if (header.newRevision < this.#controlRevision
                    || (header.newRevision > this.#controlRevision
                        && this.#controlRevision !== 0n
                        && header.baseRevision !== this.#controlRevision)) {
                    throw new Error("stale or non-contiguous Voplay audio control revision");
                }
                const nextControl = decodeAudioControl(packet);
                if (header.newRevision === this.#controlRevision
                    && (this.#lastControl === null
                        || header.commitId !== this.#lastControl.header.commitId
                        || !isBrowserAudioTombstoneUpdate(this.#control, nextControl))) {
                    throw new Error("invalid same-revision Voplay audio control update");
                }
                if (this.#audioContext !== null)
                    this.#capturePersistentRecoveryOffsets();
                const currentSources = new Map(this.#control.entries
                    .filter((entry) => entry.kind === 4)
                    .map((entry) => [handleKey(entry.handle), entry.descriptor]));
                const incomingSources = new Map(nextControl.entries
                    .filter((entry) => entry.kind === 4)
                    .map((entry) => [handleKey(entry.handle), entry.descriptor]));
                for (const key of this.#recoveryOffsetsMillis.keys()) {
                    const current = currentSources.get(key);
                    const incoming = incomingSources.get(key);
                    if (current === undefined
                        || incoming === undefined
                        || !sameBytes(current, incoming)) {
                        this.#recoveryOffsetsMillis.delete(key);
                    }
                }
                for (const key of this.#completedPersistent) {
                    const current = currentSources.get(key);
                    const incoming = incomingSources.get(key);
                    if (current === undefined
                        || incoming === undefined
                        || !sameBytes(current, incoming)) {
                        this.#completedPersistent.delete(key);
                    }
                }
                this.#control = nextControl;
                const retainedSourceKeys = new Set([
                    ...nextControl.sources.map((source) => handleKey(source.handle)),
                    ...nextControl.retiringSources.map((source) => handleKey(source.value.handle)),
                ]);
                for (const key of this.#recoveryOffsetsMillis.keys()) {
                    if (!retainedSourceKeys.has(key))
                        this.#recoveryOffsetsMillis.delete(key);
                }
                for (const key of this.#completedPersistent) {
                    if (!retainedSourceKeys.has(key))
                        this.#completedPersistent.delete(key);
                }
                this.#controlRevision = header.newRevision;
                this.#lastControl = packet;
                this.#lastObservedProgress = "";
                this.#releaseReadyRetirements();
                this.#realizationFailures = this.#audioContext === null
                    ? new Set()
                    : await this.#rebuildGraph(false, true);
                await this.#reply(packet, 11 /* MessageKind.AudioControlAck */, new Uint8Array(), {
                    requiredControlRevision: 0n,
                });
                await this.#publishRealizationResults(packet);
                await this.#publishControlObservation();
                return;
            case 36 /* MessageKind.AudioAssetData */:
                this.#requireRunning(header.engine);
                await this.#applyAudioAsset(packet);
                return;
            case 9 /* MessageKind.AudioEvent */:
                this.#requireRunning(header.engine);
                if (header.sequence === 0n
                    || header.commitId === 0n
                    || header.newRevision === 0n
                    || header.sourceSimulationRevision === 0n) {
                    throw new Error("invalid Voplay AudioEvent header");
                }
                const event = decodeOneShot(payload, header.engine);
                const deadlineBudgetMillis = header.newRevision > this.#nowMillis
                    ? header.newRevision - this.#nowMillis
                    : 0n;
                const deadlineStarted = performance.now();
                let outcome = AUDIO_FAILED_OUTCOME;
                if (header.newRevision <= this.#nowMillis) {
                    outcome = AUDIO_DEADLINE_EXCEEDED_OUTCOME;
                }
                else if (header.requiredControlRevision > this.#controlRevision) {
                    outcome = AUDIO_CONTROL_UNAVAILABLE_OUTCOME;
                }
                else if (!this.#interactive) {
                    outcome = AUDIO_DROPPED_OUTCOME;
                }
                else if (this.#deviceState === "ready-locked") {
                    outcome = AUDIO_LOCKED_OUTCOME;
                }
                else if (this.#deviceState === "suspended"
                    || this.#deviceState === "lost") {
                    outcome = AUDIO_DROPPED_OUTCOME;
                }
                else if (this.#deviceState === "active" && this.#audioContext !== null) {
                    if (!this.#control.buses.some((bus) => sameHandle(bus.handle, event.bus))) {
                        outcome = AUDIO_CONTROL_UNAVAILABLE_OUTCOME;
                    }
                    else {
                        try {
                            const played = await this.#playOneShot(event, deadlineBudgetMillis, deadlineStarted);
                            outcome = played === "played"
                                ? 1
                                : played === "deadline"
                                    ? AUDIO_DEADLINE_EXCEEDED_OUTCOME
                                    : played === "dropped"
                                        ? AUDIO_DROPPED_OUTCOME
                                        : AUDIO_FAILED_OUTCOME;
                        }
                        catch (error) {
                            outcome = AUDIO_FAILED_OUTCOME;
                            this.#host?.log(`Voplay one-shot audio event failed: ${errorMessage(error)}`);
                        }
                    }
                }
                await this.#reply(packet, 10 /* MessageKind.AudioEventResult */, Uint8Array.of(outcome), {
                    commitId: header.commitId,
                    baseRevision: 0n,
                    newRevision: this.#controlRevision,
                    requiredControlRevision: 0n,
                    sourceSimulationRevision: 0n,
                });
                this.#observedAudioEventSequence =
                    this.#observedAudioEventSequence > header.sequence
                        ? this.#observedAudioEventSequence
                        : header.sequence;
                if (this.#releaseReadyRetirements() && this.#audioContext !== null) {
                    this.#capturePersistentRecoveryOffsets();
                    this.#realizationFailures = await this.#rebuildGraph(false, true);
                    if (this.#lastControl !== null) {
                        await this.#publishRealizationResults(this.#lastControl);
                    }
                }
                await this.#publishControlObservation();
                return;
            case 48 /* MessageKind.ControlObservedAck */:
                this.#requireRunning(header.engine);
                this.#acceptControlObservedAck(packet);
                return;
            case 31 /* MessageKind.DeviceEvent */: {
                this.#requireRunning(header.engine);
                if (payload.byteLength < 1)
                    throw new Error("truncated Voplay audio DeviceEvent");
                const tag = payload[0];
                if (tag === 1 && (payload.byteLength === 17 || payload.byteLength === 75)) {
                    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
                    const endpoint = {
                        index: view.getUint32(1, true),
                        generation: view.getUint32(5, true),
                    };
                    const gesture = {
                        index: view.getUint32(9, true),
                        generation: view.getUint32(13, true),
                    };
                    if (!sameHandle(endpoint, this.#endpointGeneration)
                        || !validGenerationalHandle(gesture)) {
                        throw new Error("invalid Voplay browser audio gesture identity");
                    }
                    if (payload.byteLength === 75)
                        validateBrowserAudioPermit(payload.subarray(17));
                    await this.#unlock();
                }
                else if (tag === 2 && payload.byteLength === 1) {
                    if (this.#deviceState !== "lost")
                        this.#deviceLosses += 1n;
                    this.#deviceState = "lost";
                    this.#capturePersistentRecoveryOffsets();
                    this.#captureAudioFrameOffset();
                    this.#disconnectGraph();
                    this.#clearDecodedAssets();
                    if (this.#audioContext !== null) {
                        this.#audioContext.onstatechange = null;
                        await this.#audioContext.close();
                        this.#audioContext = null;
                    }
                }
                else if (tag === 3 && (payload.byteLength === 1 || payload.byteLength === 59)) {
                    if (payload.byteLength === 59)
                        validateBrowserAudioPermit(payload.subarray(1));
                    this.#deviceState = "ready-locked";
                }
                else if (tag === 4 && payload.byteLength === 9) {
                    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
                    const nowMillis = view.getBigUint64(1, true);
                    if (nowMillis < this.#nowMillis) {
                        throw new Error("Voplay audio clock regressed");
                    }
                    this.#nowMillis = nowMillis;
                }
                else {
                    throw new Error("unsupported Voplay audio DeviceEvent");
                }
                if (this.#releaseReadyRetirements() && this.#audioContext !== null) {
                    this.#capturePersistentRecoveryOffsets();
                    this.#realizationFailures = await this.#rebuildGraph(false, true);
                    if (this.#lastControl !== null) {
                        await this.#publishRealizationResults(this.#lastControl);
                    }
                }
                await this.#publishControlObservation();
                return;
            }
            case 14 /* MessageKind.EngineSuspend */:
                this.#requireRunning(header.engine);
                if (payload.byteLength !== 0)
                    throw new Error("audio lifecycle payload must be empty");
                if (this.#deviceState === "active" && this.#audioContext !== null) {
                    await this.#audioContext.suspend();
                    this.#deviceState = "suspended";
                }
                return;
            case 15 /* MessageKind.EngineResume */:
                this.#requireRunning(header.engine);
                if (payload.byteLength !== 0)
                    throw new Error("audio lifecycle payload must be empty");
                if (this.#deviceState === "suspended" && this.#audioContext !== null) {
                    const context = this.#audioContext;
                    try {
                        await context.resume();
                        if (this.#audioContext === context) {
                            this.#deviceState = context.state === "running" ? "active" : "suspended";
                        }
                    }
                    catch (error) {
                        this.#host?.log(`Voplay audio lifecycle resume deferred: ${errorMessage(error)}`);
                    }
                }
                if (this.#deviceState === "active")
                    await this.#publishControlObservation();
                return;
            case 32 /* MessageKind.WorkerWake */:
                this.#requireRunning(header.engine);
                if (payload.byteLength !== 0)
                    throw new Error("audio lifecycle payload must be empty");
                if (this.#releaseReadyRetirements() && this.#audioContext !== null) {
                    this.#capturePersistentRecoveryOffsets();
                    this.#realizationFailures = await this.#rebuildGraph(false, true);
                    if (this.#lastControl !== null) {
                        await this.#publishRealizationResults(this.#lastControl);
                    }
                }
                await this.#publishControlObservation();
                return;
            case 16 /* MessageKind.EngineClose */:
                this.#requireRunning(header.engine);
                if (payload.byteLength !== 0)
                    throw new Error("audio close payload must be empty");
                this.#state = "closed";
                this.#deviceState = "lost";
                if (this.#audioContext !== null) {
                    this.#audioContext.onstatechange = null;
                    await this.#audioContext.close();
                    this.#audioContext = null;
                }
                this.#disconnectGraph();
                this.#control = {
                    buses: [], sources: [], retiringBuses: [], retiringSources: [], entries: [],
                };
                this.#controlRevision = 0n;
                this.#lastControl = null;
                this.#lastObservedProgress = "";
                this.#publishedObservations.clear();
                this.#realizationFailures.clear();
                this.#recoveryOffsetsMillis.clear();
                this.#completedPersistent.clear();
                this.#clearDecodedAssets();
                this.#audioAssets.clear();
                this.#audioAssetRevisions.clear();
                this.#audioAssetTombstones.clear();
                this.#audioAssetBytes = 0;
                await this.#reply(packet, 17 /* MessageKind.EngineClosed */, new Uint8Array());
                this.#engine = null;
                return;
            default:
                throw new Error(`unsupported Voplay audio packet ${header.kind}`);
        }
    }
    async #unlock() {
        if (this.#deviceState !== "ready-locked" || this.#host === null)
            return;
        this.#activationAttempts += 1n;
        let context = null;
        try {
            const AudioContextConstructor = window.AudioContext;
            context = new AudioContextConstructor({ latencyHint: "interactive" });
            await context.resume();
            if (this.#host === null || this.#deviceState !== "ready-locked") {
                await context.close();
                return;
            }
            this.#audioContext = context;
            this.#observeContext(context);
            this.#deviceState = context.state === "running" ? "active" : "suspended";
            if (this.#deviceState === "active") {
                const recovering = this.#deviceRecoveries < this.#deviceLosses;
                this.#realizationFailures = await this.#rebuildGraph(recovering);
                if (recovering)
                    this.#deviceRecoveries += 1n;
                if (this.#lastControl !== null) {
                    await this.#publishRealizationResults(this.#lastControl);
                }
                await this.#publishControlObservation();
            }
            this.#host.log(`Voplay browser audio device ${this.#deviceState}`);
        }
        catch (error) {
            if (context !== null)
                await this.#discardContext(context, false);
            this.#host?.reportError(`Voplay audio unlock denied: ${errorMessage(error)}`);
        }
    }
    async #activateFromGesture() {
        if (this.#deviceState === "ready-locked") {
            await this.#unlock();
            return;
        }
        const context = this.#audioContext;
        if (this.#deviceState !== "suspended" || context === null)
            return;
        try {
            await context.resume();
            if (this.#audioContext !== context)
                return;
            this.#deviceState = context.state === "running" ? "active" : "suspended";
            if (this.#deviceState === "active" && this.#busNodes.size === 0) {
                this.#realizationFailures = await this.#rebuildGraph();
                if (this.#lastControl !== null) {
                    await this.#publishRealizationResults(this.#lastControl);
                }
            }
            if (this.#deviceState === "active")
                await this.#publishControlObservation();
        }
        catch (error) {
            await this.#discardContext(context, true);
            this.#host?.reportError(`Voplay audio resume denied: ${errorMessage(error)}`);
        }
    }
    async #discardContext(context, preserveTimeline) {
        context.onstatechange = null;
        if (this.#audioContext === context) {
            if (preserveTimeline) {
                this.#capturePersistentRecoveryOffsets();
                this.#captureAudioFrameOffset();
                if (this.#state === "running")
                    this.#deviceLosses += 1n;
            }
            this.#disconnectGraph();
            this.#clearDecodedAssets();
            this.#audioContext = null;
            this.#deviceState = this.#state === "running" ? "ready-locked" : "lost";
        }
        try {
            await context.close();
        }
        catch {
            // The device is already unusable; local ownership was cleared above.
        }
    }
    #observeContext(context) {
        context.onstatechange = () => {
            if (this.#audioContext !== context)
                return;
            const state = String(context.state);
            if (state === "running") {
                this.#deviceState = "active";
                return;
            }
            if (state === "closed") {
                this.#capturePersistentRecoveryOffsets();
                this.#captureAudioFrameOffset();
                context.onstatechange = null;
                this.#audioContext = null;
                this.#deviceState = this.#state === "running" ? "ready-locked" : "lost";
                if (this.#state === "running")
                    this.#deviceLosses += 1n;
                this.#disconnectGraph();
                this.#clearDecodedAssets();
                if (this.#state === "running") {
                    this.#host?.log("Voplay browser audio device lost; awaiting gesture recovery");
                }
                return;
            }
            this.#deviceState = "suspended";
        };
    }
    async #rebuildGraph(recovering = false, preserveTimeline = false) {
        const context = this.#audioContext;
        const failures = new Set();
        if (context === null)
            return failures;
        this.#disconnectGraph();
        const buses = [
            ...this.#control.buses,
            ...this.#control.retiringBuses.map((retiring) => retiring.value),
        ];
        const sources = [
            ...this.#control.sources,
            ...this.#control.retiringSources.map((retiring) => retiring.value),
        ];
        for (const bus of buses) {
            const node = context.createGain();
            node.gain.value = bus.mute || (!this.#interactive && bus.parent === null)
                ? 0
                : bus.gain;
            this.#busNodes.set(handleKey(bus.handle), node);
        }
        for (const bus of buses) {
            const node = this.#busNodes.get(handleKey(bus.handle));
            if (node === undefined)
                continue;
            if (bus.parent === null) {
                node.connect(context.destination);
            }
            else {
                const parent = this.#busNodes.get(handleKey(bus.parent));
                if (parent === undefined)
                    throw new Error("Voplay audio bus parent is unavailable");
                node.connect(parent);
            }
        }
        const listener = this.#control.buses.find((bus) => bus.listener !== null)?.listener;
        if (listener !== undefined && listener !== null)
            this.#applyListener(listener);
        this.#syncBusMixState();
        for (const source of sources) {
            try {
                await this.#startPersistent(source, recovering, preserveTimeline);
            }
            catch (error) {
                failures.add(handleKey(source.handle));
                this.#host?.log(`Voplay persistent audio source ${handleKey(source.handle)} awaits realization retry: ${errorMessage(error)}`);
            }
        }
        const physicalSources = new Set(sources.map((source) => handleKey(source.handle)));
        for (const key of this.#recoveryOffsetsMillis.keys()) {
            if (!physicalSources.has(key))
                this.#recoveryOffsetsMillis.delete(key);
        }
        return failures;
    }
    async #startPersistent(source, recovering = false, preserveTimeline = false) {
        const key = handleKey(source.handle);
        if (this.#completedPersistent.has(key))
            return;
        if (recovering && source.recovery === 3) {
            this.#recoveryOffsetsMillis.delete(key);
            this.#completedPersistent.add(key);
            return;
        }
        const context = this.#audioContext;
        const bus = this.#busNodes.get(handleKey(source.bus));
        if (context === null || bus === undefined)
            return;
        if (!this.#persistentNodes.has(key)
            && this.#persistentNodes.size + this.#oneShotNodes.size >= MAX_AUDIO_VOICES) {
            throw new Error("Voplay browser audio voice capacity exceeded");
        }
        if (this.#nextPersistentStart > 0xffffffffffffffffn) {
            throw new Error("Voplay persistent audio start identity exhausted");
        }
        const start = this.#nextPersistentStart++;
        this.#persistentStarts.set(key, start);
        const buffer = await this.#decodeAsset(source.asset);
        this.#syncInteractiveOutput();
        if (this.#audioContext !== context
            || ![
                ...this.#control.sources,
                ...this.#control.retiringSources.map((retiring) => retiring.value),
            ].includes(source)
            || this.#persistentStarts.get(key) !== start) {
            return;
        }
        if (!this.#persistentNodes.has(key)
            && this.#persistentNodes.size + this.#oneShotNodes.size >= MAX_AUDIO_VOICES) {
            throw new Error("Voplay browser audio voice capacity exceeded");
        }
        const node = context.createBufferSource();
        const gain = context.createGain();
        node.buffer = buffer;
        node.loop = source.loop;
        gain.gain.value = source.gain;
        node.connect(gain);
        const nodes = [gain, ...this.#connectSpatial(gain, bus, source.spatial)];
        const offsetMillis = recovering
            ? source.recovery === 2
                ? 0n
                : this.#recoveryOffsetsMillis.get(key) ?? source.transportAnchorMillis
            : preserveTimeline
                ? this.#recoveryOffsetsMillis.get(key) ?? source.transportAnchorMillis
                : source.transportAnchorMillis;
        const requestedOffset = Number(offsetMillis) / 1000;
        const offset = source.loop && buffer.duration > 0
            ? requestedOffset % buffer.duration
            : Math.min(requestedOffset, buffer.duration);
        const previous = this.#persistentNodes.get(key);
        node.onended = () => {
            if (this.#persistentNodes.get(key)?.source === node) {
                this.#persistentNodes.delete(key);
                this.#persistentStarts.delete(key);
                if (!source.loop)
                    this.#completedPersistent.add(key);
                this.#syncBusMixState();
            }
            node.disconnect();
            for (const owned of nodes)
                owned.disconnect();
        };
        try {
            node.start(0, offset);
        }
        catch (error) {
            node.onended = null;
            node.disconnect();
            for (const owned of nodes)
                owned.disconnect();
            throw error;
        }
        if (previous !== undefined) {
            previous.source.onended = null;
            try {
                previous.source.stop();
            }
            catch { }
            previous.source.disconnect();
            for (const owned of previous.nodes)
                owned.disconnect();
        }
        this.#completedPersistent.delete(key);
        this.#persistentNodes.set(key, {
            source: node,
            nodes,
            bus: source.bus,
            asset: source.asset,
            gainNode: gain,
            baseGain: source.gain,
            startedAtSeconds: context.currentTime,
            offsetSeconds: offset,
            durationSeconds: buffer.duration,
            loop: source.loop,
        });
        this.#recoveryOffsetsMillis.delete(key);
        this.#syncBusMixState();
    }
    async #playOneShot(event, deadlineBudgetMillis, deadlineStarted) {
        if (!this.#control.buses.some((bus) => sameHandle(bus.handle, event.bus))) {
            return "failed";
        }
        if (this.#persistentNodes.size + this.#oneShotNodes.size >= MAX_AUDIO_VOICES) {
            return "failed";
        }
        const context = this.#audioContext;
        const bus = this.#busNodes.get(handleKey(event.bus));
        if (context === null || bus === undefined)
            return "failed";
        const buffer = await this.#decodeAsset(event.asset);
        this.#syncInteractiveOutput();
        const elapsedMillis = BigInt(Math.max(0, Math.floor(performance.now() - deadlineStarted)));
        if (deadlineBudgetMillis === 0n || elapsedMillis >= deadlineBudgetMillis) {
            return "deadline";
        }
        if (!this.#interactive)
            return "dropped";
        if (this.#audioContext !== context || this.#deviceState !== "active")
            return "dropped";
        if (this.#persistentNodes.size + this.#oneShotNodes.size >= MAX_AUDIO_VOICES) {
            return "failed";
        }
        const node = context.createBufferSource();
        const gain = context.createGain();
        node.buffer = buffer;
        gain.gain.value = event.gain;
        node.connect(gain);
        const nodes = [gain, ...this.#connectSpatial(gain, bus, event.spatial)];
        if (this.#nextOneShot > 0xffffffffffffffffn) {
            throw new Error("Voplay one-shot audio identity exhausted");
        }
        const id = this.#nextOneShot;
        this.#nextOneShot += 1n;
        node.onended = () => {
            if (this.#oneShotNodes.delete(id))
                this.#oneShotsFinished += 1n;
            this.#syncBusMixState();
            node.disconnect();
            for (const owned of nodes)
                owned.disconnect();
        };
        this.#oneShotNodes.set(id, {
            source: node,
            nodes,
            bus: event.bus,
            asset: event.asset,
            gainNode: gain,
            baseGain: event.gain,
        });
        try {
            node.start();
        }
        catch {
            node.onended = null;
            this.#oneShotNodes.delete(id);
            node.disconnect();
            for (const owned of nodes)
                owned.disconnect();
            this.#syncBusMixState();
            return "failed";
        }
        this.#oneShotsStarted += 1n;
        this.#syncBusMixState();
        return "played";
    }
    #connectSpatial(source, bus, spatial) {
        const context = this.#audioContext;
        if (context === null || spatial === null) {
            source.connect(bus);
            return [];
        }
        const panner = context.createPanner();
        panner.panningModel = "HRTF";
        panner.distanceModel = "inverse";
        panner.refDistance = spatial.minDistance / 1000;
        panner.maxDistance = spatial.maxDistance / 1000;
        const position = spatial.position.map((component) => component / 1000);
        const compatiblePanner = panner;
        if (compatiblePanner.positionX !== undefined
            && compatiblePanner.positionY !== undefined
            && compatiblePanner.positionZ !== undefined) {
            compatiblePanner.positionX.value = position[0];
            compatiblePanner.positionY.value = position[1];
            compatiblePanner.positionZ.value = position[2];
        }
        else if (compatiblePanner.setPosition !== undefined) {
            compatiblePanner.setPosition(...position);
        }
        else {
            throw new Error("browser spatial audio position API unavailable");
        }
        source.connect(panner);
        panner.connect(bus);
        return [panner];
    }
    #decodeAsset(asset) {
        const key = handleKey(asset);
        const cached = this.#decodedAssets.get(key);
        if (cached !== undefined) {
            cached.lastUse = this.#takeDecodedUse();
            return cached.promise;
        }
        const context = this.#audioContext;
        if (context === null)
            return Promise.reject(new Error("Voplay audio device is unavailable"));
        if (this.#audioAssetTombstones.has(key)) {
            return Promise.reject(new Error("Voplay audio asset was removed"));
        }
        const resident = this.#audioAssets.get(key);
        const buffers = this.#assetBuffers;
        if (resident === undefined && buffers === null) {
            return Promise.reject(new Error("Voplay audio asset is not resident"));
        }
        const raw = resident === undefined
            ? buffers.read(asset).then((bytes) => context.decodeAudioData(bytes.slice(0)))
            : context.decodeAudioData(resident.bytes.slice().buffer);
        const record = {
            promise: raw,
            bytes: 0,
            lastUse: this.#takeDecodedUse(),
        };
        const pending = raw.then((buffer) => {
            const bytes = buffer.length * buffer.numberOfChannels * 4;
            if (!Number.isSafeInteger(bytes)
                || bytes <= 0
                || bytes > MAX_DECODED_AUDIO_BYTES) {
                throw new Error("Voplay decoded audio asset capacity exceeded");
            }
            if (this.#decodedAssets.get(key) === record) {
                record.bytes = bytes;
                this.#decodedAudioBytes += bytes;
                this.#evictDecodedAssets(key);
            }
            return buffer;
        });
        record.promise = pending;
        this.#decodedAssets.set(key, record);
        void pending.catch(() => {
            if (this.#decodedAssets.get(key) === record)
                this.#dropDecodedAsset(key);
        });
        return pending;
    }
    #takeDecodedUse() {
        const use = this.#decodedUseClock;
        if (use >= 0xffffffffffffffffn) {
            const ordered = [...this.#decodedAssets.entries()].sort((left, right) => left[1].lastUse < right[1].lastUse ? -1 : 1);
            let next = 1n;
            for (const [, record] of ordered)
                record.lastUse = next++;
            this.#decodedUseClock = next + 1n;
            return next;
        }
        this.#decodedUseClock = use + 1n;
        return use;
    }
    #evictDecodedAssets(protectedKey) {
        while (this.#decodedAudioBytes > MAX_DECODED_AUDIO_BYTES) {
            let candidate = null;
            for (const entry of this.#decodedAssets) {
                if (entry[0] === protectedKey || entry[1].bytes === 0)
                    continue;
                if (candidate === null || entry[1].lastUse < candidate[1].lastUse) {
                    candidate = entry;
                }
            }
            if (candidate === null) {
                throw new Error("Voplay decoded audio residency cannot satisfy its byte budget");
            }
            this.#dropDecodedAsset(candidate[0]);
        }
    }
    #dropDecodedAsset(key) {
        const record = this.#decodedAssets.get(key);
        if (record === undefined)
            return;
        this.#decodedAudioBytes -= record.bytes;
        this.#decodedAssets.delete(key);
    }
    #clearDecodedAssets() {
        this.#decodedAssets.clear();
        this.#decodedAudioBytes = 0;
        this.#decodedUseClock = 1n;
    }
    async #applyAudioAsset(packet) {
        const { header, payload } = packet;
        if (payload.byteLength < 25
            || payload[0] !== 0x56
            || payload[1] !== 0x50
            || payload[2] !== 0x41
            || payload[3] !== 0x32) {
            throw new Error("invalid Voplay browser audio asset");
        }
        const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
        const action = payload[4];
        const asset = {
            index: view.getUint32(5, true),
            generation: view.getUint32(9, true),
        };
        const revision = view.getBigUint64(13, true);
        const byteLength = view.getUint32(21, true);
        const identity = BigInt(asset.index) | (BigInt(asset.generation) << 32n);
        const key = handleKey(asset);
        const current = this.#audioAssets.get(key);
        const currentRevision = this.#audioAssetRevisions.get(key);
        if (asset.index === 0xffff_ffff
            || asset.generation === 0
            || revision === 0n
            || header.commitId !== identity
            || header.newRevision !== revision
            || (currentRevision !== undefined && currentRevision >= revision)
            || payload.byteLength !== 25 + byteLength) {
            throw new Error("stale Voplay browser audio asset");
        }
        if (action === 1 && byteLength > 0) {
            const nextBytes = this.#audioAssetBytes
                - (current?.bytes.byteLength ?? 0)
                + byteLength;
            if ((current === undefined && this.#audioAssets.size >= MAX_AUDIO_ASSETS)
                || nextBytes > MAX_AUDIO_ASSET_BYTES) {
                throw new Error("Voplay browser audio asset capacity exceeded");
            }
            const bytes = payload.slice(25);
            this.#audioAssets.set(key, { revision, bytes });
            this.#audioAssetTombstones.delete(key);
            this.#audioAssetBytes = nextBytes;
            this.#dropDecodedAsset(key);
            this.#stopOneShotsForAsset(asset);
            const physicalSources = [
                ...this.#control.sources,
                ...this.#control.retiringSources.map((retiring) => retiring.value),
            ];
            for (const source of physicalSources) {
                if (handleKey(source.asset) === key) {
                    this.#completedPersistent.delete(handleKey(source.handle));
                }
            }
            if (this.#deviceState === "active") {
                this.#capturePersistentRecoveryOffsets();
                for (const source of physicalSources) {
                    if (handleKey(source.asset) !== key)
                        continue;
                    const sourceKey = handleKey(source.handle);
                    try {
                        await this.#startPersistent(source, false, this.#recoveryOffsetsMillis.has(sourceKey));
                        this.#realizationFailures.delete(sourceKey);
                    }
                    catch {
                        this.#realizationFailures.add(sourceKey);
                    }
                }
            }
        }
        else if (action === 2 && byteLength === 0) {
            if (current !== undefined) {
                this.#audioAssets.delete(key);
                this.#audioAssetBytes -= current.bytes.byteLength;
            }
            this.#audioAssetTombstones.add(key);
            this.#dropDecodedAsset(key);
            this.#stopOneShotsForAsset(asset);
            const physicalSources = [
                ...this.#control.sources,
                ...this.#control.retiringSources.map((retiring) => retiring.value),
            ];
            for (const source of physicalSources) {
                if (handleKey(source.asset) !== key)
                    continue;
                const sourceKey = handleKey(source.handle);
                this.#realizationFailures.add(sourceKey);
                this.#persistentStarts.delete(sourceKey);
                const record = this.#persistentNodes.get(sourceKey);
                if (record === undefined)
                    continue;
                record.source.onended = null;
                try {
                    record.source.stop();
                }
                catch { }
                record.source.disconnect();
                for (const node of record.nodes)
                    node.disconnect();
                this.#persistentNodes.delete(sourceKey);
            }
            this.#syncBusMixState();
        }
        else {
            throw new Error("unsupported Voplay browser audio asset action");
        }
        this.#audioAssetRevisions.set(key, revision);
        await this.#reply(packet, 39 /* MessageKind.AudioAssetAck */, new Uint8Array(), {
            commitId: identity,
            baseRevision: 0n,
            newRevision: revision,
            requiredControlRevision: 0n,
        });
        if (this.#deviceState === "active" && this.#lastControl !== null) {
            await this.#publishRealizationResults(this.#lastControl);
        }
    }
    #disconnectGraph() {
        for (const record of this.#persistentNodes.values()) {
            record.source.onended = null;
            try {
                record.source.stop();
            }
            catch { }
            record.source.disconnect();
            for (const node of record.nodes)
                node.disconnect();
        }
        this.#persistentNodes.clear();
        this.#persistentStarts.clear();
        for (const record of this.#oneShotNodes.values()) {
            record.source.onended = null;
            try {
                record.source.stop();
            }
            catch { }
            record.source.disconnect();
            for (const node of record.nodes)
                node.disconnect();
        }
        this.#oneShotNodes.clear();
        for (const node of this.#busNodes.values())
            node.disconnect();
        this.#busNodes.clear();
    }
    #stopOneShotsForAsset(asset) {
        for (const [id, record] of this.#oneShotNodes) {
            if (!sameHandle(record.asset, asset))
                continue;
            record.source.onended = null;
            try {
                record.source.stop();
            }
            catch { }
            record.source.disconnect();
            for (const node of record.nodes)
                node.disconnect();
            this.#oneShotNodes.delete(id);
            this.#oneShotsFinished += 1n;
        }
        this.#syncBusMixState();
    }
    #syncInteractiveOutput() {
        const interactive = this.#isInteractive();
        if (interactive !== this.#interactive)
            this.#interactive = interactive;
        this.#syncBusMixState();
    }
    #syncBusMixState() {
        const buses = [
            ...this.#control.buses,
            ...this.#control.retiringBuses.map((retiring) => retiring.value),
        ];
        const byKey = new Map(buses.map((bus) => [handleKey(bus.handle), bus]));
        const isDescendantOrSame = (candidate, ancestor) => {
            let cursor = byKey.get(handleKey(candidate));
            while (cursor !== undefined) {
                if (sameHandle(cursor.handle, ancestor))
                    return true;
                cursor = cursor.parent === null
                    ? undefined
                    : byKey.get(handleKey(cursor.parent));
            }
            return false;
        };
        const soloBuses = buses.filter((bus) => bus.solo);
        const soloActive = soloBuses.length > 0;
        const voices = [
            ...this.#persistentNodes.values(),
            ...this.#oneShotNodes.values(),
        ];
        const voiceBuses = voices.map((voice) => voice.bus);
        const audible = (bus) => !soloActive || soloBuses.some((solo) => isDescendantOrSame(bus.handle, solo.handle));
        const triggerActive = (trigger) => voiceBuses.some((bus) => {
            const descriptor = byKey.get(handleKey(bus));
            return descriptor !== undefined
                && audible(descriptor)
                && isDescendantOrSame(bus, trigger);
        });
        const ducking = buses.flatMap((bus) => bus.ducking);
        for (const bus of buses) {
            const node = this.#busNodes.get(handleKey(bus.handle));
            if (node === undefined)
                continue;
            const gain = bus.mute || (!this.#interactive && bus.parent === null)
                ? 0
                : bus.gain;
            node.gain.value = gain;
        }
        for (const voice of voices) {
            const bus = byKey.get(handleKey(voice.bus));
            let gain = bus === undefined || !audible(bus) ? 0 : voice.baseGain;
            for (const rule of ducking) {
                if (isDescendantOrSame(voice.bus, rule.target)
                    && triggerActive(rule.trigger)) {
                    gain *= rule.gain;
                }
            }
            voice.gainNode.gain.value = gain;
        }
    }
    #applyListener(listener) {
        const context = this.#audioContext;
        if (context === null)
            return;
        const position = listener.position.map((component) => component / 1000);
        const rightLength = Math.hypot(...listener.right);
        const right = listener.right.map((component) => component / rightLength);
        const referenceUp = Math.abs(right[1]) < 0.99 ? [0, 1, 0] : [0, 0, 1];
        const rawForward = [
            referenceUp[1] * right[2] - referenceUp[2] * right[1],
            referenceUp[2] * right[0] - referenceUp[0] * right[2],
            referenceUp[0] * right[1] - referenceUp[1] * right[0],
        ];
        const forwardLength = Math.hypot(...rawForward);
        const forward = rawForward.map((component) => component / forwardLength);
        const up = [
            right[1] * forward[2] - right[2] * forward[1],
            right[2] * forward[0] - right[0] * forward[2],
            right[0] * forward[1] - right[1] * forward[0],
        ];
        const listenerNode = context.listener;
        if (listenerNode.positionX !== undefined
            && listenerNode.positionY !== undefined
            && listenerNode.positionZ !== undefined) {
            listenerNode.positionX.value = position[0];
            listenerNode.positionY.value = position[1];
            listenerNode.positionZ.value = position[2];
        }
        else if (listenerNode.setPosition !== undefined) {
            listenerNode.setPosition(...position);
        }
        else {
            throw new Error("browser audio listener position API unavailable");
        }
        if (listenerNode.forwardX !== undefined
            && listenerNode.forwardY !== undefined
            && listenerNode.forwardZ !== undefined
            && listenerNode.upX !== undefined
            && listenerNode.upY !== undefined
            && listenerNode.upZ !== undefined) {
            listenerNode.forwardX.value = forward[0];
            listenerNode.forwardY.value = forward[1];
            listenerNode.forwardZ.value = forward[2];
            listenerNode.upX.value = up[0];
            listenerNode.upY.value = up[1];
            listenerNode.upZ.value = up[2];
        }
        else if (listenerNode.setOrientation !== undefined) {
            listenerNode.setOrientation(...forward, ...up);
        }
        else {
            throw new Error("browser audio listener orientation API unavailable");
        }
    }
    #releaseReadyRetirements() {
        const domainFrame = this.#audioDomainFrame();
        const ready = (fence) => fence.renderRevision === 0n
            && fence.eventSequence === 0n
            && domainFrame >= fence.domainFrame
            && this.#observedAudioEventSequence >= fence.audioEventSequence;
        const retiringBuses = this.#control.retiringBuses.filter((retiring) => !ready(retiring.fence));
        const retiringSources = this.#control.retiringSources.filter((retiring) => !ready(retiring.fence));
        if (retiringBuses.length === this.#control.retiringBuses.length
            && retiringSources.length === this.#control.retiringSources.length) {
            return false;
        }
        const retainedSources = new Set(retiringSources.map((retiring) => handleKey(retiring.value.handle)));
        for (const retiring of this.#control.retiringSources) {
            const key = handleKey(retiring.value.handle);
            if (!retainedSources.has(key)) {
                this.#recoveryOffsetsMillis.delete(key);
                this.#completedPersistent.delete(key);
            }
        }
        this.#control = {
            ...this.#control,
            retiringBuses,
            retiringSources,
        };
        return true;
    }
    #validateEnvelope(packet) {
        const lane = this.#lane;
        if (lane === null)
            throw new Error("Voplay audio lane is closed");
        if (packet.header.channelEpoch !== BigInt(lane.binding.channelEpoch)) {
            throw new Error("Voplay audio packet channel epoch mismatch");
        }
        const lifecyclePacket = packet.header.kind === 12 /* MessageKind.EngineStart */
            || packet.header.kind === 14 /* MessageKind.EngineSuspend */
            || packet.header.kind === 15 /* MessageKind.EngineResume */
            || packet.header.kind === 16 /* MessageKind.EngineClose */
            || packet.header.kind === 32 /* MessageKind.WorkerWake */;
        const sameRevisionControl = packet.header.kind === 8 /* MessageKind.AudioControlTransaction */
            && this.#controlRevision !== 0n
            && packet.header.newRevision === this.#controlRevision;
        if (packet.header.kind !== 36 /* MessageKind.AudioAssetData */
            && packet.header.kind !== 48 /* MessageKind.ControlObservedAck */
            && !lifecyclePacket
            && !sameRevisionControl
            && packet.header.sequence <= this.#lastSequence) {
            throw new Error("Voplay audio packet sequence regression");
        }
        if (packet.header.kind !== 36 /* MessageKind.AudioAssetData */
            && packet.header.kind !== 48 /* MessageKind.ControlObservedAck */
            && !lifecyclePacket
            && !sameRevisionControl) {
            this.#lastSequence = packet.header.sequence;
        }
    }
    async #publishRealizationResults(source) {
        const controls = [
            ...this.#control.buses.map((bus) => ({ kind: 3, handle: bus.handle })),
            ...this.#control.sources.map((audioSource) => ({ kind: 4, handle: audioSource.handle })),
        ];
        const payload = new Uint8Array(32 + controls.length * 12);
        const view = new DataView(payload.buffer);
        payload.set([0x56, 0x43, 0x52, 0x52], 0);
        view.setUint16(4, 1, true);
        payload[6] = 2;
        view.setUint32(8, this.#endpointGeneration.index, true);
        view.setUint32(12, this.#endpointGeneration.generation, true);
        view.setBigUint64(16, source.header.newRevision, true);
        view.setUint32(24, controls.length, true);
        controls.forEach((control, index) => {
            const offset = 32 + index * 12;
            payload[offset] = control.kind;
            payload[offset + 1] = control.kind === 4
                && this.#realizationFailures.has(handleKey(control.handle))
                ? 2
                : 1;
            view.setUint32(offset + 4, control.handle.index, true);
            view.setUint32(offset + 8, control.handle.generation, true);
        });
        await this.#reply(source, 46 /* MessageKind.ControlRealizationResult */, payload, {
            requiredControlRevision: source.header.newRevision,
        });
    }
    async #publishControlObservation() {
        const source = this.#lastControl;
        if (source === null)
            return;
        const domainFrame = this.#audioDomainFrame();
        const key = `${source.header.commitId}:${source.header.newRevision}:${domainFrame}:${this.#observedAudioEventSequence}`;
        if (key === this.#lastObservedProgress)
            return;
        const payload = new Uint8Array(56);
        const view = new DataView(payload.buffer);
        payload.set([0x56, 0x43, 0x4f, 0x31], 0);
        view.setUint16(4, 2, true);
        payload[6] = 2;
        view.setBigUint64(8, source.header.commitId, true);
        view.setBigUint64(16, source.header.newRevision, true);
        view.setBigUint64(24, 0n, true);
        view.setBigUint64(32, domainFrame, true);
        view.setBigUint64(40, 0n, true);
        view.setBigUint64(48, this.#observedAudioEventSequence, true);
        await this.#reply(source, 47 /* MessageKind.ControlObserved */, payload, {
            requiredControlRevision: source.header.newRevision,
        });
        const observation = `${source.header.commitId}:${source.header.newRevision}`;
        this.#publishedObservations.delete(observation);
        this.#publishedObservations.add(observation);
        while (this.#publishedObservations.size > MAX_AUDIO_OBSERVATION_HISTORY) {
            const oldest = this.#publishedObservations.values().next().value;
            if (oldest === undefined)
                break;
            this.#publishedObservations.delete(oldest);
        }
        this.#lastObservedProgress = key;
    }
    #acceptControlObservedAck(packet) {
        const source = this.#lastControl;
        const payload = packet.payload;
        if (source === null || payload.byteLength !== 56) {
            throw new Error("unexpected Voplay audio control observation ACK");
        }
        const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
        const transaction = view.getBigUint64(8, true);
        const revision = view.getBigUint64(16, true);
        if (payload[0] !== 0x56 || payload[1] !== 0x43
            || payload[2] !== 0x4f || payload[3] !== 0x31
            || view.getUint16(4, true) !== 2
            || payload[6] !== 2 || payload[7] !== 0
            || transaction === 0n || revision === 0n
            || packet.header.commitId !== transaction
            || packet.header.newRevision !== revision
            || packet.header.requiredControlRevision !== revision
            || !this.#publishedObservations.has(`${transaction}:${revision}`)
            || revision > source.header.newRevision
            || (revision === source.header.newRevision
                && transaction !== source.header.commitId)) {
            throw new Error("invalid Voplay audio control observation ACK");
        }
    }
    #audioDomainFrame() {
        const context = this.#audioContext;
        const frame = this.#audioFrameOffset + (context === null ? 0n : audioContextFrame(context));
        if (frame > MAX_U64)
            throw new Error("Voplay audio domain frame exhausted");
        return frame;
    }
    #captureAudioFrameOffset() {
        const context = this.#audioContext;
        if (context === null)
            return;
        const frame = this.#audioFrameOffset + audioContextFrame(context);
        if (frame > MAX_U64)
            throw new Error("Voplay audio domain frame exhausted");
        this.#audioFrameOffset = frame;
    }
    #capturePersistentRecoveryOffsets() {
        const context = this.#audioContext;
        if (context === null)
            return;
        for (const [key, record] of this.#persistentNodes) {
            const elapsed = Math.max(0, context.currentTime - record.startedAtSeconds);
            const unbounded = record.offsetSeconds + elapsed;
            const position = record.loop && record.durationSeconds > 0
                ? unbounded % record.durationSeconds
                : Math.min(unbounded, record.durationSeconds);
            this.#recoveryOffsetsMillis.set(key, BigInt(Math.max(0, Math.floor(position * 1000))));
        }
    }
    #requireRunning(engine) {
        if (this.#state !== "running" || this.#engine === null || !sameHandle(this.#engine, engine)) {
            throw new Error("Voplay audio provider is not running for this engine");
        }
    }
    async #reply(source, kind, payload, overrides = {}) {
        const lane = this.#lane;
        if (lane === null)
            throw new Error("Voplay audio lane is closed");
        const header = source.header;
        await lane.submit(encodeFrameworkPacket({
            kind,
            engine: header.engine,
            channelEpoch: header.channelEpoch,
            commitId: overrides.commitId ?? header.commitId,
            baseRevision: overrides.baseRevision ?? header.baseRevision,
            newRevision: overrides.newRevision ?? header.newRevision,
            requiredControlRevision: overrides.requiredControlRevision ?? header.requiredControlRevision,
            sourceSimulationRevision: overrides.sourceSimulationRevision ?? header.sourceSimulationRevision,
            sequence: header.sequence,
        }, payload), header.sequence);
    }
}
function decodeAudioControl(packet) {
    const { header, payload } = packet;
    if (header.newRevision === 0n
        || header.commitId === 0n
        || header.baseRevision + 1n !== header.newRevision
        || payload.byteLength < AUDIO_CONTROL_PREFIX_BYTES) {
        throw new Error("invalid Voplay audio control header");
    }
    for (let index = 0; index < AUDIO_CONTROL_MAGIC.length; index += 1) {
        if (payload[index] !== AUDIO_CONTROL_MAGIC[index]) {
            throw new Error("invalid Voplay audio control magic");
        }
    }
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    if (view.getUint16(4, true) !== 1 || view.getUint16(6, true) !== 0) {
        throw new Error("unsupported Voplay audio control version");
    }
    if (view.getBigUint64(8, true) !== header.commitId) {
        throw new Error("Voplay audio control transaction mismatch");
    }
    const count = view.getUint32(16, true);
    if (count > MAX_AUDIO_CONTROL_ENTRIES) {
        throw new Error("Voplay audio control entry capacity exceeded");
    }
    let offset = AUDIO_CONTROL_PREFIX_BYTES;
    let descriptorBytes = 0;
    const identities = new Set();
    const buses = [];
    const sources = [];
    const retiringBuses = [];
    const retiringSources = [];
    const entries = [];
    for (let index = 0; index < count; index += 1) {
        if (payload.byteLength - offset < AUDIO_CONTROL_ENTRY_PREFIX_BYTES) {
            throw new Error("truncated Voplay audio control entry");
        }
        const kind = payload[offset];
        const state = payload[offset + 1];
        if ((kind !== 3 && kind !== 4)
            || (state !== 1 && state !== 2)
            || view.getUint16(offset + 2, true) !== 0) {
            throw new Error("invalid Voplay audio control entry");
        }
        const handleIndex = view.getUint32(offset + 4, true);
        const handleGeneration = view.getUint32(offset + 8, true);
        if (handleIndex === 0xffffffff || handleGeneration === 0) {
            throw new Error("invalid Voplay audio control handle");
        }
        const identity = `${kind}:${handleIndex}:${handleGeneration}`;
        if (identities.has(identity))
            throw new Error("duplicate Voplay audio control entry");
        identities.add(identity);
        const descriptorLength = view.getUint32(offset + 12, true);
        const fence = {
            renderRevision: view.getBigUint64(offset + 16, true),
            domainFrame: view.getBigUint64(offset + 24, true),
            eventSequence: view.getBigUint64(offset + 32, true),
            audioEventSequence: view.getBigUint64(offset + 40, true),
        };
        if (state === 1
            && (fence.renderRevision !== 0n || fence.domainFrame !== 0n
                || fence.eventSequence !== 0n || fence.audioEventSequence !== 0n)) {
            throw new Error("desired Voplay audio control carries a retirement fence");
        }
        descriptorBytes += descriptorLength;
        if (descriptorBytes > MAX_AUDIO_DESCRIPTOR_BYTES) {
            throw new Error("Voplay audio descriptor capacity exceeded");
        }
        offset += AUDIO_CONTROL_ENTRY_PREFIX_BYTES;
        if (descriptorLength > payload.byteLength - offset) {
            throw new Error("truncated Voplay audio descriptor");
        }
        const descriptor = payload.subarray(offset, offset + descriptorLength);
        const handle = { index: handleIndex, generation: handleGeneration };
        entries.push({
            kind,
            handle,
            state: state,
            descriptor: descriptor.slice(),
            fence,
        });
        if (kind === 3) {
            const value = decodeBusDescriptor(descriptor, header.engine, handle);
            if (state === 1)
                buses.push(value);
            else
                retiringBuses.push({ value, fence });
        }
        else {
            const value = decodeSourceDescriptor(descriptor, header.engine, handle);
            if (state === 1)
                sources.push(value);
            else
                retiringSources.push({ value, fence });
        }
        offset += descriptorLength;
    }
    if (offset !== payload.byteLength)
        throw new Error("trailing Voplay audio control bytes");
    if (buses.length > MAX_AUDIO_BUSES || sources.length > MAX_PERSISTENT_SOURCES) {
        throw new Error("Voplay desired audio mixer capacity exceeded");
    }
    const busKeys = new Set(buses.map((bus) => handleKey(bus.handle)));
    if (buses.filter((bus) => bus.parent === null).length !== 1) {
        throw new Error("Voplay audio control requires one root bus");
    }
    const listenerBuses = buses.filter((bus) => bus.listener !== null);
    if (listenerBuses.length !== 1 || listenerBuses[0].parent !== null) {
        throw new Error("Voplay audio control requires one root listener");
    }
    const duckingRules = buses.flatMap((bus) => bus.ducking);
    if (duckingRules.length > MAX_AUDIO_DUCKING_RULES
        || duckingRules.some((rule) => !busKeys.has(handleKey(rule.target)))) {
        throw new Error("Voplay audio control has an invalid ducking rule");
    }
    for (const bus of buses) {
        if (bus.parent !== null && !busKeys.has(handleKey(bus.parent))) {
            throw new Error("Voplay audio control references an unknown parent bus");
        }
    }
    validateBrowserAudioBusTopology(buses, true);
    for (const source of sources) {
        if (!busKeys.has(handleKey(source.bus))) {
            throw new Error("Voplay audio source references an unknown bus");
        }
    }
    const physicalBusKeys = new Set([
        ...busKeys,
        ...retiringBuses.map((retiring) => handleKey(retiring.value.handle)),
    ]);
    for (const retiring of retiringBuses) {
        if (retiring.value.parent !== null
            && !physicalBusKeys.has(handleKey(retiring.value.parent))) {
            throw new Error("retiring Voplay audio bus references an unknown parent bus");
        }
    }
    for (const retiring of retiringSources) {
        if (!physicalBusKeys.has(handleKey(retiring.value.bus))) {
            throw new Error("retiring Voplay audio source references an unknown bus");
        }
    }
    const physicalBuses = [
        ...buses,
        ...retiringBuses.map((retiring) => retiring.value),
    ];
    const physicalListeners = physicalBuses.filter((bus) => bus.listener !== null);
    if (physicalListeners.length !== 1
        || physicalListeners[0].parent !== null
        || physicalBuses
            .flatMap((bus) => bus.ducking)
            .some((rule) => !physicalBusKeys.has(handleKey(rule.target)))) {
        throw new Error("invalid physical Voplay audio mixer topology");
    }
    validateBrowserAudioBusTopology(physicalBuses, true);
    return { buses, sources, retiringBuses, retiringSources, entries };
}
function validateBrowserAudioBusTopology(buses, requireSingleRoot) {
    const byKey = new Map(buses.map((bus) => [handleKey(bus.handle), bus]));
    if (byKey.size !== buses.length
        || requireSingleRoot && buses.filter((bus) => bus.parent === null).length !== 1) {
        throw new Error("invalid Voplay audio bus topology");
    }
    for (const bus of buses) {
        const visiting = new Set();
        let cursor = bus;
        while (cursor !== undefined && cursor.parent !== null) {
            const key = handleKey(cursor.handle);
            if (visiting.has(key)) {
                throw new Error("cyclic Voplay audio bus topology");
            }
            visiting.add(key);
            cursor = byKey.get(handleKey(cursor.parent));
            if (cursor === undefined) {
                throw new Error("Voplay audio bus topology has a missing parent");
            }
        }
    }
}
function isBrowserAudioTombstoneUpdate(current, incoming) {
    const incomingEntries = new Map(incoming.entries.map((entry) => [
        `${entry.kind}:${handleKey(entry.handle)}`,
        entry,
    ]));
    for (const entry of current.entries) {
        const incomingEntry = incomingEntries.get(`${entry.kind}:${handleKey(entry.handle)}`);
        if (incomingEntry === undefined) {
            if (entry.state !== 2)
                return false;
            continue;
        }
        if (!sameBytes(entry.descriptor, incomingEntry.descriptor))
            return false;
        if (entry.state === incomingEntry.state) {
            if (!sameRetirementFence(entry.fence, incomingEntry.fence))
                return false;
            continue;
        }
        if (entry.state !== 1 || incomingEntry.state !== 2)
            return false;
    }
    return incoming.entries.every((entry) => current.entries.some((currentEntry) => currentEntry.kind === entry.kind
        && sameHandle(currentEntry.handle, entry.handle)
        && sameBytes(currentEntry.descriptor, entry.descriptor)
        && (currentEntry.state === entry.state
            && sameRetirementFence(currentEntry.fence, entry.fence)
            || currentEntry.state === 1 && entry.state === 2)));
}
function sameRetirementFence(left, right) {
    return left.renderRevision === right.renderRevision
        && left.domainFrame === right.domainFrame
        && left.eventSequence === right.eventSequence
        && left.audioEventSequence === right.audioEventSequence;
}
function decodeBusDescriptor(bytes, engine, handle) {
    const reader = new AudioReader(bytes);
    reader.prefix(engine, 1);
    const parentTag = reader.u8();
    const parent = parentTag === 0
        ? null
        : parentTag === 1
            ? reader.stable(3)
            : invalidAudioDescriptor();
    const gain = reader.gain();
    const flags = reader.u8();
    if (flags > 3)
        throw new Error("invalid Voplay audio bus flags");
    const listenerTag = reader.u8();
    const listener = listenerTag === 0
        ? null
        : listenerTag === 1
            ? reader.listener()
            : invalidAudioDescriptor();
    const duckingCount = reader.u16();
    if (duckingCount > MAX_AUDIO_DUCKING_RULES) {
        throw new Error("Voplay audio ducking capacity exceeded");
    }
    const ducking = [];
    const duckingTargets = new Set();
    for (let index = 0; index < duckingCount; index += 1) {
        const target = reader.stable(3);
        const targetKey = handleKey(target);
        const duckingGain = reader.gain();
        if (duckingTargets.has(targetKey)) {
            throw new Error("duplicate Voplay audio ducking target");
        }
        duckingTargets.add(targetKey);
        ducking.push({ trigger: handle, target, gain: duckingGain });
    }
    reader.finish();
    return {
        handle,
        parent,
        gain,
        mute: (flags & 1) !== 0,
        solo: (flags & 2) !== 0,
        listener,
        ducking,
    };
}
function decodeSourceDescriptor(bytes, engine, handle) {
    const reader = new AudioReader(bytes);
    reader.prefix(engine, 2);
    const bus = reader.stable(3);
    const asset = reader.handle();
    const gain = reader.gain();
    const flags = reader.u8();
    if ((flags & ~3) !== 0)
        throw new Error("invalid Voplay audio source flags");
    const recovery = reader.u8();
    if (recovery < 1 || recovery > 3)
        throw new Error("invalid Voplay audio recovery policy");
    const transportAnchorMillis = reader.u64();
    if (transportAnchorMillis > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new Error("Voplay audio transport anchor exceeds browser range");
    }
    const spatial = (flags & 2) !== 0 ? reader.spatial() : null;
    reader.finish();
    return {
        handle,
        bus,
        asset,
        gain,
        loop: (flags & 1) !== 0,
        recovery: recovery,
        transportAnchorMillis,
        spatial,
    };
}
function decodeOneShot(bytes, engine) {
    if (bytes.byteLength < 24
        || bytes[0] !== 0x56 || bytes[1] !== 0x41
        || bytes[2] !== 0x45 || bytes[3] !== 0x32
        || bytes[4] !== 1) {
        throw new Error("invalid Voplay one-shot audio event");
    }
    const reader = new AudioReader(bytes.subarray(5));
    const asset = reader.handle();
    const bus = reader.handle();
    const gain = reader.gain();
    const spatialTag = reader.u8();
    const spatial = spatialTag === 0
        ? null
        : spatialTag === 1
            ? reader.spatial()
            : invalidAudioDescriptor();
    reader.finish();
    if (engine.generation === 0)
        throw new Error("invalid Voplay audio engine");
    return { asset, bus, gain, spatial };
}
class AudioReader {
    #bytes;
    #view;
    #offset = 0;
    constructor(bytes) {
        this.#bytes = bytes;
        this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    }
    prefix(engine, tag) {
        if (this.u8() !== 0x56 || this.u8() !== 0x41 || this.u8() !== 0x43
            || this.u8() !== 0x31 || this.u8() !== tag
            || this.u32() !== engine.index || this.u32() !== engine.generation) {
            throw new Error("invalid Voplay audio descriptor prefix");
        }
    }
    stable(kind) {
        if (this.u8() !== 2 || this.u8() !== kind) {
            throw new Error("invalid Voplay stable audio reference");
        }
        return this.handle();
    }
    handle() {
        const handle = { index: this.u32(), generation: this.u32() };
        if (handle.index === 0xffffffff || handle.generation === 0) {
            throw new Error("invalid Voplay audio handle");
        }
        return handle;
    }
    spatial() {
        const position = [this.i32(), this.i32(), this.i32()];
        const minDistance = this.u32();
        const maxDistance = this.u32();
        if (maxDistance === 0 || minDistance > maxDistance) {
            throw new Error("invalid Voplay spatial audio descriptor");
        }
        return { position, minDistance, maxDistance };
    }
    listener() {
        const position = [this.i32(), this.i32(), this.i32()];
        const right = [this.i16(), this.i16(), this.i16()];
        if (right.every((component) => component === 0) || right.includes(-32768)) {
            throw new Error("invalid Voplay audio listener");
        }
        return { position, right };
    }
    u8() {
        this.#require(1);
        return this.#bytes[this.#offset++];
    }
    u16() {
        this.#require(2);
        const value = this.#view.getUint16(this.#offset, true);
        this.#offset += 2;
        return value;
    }
    gain() {
        const value = this.u16();
        if (value > 32767)
            throw new Error("invalid Voplay audio gain");
        return value / 32767;
    }
    u32() {
        this.#require(4);
        const value = this.#view.getUint32(this.#offset, true);
        this.#offset += 4;
        return value;
    }
    i32() {
        this.#require(4);
        const value = this.#view.getInt32(this.#offset, true);
        this.#offset += 4;
        return value;
    }
    i16() {
        this.#require(2);
        const value = this.#view.getInt16(this.#offset, true);
        this.#offset += 2;
        return value;
    }
    u64() {
        this.#require(8);
        const value = this.#view.getBigUint64(this.#offset, true);
        this.#offset += 8;
        return value;
    }
    skip(bytes) {
        this.#require(bytes);
        this.#offset += bytes;
    }
    finish() {
        if (this.#offset !== this.#bytes.byteLength) {
            throw new Error("trailing Voplay audio descriptor bytes");
        }
    }
    #require(bytes) {
        if (this.#offset + bytes > this.#bytes.byteLength) {
            throw new Error("truncated Voplay audio descriptor");
        }
    }
}
function invalidAudioDescriptor() {
    throw new Error("invalid Voplay audio descriptor");
}
function handleKey(handle) {
    return `${handle.index}:${handle.generation}`;
}
function sameHandle(left, right) {
    return left.index === right.index && left.generation === right.generation;
}
function validGenerationalHandle(handle) {
    return handle.index !== 0xffff_ffff && handle.generation !== 0;
}
function validateBrowserAudioPermit(bytes) {
    if (bytes.byteLength !== 58) {
        throw new Error("invalid Voplay browser audio device permit length");
    }
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const handles = [0, 8, 24, 40].map((offset) => ({
        index: view.getUint32(offset, true),
        generation: view.getUint32(offset + 4, true),
    }));
    if (handles.some((handle) => !validGenerationalHandle(handle))
        || view.getBigUint64(16, true) === 0n
        || view.getBigUint64(32, true) === 0n
        || view.getUint32(48, true) === 0
        || view.getUint16(52, true) === 0
        || view.getUint32(54, true) === 0) {
        throw new Error("invalid Voplay browser audio device permit");
    }
}
function sameBytes(left, right) {
    return left.byteLength === right.byteLength
        && left.every((value, index) => value === right[index]);
}
function audioContextFrame(context) {
    const frame = Math.floor(context.currentTime * context.sampleRate);
    if (!Number.isSafeInteger(frame) || frame < 0) {
        throw new Error("invalid browser audio context frame");
    }
    return BigInt(frame);
}
function delay(milliseconds) {
    return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}
function errorMessage(error) {
    return error instanceof Error ? error.message : String(error);
}
export default new VoplayBrowserAudioProvider();
