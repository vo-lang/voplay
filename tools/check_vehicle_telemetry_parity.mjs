import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(root);
const vo = process.env.VO_BIN ?? join(dirname(root), 'volang', 'target', 'debug', 'vo');
const studioWasmDir = join(repoRoot, 'volang', 'studio', 'wasm', 'pkg');

function runNativeProbe() {
  const result = spawnSync(vo, ['run', 'tools/vehicle_telemetry_parity.vo'], {
    cwd: root,
    encoding: 'utf8',
  });
  const output = `${result.stdout ?? ''}${result.stderr ?? ''}`;
  if (result.status !== 0) {
    throw new Error(`native parity probe failed:\n${output}`);
  }
  return parseProbeOutput(output, 'native');
}

function parseProbeOutput(output, label) {
  const line = output.split(/\r?\n/).find((item) => item.startsWith('VO:PARITY PASS '));
  if (!line) {
    throw new Error(`${label} parity probe did not emit VO:PARITY PASS:\n${output}`);
  }
  const wheel = /wheel_count=(\d+)/.exec(line);
  const contacts = /contacts=(\d+)/.exec(line);
  if (!wheel || !contacts) {
    throw new Error(`${label} parity probe emitted malformed summary: ${line}`);
  }
  return {
    label,
    wheelCount: Number(wheel[1]),
    contacts: Number(contacts[1]),
    line,
  };
}

function assertNativeBaseline(nativeResult) {
  if (nativeResult.wheelCount !== 4) {
    throw new Error(`native wheel_count mismatch: ${nativeResult.wheelCount}`);
  }
  if (nativeResult.contacts <= 0) {
    throw new Error(`native contacts mismatch: ${nativeResult.contacts}`);
  }
}

const textDecoder = new TextDecoder();
const textEncoder = new TextEncoder();
const vfsFiles = new Map();
const vfsModes = new Map();
const vfsDirs = new Map([['/', 0o755]]);
let nextFd = 3;
const openFiles = new Map();
const extBindgenModules = new Map();

function normalizeVfsPath(path) {
  const raw = String(path || '/').replace(/\\/g, '/');
  const absolute = raw.startsWith('/') ? raw : `/${raw}`;
  const out = [];
  for (const part of absolute.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') out.pop();
    else out.push(part);
  }
  return `/${out.join('/')}`;
}

function parentDir(path) {
  const normalized = normalizeVfsPath(path);
  if (normalized === '/') return '/';
  const idx = normalized.lastIndexOf('/');
  return idx <= 0 ? '/' : normalized.slice(0, idx);
}

function ensureDir(path, mode = 0o755) {
  const normalized = normalizeVfsPath(path);
  if (vfsFiles.has(normalized)) return 'not a directory';
  let current = '';
  for (const part of normalized.split('/')) {
    if (!part) continue;
    current += `/${part}`;
    if (!vfsDirs.has(current)) vfsDirs.set(current, mode);
  }
  return null;
}

function writeVfsFile(path, data, mode = 0o644) {
  const normalized = normalizeVfsPath(path);
  const parentError = ensureDir(parentDir(normalized));
  if (parentError) return parentError;
  const bytes = data instanceof Uint8Array ? new Uint8Array(data) : new Uint8Array(data);
  vfsFiles.set(normalized, bytes);
  vfsModes.set(normalized, mode);
  return null;
}

function readVfsFile(path) {
  const normalized = normalizeVfsPath(path);
  const bytes = vfsFiles.get(normalized);
  if (!bytes) return [null, 'file does not exist'];
  return [new Uint8Array(bytes), null];
}

function readVfsDir(path) {
  const dir = normalizeVfsPath(path);
  if (!vfsDirs.has(dir)) return [[], 'directory does not exist'];
  const prefix = dir === '/' ? '/' : `${dir}/`;
  const entries = new Map();
  for (const child of vfsDirs.keys()) {
    if (child === dir || !child.startsWith(prefix)) continue;
    const name = child.slice(prefix.length).split('/')[0];
    if (name) entries.set(name, [name, true, vfsDirs.get(`${prefix}${name}`) ?? 0o755]);
  }
  for (const child of vfsFiles.keys()) {
    if (!child.startsWith(prefix)) continue;
    const name = child.slice(prefix.length).split('/')[0];
    if (name && !entries.has(name)) entries.set(name, [name, false, vfsModes.get(`${prefix}${name}`) ?? 0o644]);
  }
  return [[...entries.values()].sort((a, b) => a[0].localeCompare(b[0])), null];
}

