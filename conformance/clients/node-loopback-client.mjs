import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const PROTOCOL_VERSION = 1;
const REQUEST_TIMEOUT_MS = 5000;
const PRIVATE_TEXT = 'eldern ring!!!';

const executable = resolve(process.argv[2] ?? 'target/debug/semantic-engine-cli.exe');
const temporaryRoot = await mkdtemp(join(tmpdir(), 'semantic-engine-loopback-conformance-'));
const database = join(temporaryRoot, 'state.sqlite3');
let child;

try {
  let ready = await startServer();
  assert(/^http:\/\/127\.0\.0\.1:\d+$/.test(ready.address), 'server did not bind to IPv4 loopback');
  assert(/^[a-f0-9]{64}$/.test(ready.token), 'server token is not a 256-bit hexadecimal secret');
  assertEqual(ready.protocol_version, PROTOCOL_VERSION, 'startup protocol version');

  const health = await fetchJson(`${ready.address}/v1/health`);
  assertEqual(health.response.status, 200, 'health status');
  assertEqual(health.body.protocol_versions, [PROTOCOL_VERSION], 'health protocol versions');

  const unauthorized = await fetchJson(`${ready.address}/v1/commands`, {
    method: 'POST',
    body: JSON.stringify({ protocol_version: 1, request_id: 'unauthorized', command: 'stats' }),
  });
  assertEqual(unauthorized.response.status, 401, 'unauthorized status');
  assertEqual(unauthorized.body.error.code, 'unauthorized', 'unauthorized code');

  const forbidden = await fetchJson(`${ready.address}/v1/commands`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${ready.token}`,
      Origin: 'https://attacker.example',
      'Content-Type': 'application/json',
      'X-Semantic-Engine-Protocol': '1',
    },
    body: JSON.stringify({ protocol_version: 1, request_id: 'forbidden', command: 'stats' }),
  });
  assertEqual(forbidden.response.status, 403, 'forbidden origin status');
  assertEqual(forbidden.body.error.code, 'origin_forbidden', 'forbidden origin code');

  const preflight = await fetch(`${ready.address}/v1/commands`, {
    method: 'OPTIONS',
    headers: {
      Origin: 'http://localhost',
      'Access-Control-Request-Method': 'POST',
      'Access-Control-Request-Headers': 'authorization,content-type,x-semantic-engine-protocol',
    },
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  assertEqual(preflight.status, 200, 'CORS preflight status');
  assert(
    preflight.headers.get('access-control-allow-headers')?.includes('content-type'),
    'CORS preflight did not allow content-type',
  );

  let request = createRequester(ready);
  const started = await request('start', 'start_session', {
    session_id: 'node-loopback-session',
    round: {
      id: 'node-loopback-round',
      targets: [{ id: 'elden-ring', canonical: 'Elden Ring', aliases: ['ER'] }],
      policy: { accept_threshold: 0.87, review_threshold: 0.72, ambiguity_margin: 0.05 },
    },
    context_package_sha256: null,
  });
  assertEqual(started.state, 'active', 'started state');

  const submitted = await request('submit', 'submit', {
    session_id: 'node-loopback-session',
    submission: {
      message_id: 'node-loopback-message',
      participant_id: 'viewer-9',
      source_sequence: 9,
      text: PRIVATE_TEXT,
    },
  });
  assertEqual(submitted.decision, 'accepted', 'loopback fuzzy decision');

  const firstToken = ready.token;
  await stopServer();
  ready = await startServer();
  assert(ready.token !== firstToken, 'restart reused the previous bearer token');
  request = createRequester(ready);
  const restored = await request('get', 'get_session', { session_id: 'node-loopback-session' });
  assertEqual(restored.state, 'active', 'restored loopback session state');
  assertEqual(restored.latest_event_sequence, 2, 'restored loopback sequence');

  const socketPayload = await firstWebSocketPayload(ready);
  assertEqual(socketPayload.status, 'ok', 'WebSocket response status');
  assertEqual(socketPayload.result.events.length, 2, 'WebSocket restored event count');
  const encodedEvents = JSON.stringify(socketPayload);
  assert(!encodedEvents.includes(PRIVATE_TEXT), 'WebSocket exposed raw chat text');
  assert(!encodedEvents.includes('matched_expression'), 'WebSocket exposed matched expression');

  const duplicate = await request('duplicate', 'submit', {
    session_id: 'node-loopback-session',
    submission: {
      message_id: 'node-loopback-message',
      participant_id: 'viewer-9',
      source_sequence: 9,
      text: PRIVATE_TEXT,
    },
  });
  assertEqual(duplicate, submitted, 'loopback idempotent response');

  const eventsAfterDuplicate = await request('events-after-duplicate', 'events', {
    session_id: 'node-loopback-session',
    after_sequence: 0,
    limit: 100,
  });
  assertEqual(eventsAfterDuplicate.latest_sequence, 2, 'duplicate did not emit an event');
  assertEqual(eventsAfterDuplicate.events.length, 2, 'loopback event count after duplicate');

  const conflict = await request.error('conflict', 'submit', {
    session_id: 'node-loopback-session',
    submission: {
      message_id: 'node-loopback-message',
      participant_id: 'viewer-9',
      source_sequence: 9,
      text: 'different content',
    },
  });
  assertEqual(conflict.code, 'identity_conflict', 'loopback conflict code');

  const ended = await request('end', 'end_session', { session_id: 'node-loopback-session' });
  assertEqual(ended.state, 'ended', 'loopback ended state');

  const afterEnd = await request.error('after-end', 'submit', {
    session_id: 'node-loopback-session',
    submission: {
      message_id: 'late-message',
      participant_id: 'viewer-10',
      source_sequence: 10,
      text: 'Elden Ring',
    },
  });
  assertEqual(afterEnd.code, 'session_ended', 'loopback post-end code');

  const oversized = await fetchJson(`${ready.address}/v1/commands`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${ready.token}`,
      Origin: 'http://localhost',
      'Content-Type': 'application/json',
      'X-Semantic-Engine-Protocol': '1',
    },
    body: 'x'.repeat(1024 * 1024 + 1),
  });
  assertEqual(oversized.response.status, 413, 'oversized request status');
  assertEqual(oversized.body.error.code, 'request_too_large', 'oversized request code');

  await stopServer();
  const databaseBytes = await readFile(database);
  assert(!databaseBytes.includes(Buffer.from(PRIVATE_TEXT)), 'database contains raw chat text');
  process.stdout.write('Semantic Engine Node loopback conformity: PASS\n');
} finally {
  await stopServer();
  await rm(temporaryRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

async function startServer() {
  child = spawn(
    executable,
    ['loopback', '--enable', '--audit', database, '--port', '0', '--origin', 'http://localhost'],
    { env: { ...process.env }, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true },
  );
  let stdout = '';
  let stderr = '';
  child.stderr.on('data', (chunk) => {
    stderr = `${stderr}${chunk.toString('utf8')}`.slice(-8192);
  });
  return await new Promise((resolveReady, rejectReady) => {
    const timer = setTimeout(() => {
      rejectReady(new Error(`loopback startup timed out: ${stderr}`));
      child.kill();
    }, REQUEST_TIMEOUT_MS);
    child.once('error', (error) => {
      clearTimeout(timer);
      rejectReady(error);
    });
    child.once('exit', (code) => {
      clearTimeout(timer);
      rejectReady(new Error(`loopback exited before readiness with code ${code}: ${stderr}`));
    });
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString('utf8');
      const newline = stdout.indexOf('\n');
      if (newline < 0) return;
      clearTimeout(timer);
      try {
        resolveReady(JSON.parse(stdout.slice(0, newline)));
      } catch (error) {
        rejectReady(new Error(`loopback readiness is invalid JSON: ${error.message}`));
      }
    });
  });
}

