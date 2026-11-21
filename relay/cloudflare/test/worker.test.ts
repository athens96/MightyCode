/**
 * The Worker relay against the Node relay's wire (../../test/relay.test.ts, push.test.ts), run in
 * `wrangler dev` (local workerd): the same sockets, close codes and push answers.
 */

import { createHash } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { WebSocket } from 'ws';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('cloudflare:workers', () => ({
  DurableObject: class DurableObject<E = unknown> {
    protected ctx: DurableObjectState;
    protected env: E;
    constructor(ctx: DurableObjectState, env: E) {
      this.ctx = ctx;
      this.env = env;
    }
  },
}));

import { Hub, HUB_HEADERS, type HubEnv } from '../src/hub.ts';
import {
  installWorkersGlobals,
  MockDurableObjectState,
  startWorker,
  type LocalWorker,
  type MockWebSocket,
} from './harness.ts';

let BASE = '';
const MAX_CONNECTIONS = 3;
const MAX_BUFFERED = 4;
const MAX_PENDING_PER_IP = 2;

interface FrameInfo {
  data: Buffer;
  binary: boolean;
}

const sha256Hex = (value: string): string => createHash('sha256').update(value, 'utf8').digest('hex');

function identity(label: string): { serverId: string; hostToken: string } {
  const hostToken = sha256Hex(`host-secret:${label}`);
  return { serverId: sha256Hex(hostToken), hostToken };
}

let worker: LocalWorker | undefined;
let state: string;
const open: WebSocket[] = [];

function socket(query: Record<string, string>, path = '/ws'): WebSocket {
  const ws = new WebSocket(`ws://${BASE}${path}?${new URLSearchParams(query).toString()}`);
  ws.binaryType = 'nodebuffer';
  ws.on('error', () => {});
  open.push(ws);
  return ws;
}

function opened(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.once('open', () => resolve());
    ws.once('error', reject);
  });
}

function closed(ws: WebSocket): Promise<{ code: number; reason: string }> {
  return new Promise((resolve) => {
    ws.once('close', (code: number, reason: Buffer) => resolve({ code, reason: reason.toString('utf8') }));
  });
}

function nextFrame(ws: WebSocket): Promise<FrameInfo> {
  return new Promise((resolve) => ws.once('message', (data: Buffer, binary: boolean) => resolve({ data, binary })));
}

async function nextJson(ws: WebSocket): Promise<Record<string, unknown>> {
  return JSON.parse((await nextFrame(ws)).data.toString('utf8')) as Record<string, unknown>;
}

function collect(ws: WebSocket, count: number): Promise<FrameInfo[]> {
  return new Promise((resolve) => {
    const frames: FrameInfo[] = [];
    const onMessage = (data: Buffer, binary: boolean): void => {
      frames.push({ data, binary });
      if (frames.length === count) {
        ws.off('message', onMessage);
        resolve(frames);
      }
    };
    ws.on('message', onMessage);
  });
}

async function pair(label: string, connectionId: string): Promise<{ control: WebSocket; client: WebSocket; host: WebSocket }> {
  const { serverId, hostToken } = identity(label);
  const control = socket({ serverId, role: 'server', v: '1', hostToken });
  await opened(control);
  const client = socket({ serverId, role: 'client', connectionId, v: '1' });
  const connected = nextJson(control);
  await opened(client);
  await connected;
  const host = socket({ serverId, role: 'server', connectionId, v: '1', hostToken });
  await opened(host);
  return { control, client, host };
}

async function http(path: string, init?: RequestInit): Promise<Response> {
  return fetch(`http://${BASE}${path}`, init);
}

beforeAll(async () => {
  state = mkdtempSync(join(tmpdir(), 'mighty-relay-cf-'));
  worker = await startWorker(state, {
    RELAY_ATTACH_TIMEOUT_MS: '800',
    RELAY_MAX_CONNECTIONS: String(MAX_CONNECTIONS),
    RELAY_MAX_BUFFERED_FRAMES: String(MAX_BUFFERED),
    RELAY_MAX_BUFFERED_BYTES: String(64 * 1024),
    RELAY_AUTH_TIMEOUT_MS: '500',
    RELAY_HELLO_TIMEOUT_MS: '1500',
    RELAY_MAX_PENDING_PER_IP: String(MAX_PENDING_PER_IP),
  });
  BASE = worker.base;
}, 120_000);

