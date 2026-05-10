#!/usr/bin/env node

import fs from "node:fs";

const DEFAULT_LIMITS = {
  minPulseSamples: 0,
  maxSlow120: -1,
  maxSlow60: -1,
  maxPulseP99Ms: 0,
  maxWebGpuBacklogWindows: -1,
  maxGpuWorkP99Ms: 0,
  maxProbeOverheadMs: 0,
};

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help || !args.input) {
    printHelp();
    process.exit(args.help ? 0 : 2);
  }
  const reports = await readReports(args.input);
  const summary = summarizeReports(reports);
  const result = evaluateGate(summary, args.limits);
  const output = { passed: result.passed, failure: result.failure, message: result.message, summary };
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
  process.exit(result.passed ? 0 : 1);
}

function parseArgs(argv) {
  const limits = { ...DEFAULT_LIMITS };
  let input = "";
  let help = false;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "-h" || arg === "--help") {
      help = true;
      continue;
    }
    if (arg.startsWith("--")) {
      const eq = arg.indexOf("=");
      const key = eq >= 0 ? arg.slice(2, eq) : arg.slice(2);
      const raw = eq >= 0 ? arg.slice(eq + 1) : argv[++i];
      setLimit(limits, key, raw);
      continue;
    }
    if (!input) {
      input = arg;
      continue;
    }
    throw new Error(`unexpected argument: ${arg}`);
  }
  return { input, limits, help };
}

function setLimit(limits, key, raw) {
  const value = Number(raw);
  if (!Number.isFinite(value)) {
    throw new Error(`invalid numeric value for --${key}: ${raw}`);
  }
  if (key === "min-pulse-samples") limits.minPulseSamples = value;
  else if (key === "max-slow120") limits.maxSlow120 = value;
  else if (key === "max-slow60") limits.maxSlow60 = value;
  else if (key === "max-pulse-p99-ms") limits.maxPulseP99Ms = value;
  else if (key === "max-webgpu-backlog-windows") limits.maxWebGpuBacklogWindows = value;
  else if (key === "max-gpu-work-p99-ms") limits.maxGpuWorkP99Ms = value;
  else if (key === "max-probe-overhead-ms") limits.maxProbeOverheadMs = value;
  else throw new Error(`unknown option: --${key}`);
}

async function readReports(input) {
  const text = await readInputText(input);
  const trimmed = text.trim();
  if (!trimmed) return [];
  try {
    const parsed = JSON.parse(trimmed);
    return normalizeReports(parsed);
  } catch {
    const reports = [];
    for (const line of trimmed.split(/\r?\n/)) {
      const item = line.trim();
      if (!item) continue;
      reports.push(...normalizeReports(JSON.parse(item)));
    }
    return reports;
  }
}

async function readInputText(input) {
  if (input === "-") return fs.readFileSync(0, "utf8");
  if (isHttpUrl(input)) {
    const response = await fetch(input);
    if (!response.ok) {
      throw new Error(`failed to fetch ${input}: HTTP ${response.status}`);
    }
    return response.text();
  }
  return fs.readFileSync(input, "utf8");
}

function isHttpUrl(input) {
  return input.startsWith("http://") || input.startsWith("https://");
}

function normalizeReports(parsed) {
  if (Array.isArray(parsed)) return parsed.flatMap((item) => normalizeReports(item));
  if (Array.isArray(parsed?.reports)) return normalizeReports(parsed.reports);
  if (parsed?.payload) return normalizeReports(parsed.payload);
  return [parsed];
}

function summarizeReports(reports) {
  const summary = {
    schemaVersion: 1,
    reports: reports.length,
    pulseWindows: 0,
    pulseSamples: 0,
    pulseSlow120: 0,
    pulseSlow60: 0,
    pulseP99Ms: 0,
    pulseMaxMs: 0,
    webGpuWindows: 0,
    webGpuBacklogWindows: 0,
    webGpuSaturatedWindows: 0,
    gpuWorkP99Ms: 0,
    gpuWorkMaxMs: 0,
    probeOverheadMs: 0,
  };
  for (const report of reports) {
    if (report?.kind === "pulse") summarizePulse(summary, report.metrics ?? {});
    if (report?.kind === "webgpu") summarizeWebGpu(summary, report.metrics ?? {});
  }
  return summary;
}