function statVfsPath(path) {
  const normalized = normalizeVfsPath(path);
  const name = normalized === '/' ? '/' : normalized.slice(normalized.lastIndexOf('/') + 1);
  if (vfsDirs.has(normalized)) return [name, 0, vfsDirs.get(normalized) ?? 0o755, 0, true, null];
  const bytes = vfsFiles.get(normalized);
  if (!bytes) return [name, 0, 0, 0, false, 'file does not exist'];
  return [name, bytes.length, vfsModes.get(normalized) ?? 0o644, 0, false, null];
}

function dataUrlFromBytes(bytes) {
  if (!bytes) return '';
  const text = textDecoder.decode(bytes);
  return `data:text/javascript;charset=utf-8,${encodeURIComponent(text)}`;
}

function removeVfsPath(path) {
  const normalized = normalizeVfsPath(path);
  vfsFiles.delete(normalized);
  vfsModes.delete(normalized);
  vfsDirs.delete(normalized);
  return null;
}

function installWindowVfs() {
  globalThis.window = globalThis;
  globalThis._vfsMkdirAll = (path, mode) => ensureDir(path, Number(mode));
  globalThis._vfsMkdir = (path, mode) => {
    const normalized = normalizeVfsPath(path);
    if (vfsDirs.has(normalized) || vfsFiles.has(normalized)) return 'file exists';
    const parentError = ensureDir(parentDir(normalized));
    if (parentError) return parentError;
    vfsDirs.set(normalized, Number(mode));
    return null;
  };
  globalThis._vfsReadFile = readVfsFile;
  globalThis._vfsWriteFile = (path, data, mode) => writeVfsFile(path, data, Number(mode));
  globalThis._vfsReadDir = readVfsDir;
  globalThis._vfsStat = statVfsPath;
  globalThis._vfsRemove = removeVfsPath;
  globalThis._vfsRemoveAll = (path) => {
    const normalized = normalizeVfsPath(path);
    for (const key of [...vfsFiles.keys()]) {
      if (key === normalized || key.startsWith(`${normalized}/`)) {
        vfsFiles.delete(key);
        vfsModes.delete(key);
      }
    }
    for (const key of [...vfsDirs.keys()]) {
      if (key !== '/' && (key === normalized || key.startsWith(`${normalized}/`))) vfsDirs.delete(key);
    }
    return null;
  };
  globalThis._vfsRename = (oldPath, newPath) => {
    const oldNorm = normalizeVfsPath(oldPath);
    const newNorm = normalizeVfsPath(newPath);
    const bytes = vfsFiles.get(oldNorm);
    if (!bytes) return 'file does not exist';
    writeVfsFile(newNorm, bytes, vfsModes.get(oldNorm) ?? 0o644);
    removeVfsPath(oldNorm);
    return null;
  };
  globalThis._vfsChmod = (path, mode) => {
    const normalized = normalizeVfsPath(path);
    if (vfsDirs.has(normalized)) vfsDirs.set(normalized, Number(mode));
    else if (vfsFiles.has(normalized)) vfsModes.set(normalized, Number(mode));
    else return 'file does not exist';
    return null;
  };
  globalThis._vfsTruncate = (path, size) => {
    const normalized = normalizeVfsPath(path);
    const existing = vfsFiles.get(normalized);
    if (!existing) return 'file does not exist';
    const next = new Uint8Array(Number(size));
    next.set(existing.slice(0, Number(size)));
    vfsFiles.set(normalized, next);
    return null;
  };
  globalThis._vfsOpenFile = (path, _flags, mode) => {
    const normalized = normalizeVfsPath(path);
    if (!vfsFiles.has(normalized)) writeVfsFile(normalized, new Uint8Array(), Number(mode));
    const fd = nextFd++;
    openFiles.set(fd, { path: normalized, offset: 0 });
    return [fd, null];
  };
  globalThis._vfsRead = (fd, length) => {
    const file = openFiles.get(Number(fd));
    if (!file) return [null, 'bad file descriptor'];
    const bytes = vfsFiles.get(file.path) ?? new Uint8Array();
    const chunk = bytes.slice(file.offset, file.offset + Number(length));
    file.offset += chunk.length;
    return [chunk, null];
  };
  globalThis._vfsWrite = (fd, data) => {
    const file = openFiles.get(Number(fd));
    if (!file) return [0, 'bad file descriptor'];
    const bytes = vfsFiles.get(file.path) ?? new Uint8Array();
    const next = new Uint8Array(Math.max(bytes.length, file.offset + data.length));
    next.set(bytes);
    next.set(data, file.offset);
    file.offset += data.length;
    vfsFiles.set(file.path, next);
    return [data.length, null];
  };
  globalThis._vfsReadAt = (fd, length, offset) => {
    const file = openFiles.get(Number(fd));
    if (!file) return [null, 'bad file descriptor'];
    const bytes = vfsFiles.get(file.path) ?? new Uint8Array();
    return [bytes.slice(Number(offset), Number(offset) + Number(length)), null];
  };
  globalThis._vfsWriteAt = (fd, data, offset) => {
    const file = openFiles.get(Number(fd));
    if (!file) return [0, 'bad file descriptor'];
    const bytes = vfsFiles.get(file.path) ?? new Uint8Array();
    const start = Number(offset);
    const next = new Uint8Array(Math.max(bytes.length, start + data.length));
    next.set(bytes);
    next.set(data, start);
    vfsFiles.set(file.path, next);
    return [data.length, null];
  };
  globalThis._vfsSeek = (fd, offset, whence) => {
    const file = openFiles.get(Number(fd));
    if (!file) return [0, 'bad file descriptor'];
    const bytes = vfsFiles.get(file.path) ?? new Uint8Array();
    if (Number(whence) === 0) file.offset = Number(offset);
    else if (Number(whence) === 1) file.offset += Number(offset);
    else file.offset = bytes.length + Number(offset);
    return [file.offset, null];
  };
  globalThis._vfsClose = (fd) => {
    openFiles.delete(Number(fd));
    return null;
  };
  globalThis._vfsSync = () => null;
  globalThis._vfsFtruncate = (fd, size) => {
    const file = openFiles.get(Number(fd));
    return file ? globalThis._vfsTruncate(file.path, size) : 'bad file descriptor';
  };
  globalThis._vfsFstat = (fd) => {
    const file = openFiles.get(Number(fd));
    if (!file) return ['', 0, 0, 0, false, 'bad file descriptor'];
    const [, size, mode, mtime, isDir, err] = statVfsPath(file.path);
    return [size, mode, mtime, isDir, err];
  };

  globalThis.voSetupExtModule = async (key, bytes, jsGlueUrl = '') => {
    if (!jsGlueUrl) {
      extBindgenModules.set(key, {});
      return;
    }
    const glue = await import(jsGlueUrl);
    await glue.default({ module_or_path: bytes.slice() });
    if (typeof glue.__voInit === 'function') await glue.__voInit();
    extBindgenModules.set(key, glue);
  };
  globalThis.voRegisterExtModuleAlias = (existingKey, aliasKey) => {
    if (extBindgenModules.has(existingKey)) extBindgenModules.set(aliasKey, extBindgenModules.get(existingKey));
  };
  globalThis.voDisposeExtModule = (key) => {
    extBindgenModules.delete(key);
  };
  globalThis.voDisposeAllExtModules = () => {
    extBindgenModules.clear();
  };
  globalThis.voCallExt = (externName, input) => {
    let matchedKey = '';
    let matchedModule = null;
    for (const [key, mod] of extBindgenModules) {
      if (externName.startsWith(key) && key.length > matchedKey.length) {
        matchedKey = key;
        matchedModule = mod;
      }
    }
    if (!matchedModule) {
      throw new Error(`[voCallExt] No loaded module for extern: ${externName}; bindgen=[${[...extBindgenModules.keys()].join(',')}]`);
    }
    const funcName = externName.substring(matchedKey.length + 1);
    const func = matchedModule[funcName];
    if (typeof func !== 'function') {
      throw new Error(`[voCallExt] Bindgen export not found: ${funcName} in module: ${matchedKey}`);
    }
    const result = func(input);
    if (result instanceof Uint8Array) return result;
    if (typeof result === 'string') return textEncoder.encode(result);
    throw new Error(`[voCallExt] Unsupported bindgen return for ${externName}: ${typeof result}`);
  };
  globalThis.voCallExtReplay = (externName, resumeData) => globalThis.voCallExt(externName, resumeData);
}

