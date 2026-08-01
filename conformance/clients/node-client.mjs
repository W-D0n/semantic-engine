import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const PROTOCOL_VERSION = 1;
const MAX_LINE_BYTES = 1024 * 1024;
const REQUEST_TIMEOUT_MS = 5000;
const PRIVATE_TEXT = 'eldern ring!!!';

const executable = resolve(process.argv[2] ?? 'target/debug/semantic-engine-cli.exe');
const temporaryRoot = await mkdtemp(join(tmpdir(), 'semantic-engine-conformance-'));
const database = join(temporaryRoot, 'state.sqlite3');
const activeChildren = new Set();

try {
  const first = createClient(executable, database);
  const started = await first.request('start', 'start_session', {
    session_id: 'node-client-session',
    round: {
      id: 'node-client-round',
      targets: [{ id: 'elden-ring', canonical: 'Elden Ring', aliases: ['ER'] }],
      policy: { accept_threshold: 0.87, review_threshold: 0.72, ambiguity_margin: 0.05 },
    },
    context_package_sha256: null,
  });
  assertEqual(started.state, 'active', 'new session state');
  assertEqual(started.latest_event_sequence, 1, 'start sequence');

  const submitted = await first.request('submit', 'submit', {
    session_id: 'node-client-session',
    submission: {
      message_id: 'node-client-message',
      participant_id: 'viewer-7',
      source_sequence: 7,
      text: PRIVATE_TEXT,
    },
  });
  assertEqual(submitted.decision, 'accepted', 'fuzzy decision');
  await first.close();

  const second = createClient(executable, database);
  const restored = await second.request('get', 'get_session', {
    session_id: 'node-client-session',
  });
  assertEqual(restored.state, 'active', 'restored session state');
  assertEqual(restored.latest_event_sequence, 2, 'restored sequence');

  const duplicate = await second.request('duplicate', 'submit', {
    session_id: 'node-client-session',
    submission: {
      message_id: 'node-client-message',
      participant_id: 'viewer-7',
      source_sequence: 7,
      text: PRIVATE_TEXT,
    },
  });
  assertEqual(duplicate, submitted, 'idempotent validation response');

  const events = await second.request('events', 'events', {
    session_id: 'node-client-session',
    after_sequence: 0,
    limit: 100,
  });
  assertEqual(events.latest_sequence, 2, 'duplicate did not emit an event');
  assertEqual(events.events.length, 2, 'restored event count');
  const encodedEvents = JSON.stringify(events);
  assert(!encodedEvents.includes(PRIVATE_TEXT), 'event stream exposed raw chat text');
  assert(!encodedEvents.includes('matched_expression'), 'event stream exposed matched expression');

  const conflict = await second.requestError('conflict', 'submit', {
    session_id: 'node-client-session',
    submission: {
      message_id: 'node-client-message',
      participant_id: 'viewer-7',
      source_sequence: 7,
      text: 'different content',
    },
  });
  assertEqual(conflict.code, 'identity_conflict', 'conflict error code');
  assertEqual(conflict.retryable, false, 'conflict retryability');

  const ended = await second.request('end', 'end_session', {
    session_id: 'node-client-session',
  });
  assertEqual(ended.state, 'ended', 'ended state');
  const afterEnd = await second.requestError('after-end', 'submit', {
    session_id: 'node-client-session',
    submission: {
      message_id: 'late-message',
      participant_id: 'viewer-8',
      source_sequence: 8,
      text: 'Elden Ring',
    },
  });
  assertEqual(afterEnd.code, 'session_ended', 'post-end error code');
  await second.close();

  const databaseBytes = await readFile(database);
  assert(!databaseBytes.includes(Buffer.from(PRIVATE_TEXT)), 'database contains raw chat text');
  process.stdout.write('Semantic Engine Node client conformity: PASS\n');
} finally {
  for (const child of activeChildren) {
    child.kill();
    await new Promise((resolveExit) => {
      const timer = setTimeout(resolveExit, 1000);
      child.once('exit', () => {
        clearTimeout(timer);
        resolveExit();
      });
    });
  }
  await rm(temporaryRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

function createClient(command, statePath) {
  const child = spawn(command, ['serve', '--audit', statePath], {
    env: { ...process.env },
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });
  activeChildren.add(child);
  let stdoutBuffer = Buffer.alloc(0);
  let stderr = '';
  const pending = [];

  child.stdout.on('data', (chunk) => {
    stdoutBuffer = Buffer.concat([stdoutBuffer, chunk]);
    if (stdoutBuffer.length > MAX_LINE_BYTES) {
      failAll(new Error('sidecar response exceeded the client line limit'));
      child.kill();
      return;
    }
    let newline;
    while ((newline = stdoutBuffer.indexOf(10)) >= 0) {
      const line = stdoutBuffer.subarray(0, newline).toString('utf8').trim();
      stdoutBuffer = stdoutBuffer.subarray(newline + 1);
      if (!line) continue;
      const waiter = pending.shift();
      if (!waiter) {
        failAll(new Error('sidecar emitted an unsolicited response'));
        child.kill();
        return;
      }
      clearTimeout(waiter.timer);
      try {
        waiter.resolve(JSON.parse(line));
      } catch (error) {
        waiter.reject(new Error(`sidecar emitted invalid JSON: ${error.message}`));
      }
    }
  });
  child.stderr.on('data', (chunk) => {
    stderr = `${stderr}${chunk.toString('utf8')}`.slice(-8192);
  });
  child.on('error', failAll);
  child.on('exit', (code) => {
    activeChildren.delete(child);
    if (pending.length) failAll(new Error(`sidecar exited with code ${code}: ${stderr}`));
  });

  function failAll(error) {
    while (pending.length) {
      const waiter = pending.shift();
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
  }

  async function exchange(requestId, operation, params) {
    const envelope = {
      protocol_version: PROTOCOL_VERSION,
      request_id: requestId,
      command: operation,
      ...(params === undefined ? {} : { params }),
    };
    const line = `${JSON.stringify(envelope)}\n`;
    assert(Buffer.byteLength(line) <= MAX_LINE_BYTES, 'request exceeded client line limit');
    const response = await new Promise((resolveResponse, rejectResponse) => {
      const waiter = {
        requestId,
        resolve: resolveResponse,
        reject: rejectResponse,
        timer: undefined,
      };
      const timer = setTimeout(() => {
        const index = pending.indexOf(waiter);
        if (index >= 0) pending.splice(index, 1);
        rejectResponse(new Error(`request ${requestId} timed out`));
        child.kill();
      }, REQUEST_TIMEOUT_MS);
      waiter.timer = timer;
      pending.push(waiter);
      child.stdin.write(line, (error) => {
        if (!error) return;
        const index = pending.indexOf(waiter);
        if (index >= 0) pending.splice(index, 1);
        clearTimeout(waiter.timer);
        rejectResponse(error);
      });
    });
    assertEqual(response.protocol_version, PROTOCOL_VERSION, `${requestId} protocol version`);
    assertEqual(response.request_id, requestId, `${requestId} correlation`);
    return response;
  }

  return {
    async request(requestId, operation, params) {
      const response = await exchange(requestId, operation, params);
      assertEqual(response.status, 'ok', `${requestId} status`);
      assert(!('error' in response), `${requestId} mixed result and error`);
      return response.result;
    },
    async requestError(requestId, operation, params) {
      const response = await exchange(requestId, operation, params);
      assertEqual(response.status, 'error', `${requestId} status`);
      assert(!('result' in response), `${requestId} mixed error and result`);
      return response.error;
    },
    async close() {
      child.stdin.end();
      const code = await new Promise((resolveExit, rejectExit) => {
        const timer = setTimeout(() => {
          child.kill();
          rejectExit(new Error('sidecar did not stop after stdin closed'));
        }, REQUEST_TIMEOUT_MS);
        child.once('exit', (exitCode) => {
          clearTimeout(timer);
          resolveExit(exitCode);
        });
      });
      assertEqual(code, 0, `sidecar exit code (${stderr})`);
    },
  };
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function assertEqual(actual, expected, message) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
