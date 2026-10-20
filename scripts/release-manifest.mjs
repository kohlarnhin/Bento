import { execFileSync } from 'node:child_process';
import { readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { parseVersion } from './bump-version.mjs';

export function releaseNotes(cwd = process.cwd()) {
  const git = args => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  if (!git(['tag', '--merged', 'HEAD', '--list', 'v*'])) return 'Bento 首个版本';
  const previous = git(['describe', '--tags', '--match', 'v*', '--abbrev=0', 'HEAD']);
  return git(['log', '--no-merges', '--format=%s', `${previous}..HEAD`])
    .split('\n')
    .filter(subject => subject && !subject.startsWith('chore(release)'))
    .map(subject => `- ${subject}`)
    .join('\n');
}

function requireFile(directory, filename) {
  const path = join(directory, filename);
  let stat;
  try {
    stat = statSync(path);
  } catch (error) {
    if (error.code === 'ENOENT') throw new Error(`Missing release artifact: ${filename}`);
    throw error;
  }
  if (!stat.isFile() || stat.size === 0) throw new Error(`Empty or invalid release artifact: ${filename}`);
  return path;
}

export function prepareRelease({ directory, version, repository, notes, notesFile, cwd }) {
  parseVersion(version);
  if (!directory) throw new Error('Missing release directory (--directory)');
  if (typeof repository !== 'string' || !/^[\w.-]+\/[\w.-]+$/.test(repository)) {
    throw new Error('Invalid repository: expected owner/repo (--repository)');
  }
  const manifest = { version, notes: '', pub_date: new Date().toISOString(), platforms: {} };
  for (const [arch, platform] of [['arm64', 'darwin-aarch64'], ['x64', 'darwin-x86_64']]) {
    const filename = `Bento_${version}_${arch}.app.tar.gz`;
    requireFile(directory, filename);
    const signature = readFileSync(requireFile(directory, `${filename}.sig`), 'utf8').trim();
    if (!signature) throw new Error(`Empty updater signature: ${filename}.sig`);
    requireFile(directory, `Bento_${version}_${arch}.dmg`);
    manifest.platforms[platform] = {
      signature,
      url: `https://github.com/${repository}/releases/download/v${version}/${filename}`,
    };
  }
  manifest.notes = notes ?? releaseNotes(cwd);
  writeFileSync(notesFile ?? join(directory, '..', 'release-notes.md'), `${manifest.notes}\n`);
  writeFileSync(join(directory, 'latest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

function main() {
  const { values } = parseArgs({ options: {
    directory: { type: 'string' },
    version: { type: 'string' },
    repository: { type: 'string' },
    notes: { type: 'string' },
    'notes-file': { type: 'string' },
  } });
  const manifest = prepareRelease({ ...values, notesFile: values['notes-file'] });
  console.log(`Prepared Bento v${manifest.version}: six release artifacts verified, latest.json and release notes written`);
}

if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  try {
    main();
  } catch (error) {
    console.error(`Release manifest error: ${error.message}`);
    process.exitCode = 1;
  }
}
