import { deflateSync, inflateSync } from 'node:zlib';
import { readFileSync } from 'node:fs';

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

function usage() {
  return [
    'Usage:',
    '  node tools/check_render_exposure.mjs web.png [native.png]',
    '  node tools/check_render_exposure.mjs --self-test',
    '',
    'Options:',
    '  --mean-delta=<n>   Maximum allowed mean luminance delta for two images (default 0.15)',
  ].join('\n');
}

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let i = 0; i < 8; i += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const typeBuffer = Buffer.from(type, 'ascii');
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  typeBuffer.copy(out, 4);
  data.copy(out, 8);
  out.writeUInt32BE(crc32(Buffer.concat([typeBuffer, data])), 8 + data.length);
  return out;
}

function encodeTestPng(width, height, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y += 1) {
    const rowStart = y * (stride + 1);
    raw[rowStart] = 0;
    rgba.copy(raw, rowStart + 1, y * stride, (y + 1) * stride);
  }
  return Buffer.concat([
    PNG_SIGNATURE,
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', deflateSync(raw)),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

function decodePng(buffer) {
  if (!buffer.subarray(0, 8).equals(PNG_SIGNATURE)) {
    throw new Error('not a PNG file');
  }
  let offset = 8;
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colorType = 0;
  let interlace = 0;
  const idat = [];

  while (offset < buffer.length) {
    const length = buffer.readUInt32BE(offset);
    const type = buffer.toString('ascii', offset + 4, offset + 8);
    const data = buffer.subarray(offset + 8, offset + 8 + length);
    offset += 12 + length;
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      interlace = data[12];
    } else if (type === 'IDAT') {
      idat.push(data);
    } else if (type === 'IEND') {
      break;
    }
  }

  if (bitDepth !== 8) throw new Error(`unsupported PNG bit depth ${bitDepth}; expected 8`);
  if (interlace !== 0) throw new Error('interlaced PNG is not supported');
  const channels = { 0: 1, 2: 3, 4: 2, 6: 4 }[colorType];
  if (!channels) throw new Error(`unsupported PNG color type ${colorType}`);

  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const pixels = Buffer.alloc(stride * height);
  let rawOffset = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = raw[rawOffset];
    rawOffset += 1;
    const row = pixels.subarray(y * stride, (y + 1) * stride);
    const prev = y === 0 ? null : pixels.subarray((y - 1) * stride, y * stride);
    raw.copy(row, 0, rawOffset, rawOffset + stride);
    rawOffset += stride;
    unfilter(row, prev, channels, filter);
  }

  return { width, height, colorType, channels, pixels };
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  if (pb <= pc) return b;
  return c;
}

function unfilter(row, prev, bpp, filter) {
  for (let i = 0; i < row.length; i += 1) {
    const left = i >= bpp ? row[i - bpp] : 0;
    const up = prev ? prev[i] : 0;
    const upLeft = prev && i >= bpp ? prev[i - bpp] : 0;
    let add = 0;
    if (filter === 1) add = left;
    else if (filter === 2) add = up;
    else if (filter === 3) add = Math.floor((left + up) / 2);
    else if (filter === 4) add = paeth(left, up, upLeft);
    else if (filter !== 0) throw new Error(`unsupported PNG filter ${filter}`);
    row[i] = (row[i] + add) & 0xff;
  }
}

function srgbToLinear(value) {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function luminanceStats(decoded) {
  const values = [];
  const { channels, colorType, pixels } = decoded;
  for (let i = 0; i < pixels.length; i += channels) {
    let r;
    let g;
    let b;
    let a = 255;
    if (colorType === 0 || colorType === 4) {
      r = pixels[i];
      g = pixels[i];
      b = pixels[i];
      if (colorType === 4) a = pixels[i + 1];
    } else {
      r = pixels[i];
      g = pixels[i + 1];
      b = pixels[i + 2];
      if (colorType === 6) a = pixels[i + 3];
    }
    if (a < 13) continue;
    values.push(
      0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b),
    );
  }
  if (values.length === 0) throw new Error('image has no opaque pixels');
  values.sort((a, b) => a - b);
  const sum = values.reduce((acc, value) => acc + value, 0);
  const pick = (p) => values[Math.min(values.length - 1, Math.floor((values.length - 1) * p))];
  const mean = sum / values.length;
  const darkClip = values.filter((value) => value <= 0.02).length / values.length;
  const brightClip = values.filter((value) => value >= 0.98).length / values.length;
  return {
    pixels: values.length,
    mean,
    p05: pick(0.05),
    p50: pick(0.5),
    p95: pick(0.95),
    darkClip,
    brightClip,
    exposureClass: exposureClass(mean, pick(0.05), pick(0.95)),
  };
}