afterAll(async () => {
  for (const ws of open) ws.terminate();
  await worker?.stop();
  if (state) rmSync(state, { recursive: true, force: true });
});

describe('http surface', () => {
  it('answers GET /healthz with 200 ok and 404s other paths', async () => {
    const health = await http('/healthz');
    expect(health.status).toBe(200);
    expect(await health.text()).toBe('ok');
    expect((await http('/nope')).status).toBe(404);
  });

  it('refuses websocket upgrades outside /ws', async () => {
    const ws = socket({ v: '1' }, '/other');
    await expect(opened(ws)).rejects.toThrow();
  });
});

describe('query validation', () => {
  it('closes 4400 on a bad version, a bad serverId and a client without connectionId', async () => {
    expect((await closed(socket({ v: '2', serverId: 'abc', role: 'server' }))).code).toBe(4400);
    expect((await closed(socket({ v: '1', serverId: '!bad', role: 'server' }))).code).toBe(4400);
    expect((await closed(socket({ v: '1', serverId: 'abc', role: 'client' }))).code).toBe(4400);
  });
});

describe('pairing', () => {
  it('pipes text and binary frames both ways', async () => {
    const { client, host } = await pair('pipe-1', 'conn-00000001');
    client.send('client-to-host');
    const a = await nextFrame(host);
    expect([a.binary, a.data.toString('utf8')]).toEqual([false, 'client-to-host']);
    host.send('host-to-client');
    const b = await nextFrame(client);
    expect([b.binary, b.data.toString('utf8')]).toEqual([false, 'host-to-client']);
    client.send(Buffer.from([1, 2, 3, 255]));
    const c = await nextFrame(host);
    expect([c.binary, [...c.data]]).toEqual([true, [1, 2, 3, 255]]);
    host.send(Buffer.from([9, 8]));
    const d = await nextFrame(client);
    expect([d.binary, [...d.data]]).toEqual([true, [9, 8]]);
  });

  it('buffers client frames and flushes them in order', async () => {
    const { serverId, hostToken } = identity('buffer-1');
    const connectionId = 'conn-00000002';
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const client = socket({ serverId, role: 'client', connectionId, v: '1' });
    const connected = nextJson(control);
    await opened(client);
    expect(await connected).toEqual({ type: 'connected', connectionId });
    client.send('one');
    client.send('two');
    client.send(Buffer.from([3]));
    await new Promise((resolve) => setTimeout(resolve, 100));
    const host = socket({ serverId, role: 'server', connectionId, v: '1', hostToken });
    const frames = collect(host, 3);
    await opened(host);
    const received = await frames;
    expect(received.map((f) => f.data.toString('utf8'))).toEqual(['one', 'two', '\u0003']);
    expect(received.map((f) => f.binary)).toEqual([false, false, true]);
  });

  it('closes a client 4404 when no host control socket exists', async () => {
    expect((await closed(socket({ serverId: 'ghost-host', role: 'client', connectionId: 'conn-00000003', v: '1' }))).code).toBe(4404);
  });

  it('closes a host data socket 4404 for an unknown connectionId', async () => {
    const { serverId, hostToken } = identity('unknown-1');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    expect((await closed(socket({ serverId, role: 'server', connectionId: 'conn-00000004', v: '1', hostToken }))).code).toBe(4404);
  });

  it('closes the client 4504 when the host never attaches and tells the control socket', async () => {
    const { serverId, hostToken } = identity('slow-host');
    const connectionId = 'conn-00000005';
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const notices = collect(control, 2);
    const client = socket({ serverId, role: 'client', connectionId, v: '1' });
    await opened(client);
    expect((await closed(client)).code).toBe(4504);
    expect((await notices).map((f) => JSON.parse(f.data.toString('utf8')) as unknown)).toEqual([
      { type: 'connected', connectionId },
      { type: 'disconnected', connectionId },
    ]);
  });

  it('carries a frame at the 1 MiB limit', async () => {
    const { client, host } = await pair('big-frame', 'conn-00000009');
    const big = Buffer.alloc(1024 * 1024, 7);
    client.send(big);
    const got = await nextFrame(host);
    expect(got.data.byteLength).toBe(big.byteLength);
  });
});

