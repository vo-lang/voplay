// Render island bootstrap: load wasm VM, wire transport, start render loop.
// Per design doc: JS is bootstrap only; render loop runs inside Vo VM.
const DISPLAY_PULSE_DELAY_MS = 0xFFFFFFFF;
const DISPLAY_PULSE_VISIBLE_GUARD_MS = 34;
const DISPLAY_PULSE_LOST_RAF_FALLBACK_MS = 34;
const DISPLAY_PULSE_TIMER_TARGET_MS = 1000 / 60;
const DISPLAY_PULSE_VISIBLE_BACKUP_MS = DISPLAY_PULSE_TIMER_TARGET_MS * 1.08;
const DISPLAY_PULSE_RAF_HEALTHY_MS = DISPLAY_PULSE_TIMER_TARGET_MS * 1.05;
const DISPLAY_PULSE_RAF_SLOW_MS = DISPLAY_PULSE_TIMER_TARGET_MS * 1.1;
const DISPLAY_PULSE_HEALTHY_RAF_FRAMES = 2;
const DISPLAY_PULSE_SLOW_RAF_FRAMES = 1;
const DISPLAY_PULSE_TIMER_LEAD_MAX_MS = 2.0;
const DISPLAY_PULSE_TIMER_LEAD_GAIN = 0.125;
const DISPLAY_PULSE_TIMER_LEAD_DEADBAND_MS = 0.15;
const PERF_SAMPLE_WINDOW = 240;
const FRAME_BUDGET_120_MS = 1000 / 120;
const DISPLAY_PULSE_SLOW_120_MS = FRAME_BUDGET_120_MS * 1.25;
const DISPLAY_PULSE_SLOW_60_MS = (1000 / 60) * 1.1;
const WEBGPU_WORK_DONE_SAMPLE_STRIDE = 30;
const WEBGPU_WORK_DONE_HOST_SUSPEND_MS = 250;
const WEBGPU_WORK_DONE_HOST_SUSPEND_MIN_SUBMITS = 2;
const PERF_LIVENESS_INTERVAL_MS = 30000;
const PERF_REPORT_SCHEMA_VERSION = 1;
const PERF_REPORT_HISTORY_LIMIT = 64;
const PERF_REPORT_POST_QUEUE_LIMIT = 32;
const PERF_REPORT_POST_FLUSH_DELAY_MS = 50;
const PERF_PACKET_MAGIC = 0xf9;
const PERF_PACKET_VERSION = 1;
const PERF_PACKET_SCHEMA_VERSION = 1;
const PERF_PACKET_SOURCE_WEBGPU = 3;
const PERF_PACKET_HEADER_SIZE = 50;
const WEBGPU_PERF_PAYLOAD_VERSION = 1;
installWebGpuPacketBridge(globalThis);
installRendererPerfConfigBridge(globalThis);
const perfReportPostQueue = [];
let perfReportPostTimer = null;
class WebGpuPerfProbe {
    constructor() {
        this.installed = false;
        this.submitCount = 0;
        this.submitCpuWindow = [];
        this.textureAcquireWindow = [];
        this.workDoneWindow = [];
        this.probeCpuWindow = [];
        this.workDoneInFlight = 0;
        this.workDoneErrors = 0;
        this.workDoneHostSuspendedSamples = 0;
        this.lastSubmitNowMs = null;
        this.submitGapGeneration = 0;
    }
    install(globalScope) {
        if (this.installed)
            return;
        this.installed = true;
        const globals = globalScope;
        const queueCtor = globals.GPUQueue;
        const queueProto = queueCtor?.prototype;
        if (queueProto?.submit && !queueProto.__voplayOriginalSubmit) {
            queueProto.__voplayOriginalSubmit = queueProto.submit;
            queueProto.submit = function (...args) {
                const startMs = performance.now();
                try {
                    return queueProto.__voplayOriginalSubmit.apply(this, args);
                }
                finally {
                    const submitCpuMs = performance.now() - startMs;
                    const probeStartMs = performance.now();
                    globalScope.__voplayWebGpuPerfProbe?.recordSubmit(this, submitCpuMs);
                    globalScope.__voplayWebGpuPerfProbe?.recordProbeOverhead(performance.now() - probeStartMs);
                }
            };
        }
        const canvasContextCtor = globals.GPUCanvasContext;
        const canvasContextProto = canvasContextCtor?.prototype;
        if (canvasContextProto?.getCurrentTexture && !canvasContextProto.__voplayOriginalGetCurrentTexture) {
            canvasContextProto.__voplayOriginalGetCurrentTexture = canvasContextProto.getCurrentTexture;
            canvasContextProto.getCurrentTexture = function (...args) {
                const startMs = performance.now();
                try {
                    return canvasContextProto.__voplayOriginalGetCurrentTexture.apply(this, args);
                }
                finally {
                    const acquireMs = performance.now() - startMs;
                    const probeStartMs = performance.now();
                    globalScope.__voplayWebGpuPerfProbe?.recordTextureAcquire(acquireMs);
                    globalScope.__voplayWebGpuPerfProbe?.recordProbeOverhead(performance.now() - probeStartMs);
                }
            };
        }
    }
    recordTextureAcquire(durationMs) {
        pushWindowSample(this.textureAcquireWindow, durationMs, PERF_SAMPLE_WINDOW);
    }
    recordProbeOverhead(durationMs) {
        pushWindowSample(this.probeCpuWindow, durationMs, PERF_SAMPLE_WINDOW);
    }
    recordSubmit(queue, cpuMs) {
        this.submitCount += 1;
        const nowMs = performance.now();
        if (this.lastSubmitNowMs !== null && nowMs - this.lastSubmitNowMs >= WEBGPU_WORK_DONE_HOST_SUSPEND_MS) {
            this.submitGapGeneration += 1;
        }
        this.lastSubmitNowMs = nowMs;
        pushWindowSample(this.submitCpuWindow, cpuMs, PERF_SAMPLE_WINDOW);
        if (shouldSampleWebGpuWorkDone() && this.submitCount % WEBGPU_WORK_DONE_SAMPLE_STRIDE === 0) {
            this.sampleSubmittedWorkDone(queue);
        }
        if (this.submitCpuWindow.length >= PERF_SAMPLE_WINDOW) {
            this.reportWindow();
        }
    }
    sampleSubmittedWorkDone(queue) {
        if (typeof queue.onSubmittedWorkDone !== "function")
            return;
        const startMs = performance.now();
        const submitCountAtSample = this.submitCount;
        const submitGapGenerationAtSample = this.submitGapGeneration;
        this.workDoneInFlight += 1;
        try {
            Promise.resolve(queue.onSubmittedWorkDone.call(queue)).then(() => {
                this.workDoneInFlight = Math.max(0, this.workDoneInFlight - 1);
                const elapsedMs = performance.now() - startMs;
                const submitsWhilePending = this.submitCount - submitCountAtSample;
                if (isHostSuspendedWorkDoneSample(elapsedMs, submitsWhilePending, submitGapGenerationAtSample, this.submitGapGeneration)) {
                    this.workDoneHostSuspendedSamples += 1;
                    return;
                }
                pushWindowSample(this.workDoneWindow, elapsedMs, PERF_SAMPLE_WINDOW);
            }, () => {
                this.workDoneInFlight = Math.max(0, this.workDoneInFlight - 1);
                this.workDoneErrors += 1;
            });
        }
        catch {
            this.workDoneInFlight = Math.max(0, this.workDoneInFlight - 1);
            this.workDoneErrors += 1;
        }
    }
    reportWindow() {
        const textureAcquireSamples = this.textureAcquireWindow;
        const submitCpuSamples = this.submitCpuWindow;
        const workDoneSamples = this.workDoneWindow;
        const probeCpuSamples = this.probeCpuWindow;
        const workDoneInFlight = this.workDoneInFlight;
        const workDoneErrors = this.workDoneErrors;
        const workDoneHostSuspendedSamples = this.workDoneHostSuspendedSamples;
        this.textureAcquireWindow = [];
        this.submitCpuWindow = [];
        this.workDoneWindow = [];
        this.probeCpuWindow = [];
        this.workDoneErrors = 0;
        this.workDoneHostSuspendedSamples = 0;
        window.setTimeout(() => {
            this.emitWebGpuPerfWindow(textureAcquireSamples, submitCpuSamples, workDoneSamples, probeCpuSamples, workDoneInFlight, workDoneErrors, workDoneHostSuspendedSamples);
        }, 0);
    }
    emitWebGpuPerfWindow(textureAcquireSamples, submitCpuSamples, workDoneSamples, probeCpuSamples, workDoneInFlight, workDoneErrors, workDoneHostSuspendedSamples) {
        const reportStartMs = performance.now();
        const acquire = measureSamples(textureAcquireSamples);
        const submitCpu = measureSamples(submitCpuSamples);
        const workDoneRaw = measureSamples(workDoneSamples);
        const workDonePerSubmitWindow = workDoneSamples.map((sample) => sample / WEBGPU_WORK_DONE_SAMPLE_STRIDE);
        const workDone = measureSamples(workDonePerSubmitWindow);
        const probeCpu = measureSamples(probeCpuSamples);
        const slow120 = countAboveSamples(workDonePerSubmitWindow, DISPLAY_PULSE_SLOW_120_MS);
        const slow60 = countAboveSamples(workDonePerSubmitWindow, DISPLAY_PULSE_SLOW_60_MS);
        const queueDepthClass = shouldSampleWebGpuWorkDone()
            ? classifyQueueDepth(workDone, workDoneInFlight)
            : "normal";
        const sampleRate = 1 / WEBGPU_WORK_DONE_SAMPLE_STRIDE;
        const message = `webgpu window submits=${submitCpu.count}` +
            ` queue=${queueDepthClass}` +
            ` getTex p50/p90/p99/max=${formatMsValue(acquire.p50)}/${formatMsValue(acquire.p90)}/${formatMsValue(acquire.p99)}/${formatMsValue(acquire.max)}` +
            ` submitCpu p50/p90/p99/max=${formatMsValue(submitCpu.p50)}/${formatMsValue(submitCpu.p90)}/${formatMsValue(submitCpu.p99)}/${formatMsValue(submitCpu.max)}` +
            ` workDone stride=${WEBGPU_WORK_DONE_SAMPLE_STRIDE} samples=${workDone.count}` +
            ` perSubmit p50/p90/p99/max=${formatMsValue(workDone.p50)}/${formatMsValue(workDone.p90)}/${formatMsValue(workDone.p99)}/${formatMsValue(workDone.max)}` +
            ` raw p90/max=${formatMsValue(workDoneRaw.p90)}/${formatMsValue(workDoneRaw.max)}` +
            ` slow120=${slow120}/${workDone.count}` +
            ` slow60=${slow60}/${workDone.count}` +
            ` inFlight=${workDoneInFlight}` +
            ` hostSuspend=${workDoneHostSuspendedSamples}` +
            ` probeCpu p99=${formatMsValue(probeCpu.p99)}` +
            ` errors=${workDoneErrors}`;
        const reportOverheadMs = performance.now() - reportStartMs;
        globalThis.__voplayWebGpuPerfPacket = encodeWebGpuPerfPacket({
            acquire,
            submitCpu,
            workDone,
            workDoneRaw,
            probeCpu,
            queueDepthClass,
            sampleRate,
            workDoneInFlight,
            workDoneErrors,
            reportOverheadMs,
        });
        const shouldLog = shouldLogPerfReports();
        if (shouldLog) {
            console.info(`[voplay] ${message}`);
            globalThis.__voplayWebGpuPerfReport?.(message);
        }
        sendVoplayPerfReport("webgpu", message, {
            submits: submitCpu.count,
            getCurrentTexture: acquire,
            submitCpu,
            workDone,
            workDoneRaw,
            queueDepthClass,
            workDoneStride: WEBGPU_WORK_DONE_SAMPLE_STRIDE,
            sampleRate,
            workDoneSlow120: slow120,
            workDoneSlow60: slow60,
            workDoneInFlight,
            workDoneErrors,
            workDoneHostSuspendedSamples,
            probeCpu,
            reportOverheadMs,
        });
    }
}
function installWebGpuPacketBridge(globalScope) {
    if (globalScope.__voplayTakeWebGpuPerfPacket)
        return;
    globalScope.__voplayTakeWebGpuPerfPacket = () => {
        const packet = globalScope.__voplayWebGpuPerfPacket;
        globalScope.__voplayWebGpuPerfPacket = undefined;
        return packet ?? new Uint8Array(0);
    };
}
function installRendererPerfConfigBridge(globalScope) {
    if (globalScope.__voplayRendererPerfConfig)
        return;
    const initialConfig = readRendererPerfConfig();
    globalScope.__voplayRendererPerfConfig = () => mergeRendererPerfConfigs(initialConfig, readRendererPerfConfig());
}
function readRendererPerfConfig() {
    const tokens = [];
    appendRendererPerfConfigTokens(tokens, readLocationParam("voplayRendererPerf"));
    appendRendererPerfConfigTokens(tokens, readLocationParam("voplayPerfExperiment"));
    appendRendererPerfConfigTokens(tokens, readLocationParam("voplayPerfDiag"));
    try {
        appendRendererPerfConfigTokens(tokens, globalThis.localStorage?.getItem("voplay.rendererPerf"));
    }
    catch {
        // localStorage is optional in embedded or restricted contexts.
    }
    appendRendererPerfSwitch(tokens, "disableShadows", ["voplayPerfDisableShadows", "voplayDisableShadows"], "voplay.perf.disableShadows");
    appendRendererPerfSwitch(tokens, "disablePostEffects", ["voplayPerfDisablePost", "voplayDisablePost", "voplayPerfDisablePostEffects"], "voplay.perf.disablePostEffects");
    appendRendererPerfSwitch(tokens, "disableBloom", ["voplayPerfDisableBloom", "voplayDisableBloom"], "voplay.perf.disableBloom");
    appendRendererPerfSwitch(tokens, "disableSharpen", ["voplayPerfDisableSharpen", "voplayDisableSharpen"], "voplay.perf.disableSharpen");
    appendRendererPerfSwitch(tokens, "disableFxaa", ["voplayPerfDisableFxaa", "voplayDisableFxaa"], "voplay.perf.disableFxaa");
    appendRendererPerfSwitch(tokens, "disableContactAO", ["voplayPerfDisableContactAO", "voplayDisableContactAO"], "voplay.perf.disableContactAO");
    appendRendererPerfSwitch(tokens, "disablePrimitives", ["voplayPerfDisablePrimitives", "voplayDisablePrimitives"], "voplay.perf.disablePrimitives");
    appendRendererPerfSwitch(tokens, "disablePrimitiveShadows", ["voplayPerfDisablePrimitiveShadows", "voplayDisablePrimitiveShadows"], "voplay.perf.disablePrimitiveShadows");
    appendRendererPerfSwitch(tokens, "disableDecals", ["voplayPerfDisableDecals", "voplayDisableDecals"], "voplay.perf.disableDecals");
    return [...new Set(tokens)].join(",");
}
function appendRendererPerfConfigTokens(tokens, raw) {
    if (!raw)
        return;
    for (const token of raw.split(/[,\s;&|]+/)) {
        const trimmed = token.trim();
        if (trimmed)
            tokens.push(trimmed);
    }
}
function mergeRendererPerfConfigs(...configs) {
    const tokens = [];
    for (const config of configs) {
        appendRendererPerfConfigTokens(tokens, config);
    }
    return [...new Set(tokens)].join(",");
}
function appendRendererPerfSwitch(tokens, token, paramNames, storageKey) {
    const value = readRendererPerfSwitch(paramNames, storageKey);
    if (value)
        tokens.push(token);
}
function readRendererPerfSwitch(paramNames, storageKey) {
    for (const name of paramNames) {
        const raw = readLocationParam(name);
        if (raw !== null)
            return readTruthyPerfValue(raw);
    }
    try {
        const raw = globalThis.localStorage?.getItem(storageKey);
        if (raw !== null && raw !== undefined)
            return readTruthyPerfValue(raw);
    }
    catch {
        return false;
    }
    return false;
}
function readTruthyPerfValue(raw) {
    const value = raw.trim().toLowerCase();
    return value !== "" && value !== "0" && value !== "false" && value !== "off" && value !== "no";
}
function encodeWebGpuPerfPacket(metrics) {
    const payloadSize = 4 + 4 * 4 + 6 * 8;
    const data = new Uint8Array(PERF_PACKET_HEADER_SIZE + payloadSize);
    const view = new DataView(data.buffer);
    let offset = 0;
    view.setUint8(offset, PERF_PACKET_MAGIC);
    offset += 1;
    view.setUint8(offset, PERF_PACKET_VERSION);
    offset += 1;
    writeU32(view, offset, PERF_PACKET_SCHEMA_VERSION);
    offset += 4;
    writeU32(view, offset, 0);
    offset += 4;
    writeU32(view, offset, metrics.submitCpu.count);
    offset += 4;
    writeU32(view, offset, PERF_PACKET_SOURCE_WEBGPU);
    offset += 4;
    writeU32(view, offset, payloadSize);
    offset += 4;
    writeF64(view, offset, metrics.submitCpu.p90);
    offset += 8;
    writeF64(view, offset, metrics.workDone.p90);
    offset += 8;
    writeU32(view, offset, metrics.workDoneInFlight);
    offset += 4;
    writeU32(view, offset, 0);
    offset += 4;
    writeU32(view, offset, 1);
    offset += 4;
    writeU32(view, offset, WEBGPU_PERF_PAYLOAD_VERSION);
    offset += 4;
    writeU32(view, offset, webGpuQueueDepthClassCode(metrics.queueDepthClass));
    offset += 4;
    writeU32(view, offset, metrics.submitCpu.count);
    offset += 4;
    writeU32(view, offset, WEBGPU_WORK_DONE_SAMPLE_STRIDE);
    offset += 4;
    writeU32(view, offset, metrics.workDoneErrors);
    offset += 4;
    writeF64(view, offset, metrics.acquire.p90);
    offset += 8;
    writeF64(view, offset, metrics.submitCpu.p90);
    offset += 8;
    writeF64(view, offset, metrics.workDone.p90);
    offset += 8;
    writeF64(view, offset, metrics.sampleRate);
    offset += 8;
    writeF64(view, offset, metrics.probeCpu.p99);
    offset += 8;
    writeF64(view, offset, metrics.reportOverheadMs);
    return data;
}
function writeU32(view, offset, value) {
    view.setUint32(offset, clampU32(value), true);
}
function writeF64(view, offset, value) {
    view.setFloat64(offset, Number.isFinite(value) ? value : 0, true);
}
function clampU32(value) {
    if (!Number.isFinite(value) || value <= 0)
        return 0;
    return Math.min(0xffffffff, Math.floor(value));
}
function webGpuQueueDepthClassCode(value) {
    if (value === "empty")
        return 0;
    if (value === "backlogged")
        return 2;
    if (value === "saturated")
        return 3;
    return 1;
}
function pushWindowSample(samples, value, capacity) {
    samples.push(value);
    if (samples.length > capacity) {
        samples.splice(0, samples.length - capacity);
    }
}
function measureSamples(samples) {
    if (samples.length === 0) {
        return { count: 0, p50: 0, p90: 0, p99: 0, max: 0 };
    }
    const sorted = [...samples].sort((a, b) => a - b);
    return {
        count: sorted.length,
        p50: percentileSorted(sorted, 0.5),
        p90: percentileSorted(sorted, 0.9),
        p99: percentileSorted(sorted, 0.99),
        max: sorted[sorted.length - 1],
    };
}
function percentileSorted(sorted, fraction) {
    if (sorted.length === 0)
        return 0;
    const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * fraction) - 1));
    return sorted[index];
}
function countAboveSamples(samples, threshold) {
    let count = 0;
    for (const sample of samples) {
        if (sample > threshold)
            count += 1;
    }
    return count;
}
function classifyQueueDepth(workDone, inFlight) {
    if (workDone.count === 0 && inFlight === 0)
        return "empty";
    if (inFlight >= 4 || workDone.p90 >= 100)
        return "saturated";
    if (inFlight >= 2 || workDone.p90 >= FRAME_BUDGET_120_MS * 4)
        return "backlogged";
    return "normal";
}
function isHostSuspendedWorkDoneSample(elapsedMs, submitsWhilePending, submitGapGenerationAtSample, currentSubmitGapGeneration) {
    if (elapsedMs < WEBGPU_WORK_DONE_HOST_SUSPEND_MS)
        return false;
    return submitsWhilePending < WEBGPU_WORK_DONE_HOST_SUSPEND_MIN_SUBMITS
        || currentSubmitGapGeneration !== submitGapGenerationAtSample;
}
function formatMsValue(value) {
    return `${value.toFixed(2)}ms`;
}
function sendVoplayPerfReport(kind, message, metrics) {
    const mode = readVoplayPerfMode();
    const payload = {
        schemaVersion: PERF_REPORT_SCHEMA_VERSION,
        source: "voplay-render-bootstrap",
        kind,
        mode,
        message,
        metrics,
        nowMs: performance.now(),
        href: globalThis.location?.href,
        visibility: globalThis.document?.visibilityState,
        focus: globalThis.document?.hasFocus?.(),
    };
    const globalScope = globalThis;
    const reports = globalScope.__voplayPerfReports ?? [];
    reports.push(payload);
    if (reports.length > PERF_REPORT_HISTORY_LIMIT) {
        reports.splice(0, reports.length - PERF_REPORT_HISTORY_LIMIT);
    }
    globalScope.__voplayPerfReports = reports;
    if (!shouldPostPerfReports(mode)) {
        return;
    }
    queueVoplayPerfReportPost(payload);
}
function queueVoplayPerfReportPost(payload) {
    perfReportPostQueue.push(payload);
    if (perfReportPostQueue.length > PERF_REPORT_POST_QUEUE_LIMIT) {
        perfReportPostQueue.splice(0, perfReportPostQueue.length - PERF_REPORT_POST_QUEUE_LIMIT);
    }
    if (perfReportPostTimer !== null)
        return;
    try {
        perfReportPostTimer = window.setTimeout(flushVoplayPerfReportPosts, PERF_REPORT_POST_FLUSH_DELAY_MS);
    }
    catch {
        flushVoplayPerfReportPosts();
    }
}
function flushVoplayPerfReportPosts() {
    perfReportPostTimer = null;
    const pending = perfReportPostQueue.splice(0);
    if (pending.length === 0)
        return;
    postVoplayPerfReports(pending);
}
function postVoplayPerfReports(payloads) {
    let body = "";
    try {
        body = payloads.length === 1 ? JSON.stringify(payloads[0]) : JSON.stringify(payloads);
    }
    catch {
        return;
    }
    try {
        if (globalThis.navigator?.sendBeacon) {
            const blob = new Blob([body], { type: "application/json" });
            if (globalThis.navigator.sendBeacon("/__voplay_perf_report", blob)) {
                return;
            }
        }
    }
    catch {
        // Fall through to fetch.
    }
    try {
        void fetch("/__voplay_perf_report", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body,
            keepalive: true,
        }).catch(() => { });
        return;
    }
    catch {
        // Diagnostics must never affect the render loop.
    }
}
function readLocationParam(name) {
    try {
        const searchValue = new URLSearchParams(globalThis.location?.search ?? "").get(name);
        if (searchValue !== null)
            return searchValue;
        const hash = globalThis.location?.hash ?? "";
        const queryOffset = hash.indexOf("?");
        if (queryOffset >= 0) {
            return new URLSearchParams(hash.slice(queryOffset + 1)).get(name);
        }
    }
    catch {
        return null;
    }
    return null;
}
function locationHasParam(name) {
    return readLocationParam(name) !== null;
}
function readVoplayPerfMode() {
    const raw = readLocationParam("voplayPerf") ??
        readLocationParam("perf") ??
        globalThis.localStorage?.getItem("voplay.perf.mode") ??
        (shouldDefaultRunnerPerfStats() ? "stats" : null) ??
        "off";
    if (raw === "stats" || raw === "hud" || raw === "trace" || raw === "deep")
        return raw;
    if (raw === "1" || raw === "true")
        return "stats";
    return "off";
}
function shouldDefaultRunnerPerfStats() {
    try {
        const location = globalThis.location;
        if (!location || !isLocalRunnerHost(location.hostname))
            return false;
        return (location.hash ?? "").startsWith("#/runner");
    }
    catch {
        return false;
    }
}
function isLocalRunnerHost(hostname) {
    return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "::1" || hostname === "[::1]";
}
function isVoplayPerfEnabled(mode) {
    return mode !== "off";
}
function shouldEmitVerbosePerfReports(mode = readVoplayPerfMode()) {
    return mode === "trace" || mode === "deep";
}
function shouldPostPerfReports(mode = readVoplayPerfMode()) {
    return isVoplayPerfEnabled(mode) || locationHasParam("voplayPerf") || locationHasParam("perf") || locationHasParam("voplayPerfOverlay");
}
function shouldLogPerfReports(mode = readVoplayPerfMode()) {
    return shouldEmitVerbosePerfReports(mode) || locationHasParam("voplayPerfOverlay");
}
function shouldSampleWebGpuWorkDone(mode = readVoplayPerfMode()) {
    try {
        return mode === "deep"
            || mode === "trace"
            || locationHasParam("voplayPerfGpu")
            || globalThis.localStorage?.getItem("voplay.perf.gpu") === "1";
    }
    catch {
        return false;
    }
}
function shouldInstallWebGpuPerfProbe(mode = readVoplayPerfMode()) {
    return isVoplayPerfEnabled(mode) || shouldSampleWebGpuWorkDone(mode);
}
function shouldShowBootstrapPerfOverlay(mode) {
    try {
        if (locationHasParam("voplayPerfOverlay"))
            return true;
        if (globalThis.localStorage?.getItem("voplay.perf.overlay") === "1")
            return true;
        if (shouldCaptureRendererDebugStatus())
            return true;
    }
    catch {
        return false;
    }
    return mode === "deep";
}
function shouldCaptureRendererDebugStatus() {
    try {
        return locationHasParam("rendererDebug")
            || locationHasParam("debug")
            || globalThis.localStorage?.getItem("voplay.rendererDebug") === "1";
    }
    catch {
        return false;
    }
}
function readDisplayPulseMode() {
    try {
        const raw = [
            readLocationParam("voplayPulseMode"),
            readLocationParam("voplayPerfDiag"),
            readLocationParam("voplayPerfExperiment"),
            globalThis.localStorage?.getItem("voplay.pulse.mode"),
        ].filter((value) => value !== null && value !== undefined).join(",");
        for (const token of raw.split(/[,\s;&|]+/)) {
            const mode = token.trim().toLowerCase();
            if (mode === "raf" || mode === "pulseraf")
                return "raf";
            if (mode === "timer" || mode === "pulsetimer")
                return "timer";
            if (mode === "hybrid" || mode === "pulsehybrid")
                return "hybrid";
        }
    }
    catch {
        return "hybrid";
    }
    return "hybrid";
}
// RenderIsland: manages the render island VM lifecycle.
export class RenderIsland {
    constructor(config) {
        this.vm = null;
        this.perfMode = readVoplayPerfMode();
        this.displayPulseMode = readDisplayPulseMode();
        this.hostTimers = new Map();
        this.stopped = false;
        this.recentConsoleErrors = [];
        this.originalConsoleError = null;
        this.inboundFrameCount = 0;
        this.outboundFrameCount = 0;
        this.wakeCount = 0;
        this.displayPulseRafId = null;
        this.displayPulseTimerId = null;
        this.displayPulseScheduleId = 0;
        this.displayPulseSerial = 0;
        this.displayPulseLastWakeMs = null;
        this.displayPulseLastRafMs = null;
        this.displayPulseTimerLeadMs = 0;
        this.displayPulseHealthyRafFrames = 0;
        this.displayPulseSlowRafFrames = 0;
        this.displayPulseTimerCadence = false;
        this.displayPulseWaiters = new Map();
        this.displayPulseWaitWindow = [];
        this.displayPulseRafWakeWindow = 0;
        this.displayPulseTimerWakeWindow = 0;
        this.displayPulseLastSource = null;
        this.vmWakeRunWindow = [];
        this.scheduledHostEventCount = 0;
        this.scheduledDisplayPulseCount = 0;
        this.scheduledTimeoutCount = 0;
        this.lastHostEventDelayMs = null;
        this.perfLivenessTimerIds = [];
        this.pulseOverlay = null;
        this.pulseMessage = "";
        this.webGpuMessage = "";
        this.rendererDebugMessage = "";
        this.webGpuReportCallback = null;
        this.rendererDebugStatusCallback = null;
        this.config = config;
    }
    async init(voWeb) {
        this.installConsoleErrorCapture();
        this.installRendererDebugStatusProbe();
        this.installWebGpuPerfProbe();
        this.debug(`render island init.begin canvasId=${this.config.canvasId}`);
        this.debug(`render island init_vfs.begin canvasId=${this.config.canvasId}`);
        await voWeb.initVFS();
        this.debug(`render island init_vfs.ready canvasId=${this.config.canvasId}`);
        let jsGlueUrl = '';
        let blobUrlToRevoke = '';
        if (this.config.voplayWasmJsGlue && this.config.voplayWasmJsGlue.length > 0) {
            const jsText = new TextDecoder().decode(this.config.voplayWasmJsGlue);
            const blob = new Blob([jsText], { type: 'application/javascript' });
            blobUrlToRevoke = URL.createObjectURL(blob);
            jsGlueUrl = blobUrlToRevoke;
        }
        try {
            this.debug(`render island preload_ext.begin canvasId=${this.config.canvasId} bindgen=${jsGlueUrl ? 'yes' : 'no'}`);
            await voWeb.preloadExtModule("github.com/vo-lang/voplay", this.config.voplayWasm, jsGlueUrl);
            this.debug(`render island preload_ext.ready canvasId=${this.config.canvasId}`);
        }
        finally {
            if (blobUrlToRevoke)
                URL.revokeObjectURL(blobUrlToRevoke);
        }
        this.debug(`render island vm_create.begin canvasId=${this.config.canvasId}`);
        this.vm = voWeb.VoVm.withExterns(this.config.bytecode);
        this.debug(`render island vm_create.ready canvasId=${this.config.canvasId}`);
        this.debug(`render island run_init.begin canvasId=${this.config.canvasId}`);
        this.vm.runInit();
        this.debug(`render island run_init.ready canvasId=${this.config.canvasId}`);
    }
    start() {
        if (!this.vm)
            throw new Error("VM not initialized");
        this.stopped = false;
        this.config.channel.onReceive((frame) => {
            try {
                if (this.stopped || !this.vm)
                    return;
                this.inboundFrameCount += 1;
                this.debugFrameStatus(`render island inbound #${this.inboundFrameCount} bytes=${frame.byteLength}`, this.inboundFrameCount);
                this.vm.pushIslandCommand(frame);
                const outcome = this.vm.runScheduled();
                this.flush();
                this.scheduleHostEvents();
            }
            catch (error) {
                this.fail(`runScheduled inbound_bytes=${frame.byteLength}`, error);
            }
        });
        this.flush();
        this.scheduleHostEvents();
        this.ensureDisplayPulseTicker();
        this.reportBootstrapStart();
        this.schedulePerfLivenessProbes();
    }
    reportBootstrapStart() {
        if (!this.perfEnabled() || !shouldPostPerfReports(this.perfMode))
            return;
        sendVoplayPerfReport("bootstrap", "render island started", {
            ...this.collectPerfLivenessMetrics(),
            displayPulseVisibleGuardMs: DISPLAY_PULSE_VISIBLE_GUARD_MS,
            displayPulseLostRafFallbackMs: DISPLAY_PULSE_LOST_RAF_FALLBACK_MS,
            displayPulseVisibleBackupMs: DISPLAY_PULSE_VISIBLE_BACKUP_MS,
            displayPulseTimerTargetMs: DISPLAY_PULSE_TIMER_TARGET_MS,
        });
    }
    stop() {
        this.stopped = true;
        for (const handle of this.hostTimers.values()) {
            if (handle.kind === "raf") {
                cancelAnimationFrame(handle.id);
            }
            else if (handle.kind === "timeout") {
                clearTimeout(handle.id);
            }
        }
        this.hostTimers.clear();
        this.displayPulseWaiters.clear();
        this.clearDisplayPulseSchedule();
        for (const id of this.perfLivenessTimerIds) {
            window.clearTimeout(id);
        }
        this.perfLivenessTimerIds = [];
        if (this.originalConsoleError) {
            console.error = this.originalConsoleError;
            this.originalConsoleError = null;
        }
        const globalScope = globalThis;
        globalScope.voDisposeExtModule?.("github.com/vo-lang/voplay");
        if (globalScope.__voplayWebGpuPerfReport === this.webGpuReportCallback) {
            delete globalScope.__voplayWebGpuPerfReport;
        }
        if (globalScope.__voplayDebugStatus === this.rendererDebugStatusCallback) {
            delete globalScope.__voplayDebugStatus;
        }
        this.webGpuReportCallback = null;
        this.rendererDebugStatusCallback = null;
        this.pulseOverlay?.remove();
        this.pulseOverlay = null;
        this.config.channel.close();
        this.vm = null;
    }
    flush() {
        if (!this.vm)
            return;
        const cmds = this.vm.takeOutboundCommands();
        for (const frame of cmds) {
            this.outboundFrameCount += 1;
            this.debugFrameStatus(`render island outbound #${this.outboundFrameCount} bytes=${frame.byteLength}`, this.outboundFrameCount);
            this.config.channel.send(frame);
        }
    }
    scheduleHostEvents() {
        if (!this.vm || this.stopped)
            return;
        const events = this.vm.takePendingHostEvents();
        for (const ev of events) {
            if (this.hostTimers.has(ev.key))
                continue;
            this.scheduledHostEventCount += 1;
            this.lastHostEventDelayMs = ev.delayMs;
            if (ev.delayMs === DISPLAY_PULSE_DELAY_MS) {
                this.scheduledDisplayPulseCount += 1;
                this.displayPulseWaiters.set(ev.key, {
                    afterSerial: this.displayPulseSerial,
                    scheduledAtMs: performance.now(),
                });
                this.hostTimers.set(ev.key, { kind: "displayPulse" });
                this.ensureDisplayPulseTicker();
            }
            else {
                this.scheduledTimeoutCount += 1;
                const id = window.setTimeout(() => this.wakeHostEvent(ev.key, ev.delayMs), ev.delayMs);
                this.hostTimers.set(ev.key, { kind: "timeout", id });
            }
        }
    }
    schedulePerfLivenessProbes() {
        if (!this.perfEnabled() || !shouldPostPerfReports(this.perfMode))
            return;
        for (const delayMs of [1000, 5000]) {
            const id = window.setTimeout(() => {
                if (this.stopped)
                    return;
                sendVoplayPerfReport("liveness", `render island liveness delay=${delayMs}ms`, {
                    ...this.collectPerfLivenessMetrics(),
                    delayMs,
                });
            }, delayMs);
            this.perfLivenessTimerIds.push(id);
        }
        const intervalId = window.setInterval(() => {
            if (this.stopped)
                return;
            sendVoplayPerfReport("liveness", `render island liveness interval=${PERF_LIVENESS_INTERVAL_MS}ms`, {
                ...this.collectPerfLivenessMetrics(),
                intervalMs: PERF_LIVENESS_INTERVAL_MS,
            });
        }, PERF_LIVENESS_INTERVAL_MS);
        this.perfLivenessTimerIds.push(intervalId);
    }
    collectPerfLivenessMetrics() {
        return {
            canvasId: this.config.canvasId,
            perfMode: this.perfMode,
            inboundFrameCount: this.inboundFrameCount,
            outboundFrameCount: this.outboundFrameCount,
            wakeCount: this.wakeCount,
            hostTimers: this.hostTimers.size,
            displayPulseWaiters: this.displayPulseWaiters.size,
            displayPulseMode: this.displayPulseMode,
            displayPulseSerial: this.displayPulseSerial,
            displayPulseRafScheduled: this.displayPulseRafId !== null,
            displayPulseTimerScheduled: this.displayPulseTimerId !== null,
            displayPulseLastWakeMs: this.displayPulseLastWakeMs,
            displayPulseLastRafMs: this.displayPulseLastRafMs,
            displayPulseTimerLeadMs: this.displayPulseTimerLeadMs,
            displayPulseTimerCadence: this.displayPulseTimerCadence,
            displayPulseSamples: this.displayPulseWaitWindow.length,
            displayPulseRafWakeWindow: this.displayPulseRafWakeWindow,
            displayPulseTimerWakeWindow: this.displayPulseTimerWakeWindow,
            displayPulseLastSource: this.displayPulseLastSource,
            vmWakeRunSamples: this.vmWakeRunWindow.length,
            scheduledHostEventCount: this.scheduledHostEventCount,
            scheduledDisplayPulseCount: this.scheduledDisplayPulseCount,
            scheduledTimeoutCount: this.scheduledTimeoutCount,
            lastHostEventDelayMs: this.lastHostEventDelayMs,
        };
    }
    ensureDisplayPulseTicker() {
        if (this.displayPulseRafId !== null || this.displayPulseTimerId !== null || this.stopped || !this.vm)
            return;
        const scheduleId = ++this.displayPulseScheduleId;
        const nowMs = performance.now();
        const visible = document.visibilityState === "visible";
        if (this.displayPulseMode !== "timer" && visible) {
            this.displayPulseRafId = window.requestAnimationFrame(() => this.handleDisplayPulse(scheduleId, "raf"));
        }
        if (this.shouldScheduleDisplayPulseTimer(visible)) {
            const fallbackMs = this.displayPulseFallbackDelayMs(nowMs);
            this.displayPulseTimerId = window.setTimeout(() => this.handleDisplayPulse(scheduleId, "timer"), fallbackMs);
        }
    }
    shouldScheduleDisplayPulseTimer(visible) {
        if (!visible)
            return true;
        if (this.displayPulseMode === "timer")
            return true;
        // Hybrid mode starts with rAF-only visible cadence. The timer path takes
        // over only after rAF has already been observed as slow, so the backup does
        // not race healthy rAF frames and inject cadence jitter into the game clock.
        return this.displayPulseMode === "hybrid" && this.displayPulseTimerCadence;
    }
    displayPulseFallbackDelayMs(nowMs) {
        if (document.visibilityState !== "visible")
            return DISPLAY_PULSE_LOST_RAF_FALLBACK_MS;
        if (this.displayPulseMode === "timer") {
            return this.displayPulseCadenceDelayMs(nowMs);
        }
        return DISPLAY_PULSE_VISIBLE_BACKUP_MS;
    }
    displayPulseCadenceDelayMs(nowMs) {
        if (this.displayPulseLastWakeMs === null)
            return DISPLAY_PULSE_TIMER_TARGET_MS;
        const nextWakeMs = this.displayPulseLastWakeMs + DISPLAY_PULSE_TIMER_TARGET_MS - this.displayPulseTimerLeadMs;
        return Math.max(0, Math.min(DISPLAY_PULSE_VISIBLE_GUARD_MS, nextWakeMs - nowMs));
    }
    clearDisplayPulseSchedule() {
        if (this.displayPulseRafId !== null) {
            window.cancelAnimationFrame(this.displayPulseRafId);
            this.displayPulseRafId = null;
        }
        if (this.displayPulseTimerId !== null) {
            window.clearTimeout(this.displayPulseTimerId);
            this.displayPulseTimerId = null;
        }
    }
    handleDisplayPulse(scheduleId, source) {
        if (scheduleId !== this.displayPulseScheduleId)
            return;
        const nowMs = performance.now();
        this.displayPulseScheduleId += 1;
        this.clearDisplayPulseSchedule();
        this.handleDisplayPulseTick(source, nowMs);
    }
    handleDisplayPulseTick(source, nowMs) {
        if (this.stopped || !this.vm)
            return;
        const previousWakeMs = this.displayPulseLastWakeMs;
        this.displayPulseSerial += 1;
        this.displayPulseLastSource = source;
        if (source === "raf") {
            this.displayPulseRafWakeWindow += 1;
        }
        else {
            this.displayPulseTimerWakeWindow += 1;
        }
        if (source === "raf") {
            this.recordRafPulse(nowMs);
        }
        else if (this.displayPulseLastRafMs === null || nowMs - this.displayPulseLastRafMs >= DISPLAY_PULSE_RAF_SLOW_MS) {
            this.displayPulseTimerCadence = true;
        }
        if (source === "timer") {
            this.recordTimerPulse(nowMs, previousWakeMs);
        }
        this.displayPulseLastWakeMs = nowMs;
        const readyKeys = [];
        for (const [key, waiter] of this.displayPulseWaiters) {
            if (this.displayPulseSerial > waiter.afterSerial) {
                readyKeys.push(key);
            }
        }
        for (const key of readyKeys) {
            const waiter = this.displayPulseWaiters.get(key);
            if (!waiter)
                continue;
            this.displayPulseWaiters.delete(key);
            this.recordDisplayPulseWait(nowMs - waiter.scheduledAtMs);
            this.wakeHostEvent(key, DISPLAY_PULSE_DELAY_MS);
        }
        if (this.displayPulseWaiters.size > 0) {
            this.ensureDisplayPulseTicker();
        }
    }
    recordRafPulse(nowMs) {
        if (this.displayPulseLastRafMs !== null) {
            const deltaMs = nowMs - this.displayPulseLastRafMs;
            if (deltaMs <= DISPLAY_PULSE_RAF_HEALTHY_MS) {
                this.displayPulseHealthyRafFrames += 1;
                this.displayPulseSlowRafFrames = 0;
                if (this.displayPulseHealthyRafFrames >= DISPLAY_PULSE_HEALTHY_RAF_FRAMES) {
                    this.displayPulseTimerCadence = false;
                }
            }
            else if (deltaMs >= DISPLAY_PULSE_RAF_SLOW_MS) {
                this.displayPulseSlowRafFrames += 1;
                this.displayPulseHealthyRafFrames = 0;
                if (this.displayPulseSlowRafFrames >= DISPLAY_PULSE_SLOW_RAF_FRAMES) {
                    this.displayPulseTimerCadence = true;
                }
            }
        }
        this.displayPulseLastRafMs = nowMs;
    }
    recordTimerPulse(nowMs, previousWakeMs) {
        if (this.displayPulseMode !== "timer")
            return;
        if (previousWakeMs === null)
            return;
        const deltaMs = nowMs - previousWakeMs;
        if (deltaMs < 1 || deltaMs > DISPLAY_PULSE_LOST_RAF_FALLBACK_MS * 2)
            return;
        const errorMs = deltaMs - DISPLAY_PULSE_TIMER_TARGET_MS;
        if (Math.abs(errorMs) <= DISPLAY_PULSE_TIMER_LEAD_DEADBAND_MS) {
            this.displayPulseTimerLeadMs *= 1 - DISPLAY_PULSE_TIMER_LEAD_GAIN;
            if (this.displayPulseTimerLeadMs < 0.05) {
                this.displayPulseTimerLeadMs = 0;
            }
            return;
        }
        const nextLeadMs = this.displayPulseTimerLeadMs + errorMs * DISPLAY_PULSE_TIMER_LEAD_GAIN;
        this.displayPulseTimerLeadMs = Math.max(0, Math.min(DISPLAY_PULSE_TIMER_LEAD_MAX_MS, nextLeadMs));
    }
    wakeHostEvent(key, delayMs) {
        try {
            this.hostTimers.delete(key);
            if (this.stopped || !this.vm)
                return;
            this.wakeCount += 1;
            this.debugFrameStatus(`render island wake #${this.wakeCount} delay=${delayMs}`, this.wakeCount);
            const vmRunStartMs = performance.now();
            this.vm.wakeHostEvent(key);
            this.vm.runScheduled();
            this.recordVmWakeRun(performance.now() - vmRunStartMs);
            this.flush();
            this.scheduleHostEvents();
        }
        catch (error) {
            this.fail(`wakeHostEvent key=${key}`, error);
        }
    }
    fail(context, error) {
        let message = this.normalizeVmPanic(`${context}: ${this.describeError(error)}`);
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
            let message = `${error.name}: ${this.normalizeVmPanic(error.message)}`;
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
            return this.normalizeVmPanic(value);
        if (typeof value === "number" || typeof value === "boolean" || value === null || value === undefined) {
            return String(value);
        }
        try {
            return this.normalizeVmPanic(JSON.stringify(value));
        }
        catch {
            return this.normalizeVmPanic(String(value));
        }
    }
    normalizeVmPanic(message) {
        const panicMatch = message.match(/PanicUnwound \{ msg: Some\("((?:\\.|[^"\\])*)"\)/);
        if (!panicMatch) {
            return message;
        }
        try {
            return JSON.parse(`"${panicMatch[1]}"`);
        }
        catch {
            return panicMatch[1];
        }
    }
    debug(message) {
        this.config.debugLog?.(message);
    }
    debugFrameStatus(message, count) {
        if (count <= 4 || count % 60 === 0) {
            this.debug(message);
        }
    }
    perfEnabled() {
        return isVoplayPerfEnabled(this.perfMode) || locationHasParam("voplayPerf") || locationHasParam("perf") || locationHasParam("voplayPerfOverlay");
    }
    recordDisplayPulseWait(waitMs) {
        if (!this.perfEnabled())
            return;
        this.displayPulseWaitWindow.push(waitMs);
        if (this.displayPulseWaitWindow.length >= PERF_SAMPLE_WINDOW) {
            this.reportDisplayPulseWindow();
        }
    }
    recordVmWakeRun(runMs) {
        if (!this.perfEnabled())
            return;
        this.vmWakeRunWindow.push(runMs);
        if (this.vmWakeRunWindow.length > PERF_SAMPLE_WINDOW) {
            this.vmWakeRunWindow.shift();
        }
    }
    reportDisplayPulseWindow() {
        const shouldReport = shouldPostPerfReports(this.perfMode);
        const shouldShow = shouldShowBootstrapPerfOverlay(this.perfMode);
        const shouldLog = shouldLogPerfReports(this.perfMode);
        const pulseSamples = this.displayPulseWaitWindow;
        const wakeRunSamples = this.vmWakeRunWindow;
        const rafWakes = this.displayPulseRafWakeWindow;
        const timerWakes = this.displayPulseTimerWakeWindow;
        const timerLeadMs = this.displayPulseTimerLeadMs;
        this.displayPulseWaitWindow = [];
        this.vmWakeRunWindow = [];
        this.displayPulseRafWakeWindow = 0;
        this.displayPulseTimerWakeWindow = 0;
        if (!shouldReport && !shouldShow) {
            return;
        }
        window.setTimeout(() => {
            if (this.stopped)
                return;
            this.emitDisplayPulseWindow(pulseSamples, wakeRunSamples, rafWakes, timerWakes, timerLeadMs, shouldReport, shouldShow, shouldLog);
        }, 0);
    }
    emitDisplayPulseWindow(pulseSamples, wakeRunSamples, rafWakes, timerWakes, timerLeadMs, shouldReport, shouldShow, shouldLog) {
        const pulse = this.measureWindow(pulseSamples);
        const wakeRun = this.measureWindow(wakeRunSamples);
        const slow120 = this.countAbove(pulseSamples, DISPLAY_PULSE_SLOW_120_MS);
        const slow60 = this.countAbove(pulseSamples, DISPLAY_PULSE_SLOW_60_MS);
        const message = `render island pulse window samples=${pulse.count}` +
            ` wait p50/p90/p99/max=${this.formatMs(pulse.p50)}/${this.formatMs(pulse.p90)}/${this.formatMs(pulse.p99)}/${this.formatMs(pulse.max)}` +
            ` mode=${this.displayPulseMode}` +
            ` timerLead=${this.formatMs(timerLeadMs)}` +
            ` source raf/timer=${rafWakes}/${timerWakes}` +
            ` slow120=${slow120}/${pulse.count}` +
            ` slow60=${slow60}/${pulse.count}` +
            ` wakeRun samples=${wakeRun.count}` +
            ` p50/p90/p99/max=${this.formatMs(wakeRun.p50)}/${this.formatMs(wakeRun.p90)}/${this.formatMs(wakeRun.p99)}/${this.formatMs(wakeRun.max)}` +
            ` visibility=${document.visibilityState}` +
            ` focus=${document.hasFocus()}` +
            ` timers=${this.hostTimers.size}`;
        if (shouldLog) {
            this.debug(message);
            console.info(`[voplay] ${message}`);
        }
        sendVoplayPerfReport("pulse", message, {
            samples: pulse.count,
            wait: pulse,
            pulseMode: this.displayPulseMode,
            timerLeadMs,
            rafWakes,
            timerWakes,
            slow120,
            slow60,
            wakeRunSamples: wakeRun.count,
            wakeRun,
            visibility: document.visibilityState,
            focus: document.hasFocus(),
            timers: this.hostTimers.size,
        });
        if (shouldShow) {
            this.pulseMessage = message;
            this.showPerfOverlay();
        }
    }
    showPerfOverlay() {
        if (!shouldShowBootstrapPerfOverlay(this.perfMode))
            return;
        let overlay = this.pulseOverlay;
        if (!overlay) {
            overlay = document.createElement("pre");
            overlay.id = "voplay-pulse-debug-overlay";
            overlay.style.position = "fixed";
            overlay.style.right = "16px";
            overlay.style.bottom = "48px";
            overlay.style.maxWidth = "520px";
            overlay.style.margin = "0";
            overlay.style.padding = "8px 10px";
            overlay.style.border = "1px solid rgba(255, 230, 96, 0.7)";
            overlay.style.borderRadius = "4px";
            overlay.style.background = "rgba(8, 15, 28, 0.82)";
            overlay.style.color = "#f7f1c2";
            overlay.style.font = "11px ui-monospace, SFMono-Regular, Menlo, monospace";
            overlay.style.lineHeight = "1.35";
            overlay.style.whiteSpace = "pre-wrap";
            overlay.style.pointerEvents = "none";
            overlay.style.zIndex = "2147483647";
            document.body.appendChild(overlay);
            this.pulseOverlay = overlay;
        }
        const lines = ["[voplay perf]"];
        if (this.pulseMessage) {
            lines.push("[pulse]", this.pulseMessage);
        }
        if (this.webGpuMessage) {
            lines.push("[webgpu]", this.webGpuMessage);
        }
        if (this.rendererDebugMessage) {
            lines.push("[renderer]", this.rendererDebugMessage);
        }
        overlay.textContent = lines.join("\n");
    }
    measureWindow(samples) {
        if (samples.length === 0) {
            return { count: 0, p50: 0, p90: 0, p99: 0, max: 0 };
        }
        const sorted = [...samples].sort((a, b) => a - b);
        return {
            count: sorted.length,
            p50: this.percentile(sorted, 0.5),
            p90: this.percentile(sorted, 0.9),
            p99: this.percentile(sorted, 0.99),
            max: sorted[sorted.length - 1],
        };
    }
    percentile(sorted, fraction) {
        if (sorted.length === 0)
            return 0;
        const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * fraction) - 1));
        return sorted[index];
    }
    countAbove(samples, threshold) {
        let count = 0;
        for (const sample of samples) {
            if (sample > threshold)
                count += 1;
        }
        return count;
    }
    formatMs(value) {
        return `${value.toFixed(2)}ms`;
    }
    installWebGpuPerfProbe() {
        if (!this.perfEnabled() || !shouldInstallWebGpuPerfProbe(this.perfMode))
            return;
        const globalScope = globalThis;
        this.webGpuReportCallback = (message) => {
            if (this.stopped)
                return;
            this.webGpuMessage = message;
            this.showPerfOverlay();
        };
        globalScope.__voplayWebGpuPerfReport = this.webGpuReportCallback;
        if (!globalScope.__voplayWebGpuPerfProbe) {
            globalScope.__voplayWebGpuPerfProbe = new WebGpuPerfProbe();
        }
        globalScope.__voplayWebGpuPerfProbe.install(globalScope);
    }
    installRendererDebugStatusProbe() {
        if (!shouldCaptureRendererDebugStatus())
            return;
        const globalScope = globalThis;
        this.rendererDebugStatusCallback = (message) => {
            if (this.stopped)
                return;
            this.rendererDebugMessage = message;
            sendVoplayPerfReport("renderer", message, { rendererDebug: true });
            this.showPerfOverlay();
        };
        globalScope.__voplayDebugStatus = this.rendererDebugStatusCallback;
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
