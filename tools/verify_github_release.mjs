#!/usr/bin/env node

import {
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';

const MAX_RELEASE_JSON_BYTES = 8 * 1024 * 1024;
const MAX_RELEASE_ASSETS = 1_000;
const SHA256_PATTERN = /^sha256:[0-9a-f]{64}$/;
const RELEASE_TAG_PATTERN = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

function fail(message) {
  throw new Error(`release verification failed: ${message}`);
}

function parseBoolean(value, flag) {
  if (value === 'true') return true;
  if (value === 'false') return false;
  fail(`${flag} must be true or false`);
}

function parseArgs(argv) {
  if (argv.length === 1 && argv[0] === '--self-test') return { selfTest: true };
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag?.startsWith('--') || value === undefined) {
      fail('expected --assets-dir, --release-json, --tag, --draft, and --immutable values');
    }
    if (values.has(flag)) fail(`${flag} may be specified once`);
    values.set(flag, value);
  }
  const allowed = new Set(['--assets-dir', '--release-json', '--tag', '--draft', '--immutable']);
  for (const flag of values.keys()) {
    if (!allowed.has(flag)) fail(`unknown argument ${flag}`);
  }
  for (const flag of allowed) {
    if (!values.has(flag)) fail(`missing ${flag}`);
  }
  const tag = values.get('--tag');
  if (!RELEASE_TAG_PATTERN.test(tag)) {
    fail('--tag must be canonical vMAJOR.MINOR.PATCH');
  }
  return {
    selfTest: false,
    assetsDir: values.get('--assets-dir'),
    releaseJson: values.get('--release-json'),
    tag,
    draft: parseBoolean(values.get('--draft'), '--draft'),
    immutable: parseBoolean(values.get('--immutable'), '--immutable'),
  };
}

function requireRegularFile(path, label, sizeLimit = null) {
  let metadata;
  try {
    metadata = lstatSync(path, { bigint: true });
  } catch (error) {
    fail(`${label} cannot be inspected at ${path}: ${error.message}`);
  }
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    fail(`${label} must be a regular file without symbolic links: ${path}`);
  }
  if (sizeLimit !== null && metadata.size > BigInt(sizeLimit)) {
    fail(`${label} exceeds ${sizeLimit} bytes: ${path}`);
  }
  return metadata;
}