describe('control socket', () => {
  it('closes the previous control socket with 4409 and the new one inherits pending state', async () => {
    const { serverId, hostToken } = identity('dup-control');
    const connectionId = 'conn-00000006';
    const first = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(first);
    const client = socket({ serverId, role: 'client', connectionId, v: '1' });
    const connected = nextJson(first);
    await opened(client);
    expect(await connected).toEqual({ type: 'connected', connectionId });
    const second = socket({ serverId, role: 'server', v: '1', hostToken });
    const firstClosed = closed(first);
    await opened(second);
    // The client is still waiting for its host half (800 ms here), now under the new control socket.
    const host = socket({ serverId, role: 'server', connectionId, v: '1', hostToken });
    await opened(host);
    client.send('after-takeover');
    expect((await nextFrame(host)).data.toString('utf8')).toBe('after-takeover');
    expect((await firstClosed).code).toBe(4409);
  });

  it('mirrors the close code and notifies disconnected', async () => {
    const connectionId = 'conn-00000007';
    const { control, client, host } = await pair('close-mirror', connectionId);
    const hostClosed = closed(host);
    const notice = nextJson(control);
    client.close(4321, 'bye');
    expect(await hostClosed).toEqual({ code: 4321, reason: 'bye' });
    expect(await notice).toEqual({ type: 'disconnected', connectionId });
  });

  it('closes data sockets with 4410 when the control socket goes away', async () => {
    const { control, client, host } = await pair('host-offline', 'conn-00000008');
    const clientClosed = closed(client);
    const hostClosed = closed(host);
    control.close(1000, 'done');
    expect((await clientClosed).code).toBe(4410);
    expect((await hostClosed).code).toBe(4410);
  });

  it('answers a TURN request turn-unavailable and ignores anything else', async () => {
    const { serverId, hostToken } = identity('turn-off');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    control.send('not json');
    control.send(JSON.stringify({ type: 'something-else' }));
    control.send(JSON.stringify({ type: 'turn-credentials-request' }));
    expect(await nextJson(control)).toEqual({ type: 'turn-unavailable' });
  });
});

describe('host ownership', () => {
  it('refuses a control socket without the matching token', async () => {
    const { serverId } = identity('owned');
    const other = identity('intruder');
    expect((await closed(socket({ serverId, role: 'server', v: '1' }))).code).toBe(4401);
    expect((await closed(socket({ serverId, role: 'server', v: '1', hostToken: other.hostToken }))).code).toBe(4401);
  });

  it('refuses a host data socket without the matching token and still pairs the real one', async () => {
    const { serverId, hostToken } = identity('owned-data');
    const connectionId = 'conn-00000010';
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const client = socket({ serverId, role: 'client', connectionId, v: '1' });
    const connected = nextJson(control);
    await opened(client);
    await connected;
    const fake = socket({ serverId, role: 'server', connectionId, v: '1', hostToken: identity('x').hostToken });
    expect((await closed(fake)).code).toBe(4401);
    const host = socket({ serverId, role: 'server', connectionId, v: '1', hostToken });
    await opened(host);
    client.send('real');
    expect((await nextFrame(host)).data.toString('utf8')).toBe('real');
  });
});

describe('limits', () => {
  it(`rejects connection ${MAX_CONNECTIONS + 1} with 4429`, async () => {
    const { serverId, hostToken } = identity('crowd');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    for (let i = 0; i < MAX_CONNECTIONS; i++) {
      const client = socket({ serverId, role: 'client', connectionId: `crowd-0000000${i}`, v: '1' });
      await opened(client);
      client.send('hello');
      const host = socket({ serverId, role: 'server', connectionId: `crowd-0000000${i}`, v: '1', hostToken });
      await opened(host);
      const answer = nextFrame(client);
      host.send('ready');
      await answer;
    }
    expect((await closed(socket({ serverId, role: 'client', connectionId: 'crowd-overflow', v: '1' }))).code).toBe(4429);
  });

  it('closes 4413 when the client overruns the buffer before the host attaches', async () => {
    const { serverId, hostToken } = identity('overrun');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const client = socket({ serverId, role: 'client', connectionId: 'conn-00000011', v: '1' });
    await opened(client);
    const done = closed(client);
    for (let i = 0; i <= MAX_BUFFERED; i++) client.send(`f${i}`);
    expect((await done).code).toBe(4413);
  });

  it('closes 4413 when pending client frames exceed RELAY_MAX_BUFFERED_BYTES before frame cap', async () => {
    const { serverId, hostToken } = identity('byte-overrun');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const client = socket({ serverId, role: 'client', connectionId: 'conn-byte-0001', v: '1' });
    await opened(client);
    const done = closed(client);
    // 2 x 40 KiB = 80 KiB > 64 KiB cap, well below MAX_BUFFERED (4 frames).
    client.send(Buffer.alloc(40 * 1024, 1));
    client.send(Buffer.alloc(40 * 1024, 2));
    expect((await done).code).toBe(4413);
  });
});