function exposureClass(mean, p05, p95) {
  if (mean < 0.12 || p95 < 0.18) return 'underexposed';
  if (mean > 0.72 || p05 > 0.58) return 'overexposed';
  return 'balanced';
}

function formatStats(label, stats) {
  const fields = [
    `class=${stats.exposureClass}`,
    `mean=${stats.mean.toFixed(4)}`,
    `p05=${stats.p05.toFixed(4)}`,
    `p50=${stats.p50.toFixed(4)}`,
    `p95=${stats.p95.toFixed(4)}`,
    `dark_clip=${stats.darkClip.toFixed(4)}`,
    `bright_clip=${stats.brightClip.toFixed(4)}`,
  ];
  return `${label} ${fields.join(' ')}`;
}

function checkSingle(label, stats) {
  if (stats.exposureClass !== 'balanced') {
    throw new Error(`${label} screenshot is ${stats.exposureClass}: ${formatStats(label, stats)}`);
  }
  if (stats.darkClip > 0.3) {
    throw new Error(`${label} has too much near-black clipping: ${formatStats(label, stats)}`);
  }
  if (stats.brightClip > 0.2) {
    throw new Error(`${label} has too much near-white clipping: ${formatStats(label, stats)}`);
  }
}

function checkPair(aLabel, a, bLabel, b, maxMeanDelta) {
  checkSingle(aLabel, a);
  checkSingle(bLabel, b);
  if (a.exposureClass !== b.exposureClass) {
    throw new Error(`exposure class mismatch: ${aLabel}=${a.exposureClass} ${bLabel}=${b.exposureClass}`);
  }
  const delta = Math.abs(a.mean - b.mean);
  if (delta > maxMeanDelta) {
    throw new Error(`mean luminance delta ${delta.toFixed(4)} exceeds ${maxMeanDelta}`);
  }
}

function statsForPath(path) {
  return luminanceStats(decodePng(readFileSync(path)));
}

function selfTest() {
  const gray = Buffer.alloc(4 * 4 * 4);
  const bright = Buffer.alloc(4 * 4 * 4);
  for (let i = 0; i < gray.length; i += 4) {
    gray[i] = 128;
    gray[i + 1] = 128;
    gray[i + 2] = 128;
    gray[i + 3] = 255;
    bright[i] = 255;
    bright[i + 1] = 255;
    bright[i + 2] = 255;
    bright[i + 3] = 255;
  }
  const grayStats = luminanceStats(decodePng(encodeTestPng(4, 4, gray)));
  const brightStats = luminanceStats(decodePng(encodeTestPng(4, 4, bright)));
  checkSingle('gray', grayStats);
  let failed = false;
  try {
    checkPair('gray', grayStats, 'bright', brightStats, 0.15);
  } catch {
    failed = true;
  }
  if (!failed) throw new Error('self-test expected gray/bright pair to fail');
  console.log(`VO:EXPOSURE SELF_TEST PASS ${formatStats('gray', grayStats)}`);
}

function main() {
  const args = process.argv.slice(2);
  if (args.includes('--self-test')) {
    selfTest();
    return;
  }
  const maxMeanArg = args.find((arg) => arg.startsWith('--mean-delta='));
  const maxMeanDelta = maxMeanArg ? Number(maxMeanArg.slice('--mean-delta='.length)) : 0.15;
  const paths = args.filter((arg) => !arg.startsWith('--'));
  if (paths.length < 1 || paths.length > 2 || !Number.isFinite(maxMeanDelta)) {
    throw new Error(usage());
  }
  const first = statsForPath(paths[0]);
  if (paths.length === 1) {
    checkSingle('image', first);
    console.log(`VO:EXPOSURE PASS ${formatStats('image', first)}`);
    return;
  }
  const second = statsForPath(paths[1]);
  checkPair('web', first, 'native', second, maxMeanDelta);
  console.log(`VO:EXPOSURE PASS ${formatStats('web', first)} ${formatStats('native', second)} mean_delta=${Math.abs(first.mean - second.mean).toFixed(4)}`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
