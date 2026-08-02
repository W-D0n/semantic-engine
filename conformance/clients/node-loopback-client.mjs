import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const PROTOCOL_VERSION = 2;
const REQUEST_TIMEOUT_MS = 5000;
const PRIVATE_TEXT = 'eldern ring!!!';
const LEARNED_TEXT = 'the lands between';
const CONTEXT_SHA256 = 'b'.repeat(64);

const executable = resolve(process.argv[2] ?? 'target/debug/semantic-engine-cli.exe');
const temporaryRoot = await mkdtemp(join(tmpdir(), 'semantic-engine-loopback-conformance-'));
const database = join(temporaryRoot, 'state.sqlite3');
const sourcesDatabase = join(temporaryRoot, 'sources.sqlite3');
let child;

try {
  let ready = await startServer();
  assert(/^http:\/\/127\.0\.0\.1:\d+$/.test(ready.address), 'server did not bind to IPv4 loopback');
  assert(/^[a-f0-9]{64}$/.test(ready.token), 'server token is not a 256-bit hexadecimal secret');
  assertEqual(ready.protocol_version, PROTOCOL_VERSION, 'startup protocol version');

  const health = await fetchJson(`${ready.address}/v1/health`);
  assertEqual(health.response.status, 200, 'health status');
  assertEqual(health.body.protocol_versions, [PROTOCOL_VERSION], 'health protocol versions');

  const unauthorizedSources = await fetchJson(`${ready.address}/v1/sources`);
  assertEqual(unauthorizedSources.response.status, 401, 'unauthorized sources status');

  const createdSource = await sourceApiJson(ready, '/v1/sources/twitch', {
    method: 'POST',
    json: { display_name: 'Node pilot', client_id: 'publicclient123' },
  });
  assertEqual(createdSource.response.status, 201, 'source create status');
  assertEqual(createdSource.body.authenticated, false, 'new source auth state');
  assertEqual(createdSource.body.credential_id, null, 'new source credential reference');
  assert(!JSON.stringify(createdSource.body).includes('access_token'), 'source API exposed access token');
  assert(!JSON.stringify(createdSource.body).includes('refresh_token'), 'source API exposed refresh token');
  const sourceId = createdSource.body.source_id;
  const sourceRevision = createdSource.body.revision;

  const unauthorized = await fetchJson(`${ready.address}/v1/commands`, {
    method: 'POST',
    body: JSON.stringify({ protocol_version: 2, request_id: 'unauthorized', command: 'stats' }),
  });
  assertEqual(unauthorized.response.status, 401, 'unauthorized status');
  assertEqual(unauthorized.body.error.code, 'unauthorized', 'unauthorized code');

  const forbidden = await fetchJson(`${ready.address}/v1/commands`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${ready.token}`,
      Origin: 'https://attacker.example',
      'Content-Type': 'application/json',
      'X-Semantic-Engine-Protocol': String(PROTOCOL_VERSION),
    },
    body: JSON.stringify({ protocol_version: 2, request_id: 'forbidden', command: 'stats' }),
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
    context_package_sha256: CONTEXT_SHA256,
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
  const memorySource = await request('memory-source', 'submit', {
    session_id: 'node-loopback-session',
    submission: {
      message_id: 'node-loopback-memory-message',
      participant_id: 'viewer-10',
      source_sequence: 10,
      text: LEARNED_TEXT,
    },
  });
  assert(memorySource.decision !== 'accepted', 'loopback memory fixture was already accepted');
  await request('memory-resolve', 'resolve', {
    session_id: 'node-loopback-session',
    request: {
      round_id: 'node-loopback-round',
      message_id: 'node-loopback-memory-message',
      verdict: 'accepted',
      target_id: 'elden-ring',
      note: 'operator confirmed',
    },
  });
  const remembered = await request('memory-remember', 'remember_resolution', {
    session_id: 'node-loopback-session',
    message_id: 'node-loopback-memory-message',
  });
  assertEqual(remembered.state, 'active', 'loopback learned memory state');
  await request('memory-revoke', 'revoke_memory', {
    context_package_sha256: CONTEXT_SHA256,
    id: remembered.id,
  });

  const firstToken = ready.token;
  await stopServer();
  ready = await startServer();
  assert(ready.token !== firstToken, 'restart reused the previous bearer token');
  request = createRequester(ready);
  const restored = await request('get', 'get_session', { session_id: 'node-loopback-session' });
  assertEqual(restored.state, 'active', 'restored loopback session state');
  assertEqual(restored.latest_event_sequence, 4, 'restored loopback sequence');
  const restoredMemory = await request('memory-restored', 'list_memory', {
    context_package_sha256: CONTEXT_SHA256,
    limit: 10,
  });
  assertEqual(restoredMemory[0].state, 'revoked', 'loopback revoked memory survived restart');

  const restoredSources = await sourceApiJson(ready, '/v1/sources');
  assertEqual(restoredSources.response.status, 200, 'restored source list status');
  assertEqual(restoredSources.body.length, 1, 'restored source count');
  assertEqual(restoredSources.body[0].source_id, sourceId, 'restored source identity');

  const socketPayload = await firstWebSocketPayload(ready);
  assertEqual(socketPayload.status, 'ok', 'WebSocket response status');
  assertEqual(socketPayload.result.events.length, 4, 'WebSocket restored event count');
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
  assertEqual(eventsAfterDuplicate.latest_sequence, 4, 'duplicate did not emit an event');
  assertEqual(eventsAfterDuplicate.events.length, 4, 'loopback event count after duplicate');

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
      'X-Semantic-Engine-Protocol': String(PROTOCOL_VERSION),
    },
    body: 'x'.repeat(1024 * 1024 + 1),
  });
  assertEqual(oversized.response.status, 413, 'oversized request status');
  assertEqual(oversized.body.error.code, 'request_too_large', 'oversized request code');

  const deletedSource = await sourceApiFetch(
    ready,
    `/v1/sources/${sourceId}?expected_revision=${sourceRevision}`,
    { method: 'DELETE' },
  );
  assertEqual(deletedSource.status, 200, 'source deletion status');
  const deletionReceipt = await deletedSource.json();
  assertEqual(deletionReceipt.credential_purged, true, 'source credential purge receipt');
  assertEqual(deletionReceipt.durable_source_purged, true, 'source state purge receipt');
  assertEqual(deletionReceipt.provider_revocation, 'not_applicable', 'source revocation receipt');

  await stopServer();
  const databaseBytes = await readFile(database);
  assert(!databaseBytes.includes(Buffer.from(PRIVATE_TEXT)), 'database contains raw chat text');
  assert(databaseBytes.includes(Buffer.from(LEARNED_TEXT)), 'loopback consented memory text was not persisted');
  process.stdout.write('Semantic Engine Node loopback conformity: PASS\n');
} finally {
  await stopServer();
  await rm(temporaryRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

async function startServer() {
  child = spawn(
    executable,
    [
      'loopback', '--enable',
      '--audit', database,
      '--sources', sourcesDatabase,
      '--port', '0',
      '--origin', 'http://localhost',
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

async function sourceApiJson(ready, path, options = {}) {
  const response = await sourceApiFetch(ready, path, options);
  return { response, body: await response.json() };
}

async function sourceApiFetch(ready, path, { method = 'GET', json } = {}) {
  const headers = {
    Authorization: `Bearer ${ready.token}`,
    Origin: 'http://localhost',
    'X-Semantic-Engine-Protocol': String(PROTOCOL_VERSION),
  };
  if (json !== undefined) headers['Content-Type'] = 'application/json';
  return await fetch(`${ready.address}${path}`, {
    method,
    headers,
    body: json === undefined ? undefined : JSON.stringify(json),
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
}

async function firstWebSocketPayload(ready) {
  const address = ready.address.replace(/^http/, 'ws');
  const socket = new WebSocket(
    `${address}/v1/events/ws?session_id=node-loopback-session&after_sequence=0&limit=100`,
    ['semantic-engine.v2', `semantic-engine.token.${ready.token}`],
  );
  try {
    return await new Promise((resolvePayload, rejectPayload) => {
      const timer = setTimeout(() => {
        rejectPayload(new Error('WebSocket event stream timed out'));
        socket.close();
      }, REQUEST_TIMEOUT_MS);
      socket.addEventListener('open', () => {
        try {
          assertEqual(socket.protocol, 'semantic-engine.v2', 'selected WebSocket protocol');
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