describe('push', () => {
  const token = 'fcm-token-'.padEnd(40, 'x');

  it('registers, refreshes and deletes a handle with its secret', async () => {
    const made = await http('/push/v1/handles', { method: 'POST', body: JSON.stringify({ fcmToken: token }), headers: { 'content-type': 'application/json' } });
    expect(made.status).toBe(201);
    const { handle, secret } = (await made.json()) as { handle: string; secret: string };
    expect(handle).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(secret).toMatch(/^[A-Za-z0-9_-]{43}$/);
    const put = (auth: string) => http(`/push/v1/handles/${handle}`, { method: 'PUT', body: JSON.stringify({ fcmToken: `${token}2` }), headers: { authorization: `Bearer ${auth}` } });
    expect((await put(secret)).status).toBe(204);
    expect((await put('A'.repeat(43))).status).toBe(404);
    expect((await http(`/push/v1/handles/${handle}`, { method: 'DELETE', headers: { authorization: `Bearer ${secret}` } })).status).toBe(204);
    expect((await put(secret)).status).toBe(404);
  });

  it('refuses bad bodies and unknown routes', async () => {
    expect((await http('/push/v1/handles', { method: 'POST', body: 'nope' })).status).toBe(400);
    expect((await http('/push/v1/handles', { method: 'POST', body: JSON.stringify({ fcmToken: 'short' }) })).status).toBe(400);
    expect((await http('/push/v1/handles', { method: 'POST', body: JSON.stringify({ fcmToken: 'x'.repeat(9000) }) })).status).toBe(413);
    expect((await http('/push/v1/other', { method: 'POST', body: '{}' })).status).toBe(404);
  });

  it('checks the host token before anything else and answers 503 without an FCM key', async () => {
    const made = await http('/push/v1/handles', { method: 'POST', body: JSON.stringify({ fcmToken: token }) });
    const { handle } = (await made.json()) as { handle: string };
    const { serverId, hostToken } = identity('push-host');
    const send = (auth: string | null) =>
      http('/push/v1/send', {
        method: 'POST',
        body: JSON.stringify({ serverId, handle, payload: 'p'.repeat(40) }),
        headers: auth === null ? {} : { authorization: `Bearer ${auth}` },
      });
    expect((await send(null)).status).toBe(401);
    expect((await send(identity('other').hostToken)).status).toBe(401);
    const unavailable = await send(hostToken);
    expect(unavailable.status).toBe(503);
    expect(await unavailable.json()).toEqual({ error: 'push-unavailable' });
  });
});

const hostAuth = (hostToken: string): string => JSON.stringify({ type: 'host-auth', hostToken });

/**
 * The close code, or 'open' if the socket is still up after `ms`. Under `wrangler dev` the
 * close event comes some 10 s after the relay's close frame (the TCP close lags), hence the wait.
 */
function closeCode(ws: WebSocket, ms = 15_000): Promise<number | 'open'> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve('open'), ms);
    ws.once('close', (code: number) => {
      clearTimeout(timer);
      resolve(code);
    });
  });
}

