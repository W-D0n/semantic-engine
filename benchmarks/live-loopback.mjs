import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { arch, cpus, platform, release, tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';

const executable = resolve(process.argv[2] ?? 'target/release/semantic-engine-cli.exe');
const samples = integerArgument('--samples', 500, 10, 5_000);
const intervalMs = integerArgument('--interval-ms', 25, 11, 1_000);
const requestTimeoutMs = 5_000;
const temporaryRoot = await mkdtemp(join(tmpdir(), 'semantic-engine-live-benchmark-'));
const auditDatabase = join(temporaryRoot, 'state.sqlite3');
const sourcesDatabase = join(temporaryRoot, 'sources.sqlite3');
let child;

try {
  const fixture = JSON.parse(
    await readFile(resolve('packages/starter-titles/data/titles.json'), 'utf8'),
  );
  const targets = fixture.titles.map(({ id, canonical, aliases }) => ({ id, canonical, aliases }));
  const ready = await startServer();
  const request = createRequester(ready);
  await request('benchmark-start', 'start_session', {
    session_id: 'live-benchmark-session',
    round: {
      id: 'live-benchmark-round',
      targets,
      policy: { accept_threshold: 0.87, review_threshold: 0.72, ambiguity_margin: 0.05 },
    },
    context_package_sha256: null,
  });

  const expressions = [
    'Elden Ring',
    'eldern ring',
    'ER',
    'Le Voyage de Chihiro',
    'botw',
    'The Matrix',
    'retour vers le futur',
    'absolutely unrelated chat message',
  ];
  const timings = [];
  const decisions = { accepted: 0, rejected: 0, abstained: 0 };
  const benchmarkStarted = performance.now();
  for (let index = 0; index < samples; index += 1) {
    const started = performance.now();
    const validation = await request(`live-${index}`, 'submit', {
      session_id: 'live-benchmark-session',
      submission: {
        message_id: `twitch:simulated-${index}`,
        participant_id: `twitch:viewer-${index % 200}`,
        source_sequence: index,
        text: expressions[index % expressions.length],
      },
    });
    timings.push((performance.now() - started) * 1_000_000);
    decisions[validation.decision] += 1;
    if (index + 1 < samples) await delay(intervalMs);
  }
  const wallTimeMs = performance.now() - benchmarkStarted;
  timings.sort((left, right) => left - right);

  process.stdout.write(`${JSON.stringify({
    profile: 'loopback_session_submit_v1',
    transport: 'HTTP loopback + JSON + durable SQLite session/audit',
    samples,
    interval_ms: intervalMs,
    targets: targets.length,
    participants: Math.min(samples, 200),
    decisions,
    latency_ns: {
      p50: percentile(timings, 50),
      p95: percentile(timings, 95),
      p99: percentile(timings, 99),
      max: Math.round(timings.at(-1) ?? 0),
    },
    wall_time_ms: Math.round(wallTimeMs),
    machine: {
      platform: platform(),
      release: release(),
      architecture: arch(),
      cpu: cpus()[0]?.model ?? 'unknown',
      node: process.version,
    },
  }, null, 2)}\n`);
} finally {
  await stopServer();
  await rm(temporaryRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

function createRequester(ready) {
  return async (requestId, command, params) => {
    const response = await fetch(`${ready.address}/v1/commands`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${ready.token}`,
        'Content-Type': 'application/json',
        'X-Semantic-Engine-Protocol': '2',
      },
      body: JSON.stringify({ protocol_version: 2, request_id: requestId, command, params }),
      signal: AbortSignal.timeout(requestTimeoutMs),
    });
    const payload = await response.json();
    if (response.status !== 200 || payload.status !== 'ok') {
      throw new Error(`request ${requestId} failed: HTTP ${response.status} ${JSON.stringify(payload)}`);
    }
    return payload.result;
  };
}

async function startServer() {
  child = spawn(
    executable,
    [
      'loopback', '--enable',
      '--audit', auditDatabase,
      '--sources', sourcesDatabase,
      '--port', '0',
    ],
    { env: { ...process.env }, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true },
  );
  let stdout = '';
  let stderr = '';
  child.stderr.on('data', (chunk) => {
    stderr = `${stderr}${chunk.toString('utf8')}`.slice(-8192);
  });
  return await new Promise((resolveReady, rejectReady) => {
    const timer = setTimeout(() => {
      rejectReady(new Error(`benchmark host startup timed out: ${stderr}`));
      child.kill();
    }, requestTimeoutMs);
    child.once('error', (error) => {
      clearTimeout(timer);
      rejectReady(error);
    });
    child.once('exit', (code) => {
      clearTimeout(timer);
      rejectReady(new Error(`benchmark host exited with code ${code}: ${stderr}`));
    });
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8');
      const newline = stdout.indexOf('\n');
      if (newline < 0) return;
      clearTimeout(timer);
      resolveReady(JSON.parse(stdout.slice(0, newline)));
    });
  });
}

async function stopServer() {
  if (!child || child.exitCode !== null) return;
  child.kill();
  await new Promise((resolveExit) => {
    const timer = setTimeout(resolveExit, 1_000);
    child.once('exit', () => {
      clearTimeout(timer);
      resolveExit();
    });
  });
}

function percentile(sorted, value) {
  const rank = Math.max(0, Math.ceil((value / 100) * sorted.length) - 1);
  return Math.round(sorted[Math.min(rank, sorted.length - 1)] ?? 0);
}

function integerArgument(name, fallback, minimum, maximum) {
  const position = process.argv.indexOf(name);
  if (position < 0) return fallback;
  const parsed = Number.parseInt(process.argv[position + 1], 10);
  if (!Number.isInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new Error(`${name} must be an integer between ${minimum} and ${maximum}`);
  }
  return parsed;
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}
