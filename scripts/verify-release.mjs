import { readFile, stat } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const requested = process.argv[2];
const release = process.argv.includes('--release');
if (!requested || !/^[0-9]+\.[0-9]+\.[0-9]+$/.test(requested)) fail('usage: verify-release.mjs X.Y.Z [--release]');

const versions = new Map([
  ['CLI Cargo', cargoVersion(await text('apps/semantic-engine-cli/Cargo.toml'))],
  ['desktop Cargo', cargoVersion(await text('apps/desktop/src-tauri/Cargo.toml'))],
  ['desktop npm', JSON.parse(await text('apps/desktop/package.json')).version],
  ['Tauri config', JSON.parse(await text('apps/desktop/src-tauri/tauri.conf.json')).version],
]);
for (const [label, version] of versions) {
  if (version !== requested) fail(`${label} version ${version} does not match ${requested}`);
}

if (release) {
  if ((await stat(join(repositoryRoot, 'LICENSE')).catch(() => null))?.isFile() !== true) {
    fail('public release requires a root LICENSE file');
  }
  const changelog = await text('CHANGELOG.md');
  if (!changelog.includes(`## [${requested}]`)) fail(`CHANGELOG.md has no ## [${requested}] section`);
}
process.stdout.write(`${JSON.stringify({ version: requested, release, versions: Object.fromEntries(versions) })}\n`);

async function text(path) {
  return readFile(join(repositoryRoot, path), 'utf8');
}

function cargoVersion(toml) {
  const match = /^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$/m.exec(toml);
  if (!match) fail('Cargo.toml has no simple package version');
  return match[1];
}

function fail(message) {
  process.stderr.write(`verify-release: ${message}\n`);
  process.exit(1);
}
