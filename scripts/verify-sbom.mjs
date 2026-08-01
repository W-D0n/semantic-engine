import { readFile, stat } from 'node:fs/promises';
import { resolve } from 'node:path';

const path = resolve(process.argv[2] ?? 'artifacts/semantic-engine.cdx.json');
const size = (await stat(path)).size;
if (size > 16 * 1024 * 1024) fail('SBOM exceeds the 16 MiB attestation limit');

const document = JSON.parse(await readFile(path, 'utf8'));
if (document.bomFormat !== 'CycloneDX') fail('bomFormat must be CycloneDX');
if (document.specVersion !== '1.6') fail('specVersion must be 1.6');
if (!/^urn:uuid:[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(document.serialNumber)) {
  fail('serialNumber must be a deterministic UUIDv5 URN');
}
if (document.version !== 1) fail('document version must be 1');
if (!document.metadata?.component?.['bom-ref']) fail('metadata.component.bom-ref is required');
if (!Array.isArray(document.components) || document.components.length === 0) {
  fail('components must be a non-empty array');
}

const references = new Set();
for (const component of document.components) {
  for (const field of ['type', 'bom-ref', 'name', 'version', 'purl']) {
    if (typeof component[field] !== 'string' || component[field].length === 0) {
      fail(`component ${field} must be a non-empty string`);
    }
  }
  if (references.has(component['bom-ref'])) fail(`duplicate bom-ref: ${component['bom-ref']}`);
  references.add(component['bom-ref']);
}

process.stdout.write(
  `${JSON.stringify({ path, format: document.bomFormat, version: document.specVersion, components: references.size })}\n`,
);

function fail(message) {
  process.stderr.write(`verify-sbom: ${message}\n`);
  process.exit(1);
}