function summarizePulse(summary, metrics) {
  const wait = metrics.wait ?? {};
  summary.pulseWindows += 1;
  summary.pulseSamples += number(metrics.samples);
  summary.pulseSlow120 += number(metrics.slow120);
  summary.pulseSlow60 += number(metrics.slow60);
  summary.pulseP99Ms = Math.max(summary.pulseP99Ms, number(wait.p99));
  summary.pulseMaxMs = Math.max(summary.pulseMaxMs, number(wait.max));
}

function summarizeWebGpu(summary, metrics) {
  const workDone = metrics.workDone ?? {};
  const probeCpu = metrics.probeCpu ?? {};
  const queueDepthClass = String(metrics.queueDepthClass ?? "");
  summary.webGpuWindows += 1;
  if (queueDepthClass === "backlogged" || queueDepthClass === "saturated") {
    summary.webGpuBacklogWindows += 1;
  }
  if (queueDepthClass === "saturated") {
    summary.webGpuSaturatedWindows += 1;
  }
  summary.gpuWorkP99Ms = Math.max(summary.gpuWorkP99Ms, number(workDone.p99));
  summary.gpuWorkMaxMs = Math.max(summary.gpuWorkMaxMs, number(workDone.max));
  summary.probeOverheadMs = Math.max(
    summary.probeOverheadMs,
    number(probeCpu.p99),
    number(metrics.reportOverheadMs),
  );
}

function evaluateGate(summary, limits) {
  if (limits.minPulseSamples > 0 && summary.pulseSamples < limits.minPulseSamples) {
    return fail("min_pulse_samples", `pulse samples ${summary.pulseSamples} below ${limits.minPulseSamples}`);
  }
  if (limits.maxSlow120 >= 0 && summary.pulseSlow120 > limits.maxSlow120) {
    return fail("slow120", `slow120 ${summary.pulseSlow120} exceeds ${limits.maxSlow120}`);
  }
  if (limits.maxSlow60 >= 0 && summary.pulseSlow60 > limits.maxSlow60) {
    return fail("slow60", `slow60 ${summary.pulseSlow60} exceeds ${limits.maxSlow60}`);
  }
  if (limits.maxPulseP99Ms > 0 && summary.pulseP99Ms > limits.maxPulseP99Ms) {
    return fail("pulse_p99", `pulse p99 ${formatMs(summary.pulseP99Ms)} exceeds ${formatMs(limits.maxPulseP99Ms)}`);
  }
  if (limits.maxWebGpuBacklogWindows >= 0 && summary.webGpuBacklogWindows > limits.maxWebGpuBacklogWindows) {
    return fail("webgpu_backlog", `webgpu backlog windows ${summary.webGpuBacklogWindows} exceed ${limits.maxWebGpuBacklogWindows}`);
  }
  if (limits.maxGpuWorkP99Ms > 0 && summary.gpuWorkP99Ms > limits.maxGpuWorkP99Ms) {
    return fail("gpu_work_p99", `gpu work p99 ${formatMs(summary.gpuWorkP99Ms)} exceeds ${formatMs(limits.maxGpuWorkP99Ms)}`);
  }
  if (limits.maxProbeOverheadMs > 0 && summary.probeOverheadMs > limits.maxProbeOverheadMs) {
    return fail("probe_overhead", `probe overhead ${formatMs(summary.probeOverheadMs)} exceeds ${formatMs(limits.maxProbeOverheadMs)}`);
  }
  return { passed: true, failure: "", message: "perf gate passed" };
}

function fail(failure, message) {
  return { passed: false, failure, message };
}

function number(value) {
  return Number.isFinite(Number(value)) ? Number(value) : 0;
}

function formatMs(value) {
  return `${value.toFixed(2)}ms`;
}

function printHelp() {
  process.stdout.write(`Usage: node tools/perf_gate.mjs <reports.json|jsonl|url|-> [options]

Reads voplay /__voplay_perf_report payloads as endpoint JSON, JSON array, single JSON, JSONL, URL, or stdin.

Options:
  --min-pulse-samples N
  --max-slow120 N
  --max-slow60 N
  --max-pulse-p99-ms MS
  --max-webgpu-backlog-windows N
  --max-gpu-work-p99-ms MS
  --max-probe-overhead-ms MS

Use -1 for count limits to disable that check.
`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 2;
});
