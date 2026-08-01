import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';

const output = resolve(process.argv[2] ?? 'artifacts/semantic-engine.cdx.json');
const cargo = JSON.parse(
  execFileSync('cargo', ['metadata', '--locked', '--format-version', '1'], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  }),
);
const cargoLockContent = await readFile(resolve('Cargo.lock'), 'utf8');
const packageLockContent = await readFile(resolve('apps/desktop/package-lock.json'), 'utf8');
const packageLock = JSON.parse(packageLockContent);
const components = new Map();

for (const packageRecord of cargo.packages) {
  const purl = `pkg:cargo/${encodeURIComponent(packageRecord.name)}@${packageRecord.version}`;
  components.set(purl, compact({
    type: packageRecord.source ? 'library' : 'application',
    'bom-ref': purl,
    group: '',
    name: packageRecord.name,
    version: packageRecord.version,
    licenses: packageRecord.license
      ? [{ expression: packageRecord.license }]
      : undefined,
    purl,
    externalReferences: packageRecord.repository
      ? [{ type: 'vcs', url: packageRecord.repository }]
      : undefined,
    properties: [{ name: 'semantic-engine:ecosystem', value: 'cargo' }],
  }));
}

for (const [packagePath, packageRecord] of Object.entries(packageLock.packages ?? {})) {
  if (!packagePath || !packageRecord.version) continue;
  const name = packageRecord.name ?? packagePath.replace(/^node_modules\//, '');
  const encodedName = name.startsWith('@')
    ? `${encodeURIComponent(name.split('/')[0])}/${encodeURIComponent(name.split('/').slice(1).join('/'))}`
    : encodeURIComponent(name);
  const purl = `pkg:npm/${encodedName}@${packageRecord.version}`;
  components.set(purl, compact({
    type: 'library',
    'bom-ref': purl,
    group: name.startsWith('@') ? name.split('/')[0] : '',
    name: name.startsWith('@') ? name.split('/').slice(1).join('/') : name,
    version: packageRecord.version,
    licenses: packageRecord.license
      ? [{ license: { id: packageRecord.license } }]
      : undefined,
    purl,
    properties: [{ name: 'semantic-engine:ecosystem', value: 'npm' }],
  }));
}

const document = {
  bomFormat: 'CycloneDX',
  specVersion: '1.6',
  serialNumber: `urn:uuid:${uuidV5(
    'github.com/W-D0n/semantic-engine\0' + cargoLockContent + '\0' + packageLockContent,
  )}`,
  version: 1,
  metadata: {
    component: {
      type: 'application',
      'bom-ref': 'pkg:github/W-D0n/semantic-engine@0.1.0',
      group: 'W-D0n',
      name: 'semantic-engine',
      version: '0.1.0',
      purl: 'pkg:github/W-D0n/semantic-engine@0.1.0',
    },
    properties: [
      { name: 'semantic-engine:reproducible', value: 'true' },
      { name: 'semantic-engine:inputs', value: 'Cargo.lock,apps/desktop/package-lock.json' },
    ],
  },
  components: [...components.values()].sort((left, right) =>
    left['bom-ref'].localeCompare(right['bom-ref']),
  ),
};

await mkdir(dirname(output), { recursive: true });
await writeFile(output, `${JSON.stringify(document, null, 2)}\n`, 'utf8');
process.stdout.write(`CycloneDX SBOM: ${document.components.length} components -> ${output}\n`);

function compact(value) {
  return Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== undefined));
}

function uuidV5(name) {
  const dnsNamespace = Buffer.from('6ba7b8109dad11d180b400c04fd430c8', 'hex');
  const bytes = createHash('sha1').update(dnsNamespace).update(name, 'utf8').digest().subarray(0, 16);
  bytes[6] = (bytes[6] & 0x0f) | 0x50;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
