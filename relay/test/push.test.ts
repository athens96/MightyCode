import { createHash } from 'node:crypto';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { connect } from 'node:net';
import { join } from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import type { PushSendResult, PushSender } from '../src/fcm.ts';
import { pushIpKey } from '../src/push.ts';
import { startRelay, type RelayHandle } from '../src/server.ts';

const sha256Hex = (value: string): string => createHash('sha256').update(value, 'utf8').digest('hex');

function identity(label: string): { serverId: string; hostToken: string } {
  const hostToken = sha256Hex(`host-secret:${label}`);
  return { serverId: sha256Hex(hostToken), hostToken };
}

class FakeSender implements PushSender {
  readonly sent: Array<{ token: string; data: Readonly<Record<string, string>> }> = [];
  next: PushSendResult = { kind: 'sent' };
  async send(token: string, data: Readonly<Record<string, string>>): Promise<PushSendResult> {
    this.sent.push({ token, data });
    return this.next;
  }
}

const FCM_TOKEN = 'fcm-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const PAYLOAD = 'A'.repeat(100);
const relays: RelayHandle[] = [];

afterEach(async () => {
  for (const relay of relays.splice(0)) await relay.close();
});

async function relay(overrides: Parameters<typeof startRelay>[0] = {}): Promise<{ base: string; sender: FakeSender }> {
  const sender = new FakeSender();
  const handle = await startRelay({ host: '127.0.0.1', port: 0, logger: () => {}, pushSender: sender, ...overrides });
  relays.push(handle);
  return { base: `http://127.0.0.1:${handle.port}`, sender };
}