function createRequester(ready) {
  const exchange = async (requestId, command, params) => {
    const result = await fetchJson(`${ready.address}/v1/commands`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${ready.token}`,
        Origin: 'http://localhost',
        'Content-Type': 'application/json',
        'X-Semantic-Engine-Protocol': String(PROTOCOL_VERSION),
      },
      body: JSON.stringify({ protocol_version: PROTOCOL_VERSION, request_id: requestId, command, params }),
    });
    assertEqual(result.response.status, 200, `${requestId} HTTP status`);
    assertEqual(result.body.protocol_version, PROTOCOL_VERSION, `${requestId} protocol version`);
    assertEqual(result.body.request_id, requestId, `${requestId} correlation`);
    return result.body;
  };
  const requester = async (requestId, command, params) => {
    const body = await exchange(requestId, command, params);
    assertEqual(body.status, 'ok', `${requestId} status`);
    return body.result;
  };
  requester.error = async (requestId, command, params) => {
    const body = await exchange(requestId, command, params);
    assertEqual(body.status, 'error', `${requestId} status`);
    return body.error;
  };
  return requester;
}

async function fetchJson(url, options) {
  const response = await fetch(url, { ...options, signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) });
  return { response, body: await response.json() };
}

async function firstWebSocketPayload(ready) {
  const address = ready.address.replace(/^http/, 'ws');
  const socket = new WebSocket(
    `${address}/v1/events/ws?session_id=node-loopback-session&after_sequence=0&limit=100`,
    ['semantic-engine.v1', `semantic-engine.token.${ready.token}`],
  );
  try {
    return await new Promise((resolvePayload, rejectPayload) => {
      const timer = setTimeout(() => {
        rejectPayload(new Error('WebSocket event stream timed out'));
        socket.close();
      }, REQUEST_TIMEOUT_MS);
      socket.addEventListener('open', () => {
        try {
          assertEqual(socket.protocol, 'semantic-engine.v1', 'selected WebSocket protocol');
        } catch (error) {
          clearTimeout(timer);
          rejectPayload(error);
        }
      });
      socket.addEventListener('message', (event) => {
        clearTimeout(timer);
        try {
          resolvePayload(JSON.parse(String(event.data)));
        } catch (error) {
          rejectPayload(error);
        }
      }, { once: true });
      socket.addEventListener('error', () => {
        clearTimeout(timer);
        rejectPayload(new Error('WebSocket connection failed'));
      }, { once: true });
    });
  } finally {
    socket.close();
  }
}

async function stopServer() {
  if (!child || child.exitCode !== null) return;
  child.kill();
  await new Promise((resolveExit) => {
    const timer = setTimeout(resolveExit, 1000);
    child.once('exit', () => {
      clearTimeout(timer);
      resolveExit();
    });
  });
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function assertEqual(actual, expected, message) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