describe('host-auth frame (token out of the URL)', () => {
  it('opens a control socket and a host data socket that bring the token in their first frame', async () => {
    const { serverId, hostToken } = identity('frame-1');
    const connectionId = 'conn-frame-01';
    const control = socket({ serverId, role: 'server', v: '1' });
    await opened(control);
    control.send(hostAuth(hostToken));
    await new Promise((resolve) => setTimeout(resolve, 100));
    const connected = nextJson(control);
    const client = socket({ serverId, role: 'client', connectionId, v: '1' });
    await opened(client);
    expect(await connected).toEqual({ type: 'connected', connectionId });
    client.send('hello-from-client');
    const host = socket({ serverId, role: 'server', connectionId, v: '1' });
    const buffered = nextFrame(host);
    await opened(host);
    host.send(hostAuth(hostToken));
    host.send('ready-from-host');
    expect((await nextFrame(client)).data.toString('utf8')).toBe('ready-from-host');
    expect((await buffered).data.toString('utf8')).toBe('hello-from-client');
  }, 40_000);

  it('closes 4401 on a wrong token, a binary first frame and no frame in time, and none becomes the host', async () => {
    const { serverId, hostToken } = identity('frame-2');
    // Two at a time: one address may keep only MAX_PENDING_PER_IP sockets waiting.
    const [wrong, mute] = [0, 1].map(() => socket({ serverId, role: 'server', v: '1' })) as [WebSocket, WebSocket];
    await Promise.all([opened(wrong), opened(mute)]);
    const codes = Promise.all([closeCode(wrong), closeCode(mute)]);
    wrong.send(hostAuth(identity('frame-2-other').hostToken));
    expect(await codes).toEqual([4401, 4401]);
    const binary = socket({ serverId, role: 'server', v: '1' });
    await opened(binary);
    const binaryCode = closeCode(binary);
    binary.send(Buffer.from(hostAuth(hostToken)));
    expect(await binaryCode).toBe(4401);
    expect((await closed(socket({ serverId, role: 'client', connectionId: 'conn-frame-02', v: '1' }))).code).toBe(4404);
  }, 40_000);

  it(`keeps one address to ${MAX_PENDING_PER_IP} sockets owing their host-auth frame`, async () => {
    const { serverId, hostToken } = identity('frame-3');
    const waiting = Array.from({ length: MAX_PENDING_PER_IP }, () => socket({ serverId, role: 'server', v: '1' }));
    await Promise.all(waiting.map(opened));
    expect(await closeCode(socket({ serverId, role: 'server', v: '1' }))).toBe(4429);
    waiting[0]!.send(hostAuth(hostToken));
    await new Promise((resolve) => setTimeout(resolve, 100));
    const next = socket({ serverId, role: 'server', v: '1' });
    await opened(next);
    next.send(hostAuth(hostToken));
    // The newest control socket replaces the one that authenticated first.
    expect(await closeCode(waiting[0]!)).toBe(4409);
  }, 40_000);
});

describe('pre-auth slot squatting', () => {
  it('closes 4408 a client that sends nothing within the hello deadline and keeps one that does', async () => {
    const { serverId, hostToken } = identity('hello-1');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const mute = socket({ serverId, role: 'client', connectionId: 'conn-mute-0001', v: '1' });
    const talker = socket({ serverId, role: 'client', connectionId: 'conn-talk-0001', v: '1' });
    await Promise.all([opened(mute), opened(talker)]);
    talker.send('hello');
    // Both host halves attach, so the attach timeout is out of the picture.
    const muteHost = socket({ serverId, role: 'server', connectionId: 'conn-mute-0001', v: '1', hostToken });
    const talkerHost = socket({ serverId, role: 'server', connectionId: 'conn-talk-0001', v: '1', hostToken });
    await Promise.all([opened(muteHost), opened(talkerHost)]);
    expect(await closeCode(mute)).toBe(4408);
    expect(await closeCode(talker, 500)).toBe('open');
  }, 40_000);

  it(`keeps one address to ${MAX_PENDING_PER_IP} client sockets the host has not answered`, async () => {
    const { serverId, hostToken } = identity('pending-1');
    const control = socket({ serverId, role: 'server', v: '1', hostToken });
    await opened(control);
    const first = socket({ serverId, role: 'client', connectionId: 'conn-pend-0001', v: '1' });
    const second = socket({ serverId, role: 'client', connectionId: 'conn-pend-0002', v: '1' });
    await Promise.all([opened(first), opened(second)]);
    first.send('hello');
    second.send('hello');
    expect(await closeCode(socket({ serverId, role: 'client', connectionId: 'conn-pend-0003', v: '1' }))).toBe(4429);
    const host = socket({ serverId, role: 'server', connectionId: 'conn-pend-0001', v: '1', hostToken });
    await opened(host);
    const answer = nextFrame(first);
    host.send('ready');
    await answer;
    const next = socket({ serverId, role: 'client', connectionId: 'conn-pend-0004', v: '1' });
    await opened(next);
    next.send('hello');
    expect(await closeCode(next, 300)).toBe('open');
  }, 40_000);
});

