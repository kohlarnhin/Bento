import { randomUUID } from 'node:crypto';
import { readFileSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export function parseVersion(version) {
  // Minor and patch carry at 10; the major version is unbounded (9.9.9 -> 10.0.0).
  if (typeof version !== 'string' || !/^(0|[1-9][0-9]*)\.[0-9]\.[0-9]$/.test(version)) {
    throw new Error(`Invalid version ${JSON.stringify(version)}: expected x.y.z with y and z from 0 to 9`);
  }
  return version.split('.').map(Number);
}

export function nextVersion(version) {
  let [major, minor, patch] = parseVersion(version);
  patch += 1;
  if (patch === 10) {
    patch = 0;
    minor += 1;
  }
  if (minor === 10) {
    minor = 0;
    major += 1;
  }
  return `${major}.${minor}.${patch}`;
}

function versionField(text, pattern, label) {
  const matches = [...text.matchAll(pattern)];
  if (matches.length !== 1) throw new Error(`${label}: expected exactly one version field`);
  const match = matches[0];
  parseVersion(match[2]);
  return {
    version: match[2],
    replace: version => text.slice(0, match.index) + match[1] + version + match[3] + text.slice(match.index + match[0].length),
  };
}

function jsonVersion(text, label) {
  const json = JSON.parse(text);
  const field = versionField(text, /^([ \t]*"version"[ \t]*:[ \t]*")([^"\r\n]*)(")/gm, label);
  if (json.version !== field.version) throw new Error(`${label}: missing top-level version`);
  return field;
}

function tomlVersion(text, label, lockfile) {
  const blocks = text.split(/(?=^\[[^\r\n]+\][ \t]*\r?$)/m);
  const candidates = blocks.filter(block => lockfile
    ? block.startsWith('[[package]]') && /^name[ \t]*=[ \t]*"apphub"[ \t]*\r?$/m.test(block)
    : block.startsWith('[package]'));
  if (candidates.length !== 1) throw new Error(`${label}: expected exactly one ${lockfile ? 'apphub package' : '[package] section'}`);
  const block = candidates[0];
  const field = versionField(block, /^(version[ \t]*=[ \t]*")([^"\r\n]*)(")/gm, label);
  return { version: field.version, replace: version => text.replace(block, () => field.replace(version)) };
}

function readVersions(root) {
  const names = ['package.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock'];
  const files = names.map(name => {
    const path = resolve(root, name);
    const text = readFileSync(path, 'utf8');
    const field = name.endsWith('.json') ? jsonVersion(text, name) : tomlVersion(text, name, name.endsWith('.lock'));
    return { name, path, text, mode: statSync(path).mode, ...field };
  });
  if (files.some(file => file.version !== files[0].version)) {
    throw new Error(`Version mismatch: ${files.map(file => `${file.name}=${file.version}`).join(', ')}`);
  }
  return files;
}

function writeVersions(files, version) {
  // Validate and prepare every replacement before touching any original file.
  const transaction = randomUUID();
  const prepared = files.map(file => ({
    ...file,
    updated: file.replace(version),
    staging: `${file.path}.${transaction}.tmp`,
    backup: `${file.path}.${transaction}.bak`,
  }));
  const replaced = [];
  try {
    for (const file of prepared) {
      writeFileSync(file.staging, file.updated, { flag: 'wx', mode: file.mode });
      writeFileSync(file.backup, file.text, { flag: 'wx', mode: file.mode });
    }
    for (const file of prepared) {
      renameSync(file.staging, file.path);
      replaced.push(file);
    }
  } catch (error) {
    for (const file of replaced.reverse()) renameSync(file.backup, file.path);
    throw error;
  } finally {
    for (const file of prepared) {
      rmSync(file.staging, { force: true });
      rmSync(file.backup, { force: true });
    }
  }
}

function main(args) {
  if (args.length > 1 || (args.length === 1 && !['--print', '--check'].includes(args[0]))) {
    throw new Error('Usage: node scripts/bump-version.mjs [--print | --check]');
  }
  const files = readVersions(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
  const current = files[0].version;
  if (args[0] === '--check') {
    console.log(`Version check passed: ${current} in all four files`);
    return;
  }
  const next = nextVersion(current);
  if (args[0] !== '--print') writeVersions(files, next);
  console.log(`${current} -> ${next}`);
}

if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`Version error: ${error.message}`);
    process.exitCode = 1;
  }
}
