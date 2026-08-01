import { createHash } from 'node:crypto';
import { readFile, stat } from 'node:fs/promises';
import { isAbsolute, relative, resolve } from 'node:path';

const root = resolve(process.argv[2] ?? '');
if (!process.argv[2] || (await stat(root).catch(() => null))?.isDirectory() !== true) {
  fail('usage: verify-checksums.mjs <package-directory>');
}
const checksumPath = resolve(root, 'SHA256SUMS.txt');
const lines = (await readFile(checksumPath, 'utf8')).trimEnd().split('\n');
if (lines.length === 0) fail('checksum file is empty');
const seen = new Set();
for (const [index, line] of lines.entries()) {
  const match = /^([0-9a-f]{64})  ([A-Za-z0-9._/ -]+)$/.exec(line.replace(/\r$/, ''));
  if (!match) fail(`invalid checksum line ${index + 1}`);
  const relativePath = match[2];
  if (seen.has(relativePath)) fail(`duplicate checksum path: ${relativePath}`);
  seen.add(relativePath);
  const file = resolve(root, relativePath);
  const child = relative(root, file);
  if (!child || child.startsWith('..') || isAbsolute(child)) fail(`unsafe checksum path: ${relativePath}`);
  if ((await stat(file).catch(() => null))?.isFile() !== true) fail(`missing file: ${relativePath}`);
  const digest = createHash('sha256').update(await readFile(file)).digest('hex');
  if (digest !== match[1]) fail(`checksum mismatch: ${relativePath}`);
}
process.stdout.write(`${JSON.stringify({ package: root, verified: seen.size })}\n`);

function fail(message) {
  process.stderr.write(`verify-checksums: ${message}\n`);
  process.exit(1);
}
