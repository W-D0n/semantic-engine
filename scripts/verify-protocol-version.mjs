import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const rust = await readFile(resolve(root, 'crates/semantic-engine-protocol/src/lib.rs'), 'utf8');
const match = rust.match(/pub const PROTOCOL_VERSION: u32 = (\d+);/);
if (!match) throw new Error('PROTOCOL_VERSION is missing');
const expected = Number(match[1]);

const operationalFiles = [
  'scripts/package-cli.mjs',
  'benchmarks/live-loopback.mjs',
  'apps/desktop/src/lib/LoopbackPanel.svelte',
  'conformance/clients/node-client.mjs',
  'conformance/clients/node-loopback-client.mjs',
  'contracts/loopback-openapi.yaml',
  'contracts/protocol-request.schema.json',
  'contracts/protocol-response.schema.json',
  'docs/integration/jsonl-sidecar.md',
  'docs/integration/loopback-api.md',
];
const stale = [];
for (const relative of operationalFiles) {
  const content = await readFile(resolve(root, relative), 'utf8');
  const patterns = [
    /protocol_version\s*[:=]\s*1\b/g,
    /protocol_version"\s*:\s*1\b/g,
    /X-Semantic-Engine-Protocol[^\n]*["'`]1["'`]/g,
    /semantic-engine\.v1\b/g,
    /protocol public v1\b/gi,
  ];
  for (const pattern of patterns) {
    if (pattern.test(content)) stale.push(`${relative}: ${pattern.source}`);
  }
}
if (expected !== 2 || stale.length) {
  throw new Error(`protocol version gate failed (expected=${expected})\n${stale.join('\n')}`);
}
console.log(JSON.stringify({ protocol_version: expected, checked: operationalFiles.length }));
