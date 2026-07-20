import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(root);
const vo = process.env.VO_BIN ?? join(dirname(root), 'volang', 'target', 'debug', 'vo');
const studioWasmDir = join(repoRoot, 'volang', 'apps', 'studio', 'public', 'wasm');
const nativeProbe = join(root, 'tools', 'vehicle_telemetry_parity.vo');

function runNativeProbe() {
  const result = spawnSync(vo, ['run', nativeProbe], {
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
const externTextDecoder = new TextDecoder('utf-8', { fatal: true });
const textEncoder = new TextEncoder();
const vfsFiles = new Map();
const vfsModes = new Map();
const vfsDirs = new Map([['/', 0o755]]);
let vfsCwd = '/';
let nextFd = 3;
const openFiles = new Map();
const extBindgenModules = new Map();
const extStandaloneInstances = new Map();
const activeExtArtifacts = new Map();
const pendingExtLoads = new Map();
const extLoadLeases = new Map();
const extLoadHandleLeases = new WeakMap();
let nextExtArtifactGeneration = 0;
let nextExtLeaseGeneration = 0;

function bytesEqual(left, right) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return false;
  }
  return true;
}

function nextLifecycleGeneration(current, label) {
  if (!Number.isSafeInteger(current) || current < 0 || current >= Number.MAX_SAFE_INTEGER) {
    throw new Error(`${label} generation exhausted`);
  }
  return current + 1;
}

function allocateExtLease(key, artifactToken) {
  nextExtLeaseGeneration = nextLifecycleGeneration(nextExtLeaseGeneration, 'extension lease');
  const leaseToken = `telemetry-lease:${nextExtLeaseGeneration}`;
  extLoadLeases.set(leaseToken, { key, artifactToken });
  return leaseToken;
}

function extensionLoadHandle(key, artifactToken, leaseToken, ready) {
  const handle = Object.freeze({ artifactToken, leaseToken, ready });
  extLoadHandleLeases.set(handle, { key, artifactToken, leaseToken });
  return handle;
}

function sameExtArtifact(candidate, bytes, jsGlueUrl) {
  return candidate.jsGlueUrl === jsGlueUrl && bytesEqual(candidate.bytes, bytes);
}

function uniqueModuleImportUrl(jsGlueUrl, artifactToken) {
  const url = new URL(jsGlueUrl, import.meta.url);
  url.hash = `vo-load=${encodeURIComponent(artifactToken)}`;
  return url.href;
}

function wasmU32(value, label) {
  if (
    typeof value !== 'number' ||
    !Number.isInteger(value) ||
    value < -0x8000_0000 ||
    value > 0xffff_ffff
  ) {
    throw new Error(`${label} is outside the wasm32 i32/u32 domain`);
  }
  return value >>> 0;
}

function validateStandaloneRange(ptr, length, memoryBytes, label) {
  if (!Number.isInteger(length) || length < 0 || length > 0xffff_ffff) {
    throw new Error(`${label} length is outside the u32 domain`);
  }
  const end = ptr + length;
  if (end > 0x1_0000_0000 || end > memoryBytes) {
    throw new Error(`${label} range exceeds standalone memory`);
  }
}

function bestEffortDealloc(dealloc, ptr, size, label, errors) {
  try {
    dealloc(ptr, size);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    errors.push(`${label}: ${detail}`);
  }
}

function throwStandaloneFailure(externName, primaryError, cleanupErrors) {
  if (cleanupErrors.length === 0) {
    if (primaryError instanceof Error) throw primaryError;
    throw new Error(String(primaryError));
  }
  const primaryDetail = primaryError instanceof Error ? primaryError.message : String(primaryError);
  throw new Error(
    `standalone call ${externName} failed: ${primaryDetail}; cleanup failures: ${cleanupErrors.join('; ')}`,
    { cause: primaryError },
  );
}

function decodeCanonicalExternName(encoded) {
  const bytes = textEncoder.encode(encoded);
  if (bytes.length < 5 || String.fromCharCode(...bytes.subarray(0, 4)) !== 'vo1:') {
    throw new Error(`invalid canonical extern name: ${encoded}`);
  }
  const cursor = { value: 4 };
  const readLength = (field) => {
    const start = cursor.value;
    let value = 0;
    while (cursor.value < bytes.length && bytes[cursor.value] !== 0x3a) {
      const byte = bytes[cursor.value];
      if (byte < 0x30 || byte > 0x39) throw new Error(`extern ${field} length is invalid`);
      value = value * 10 + byte - 0x30;
      if (!Number.isSafeInteger(value)) throw new Error(`extern ${field} length overflow`);
      cursor.value += 1;
    }
    if (cursor.value === start || cursor.value >= bytes.length || value === 0) {
      throw new Error(`extern ${field} length is missing`);
    }
    if (bytes[start] === 0x30 && cursor.value - start > 1) {
      throw new Error(`extern ${field} length has a leading zero`);
    }
    cursor.value += 1;
    return value;
  };
  const packageLength = readLength('package');
  const packageEnd = cursor.value + packageLength;
  if (!Number.isSafeInteger(packageEnd) || packageEnd >= bytes.length || bytes[packageEnd] !== 0x3a) {
    throw new Error('extern package length does not match its payload');
  }
  const packageName = externTextDecoder.decode(bytes.subarray(cursor.value, packageEnd));
  cursor.value = packageEnd + 1;
  const functionLength = readLength('function');
  const functionEnd = cursor.value + functionLength;
  if (functionEnd !== bytes.length) throw new Error('extern function length does not match its payload');
  const functionName = externTextDecoder.decode(bytes.subarray(cursor.value, functionEnd));
  return { packageName, functionName };
}

function standaloneExportKey(encoded) {
  let key = '__vo_ext_';
  for (const byte of textEncoder.encode(encoded)) key += byte.toString(16).padStart(2, '0');
  return key;
}

function selectExtensionOwner(packageName) {
  let matched = null;
  const owners = new Set([...extBindgenModules.keys(), ...extStandaloneInstances.keys()]);
  for (const owner of owners) {
    if (
      (packageName === owner || packageName.startsWith(`${owner}/`)) &&
      (matched === null || owner.length > matched.length)
    ) {
      matched = owner;
    }
  }
  return matched;
}

function normalizeVfsPath(path) {
  const raw = String(path || '.').replace(/\\/g, '/');
  const absolute = raw.startsWith('/') ? raw : `${vfsCwd === '/' ? '' : vfsCwd}/${raw}`;
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
  vfsCwd = '/';
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
  globalThis._vfsReadFileLimited = (path, maxBytes) => {
    const limit = Number(maxBytes);
    if (!Number.isSafeInteger(limit) || limit < 0) return [null, 'invalid argument'];
    const [bytes, error] = readVfsFile(path);
    if (error) return [null, error];
    if (bytes.length > limit) return [null, 'file too large'];
    return [bytes, null];
  };
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
  globalThis._vfsRenameNoreplace = (oldPath, newPath) => {
    const normalized = normalizeVfsPath(newPath);
    if (vfsFiles.has(normalized) || vfsDirs.has(normalized)) return 'file exists';
    return globalThis._vfsRename(oldPath, newPath);
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
  globalThis._vfsGetwd = () => [vfsCwd, null];
  globalThis._vfsChdir = (path) => {
    const normalized = normalizeVfsPath(path);
    if (!vfsDirs.has(normalized)) return 'directory does not exist';
    vfsCwd = normalized;
    return null;
  };
  globalThis._vfsResolveGuestPath = (path) => [normalizeVfsPath(path), null];
  globalThis._vfsGuestGetwd = () => [vfsCwd, null];

  const disposeBindgenModule = (module, operation = 'voDisposeExtModule') => {
    if (typeof module?.__voDispose !== 'function') return;
    try {
      module.__voDispose();
    } catch (error) {
      console.error(`[${operation}] bindgen dispose failed:`, error);
    }
  };
  const disposePreparedExtension = (prepared) => {
    if (prepared?.mode === 'bindgen') disposeBindgenModule(prepared.module, 'voAbortExtModuleLoad');
  };
  const removeExtLeases = (key, artifactToken) => {
    for (const [leaseToken, lease] of extLoadLeases) {
      if (lease.key === key && (artifactToken === undefined || lease.artifactToken === artifactToken)) {
        extLoadLeases.delete(leaseToken);
      }
    }
  };
  const cancelPendingExtLoad = (key, artifactToken) => {
    const pending = pendingExtLoads.get(key);
    if (!pending || (artifactToken !== undefined && pending.artifactToken !== artifactToken)) return false;
    pending.aborted = true;
    pendingExtLoads.delete(key);
    removeExtLeases(key, pending.artifactToken);
    const prepared = pending.prepared;
    pending.prepared = null;
    disposePreparedExtension(prepared);
    return true;
  };
  const abortExtLoad = (key, artifactToken, leaseToken) => {
    const lease = extLoadLeases.get(leaseToken);
    if (lease?.key !== key || lease.artifactToken !== artifactToken) return;
    extLoadLeases.delete(leaseToken);
    if (activeExtArtifacts.get(key)?.artifactToken === artifactToken) return;
    const pending = pendingExtLoads.get(key);
    if (!pending || pending.artifactToken !== artifactToken) return;
    const hasAnotherLease = [...extLoadLeases.values()].some(
      (candidate) => candidate.key === key && candidate.artifactToken === artifactToken,
    );
    if (!hasAnotherLease) cancelPendingExtLoad(key, artifactToken);
  };
  globalThis.voSetupExtModule = (key, bytes, jsGlueUrl = '') => {
    if (typeof key !== 'string' || key.length === 0) throw new Error('extension owner must be non-empty');
    if (!(bytes instanceof Uint8Array)) throw new Error(`extension '${key}' bytes must be a Uint8Array`);
    if (typeof jsGlueUrl !== 'string') throw new Error(`extension '${key}' glue URL must be a string`);
    const moduleBytes = bytes.slice();
    const active = activeExtArtifacts.get(key);
    if (active) {
      const bindgenLoaded = extBindgenModules.has(key) && !extStandaloneInstances.has(key);
      const standaloneLoaded = extStandaloneInstances.has(key) && !extBindgenModules.has(key);
      if (bindgenLoaded === standaloneLoaded) {
        throw new Error(`extension '${key}' has inconsistent active state`);
      }
      if (!sameExtArtifact(active, moduleBytes, jsGlueUrl)) {
        throw new Error(`extension '${key}' is already loaded with a different artifact`);
      }
      const leaseToken = allocateExtLease(key, active.artifactToken);
      return extensionLoadHandle(key, active.artifactToken, leaseToken, Promise.resolve());
    }
    const existingPending = pendingExtLoads.get(key);
    if (existingPending) {
      if (!sameExtArtifact(existingPending, moduleBytes, jsGlueUrl)) {
        throw new Error(`extension '${key}' is already loading a different artifact`);
      }
      const leaseToken = allocateExtLease(key, existingPending.artifactToken);
      return extensionLoadHandle(key, existingPending.artifactToken, leaseToken, existingPending.ready);
    }
    if (extBindgenModules.has(key) || extStandaloneInstances.has(key)) {
      throw new Error(`extension '${key}' has untracked loaded state`);
    }
    nextExtArtifactGeneration = nextLifecycleGeneration(nextExtArtifactGeneration, 'extension artifact');
    const artifactToken = `telemetry-artifact:${nextExtArtifactGeneration}`;
    const leaseToken = allocateExtLease(key, artifactToken);
    const pending = {
      bytes: moduleBytes,
      jsGlueUrl,
      artifactToken,
      prepared: null,
      aborted: false,
      ready: null,
    };
    pendingExtLoads.set(key, pending);
    pending.ready = (async () => {
      if (jsGlueUrl) {
        const glue = await import(uniqueModuleImportUrl(jsGlueUrl, artifactToken));
        try {
          if (pending.aborted || pendingExtLoads.get(key) !== pending) {
            throw new Error(`extension '${key}' load was aborted`);
          }
          if (typeof glue.default !== 'function') {
            throw new Error(`extension '${key}' bindgen glue has no default initializer`);
          }
          const initialized = await glue.default({ module_or_path: moduleBytes.slice() });
          const version = initialized?.vo_ext_protocol_version;
          if (typeof version !== 'function' || version() !== 3) {
            throw new Error(`extension '${key}' bindgen module does not implement protocol v3`);
          }
          if (typeof glue.__voInit === 'function') await glue.__voInit();
          if (pending.aborted || pendingExtLoads.get(key) !== pending) {
            throw new Error(`extension '${key}' load was aborted`);
          }
          pending.prepared = { mode: 'bindgen', module: glue };
        } catch (error) {
          disposeBindgenModule(glue);
          throw error;
        }
      } else {
        const module = await WebAssembly.compile(moduleBytes.slice());
        const imports = {};
        for (const descriptor of WebAssembly.Module.imports(module)) {
          if (descriptor.kind !== 'function') {
            throw new Error(`extension '${key}' has unsupported ${descriptor.kind} import '${descriptor.module}.${descriptor.name}'`);
          }
          imports[descriptor.module] ??= {};
          imports[descriptor.module][descriptor.name] = () => 0;
        }
        const instance = await WebAssembly.instantiate(module, imports);
        const version = instance.exports.vo_ext_protocol_version;
        if (typeof version !== 'function' || version() !== 3) {
          throw new Error(`extension '${key}' standalone module does not implement protocol v3`);
        }
        if (pending.aborted || pendingExtLoads.get(key) !== pending) {
          throw new Error(`extension '${key}' load was aborted`);
        }
        pending.prepared = { mode: 'standalone', instance };
      }
    })();
    void pending.ready.catch(() => {
      if (pendingExtLoads.get(key) === pending) {
        cancelPendingExtLoad(key, artifactToken);
      }
    });
    return extensionLoadHandle(key, artifactToken, leaseToken, pending.ready);
  };
  globalThis.voIsExtModuleLoadCurrent = (key, artifactToken) => {
    const pending = pendingExtLoads.get(key);
    if (pending?.artifactToken === artifactToken) return pending.prepared !== null && !pending.aborted;
    return (
      activeExtArtifacts.get(key)?.artifactToken === artifactToken &&
      (extBindgenModules.has(key) !== extStandaloneInstances.has(key))
    );
  };
  globalThis.voCommitExtModule = (key, artifactToken, leaseToken) => {
    const lease = extLoadLeases.get(leaseToken);
    if (lease?.key !== key || lease.artifactToken !== artifactToken) return false;
    const active = activeExtArtifacts.get(key);
    if (active) {
      const bindgenLoaded = extBindgenModules.has(key) && !extStandaloneInstances.has(key);
      const standaloneLoaded = extStandaloneInstances.has(key) && !extBindgenModules.has(key);
      if (active.artifactToken !== artifactToken || bindgenLoaded === standaloneLoaded) return false;
      extLoadLeases.delete(leaseToken);
      return true;
    }
    const pending = pendingExtLoads.get(key);
    if (
      !pending ||
      pending.artifactToken !== artifactToken ||
      pending.aborted ||
      pending.prepared === null
    ) {
      return false;
    }
    const prepared = pending.prepared;
    pending.prepared = null;
    try {
      if (prepared.mode === 'bindgen') {
        extBindgenModules.set(key, prepared.module);
      } else {
        extStandaloneInstances.set(key, prepared.instance);
      }
      activeExtArtifacts.set(key, {
        artifactToken,
        bytes: pending.bytes,
        jsGlueUrl: pending.jsGlueUrl,
      });
      pendingExtLoads.delete(key);
      extLoadLeases.delete(leaseToken);
      return true;
    } catch (error) {
      extBindgenModules.delete(key);
      extStandaloneInstances.delete(key);
      activeExtArtifacts.delete(key);
      pendingExtLoads.delete(key);
      pending.aborted = true;
      removeExtLeases(key, artifactToken);
      disposePreparedExtension(prepared);
      throw error;
    }
  };
  globalThis.voAbortExtModuleLoad = abortExtLoad;
  globalThis.voAbortExtModuleLoadHandle = (handle) => {
    const lease = extLoadHandleLeases.get(handle);
    if (lease) abortExtLoad(lease.key, lease.artifactToken, lease.leaseToken);
  };
  globalThis.voRegisterExtModuleAlias = (existingKey, aliasKey) => {
    const sourceBindgen = extBindgenModules.has(existingKey);
    const sourceStandalone = extStandaloneInstances.has(existingKey);
    const aliasHasLease = [...extLoadLeases.values()].some((lease) => lease.key === aliasKey);
    if (
      extBindgenModules.has(aliasKey) ||
      extStandaloneInstances.has(aliasKey) ||
      activeExtArtifacts.has(aliasKey) ||
      pendingExtLoads.has(aliasKey) ||
      aliasHasLease ||
      !activeExtArtifacts.has(existingKey) ||
      sourceBindgen === sourceStandalone
    ) {
      return;
    }
    try {
      if (sourceBindgen) {
        extBindgenModules.set(aliasKey, extBindgenModules.get(existingKey));
      } else {
        extStandaloneInstances.set(aliasKey, extStandaloneInstances.get(existingKey));
      }
      activeExtArtifacts.set(aliasKey, activeExtArtifacts.get(existingKey));
    } catch (error) {
      extBindgenModules.delete(aliasKey);
      extStandaloneInstances.delete(aliasKey);
      activeExtArtifacts.delete(aliasKey);
      throw error;
    }
  };
  globalThis.voDisposeExtModule = (key) => {
    cancelPendingExtLoad(key);
    const bindgen = extBindgenModules.get(key);
    extBindgenModules.delete(key);
    extStandaloneInstances.delete(key);
    activeExtArtifacts.delete(key);
    removeExtLeases(key);
    if (bindgen && ![...extBindgenModules.values()].includes(bindgen)) {
      disposeBindgenModule(bindgen);
    }
  };
  globalThis.voDisposeAllExtModules = () => {
    for (const key of [...pendingExtLoads.keys()]) cancelPendingExtLoad(key);
    const bindgenModules = new Set(extBindgenModules.values());
    extBindgenModules.clear();
    extStandaloneInstances.clear();
    activeExtArtifacts.clear();
    extLoadLeases.clear();
    for (const module of bindgenModules) disposeBindgenModule(module, 'voDisposeAllExtModules');
  };
  globalThis.voCallExt = (externName, input) => {
    if (!(input instanceof Uint8Array)) throw new Error('[voCallExt] Input must be a Uint8Array');
    const decoded = decodeCanonicalExternName(externName);
    const matchedKey = selectExtensionOwner(decoded.packageName);
    if (matchedKey === null) throw new Error(`[voCallExt] No loaded module owns ${decoded.packageName}`);

    const bindgen = extBindgenModules.get(matchedKey);
    if (bindgen) {
      const exportKey = standaloneExportKey(externName);
      const func = bindgen[exportKey];
      if (typeof func !== 'function') {
        throw new Error(`[voCallExt] Bindgen export not found: ${exportKey} in ${matchedKey}`);
      }
      const result = func(input);
      if (result instanceof Promise) throw new Error(`[voCallExt] Async bindgen export is unsupported: ${externName}`);
      if (result instanceof Uint8Array) return result;
      throw new Error(`[voCallExt] Unsupported bindgen return for ${externName}: ${typeof result}`);
    }

    const instance = extStandaloneInstances.get(matchedKey);
    if (!instance) throw new Error(`[voCallExt] Extension ${matchedKey} has no callable instance`);
    const func = instance.exports[standaloneExportKey(externName)];
    const alloc = instance.exports.vo_alloc;
    const dealloc = instance.exports.vo_dealloc;
    const memory = instance.exports.memory;
    if (typeof func !== 'function' || typeof alloc !== 'function' || typeof dealloc !== 'function') {
      throw new Error(`[voCallExt] Standalone ABI exports are incomplete for ${externName}`);
    }
    if (!(memory instanceof WebAssembly.Memory)) {
      throw new Error(`[voCallExt] Standalone memory export is missing for ${matchedKey}`);
    }
    let inputPtr = 0;
    let outLengthPtr = 0;
    let outputPtr = 0;
    let outputLength = 0;
    let inputOwned = false;
    let outLengthOwned = false;
    let outputOwned = false;
    let result;
    let callFailed = false;
    let primaryError;
    const cleanupErrors = [];
    try {
      if (input.length > 0xffff_ffff) throw new Error('standalone input exceeds the u32 length domain');
      if (input.length > 0) {
        inputPtr = wasmU32(alloc(input.length), 'input pointer');
        if (inputPtr === 0) throw new Error('standalone input allocation failed');
        validateStandaloneRange(inputPtr, input.length, memory.buffer.byteLength, 'input');
        inputOwned = true;
        new Uint8Array(memory.buffer, inputPtr, input.length).set(input);
      }
      outLengthPtr = wasmU32(alloc(4), 'output-length pointer');
      if (outLengthPtr === 0) throw new Error('standalone output-length allocation failed');
      validateStandaloneRange(outLengthPtr, 4, memory.buffer.byteLength, 'output length');
      if (input.length > 0 && outLengthPtr < inputPtr + input.length && inputPtr < outLengthPtr + 4) {
        throw new Error('standalone allocator returned overlapping bridge allocations');
      }
      outLengthOwned = true;
      outputPtr = wasmU32(func(inputPtr, input.length, outLengthPtr), 'output pointer');
      outputLength = new DataView(memory.buffer, outLengthPtr, 4).getUint32(0, true);
      if (outputPtr === 0 && outputLength !== 0) throw new Error('null output has a non-zero length');
      validateStandaloneRange(outputPtr, outputLength, memory.buffer.byteLength, 'output');
      const outputEnd = outputPtr + outputLength;
      const inputEnd = inputPtr + input.length;
      if (
        (outputPtr === inputPtr && (outputLength !== 0 || input.length !== 0)) ||
        outputPtr === outLengthPtr ||
        (outputLength > 0 && input.length > 0 && outputPtr < inputEnd && inputPtr < outputEnd) ||
        (outputLength > 0 && outputPtr < outLengthPtr + 4 && outLengthPtr < outputEnd)
      ) {
        throw new Error('standalone output overlaps bridge-owned metadata');
      }
      outputOwned = outputPtr !== 0;
      result = new Uint8Array(memory.buffer, outputPtr, outputLength).slice();
    } catch (error) {
      callFailed = true;
      primaryError = error;
    } finally {
      if (outputOwned) bestEffortDealloc(dealloc, outputPtr, outputLength, 'output deallocation', cleanupErrors);
      if (outLengthOwned) bestEffortDealloc(dealloc, outLengthPtr, 4, 'output-length deallocation', cleanupErrors);
      if (inputOwned) bestEffortDealloc(dealloc, inputPtr, input.length, 'input deallocation', cleanupErrors);
    }
    if (callFailed) throwStandaloneFailure(externName, primaryError, cleanupErrors);
    if (cleanupErrors.length > 0) {
      throw new Error(`standalone call ${externName} cleanup failed: ${cleanupErrors.join('; ')}`);
    }
    return result;
  };
  globalThis.voCallExtReplay = (externName, resumeData) => globalThis.voCallExt(externName, resumeData);
}

function shouldIncludeFile(path) {
  return (
    path.endsWith('.vo') ||
    path.endsWith('/vo.mod') ||
    path.endsWith('/vo.work') ||
    path.endsWith('/vo.lock') ||
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
    new TextEncoder().encode(`format = 1\nmodule = "github.com/vo-lang/vehicle-telemetry-parity-web"\nversion = "0.1.0"\nvo = "0.1.0"\n\n[dependencies]\n"github.com/vo-lang/vogui" = "^0.1.0"\n"github.com/vo-lang/vopack" = "^0.1.0"\n"github.com/vo-lang/voplay" = "^0.1.0"\n`),
  );
  writeVfsFile(
    join(webProjectRoot, 'vo.work'),
    new TextEncoder().encode(`format = 1\nmembers = ["../vogui", "../vopack", "../voplay"]\n`),
  );
  writeVfsFile(join(webProjectRoot, 'main.vo'), probeSource);

  const wasm = await import(join(studioWasmDir, 'vo_studio_wasm.js'));
  if (typeof wasm.default === 'function') {
    await wasm.default({ module_or_path: readFileSync(join(studioWasmDir, 'vo_studio_wasm_bg.wasm')) });
  }
  await wasm.initVFS();
  const entry = join(webProjectRoot, 'main.vo');
  await wasm.prepareEntry(entry, 'auto');
  const compileResult = wasm.compileGui(entry, 'auto');
  for (const ext of compileResult.wasmExtensions ?? []) {
    await wasm.preloadExtModule(ext.moduleKey, ext.wasmBytes, dataUrlFromBytes(ext.jsGlueBytes));
  }
  const result = wasm.compileRunEntry(entry, 'auto');
  try {
    if (
      typeof result !== 'object' ||
      result === null ||
      typeof result.output !== 'string' ||
      !Number.isSafeInteger(result.exitCode)
    ) {
      throw new Error('web parity probe returned an invalid StudioRunResult');
    }
    if (result.exitCode !== 0) {
      throw new Error(`web parity probe exited with status ${result.exitCode}:\n${result.output}`);
    }
    return parseProbeOutput(result.output, 'web');
  } finally {
    if (typeof result?.free === 'function') result.free();
  }
}

async function runSelfTest() {
  installWindowVfs();

  ensureDir('/telemetry-selftest/work/child');
  assert.equal(globalThis._vfsChdir('/telemetry-selftest/work/child'), null);
  assert.deepEqual(globalThis._vfsGetwd(), ['/telemetry-selftest/work/child', null]);
  assert.deepEqual(globalThis._vfsGuestGetwd(), ['/telemetry-selftest/work/child', null]);
  assert.deepEqual(
    globalThis._vfsResolveGuestPath('../artifact.bin'),
    ['/telemetry-selftest/work/artifact.bin', null],
  );
  assert.equal(globalThis._vfsChdir('..'), null);
  assert.deepEqual(globalThis._vfsGetwd(), ['/telemetry-selftest/work', null]);
  installWindowVfs();
  assert.deepEqual(globalThis._vfsGetwd(), ['/', null]);

  assert.equal(wasmU32(-0x8000_0000, 'selftest pointer'), 0x8000_0000);
  assert.equal(wasmU32(-1, 'selftest pointer'), 0xffff_ffff);
  assert.equal(wasmU32(0xffff_ffff, 'selftest pointer'), 0xffff_ffff);
  assert.throws(() => wasmU32(-0x8000_0001, 'selftest pointer'), /wasm32/);

  const cleanupCalls = [];
  const cleanupErrors = [];
  const failingDealloc = (ptr, size) => {
    cleanupCalls.push([ptr, size]);
    if (ptr === 1) throw new Error('first cleanup failed');
  };
  bestEffortDealloc(failingDealloc, 1, 8, 'first', cleanupErrors);
  bestEffortDealloc(failingDealloc, 2, 4, 'second', cleanupErrors);
  assert.deepEqual(cleanupCalls, [[1, 8], [2, 4]]);
  assert.deepEqual(cleanupErrors, ['first: first cleanup failed']);
  const primaryError = new Error('primary failure');
  assert.throws(
    () => throwStandaloneFailure('selftest', primaryError, cleanupErrors),
    (error) => error.cause === primaryError && /primary failure/.test(error.message) && /first cleanup failed/.test(error.message),
  );

  const owner = 'github.com/vo-lang/telemetry-selftest';
  const functionName = 'Echo';
  const encodedExtern = `vo1:${textEncoder.encode(owner).length}:${owner}:${textEncoder.encode(functionName).length}:${functionName}`;
  const exportKey = standaloneExportKey(encodedExtern);
  const stateKey = '__voTelemetryParitySelfTestState';
  globalThis[stateKey] = { init: 0, asyncInit: 0, dispose: 0 };
  const glueSource = `
const state = globalThis[${JSON.stringify(stateKey)}];
let initialized;
export default async function init() {
  await Promise.resolve();
  if (initialized) return initialized;
  state.init += 1;
  initialized = { vo_ext_protocol_version() { return 3; } };
  return initialized;
}
export async function __voInit() { state.asyncInit += 1; }
export function __voDispose() { state.dispose += 1; }
export function ${exportKey}(input) { return input.slice(); }
`;
  const glueUrl = dataUrlFromBytes(textEncoder.encode(glueSource));
  const moduleBytes = new Uint8Array([0x00, 0x61, 0x73, 0x6d]);

  const failedOwner = `${owner}/failed-load`;
  const failingGlueUrl = dataUrlFromBytes(textEncoder.encode(`
export default async function init() { throw new Error('expected load failure'); }
`));
  const failedFirst = globalThis.voSetupExtModule(failedOwner, moduleBytes, failingGlueUrl);
  const failedConcurrent = globalThis.voSetupExtModule(failedOwner, moduleBytes, failingGlueUrl);
  assert.equal(failedFirst.ready, failedConcurrent.ready);
  await assert.rejects(failedFirst.ready, /expected load failure/);
  assert.equal(pendingExtLoads.has(failedOwner), false);
  assert.equal(
    [...extLoadLeases.values()].some((lease) => lease.key === failedOwner),
    false,
  );
  const failedRetry = globalThis.voSetupExtModule(
    failedOwner,
    new Uint8Array([...moduleBytes, 0x01]),
    failingGlueUrl,
  );
  assert.notEqual(failedRetry.artifactToken, failedFirst.artifactToken);
  await assert.rejects(failedRetry.ready, /expected load failure/);
  assert.equal(pendingExtLoads.has(failedOwner), false);

  const first = globalThis.voSetupExtModule(owner, moduleBytes, glueUrl);
  const concurrent = globalThis.voSetupExtModule(owner, moduleBytes, glueUrl);
  assert.equal(first.artifactToken, concurrent.artifactToken);
  assert.notEqual(first.leaseToken, concurrent.leaseToken);
  await Promise.all([first.ready, concurrent.ready]);
  assert.equal(globalThis.voIsExtModuleLoadCurrent(owner, first.artifactToken), true);
  assert.equal(globalThis.voCommitExtModule(owner, first.artifactToken, first.leaseToken), true);
  assert.equal(globalThis.voCommitExtModule(owner, concurrent.artifactToken, concurrent.leaseToken), true);
  assert.equal(globalThis[stateKey].init, 1);
  assert.equal(globalThis[stateKey].asyncInit, 1);

  const idempotent = globalThis.voSetupExtModule(owner, moduleBytes, glueUrl);
  await idempotent.ready;
  assert.equal(idempotent.artifactToken, first.artifactToken);
  assert.equal(globalThis.voCommitExtModule(owner, idempotent.artifactToken, idempotent.leaseToken), true);
  assert.equal(globalThis[stateKey].init, 1);
  assert.deepEqual(
    globalThis.voCallExt(encodedExtern, new Uint8Array([0x11, 0x80, 0xff, 0x42])),
    new Uint8Array([0x11, 0x80, 0xff, 0x42]),
  );

  globalThis.voDisposeExtModule(owner);
  assert.equal(globalThis[stateKey].dispose, 1);
  const reloaded = globalThis.voSetupExtModule(owner, moduleBytes, glueUrl);
  await reloaded.ready;
  assert.notEqual(reloaded.artifactToken, first.artifactToken);
  assert.equal(globalThis.voCommitExtModule(owner, reloaded.artifactToken, reloaded.leaseToken), true);
  assert.equal(globalThis[stateKey].init, 2);
  assert.equal(globalThis[stateKey].asyncInit, 2);

  globalThis.voDisposeAllExtModules();
  assert.equal(globalThis[stateKey].dispose, 2);
  assert.equal(pendingExtLoads.size, 0);
  assert.equal(extLoadLeases.size, 0);
  assert.equal(activeExtArtifacts.size, 0);
  assert.equal(extBindgenModules.size, 0);
  assert.equal(extStandaloneInstances.size, 0);
  delete globalThis[stateKey];
}

function assertParity(nativeResult, webResult) {
  if (webResult.wheelCount !== nativeResult.wheelCount) {
    throw new Error(`web wheel_count mismatch: native=${nativeResult.wheelCount} web=${webResult.wheelCount}`);
  }
  if (webResult.contacts !== nativeResult.contacts) {
    throw new Error(`web contacts mismatch: native=${nativeResult.contacts} web=${webResult.contacts}`);
  }
}

if (process.argv.includes('--selftest')) {
  await runSelfTest();
  console.log('VO:TELEMETRY_PARITY_SELFTEST PASS');
} else {
  const nativeResult = runNativeProbe();
  assertNativeBaseline(nativeResult);
  const webResult = await runWebProbe();
  assertParity(nativeResult, webResult);
  console.log(`VO:PARITY_NATIVE ${nativeResult.line}`);
  console.log(`VO:PARITY_WEB ${webResult.line}`);
}