describe('Hub Durable Object alarms & hibernation', () => {
  let restoreGlobals: (() => void) | undefined;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-11-21T00:00:00.000Z'));
    restoreGlobals = installWorkersGlobals();
  });

  afterEach(() => {
    restoreGlobals?.();
    restoreGlobals = undefined;
    vi.useRealTimers();
  });

  function hubRequest(headers: Record<string, string>): Request {
    return new Request('https://hub.invalid/ws', { method: 'GET', headers });
  }

  async function openSocket(hub: Hub, headers: Record<string, string>): Promise<MockWebSocket> {
    const res = (await hub.fetch(hubRequest(headers))) as Response & { webSocket: MockWebSocket | null };
    expect(res.status).toBe(101);
    const server = res.webSocket?.peer;
    if (!server) throw new Error('expected server WebSocket from Hub.fetch');
    return server;
  }

  it('closes an expired pre-auth socket via alarm() even after simulated DO hibernation', async () => {
    const { serverId } = identity('alarm-auth-hibernate');
    const ctx = new MockDurableObjectState();
    const env: HubEnv = { RELAY_AUTH_TIMEOUT_MS: '500', RELAY_MAX_PENDING_PER_IP: '2' };
    const hub = new Hub(ctx as unknown as DurableObjectState, env);

    const serverWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'control',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.ip]: '203.0.113.10',
      [HUB_HEADERS.auth]: 'frame',
    });

    expect(await ctx.storage.getAlarm()).toBe(Date.now() + 500);
    // No permanent 'auth' tag is ever attached.
    expect(ctx.getWebSockets('auth')).toEqual([]);
    expect(ctx.getTags(serverWs)).toEqual(['control']);

    // Simulate DO hibernation/eviction: discard `hub` and reconstruct a fresh Hub on the same `ctx`.
    const wokenHub = new Hub(ctx as unknown as DurableObjectState, env);
    vi.advanceTimersByTime(500);

    expect(await ctx.fireAlarm(wokenHub)).toBe(true);
    expect(serverWs.closed).toEqual({ code: 4401, reason: 'host-auth frame missing' });
    expect(await ctx.storage.getAlarm()).toBeNull();
  });

  it('never leaks an auth tag on authenticated control or host-data sockets and clears the alarm', async () => {
    const { serverId, hostToken } = identity('no-auth-tag-leak');
    const ctx = new MockDurableObjectState();
    const env: HubEnv = { RELAY_AUTH_TIMEOUT_MS: '500', RELAY_HELLO_TIMEOUT_MS: '1500', RELAY_ATTACH_TIMEOUT_MS: '2000' };
    const hub = new Hub(ctx as unknown as DurableObjectState, env);

    const controlWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'control',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.ip]: '203.0.113.11',
      [HUB_HEADERS.auth]: 'frame',
    });
    expect(ctx.getWebSockets('auth')).toEqual([]);
    expect(await ctx.storage.getAlarm()).toBe(Date.now() + 500);

    await hub.webSocketMessage(controlWs as unknown as globalThis.WebSocket, hostAuth(hostToken));
    expect(ctx.getWebSockets('auth')).toEqual([]);
    expect(ctx.getTags(controlWs)).toEqual(['control']);
    expect(await ctx.storage.getAlarm()).toBeNull();

    const clientWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'client-data',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.connectionId]: 'conn-clean-1',
      [HUB_HEADERS.ip]: '198.51.100.5',
      [HUB_HEADERS.auth]: 'query',
    });
    await hub.webSocketMessage(clientWs as unknown as globalThis.WebSocket, 'hello');

    const hostWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'host-data',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.connectionId]: 'conn-clean-1',
      [HUB_HEADERS.ip]: '203.0.113.11',
      [HUB_HEADERS.auth]: 'frame',
    });
    await hub.webSocketMessage(hostWs as unknown as globalThis.WebSocket, hostAuth(hostToken));

    expect(ctx.getWebSockets('auth')).toEqual([]);
    expect(ctx.getTags(hostWs)).toEqual(['host', 'h:conn-clean-1']);
    expect(hostWs.sent).toEqual(['hello']);
    expect(await ctx.storage.getAlarm()).toBeNull();
  });

  it('closes an expired pre-hello client socket via alarm() with 4408 after simulated DO hibernation', async () => {
    const { serverId } = identity('alarm-hello-hibernate');
    const ctx = new MockDurableObjectState();
    const env: HubEnv = { RELAY_HELLO_TIMEOUT_MS: '600', RELAY_ATTACH_TIMEOUT_MS: '2000' };
    const hub = new Hub(ctx as unknown as DurableObjectState, env);

    const controlWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'control',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.ip]: '203.0.113.20',
      [HUB_HEADERS.auth]: 'query',
    });
    const clientWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'client-data',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.connectionId]: 'conn-mute-hib',
      [HUB_HEADERS.ip]: '198.51.100.20',
      [HUB_HEADERS.auth]: 'query',
    });
    // Host attaches promptly, clearing attachDeadline while helloDeadline stays persisted.
    const hostWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'host-data',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.connectionId]: 'conn-mute-hib',
      [HUB_HEADERS.ip]: '203.0.113.20',
      [HUB_HEADERS.auth]: 'query',
    });
    expect(await ctx.storage.getAlarm()).toBe(Date.now() + 600);

    // Simulate DO hibernation: fresh Hub instance has no in-memory timers or maps.
    const wokenHub = new Hub(ctx as unknown as DurableObjectState, env);
    vi.advanceTimersByTime(600);

    expect(await ctx.fireAlarm(wokenHub)).toBe(true);
    expect(clientWs.closed).toEqual({ code: 4408, reason: 'hello timeout' });
    expect(hostWs.closed).toEqual({ code: 4408, reason: 'hello timeout' });
    expect(controlWs.sent.map((frame) => JSON.parse(String(frame)) as unknown)).toEqual([
      { type: 'connected', connectionId: 'conn-mute-hib' },
      { type: 'disconnected', connectionId: 'conn-mute-hib' },
    ]);
    expect(await ctx.storage.getAlarm()).toBeNull();
  });

  it('closes an unattached client socket via alarm() with 4504 after hello and simulated DO hibernation', async () => {
    const { serverId } = identity('alarm-attach-hibernate');
    const ctx = new MockDurableObjectState();
    const env: HubEnv = { RELAY_HELLO_TIMEOUT_MS: '500', RELAY_ATTACH_TIMEOUT_MS: '1200' };
    const hub = new Hub(ctx as unknown as DurableObjectState, env);

    const controlWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'control',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.ip]: '203.0.113.30',
      [HUB_HEADERS.auth]: 'query',
    });
    const start = Date.now();
    const clientWs = await openSocket(hub, {
      [HUB_HEADERS.kind]: 'client-data',
      [HUB_HEADERS.serverId]: serverId,
      [HUB_HEADERS.connectionId]: 'conn-attach-hib',
      [HUB_HEADERS.ip]: '198.51.100.30',
      [HUB_HEADERS.auth]: 'query',
    });
    expect(await ctx.storage.getAlarm()).toBe(start + 500);

    // Client sends hello at +100ms: helloDeadline clears and alarm shifts to attachDeadline (+1200ms).
    vi.advanceTimersByTime(100);
    await hub.webSocketMessage(clientWs as unknown as globalThis.WebSocket, 'hello');
    expect(await ctx.storage.getAlarm()).toBe(start + 1200);

    // Simulate DO hibernation and advance to attachDeadline.
    const wokenHub = new Hub(ctx as unknown as DurableObjectState, env);
    vi.advanceTimersByTime(1100);

    expect(await ctx.fireAlarm(wokenHub)).toBe(true);
    expect(clientWs.closed).toEqual({ code: 4504, reason: 'host data socket timeout' });
    expect(controlWs.sent.map((frame) => JSON.parse(String(frame)) as unknown)).toEqual([
      { type: 'connected', connectionId: 'conn-attach-hib' },
      { type: 'disconnected', connectionId: 'conn-attach-hib' },
    ]);
    expect(await ctx.storage.getAlarm()).toBeNull();
  });
});