function call(base: string, method: string, path: string, body?: unknown, bearer?: string): Promise<Response> {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (bearer !== undefined) headers['authorization'] = `Bearer ${bearer}`;
  return fetch(`${base}${path}`, { method, headers, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
}

async function register(base: string): Promise<{ handle: string; secret: string }> {
  const res = await call(base, 'POST', '/push/v1/handles', { fcmToken: FCM_TOKEN });
  expect(res.status).toBe(201);
  return (await res.json()) as { handle: string; secret: string };
}

function send(base: string, who: { serverId: string; hostToken: string }, handle: string, payload = PAYLOAD): Promise<Response> {
  return call(base, 'POST', '/push/v1/send', { serverId: who.serverId, handle, payload }, who.hostToken);
}

/**
 * Writes `request` on a raw socket (and, when `keepSending`, keeps writing chunks after it
 * like a client streaming a huge body), then gives the response head and whether the relay
 * closed the connection within two seconds.
 */
function rawRequest(port: number, request: string, keepSending = false): Promise<{ head: string; closed: boolean }> {
  return new Promise((resolve) => {
    const socket = connect(port, '127.0.0.1');
    let received = '';
    let pump: NodeJS.Timeout | undefined;
    const finish = (closed: boolean): void => {
      clearInterval(pump);
      clearTimeout(timer);
      socket.destroy();
      resolve({ head: received.split('\r\n\r\n')[0] ?? '', closed });
    };
    const timer = setTimeout(() => finish(false), 2000);
    socket.on('data', (data: Buffer) => { received += data.toString('latin1'); });
    socket.on('close', () => finish(true));
    socket.on('error', () => {});
    socket.write(request);
    if (keepSending) {
      const more = `${(4096).toString(16)}\r\n${'y'.repeat(4096)}\r\n`;
      pump = setInterval(() => { if (!socket.destroyed) socket.write(more); }, 5);
    }
  });
}

describe('push handles', () => {
  it('hands out a handle and a secret for a token', async () => {
    const { base } = await relay();
    const made = await register(base);
    expect(made.handle).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(made.secret).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });

  it('refuses a malformed token or body', async () => {
    const { base } = await relay();
    expect((await call(base, 'POST', '/push/v1/handles', { fcmToken: 'short' })).status).toBe(400);
    expect((await call(base, 'POST', '/push/v1/handles', 'nope')).status).toBe(400);
  });

  it('refreshes and deletes only with the secret', async () => {
    const { base, sender } = await relay();
    const host = identity('refresh');
    const made = await register(base);
    const NEW = 'fcm-token-bbbbbbbbbbbbbbbbbbbbbbbbbbbb';
    expect((await call(base, 'PUT', `/push/v1/handles/${made.handle}`, { fcmToken: NEW }, 'x'.repeat(43))).status).toBe(404);
    expect((await call(base, 'PUT', `/push/v1/handles/${made.handle}`, { fcmToken: NEW }, made.secret)).status).toBe(204);
    expect((await send(base, host, made.handle)).status).toBe(202);
    expect(sender.sent.at(-1)?.token).toBe(NEW);
    expect((await call(base, 'DELETE', `/push/v1/handles/${made.handle}`, undefined, made.secret)).status).toBe(204);
    expect((await send(base, host, made.handle)).status).toBe(404);
  });

  it('limits handle operations per IP', async () => {
    const { base } = await relay({ pushMaxHandleOpsPerIp: 2 });
    await register(base);
    await register(base);
    const res = await call(base, 'POST', '/push/v1/handles', { fcmToken: FCM_TOKEN });
    expect(res.status).toBe(429);
    expect(Number(res.headers.get('retry-after'))).toBeGreaterThan(0);
  });
});

describe('push limits', () => {
  function registerFrom(base: string, ip: string, fcmToken = FCM_TOKEN): Promise<Response> {
    return fetch(`${base}/push/v1/handles`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-forwarded-for': ip },
      body: JSON.stringify({ fcmToken }),
    });
  }

  it('keys IPv6 by its /64 and IPv4 as is', () => {
    expect(pushIpKey('2001:db8:1:2:aaaa::1')).toBe('2001:db8:1:2::/64');
    expect(pushIpKey('2001:0db8:0001:0002:ffff:ffff:ffff:ffff')).toBe('2001:db8:1:2::/64');
    expect(pushIpKey('2001:db8::1')).toBe('2001:db8:0:0::/64');
    expect(pushIpKey('::1')).toBe('0:0:0:0::/64');
    expect(pushIpKey('203.0.113.7')).toBe('203.0.113.7');
    expect(pushIpKey('::ffff:203.0.113.7')).toBe('203.0.113.7');
    expect(pushIpKey('::ffff:cb00:7107')).toBe('203.0.113.7');
    expect(pushIpKey('fe80::1%en0')).toBe('fe80:0:0:0::/64');
  });

  it('shares one handle budget across a whole IPv6 /64', async () => {
    const { base } = await relay({ trustProxy: true, pushMaxHandleOpsPerIp: 2 });
    expect((await registerFrom(base, '2001:db8:1:2::1')).status).toBe(201);
    expect((await registerFrom(base, '2001:db8:1:2::2')).status).toBe(201);
    expect((await registerFrom(base, '2001:db8:1:2:ffff::3')).status).toBe(429);
    expect((await registerFrom(base, '2001:db8:1:3::1')).status).toBe(201);
  });

  it('counts an IPv4-mapped address as its IPv4 address', async () => {
    const { base } = await relay({ trustProxy: true, pushMaxHandleOpsPerIp: 1 });
    expect((await registerFrom(base, '203.0.113.7')).status).toBe(201);
    expect((await registerFrom(base, '::ffff:203.0.113.7')).status).toBe(429);
  });

  it('limits registrations of new tokens across all IPs', async () => {
    const { base } = await relay({ trustProxy: true, pushMaxRegistrationsGlobal: 2 });
    expect((await registerFrom(base, '203.0.113.1', `${FCM_TOKEN}1`)).status).toBe(201);
    expect((await registerFrom(base, '203.0.113.2', `${FCM_TOKEN}2`)).status).toBe(201);
    const res = await registerFrom(base, '203.0.113.3', `${FCM_TOKEN}3`);
    expect(res.status).toBe(429);
    expect(Number(res.headers.get('retry-after'))).toBeGreaterThan(0);
    // A phone pairing one more host, or registering again, adds no new token and still gets through.
    expect((await registerFrom(base, '203.0.113.1', `${FCM_TOKEN}1`)).status).toBe(201);
  });

  it('limits registrations per IP per day', async () => {
    const { base } = await relay({ trustProxy: true, pushMaxRegistrationsPerIpDay: 2 });
    expect((await registerFrom(base, '203.0.113.9', `${FCM_TOKEN}a`)).status).toBe(201);
    expect((await registerFrom(base, '203.0.113.9', `${FCM_TOKEN}b`)).status).toBe(201);
    const res = await registerFrom(base, '203.0.113.9', `${FCM_TOKEN}c`);
    expect(res.status).toBe(429);
    expect(Number(res.headers.get('retry-after'))).toBeGreaterThan(600);
    expect((await registerFrom(base, '203.0.113.10', `${FCM_TOKEN}c`)).status).toBe(201);
  });

  it('replaces the oldest handle of a token past its cap instead of growing', async () => {
    const { base } = await relay({ pushMaxHandlesPerToken: 2 });
    const first = await register(base);
    const second = await register(base);
    const third = await register(base);
    const host = identity('per-token');
    expect((await send(base, host, first.handle)).status).toBe(404);
    expect((await send(base, host, second.handle)).status).toBe(202);
    expect((await send(base, identity('per-token-2'), third.handle)).status).toBe(202);
  });

  it('keeps a serverId to its quota of bound handles', async () => {
    const { base } = await relay({ pushMaxHandlesPerServerId: 1 });
    const host = identity('quota');
    const older = await register(base);
    expect((await send(base, host, older.handle)).status).toBe(202);
    const res = await call(base, 'POST', '/push/v1/handles', { fcmToken: `${FCM_TOKEN}-other` });
    const newer = (await res.json()) as { handle: string };
    expect((await send(base, host, newer.handle)).status).toBe(202);
    expect((await send(base, host, older.handle)).status).toBe(404);
  });

  it('logs a full store without identifiers and makes room from unused handles', async () => {
    const lines: string[] = [];
    const { base } = await relay({ pushMaxHandles: 1, logger: (line) => lines.push(line) });
    const first = await register(base);
    await register(base);
    expect(lines).toContain('[push] handle store full');
    expect((await send(base, identity('host-a'), first.handle)).status).toBe(404);
  });

  it('refuses unauthenticated registrations with 503 when full of bound handles instead of evicting them', async () => {
    const lines: string[] = [];
    const { base } = await relay({ pushMaxHandles: 2, logger: (line) => lines.push(line) });
    const hostA = identity('bound-a');
    const hostB = identity('bound-b');
    const first = await register(base);
    expect((await send(base, hostA, first.handle)).status).toBe(202);
    const secondRes = await call(base, 'POST', '/push/v1/handles', { fcmToken: `${FCM_TOKEN}-b` });
    expect(secondRes.status).toBe(201);
    const second = (await secondRes.json()) as { handle: string; secret: string };
    expect((await send(base, hostB, second.handle)).status).toBe(202);

    // Flood of unauthenticated registrations with fresh tokens must return 503 and preserve bound handles.
    for (let i = 0; i < 3; i += 1) {
      const refused = await call(base, 'POST', '/push/v1/handles', { fcmToken: `${FCM_TOKEN}-flood-${i}` });
      expect(refused.status).toBe(503);
      expect(await refused.json()).toEqual({ error: 'full' });
    }
    expect(lines).toContain('[push] handle store full');
    expect((await send(base, hostA, first.handle)).status).toBe(202);
    expect((await send(base, hostB, second.handle)).status).toBe(202);
  });
});

describe('push send', () => {
  it('passes the sealed payload through untouched to FCM', async () => {
    const { base, sender } = await relay();
    const made = await register(base);
    const res = await send(base, identity('host-a'), made.handle);
    expect(res.status).toBe(202);
    expect(await res.json()).toEqual({ ok: true });
    expect(sender.sent).toEqual([{ token: FCM_TOKEN, data: { v: '1', h: made.handle, p: PAYLOAD } }]);
  });

  it('refuses a host token that does not hash to the serverId', async () => {
    const { base, sender } = await relay();
    const made = await register(base);
    const real = identity('host-a');
    const res = await call(base, 'POST', '/push/v1/send', { serverId: real.serverId, handle: made.handle, payload: PAYLOAD }, identity('other').hostToken);
    expect(res.status).toBe(401);
    expect((await call(base, 'POST', '/push/v1/send', { serverId: real.serverId, handle: made.handle, payload: PAYLOAD })).status).toBe(401);
    expect(sender.sent).toHaveLength(0);
  });

  it('keeps a handle to the first host that used it', async () => {
    const { base, sender } = await relay();
    const made = await register(base);
    expect((await send(base, identity('first'), made.handle)).status).toBe(202);
    const res = await send(base, identity('second'), made.handle);
    expect(res.status).toBe(403);
    expect(await res.json()).toEqual({ error: 'foreign-handle' });
    expect(sender.sent).toHaveLength(1);
  });

  it('forgets a handle whose token FCM calls gone and says 410', async () => {
    const { base, sender } = await relay();
    const made = await register(base);
    sender.next = { kind: 'invalid-token' };
    expect((await send(base, identity('host-a'), made.handle)).status).toBe(410);
    expect((await send(base, identity('host-a'), made.handle)).status).toBe(404);
  });

  it('keeps a handle FCM refuses for a setup reason and logs only the FCM code', async () => {
    const lines: string[] = [];
    const { base, sender } = await relay({ logger: (line) => lines.push(line) });
    const made = await register(base);
    const host = identity('host-a');
    sender.next = { kind: 'failed', status: 403, code: 'SENDER_ID_MISMATCH' };
    const res = await send(base, host, made.handle);
    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({ error: 'fcm-failed', fcmError: 'SENDER_ID_MISMATCH' });
    expect(lines).toContain(`[${host.serverId.slice(0, 8)}] push failed: fcm 403 SENDER_ID_MISMATCH`);
    expect(lines.join('\n')).not.toContain(made.handle);
    expect(lines.join('\n')).not.toContain(FCM_TOKEN);
    sender.next = { kind: 'sent' };
    expect((await send(base, host, made.handle)).status).toBe(202);
  });

  it('answers 502 with Retry-After when FCM is busy', async () => {
    const { base, sender } = await relay();
    const made = await register(base);
    sender.next = { kind: 'retry', retryAfterSecs: 12 };
    const res = await send(base, identity('host-a'), made.handle);
    expect(res.status).toBe(502);
    expect(res.headers.get('retry-after')).toBe('12');
  });

  it('keeps a Retry-After from FCM to at most an hour', async () => {
    const { base, sender } = await relay();
    const made = await register(base);
    sender.next = { kind: 'retry', retryAfterSecs: 999_999 };
    const res = await send(base, identity('host-a'), made.handle);
    expect(res.status).toBe(502);
    expect(res.headers.get('retry-after')).toBe('3600');
  });

  it('turns sending off at startup when the service account key does not parse', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'relay-push-'));
    try {
      const file = join(dir, 'account.json');
      const fake = '-----BEGIN PRIVATE KEY-----\nnot-a-key\n-----END PRIVATE KEY-----\n';
      await writeFile(file, JSON.stringify({ project_id: 'demo-project', client_email: 'sender@example.invalid', private_key: fake }));
      const lines: string[] = [];
      const handle = await startRelay({ host: '127.0.0.1', port: 0, logger: (line) => lines.push(line), fcmServiceAccountFile: file });
      relays.push(handle);
      const base = `http://127.0.0.1:${handle.port}`;
      const made = await register(base);
      expect((await send(base, identity('host-a'), made.handle)).status).toBe(503);
      expect(lines).toContain('[push] FCM disabled: service account unreadable');
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it('refuses an oversized or malformed payload', async () => {
    const { base } = await relay();
    const made = await register(base);
    expect((await send(base, identity('host-a'), made.handle, 'A'.repeat(3073))).status).toBe(413);
    expect((await send(base, identity('host-a'), made.handle, 'not base64url!')).status).toBe(400);
  });

  it('rate-limits per serverId', async () => {
    const { base } = await relay({ pushMaxSendPerServerId: 2 });
    const made = await register(base);
    const host = identity('busy');
    expect((await send(base, host, made.handle)).status).toBe(202);
    expect((await send(base, host, made.handle)).status).toBe(202);
    const res = await send(base, host, made.handle);
    expect(res.status).toBe(429);
    expect(Number(res.headers.get('retry-after'))).toBeGreaterThan(0);
  });

  it('spends the shared send budget only on sends that reach FCM', async () => {
    const { base, sender } = await relay({ pushMaxSendGlobal: 1 });
    const made = await register(base);
    const owner = identity('owner');
    const stranger = identity('stranger');
    expect((await send(base, owner, 'B'.repeat(43))).status).toBe(404);
    expect((await send(base, owner, 'C'.repeat(43))).status).toBe(404);
    expect((await send(base, owner, made.handle)).status).toBe(202);
    expect((await send(base, stranger, made.handle)).status).toBe(403);
    const limited = await send(base, owner, made.handle);
    expect(limited.status).toBe(429);
    expect(Number(limited.headers.get('retry-after'))).toBeGreaterThan(0);
    expect(sender.sent).toHaveLength(1);
  });

  it('does not bind or evict handles when rejected by the global send rate limit', async () => {
    const { base } = await relay({
      pushMaxSendGlobal: 1,
      pushMaxHandlesPerServerId: 1,
      pushRateWindowMs: 80,
    });
    const hostA = identity('global-no-mutate-a');
    const hostB = identity('global-no-mutate-b');
    const h1 = await register(base);
    const h2Res = await call(base, 'POST', '/push/v1/handles', { fcmToken: `${FCM_TOKEN}-second` });
    expect(h2Res.status).toBe(201);
    const h2 = (await h2Res.json()) as { handle: string };

    // Spend the 1 global slot binding h1 to hostA.
    expect((await send(base, hostA, h1.handle)).status).toBe(202);
    // While globally rate-limited, hostA tries to send to unbound h2: rejected 429.
    expect((await send(base, hostA, h2.handle)).status).toBe(429);

    // Wait for the short rate window to pass.
    await new Promise((resolve) => setTimeout(resolve, 120));
    // h2 must still be unbound so hostB can bind it (not 403 foreign-handle).
    expect((await send(base, hostB, h2.handle)).status).toBe(202);

    await new Promise((resolve) => setTimeout(resolve, 120));
    // h1 must still be bound to hostA (not evicted by #makeRoomFor during the 429).
    expect((await send(base, hostA, h1.handle)).status).toBe(202);
  });

  it('refuses a guessed or foreign handle once its serverId or IP is over budget', async () => {
    const { base } = await relay({ pushMaxSendPerServerId: 1 });
    const host = identity('guesser');
    expect((await send(base, host, 'B'.repeat(43))).status).toBe(404);
    expect((await send(base, host, 'C'.repeat(43))).status).toBe(429);
  });

  it('says 503 when no FCM sender is configured', async () => {
    const { base } = await relay({ pushSender: null });
    const made = await register(base);
    expect((await send(base, identity('host-a'), made.handle)).status).toBe(503);
  });

  it('answers an oversized body 413 and closes without reading the rest', async () => {
    const { base } = await relay();
    const { port } = new URL(base);
    // Declared too large: answered before any body byte is read.
    const declared = await rawRequest(Number(port), 'POST /push/v1/handles HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 104857600\r\n\r\n{"fcmToken":"');
    expect(declared.head).toMatch(/^HTTP\/1\.1 413 /);
    expect(declared.head.toLowerCase()).toContain('connection: close');
    expect(declared.closed).toBe(true);
    // Streamed (chunked) past the limit: answered once the limit is passed, then closed while the client still sends.
    const chunk = 'x'.repeat(4096);
    const body = Array.from({ length: 4 }, () => `${(4096).toString(16)}\r\n${chunk}\r\n`).join('');
    const streamed = await rawRequest(Number(port), `PUT /push/v1/handles/${'A'.repeat(43)} HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n${body}`, true);
    expect(streamed.head).toMatch(/^HTTP\/1\.1 413 /);
    expect(streamed.closed).toBe(true);
    // An unknown route reads no more than the limit either.
    const unknown = await rawRequest(Number(port), 'POST /push/v1/nope HTTP/1.1\r\nHost: x\r\nContent-Length: 104857600\r\n\r\n');
    expect(unknown.head).toMatch(/^HTTP\/1\.1 413 /);
    expect(unknown.closed).toBe(true);
  });

  it('keeps answering /healthz and 404 elsewhere', async () => {
    const { base } = await relay();
    expect((await fetch(`${base}/healthz`)).status).toBe(200);
    expect((await fetch(`${base}/push/v1/nope`)).status).toBe(404);
  });
});
