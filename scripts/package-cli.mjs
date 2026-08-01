import { createHash, randomUUID } from 'node:crypto';
import { cp, mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const options = parseArguments(process.argv.slice(2));
const platforms = new Set(['linux', 'macos', 'windows']);
const architectures = new Set(['arm64', 'x64']);

if (!platforms.has(options.platform) || !architectures.has(options.arch)) {
  fail('platform must be linux, macos or windows and arch must be arm64 or x64');
}

const binary = resolvePath(options.binary);
const output = resolvePath(options.output);
assertInsideRepository(output, 'output');
if ((await stat(binary).catch(() => null))?.isFile() !== true) fail(`binary does not exist: ${binary}`);
if (await stat(output).catch(() => null)) fail(`output already exists: ${output}`);
const licensePath = join(repositoryRoot, 'LICENSE');
const hasLicense = (await stat(licensePath).catch(() => null))?.isFile() === true;
if (options.release && !hasLicense) fail('release packaging requires a root LICENSE file');

const version = cargoPackageVersion(
  await readFile(join(repositoryRoot, 'apps', 'semantic-engine-cli', 'Cargo.toml'), 'utf8'),
);
const packageName = `semantic-engine-cli-${version}-${options.platform}-${options.arch}`;
if (basename(output) !== packageName) fail(`output directory must be named ${packageName}`);

await mkdir(dirname(output), { recursive: true });
const packageRoot = join(dirname(output), `.${basename(output)}-${randomUUID()}.tmp`);
await mkdir(packageRoot, { recursive: false });
let published = false;
try {
  const executableName = options.platform === 'windows' ? 'semantic-engine-cli.exe' : 'semantic-engine-cli';
  await cp(binary, join(packageRoot, executableName));
  for (const path of ['README.md', 'SECURITY.md', 'CHANGELOG.md']) {
    await cp(join(repositoryRoot, path), join(packageRoot, path));
  }
  for (const path of ['contracts', 'conformance/clients', 'packages/starter-titles']) {
    await cp(join(repositoryRoot, path), join(packageRoot, path), { recursive: true });
  }

  if (hasLicense) {
    await cp(licensePath, join(packageRoot, 'LICENSE'));
  } else {
    await writeFile(
      join(packageRoot, 'PREVIEW-NOT-LICENSED.txt'),
      'Preview artifact for validation only. No software license has been granted yet; do not redistribute.\n',
      'utf8',
    );
  }

  await writeFile(
    join(packageRoot, 'PACKAGE-README.txt'),
    [
      `Semantic Engine CLI ${version}`,
      `Platform: ${options.platform}-${options.arch}`,
      `Commit: ${options.commit}`,
      '',
      `Run ./${executableName} --help for commands.`,
      'The contracts and starter context are included for offline integration.',
      options.release
        ? 'Release package: verify SHA256SUMS.txt and the GitHub artifact attestation before use.'
        : 'Preview package: validation only, not licensed for redistribution.',
      '',
    ].join('\n'),
    'utf8',
  );

  const filesBeforeManifest = await listFiles(packageRoot);
  const manifest = {
    schema_version: 1,
    product: 'semantic-engine-cli',
    version,
    platform: options.platform,
    architecture: options.arch,
    commit: options.commit,
    release: options.release,
    source_contract_version: 2,
    protocol_version: 1,
    files: await describeFiles(packageRoot, filesBeforeManifest),
  };
  await writeFile(
    join(packageRoot, 'release-manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
    'utf8',
  );

  const checksumFiles = await listFiles(packageRoot);
  const checksumLines = [];
  for (const path of checksumFiles) {
    checksumLines.push(`${await sha256(join(packageRoot, path))}  ${path.replaceAll(sep, '/')}`);
  }
  await writeFile(join(packageRoot, 'SHA256SUMS.txt'), `${checksumLines.join('\n')}\n`, 'utf8');
  await rename(packageRoot, output);
  published = true;
  process.stdout.write(`${JSON.stringify({ package_name: packageName, output, files: checksumFiles.length })}\n`);
} finally {
  if (!published) await rm(packageRoot, { recursive: true, force: true });
}

function parseArguments(arguments_) {
  const values = { release: false };
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === '--release') {
      values.release = true;
      continue;
    }
    if (!argument.startsWith('--')) fail(`unexpected argument: ${argument}`);
    const key = argument.slice(2).replaceAll('-', '_');
    const value = arguments_[index + 1];
    if (!value || value.startsWith('--')) fail(`missing value for ${argument}`);
    values[key] = value;
    index += 1;
  }
  for (const key of ['binary', 'platform', 'arch', 'output', 'commit']) {
    if (!values[key]) fail(`--${key.replaceAll('_', '-')} is required`);
  }
  if (!/^[0-9a-f]{40}$/.test(values.commit)) fail('--commit must be a full lowercase Git SHA');
  return values;
}

function resolvePath(path) {
  return isAbsolute(path) ? resolve(path) : resolve(repositoryRoot, path);
}

function assertInsideRepository(path, label) {
  const child = relative(repositoryRoot, path);
  if (!child || child.startsWith('..') || isAbsolute(child)) fail(`${label} must be inside the repository`);
}

function cargoPackageVersion(toml) {
  const match = /^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*$/m.exec(toml);
  if (!match) fail('CLI Cargo.toml has no simple semantic version');
  return match[1];
}

async function listFiles(root, current = '') {
  const entries = await readdir(join(root, current), { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) => left.name.localeCompare(right.name, 'en'))) {
    const path = join(current, entry.name);
    if (entry.isSymbolicLink()) fail(`package input contains a symbolic link: ${path}`);
    if (entry.isDirectory()) files.push(...(await listFiles(root, path)));
    else if (entry.isFile() && entry.name !== 'SHA256SUMS.txt') files.push(path);
    else if (!entry.isFile()) fail(`package input contains a special file: ${path}`);
  }
  return files;
}

async function describeFiles(root, paths) {
  const descriptions = [];
  for (const path of paths) {
    const file = join(root, path);
    descriptions.push({ path: path.replaceAll(sep, '/'), bytes: (await stat(file)).size, sha256: await sha256(file) });
  }
  return descriptions;
}

async function sha256(path) {
  return createHash('sha256').update(await readFile(path)).digest('hex');
}

function fail(message) {
  process.stderr.write(`package-cli: ${message}\n`);
  process.exit(1);
}