function shouldIncludeFile(path) {
  return (
    path.endsWith('.vo') ||
    path.endsWith('/vo.mod') ||
    path.endsWith('/vo.work') ||
    path.endsWith('/vo.lock') ||
    path.endsWith('/vo.web.json') ||
    path.includes('/web-artifacts/')
  );
}

function addFsTreeToVfs(fsRoot) {
  if (!existsSync(fsRoot)) return;
  const walk = (dir) => {
    ensureDir(dir);
    for (const entry of readdirSync(dir)) {
      if (entry === '.git' || entry === 'node_modules' || entry === 'target' || entry === 'dist') continue;
      const full = join(dir, entry);
      const stat = statSync(full);
      if (stat.isDirectory()) {
        walk(full);
      } else if (shouldIncludeFile(full)) {
        writeVfsFile(full, readFileSync(full), 0o644);
      }
    }
  };
  walk(fsRoot);
}

async function runWebProbe() {
  installWindowVfs();
  addFsTreeToVfs(root);
  addFsTreeToVfs(join(repoRoot, 'vogui'));
  addFsTreeToVfs(join(repoRoot, 'vopack'));
  const probeSource = readFileSync(join(root, 'tools', 'vehicle_telemetry_parity.vo'));
  const webProjectRoot = join(repoRoot, '.vehicle-telemetry-parity-web');
  writeVfsFile(
    join(webProjectRoot, 'vo.mod'),
    new TextEncoder().encode(`module github.com/vo-lang/vehicle-telemetry-parity-web\n\nvo ^0.1.0\n\nrequire github.com/vo-lang/vogui v0.1.11\nrequire github.com/vo-lang/vopack v0.1.0\nrequire github.com/vo-lang/voplay v0.1.18\n`),
  );
  writeVfsFile(
    join(webProjectRoot, 'vo.work'),
    new TextEncoder().encode(`version = 1\n\n[[use]]\npath = "../vogui"\n\n[[use]]\npath = "../vopack"\n\n[[use]]\npath = "../voplay"\n`),
  );
  writeVfsFile(join(webProjectRoot, 'main.vo'), probeSource);

  const wasm = await import(join(studioWasmDir, 'vo_studio_wasm.js'));
  if (typeof wasm.default === 'function') {
    await wasm.default(readFileSync(join(studioWasmDir, 'vo_studio_wasm_bg.wasm')));
  }
  await wasm.initVFS();
  const entry = join(webProjectRoot, 'main.vo');
  await wasm.prepareEntry(entry, 'auto');
  const compileResult = wasm.compileGui(entry, 'auto');
  for (const ext of compileResult.wasmExtensions ?? []) {
    await wasm.preloadExtModule(ext.moduleKey, ext.wasmBytes, dataUrlFromBytes(ext.jsGlueBytes));
  }
  return parseProbeOutput(wasm.compileRunEntry(entry, 'auto'), 'web');
}

function assertParity(nativeResult, webResult) {
  if (webResult.wheelCount !== nativeResult.wheelCount) {
    throw new Error(`web wheel_count mismatch: native=${nativeResult.wheelCount} web=${webResult.wheelCount}`);
  }
  if (webResult.contacts !== nativeResult.contacts) {
    throw new Error(`web contacts mismatch: native=${nativeResult.contacts} web=${webResult.contacts}`);
  }
}

const nativeResult = runNativeProbe();
assertNativeBaseline(nativeResult);
const webResult = await runWebProbe();
assertParity(nativeResult, webResult);
console.log(`VO:PARITY_NATIVE ${nativeResult.line}`);
console.log(`VO:PARITY_WEB ${webResult.line}`);