function sha256RegularFile(path) {
  requireRegularFile(path, 'local release asset');
  let fd;
  try {
    fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    const before = fstatSync(fd, { bigint: true });
    if (!before.isFile()) fail(`local release asset is not a regular file: ${path}`);
    const hash = createHash('sha256');
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    let size = 0n;
    for (;;) {
      const count = readSync(fd, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
      size += BigInt(count);
    }
    const after = fstatSync(fd, { bigint: true });
    if (
      before.dev !== after.dev ||
      before.ino !== after.ino ||
      before.size !== after.size ||
      before.mtimeNs !== after.mtimeNs ||
      size !== before.size
    ) {
      fail(`local release asset changed while it was hashed: ${path}`);
    }
    if (size > BigInt(Number.MAX_SAFE_INTEGER)) {
      fail(`local release asset is too large to represent exactly: ${path}`);
    }
    return { size: Number(size), digest: `sha256:${hash.digest('hex')}` };
  } catch (error) {
    if (error.message?.startsWith('release verification failed:')) throw error;
    fail(`local release asset cannot be read at ${path}: ${error.message}`);
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

function snapshotLocalAssets(assetsDir) {
  let directory;
  try {
    directory = lstatSync(assetsDir);
  } catch (error) {
    fail(`release asset directory cannot be inspected at ${assetsDir}: ${error.message}`);
  }
  if (!directory.isDirectory() || directory.isSymbolicLink()) {
    fail(`release asset directory must be a real directory: ${assetsDir}`);
  }
  const entries = readdirSync(assetsDir, { withFileTypes: true });
  if (entries.length === 0) fail('release asset directory is empty');
  if (entries.length > MAX_RELEASE_ASSETS) {
    fail(`release asset directory exceeds ${MAX_RELEASE_ASSETS} entries`);
  }
  const assets = new Map();
  for (const entry of entries) {
    if (!entry.isFile() || entry.isSymbolicLink()) {
      fail(`release asset directory contains a non-file entry: ${entry.name}`);
    }
    if (basename(entry.name) !== entry.name || entry.name === '.' || entry.name === '..') {
      fail(`release asset has an invalid flat name: ${entry.name}`);
    }
    const facts = sha256RegularFile(join(assetsDir, entry.name));
    if (assets.has(entry.name)) fail(`duplicate local release asset ${entry.name}`);
    assets.set(entry.name, facts);
  }
  return assets;
}

function readReleaseJson(path) {
  requireRegularFile(path, 'GitHub release response', MAX_RELEASE_JSON_BYTES);
  let value;
  try {
    value = JSON.parse(readFileSync(path, 'utf8'));
  } catch (error) {
    fail(`GitHub release response is invalid JSON: ${error.message}`);
  }
  if (value === null || Array.isArray(value) || typeof value !== 'object') {
    fail('GitHub release response must be an object');
  }
  return value;
}

function verifyRelease(localAssets, release, { tag, draft, immutable }) {
  if (release.tag_name !== tag) {
    fail(`GitHub release tag is ${JSON.stringify(release.tag_name)}, expected ${JSON.stringify(tag)}`);
  }
  if (release.draft !== draft) {
    fail(`GitHub release draft state is ${JSON.stringify(release.draft)}, expected ${draft}`);
  }
  if (release.immutable !== immutable) {
    fail(`GitHub release immutable state is ${JSON.stringify(release.immutable)}, expected ${immutable}`);
  }
  if (!Array.isArray(release.assets)) fail('GitHub release assets must be an array');
  if (release.assets.length > MAX_RELEASE_ASSETS) {
    fail(`GitHub release exceeds ${MAX_RELEASE_ASSETS} assets`);
  }
  const remoteNames = new Set();
  for (const asset of release.assets) {
    if (asset === null || Array.isArray(asset) || typeof asset !== 'object') {
      fail('GitHub release contains an invalid asset entry');
    }
    const { name, state, size, digest } = asset;
    if (typeof name !== 'string' || name.length === 0 || basename(name) !== name) {
      fail(`GitHub release contains an invalid asset name: ${JSON.stringify(name)}`);
    }
    if (remoteNames.has(name)) fail(`GitHub release contains duplicate asset ${name}`);
    remoteNames.add(name);
    if (state !== 'uploaded') fail(`GitHub release asset ${name} has state ${JSON.stringify(state)}`);
    if (!Number.isSafeInteger(size) || size < 0) {
      fail(`GitHub release asset ${name} has invalid size ${JSON.stringify(size)}`);
    }
    if (typeof digest !== 'string' || !SHA256_PATTERN.test(digest)) {
      fail(`GitHub release asset ${name} has invalid SHA-256 digest ${JSON.stringify(digest)}`);
    }
    const expected = localAssets.get(name);
    if (!expected) fail(`GitHub release contains unexpected asset ${name}`);
    if (size !== expected.size) {
      fail(`GitHub release asset ${name} has size ${size}, expected ${expected.size}`);
    }
    if (digest !== expected.digest) {
      fail(`GitHub release asset ${name} has digest ${digest}, expected ${expected.digest}`);
    }
  }
  const missing = [...localAssets.keys()].filter((name) => !remoteNames.has(name)).sort();
  if (missing.length > 0) fail(`GitHub release is missing assets: ${missing.join(', ')}`);
}

function runSelfTest() {
  const root = mkdtempSync(join(tmpdir(), 'voplay-release-verify-'));
  try {
    const assetsDir = join(root, 'assets');
    mkdirSync(assetsDir);
    writeFileSync(join(assetsDir, 'vo.release.json'), '{}\n');
    writeFileSync(join(assetsDir, 'vo.package.json'), '{"schema_version":1}\n');
    const local = snapshotLocalAssets(assetsDir);
    const release = {
      tag_name: 'v1.2.3',
      draft: true,
      immutable: false,
      assets: [...local.entries()].map(([name, facts]) => ({
        name,
        state: 'uploaded',
        size: facts.size,
        digest: facts.digest,
      })),
    };
    verifyRelease(local, release, { tag: 'v1.2.3', draft: true, immutable: false });
    release.draft = false;
    release.immutable = true;
    verifyRelease(local, release, { tag: 'v1.2.3', draft: false, immutable: true });
    release.immutable = false;
    let rejected = false;
    try {
      verifyRelease(local, release, { tag: 'v1.2.3', draft: false, immutable: true });
    } catch {
      rejected = true;
    }
    if (!rejected) fail('self-test expected a published mutable release to be rejected');
    release.immutable = true;
    release.assets[0].digest = `sha256:${'0'.repeat(64)}`;
    rejected = false;
    try {
      verifyRelease(local, release, { tag: 'v1.2.3', draft: false, immutable: true });
    } catch {
      rejected = true;
    }
    if (!rejected) fail('self-test expected a remote digest mismatch to be rejected');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
  process.stdout.write('voplay GitHub release verifier self-test passed\n');
}

const options = parseArgs(process.argv.slice(2));
if (options.selfTest) {
  runSelfTest();
} else {
  const localAssets = snapshotLocalAssets(options.assetsDir);
  const release = readReleaseJson(options.releaseJson);
  verifyRelease(localAssets, release, options);
  process.stdout.write(`verified ${localAssets.size} exact GitHub release assets for ${options.tag}\n`);
}
