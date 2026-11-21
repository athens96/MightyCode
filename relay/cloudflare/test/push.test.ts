/**
 * The PushStore's limits under `wrangler dev`: per-token and per-serverId caps, a full store
 * that makes room instead of refusing, and rate windows that survive a restart of the object.
 * Sends go to an FCM sender whose token exchange cannot connect, so a bound handle answers
 * 502 (`fcm-failed`) and an unknown one 404 — which is all these tests need to tell apart.
 */

import { createHash, generateKeyPairSync } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterAll, describe, expect, it } from 'vitest';

import { startWorker, type LocalWorker } from './harness.ts';

const sha256Hex = (value: string): string => createHash('sha256').update(value, 'utf8').digest('hex');

function identity(label: string): { serverId: string; hostToken: string } {
  const hostToken = sha256Hex(`host-secret:${label}`);
  return { serverId: sha256Hex(hostToken), hostToken };
}

const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
/** A well-formed service account whose token exchange goes to a closed local port. */
const SERVICE_ACCOUNT = JSON.stringify({
  project_id: 'demo-project',
  client_email: 'sender@example.invalid',
  private_key: privateKey.export({ type: 'pkcs8', format: 'pem' }).toString(),
  token_uri: 'http://127.0.0.1:9/token',
});

const BASE_VARS = {
  FCM_SERVICE_ACCOUNT: SERVICE_ACCOUNT,
  PUSH_MAX_HANDLE_OPS_PER_IP: '1000',
  PUSH_MAX_REGISTRATIONS_PER_IP_DAY: '1000',
  PUSH_MAX_SEND_PER_IP: '1000',
  PUSH_MAX_HANDLES_PER_TOKEN: '2',
  PUSH_MAX_HANDLES_PER_SERVER_ID: '2',
};

const states: string[] = [];
const workers: LocalWorker[] = [];

function newState(): string {
  const state = mkdtempSync(join(tmpdir(), 'mighty-relay-cf-push-'));
  states.push(state);
  return state;
}

async function start(state: string, vars: Record<string, string>): Promise<LocalWorker> {
  const worker = await startWorker(state, vars);
  workers.push(worker);
  return worker;
}

afterAll(async () => {
  for (const worker of workers) await worker.stop();
  for (const state of states) rmSync(state, { recursive: true, force: true });
});

async function register(base: string, fcmToken: string): Promise<Response> {
  return fetch(`http://${base}/push/v1/handles`, { method: 'POST', body: JSON.stringify({ fcmToken }) });
}

async function handleFor(base: string, fcmToken: string): Promise<string> {
  const res = await register(base, fcmToken);
  expect(res.status).toBe(201);
  return ((await res.json()) as { handle: string }).handle;
}

async function send(base: string, label: string, handle: string): Promise<number> {
  const { serverId, hostToken } = identity(label);
  const res = await fetch(`http://${base}/push/v1/send`, {
    method: 'POST',
    headers: { authorization: `Bearer ${hostToken}` },
    body: JSON.stringify({ serverId, handle, payload: 'p'.repeat(40) }),
  });
  await res.arrayBuffer();
  return res.status;
}

const token = (name: string): string => `fcm-token-${name}`.padEnd(40, 'x');

describe('push store limits', () => {
  it('keeps a token to its cap of handles, a serverId to its quota, and one handle per token and serverId', async () => {
    const state = newState();
    const { base } = await start(state, BASE_VARS);

    // Per token (cap 2): the third registration replaces the least recently used.
    const t1 = await handleFor(base, token('per-token'));
    const t2 = await handleFor(base, token('per-token'));
    const t3 = await handleFor(base, token('per-token'));
    expect(await send(base, 'host-a', t1)).toBe(404);
    expect(await send(base, 'host-a', t2)).toBe(502);
    expect(await send(base, 'host-b', t3)).toBe(502);

    // The same token bound again to the same serverId keeps only the newer handle.
    const before = await handleFor(base, token('again'));
    expect(await send(base, 'host-c', before)).toBe(502);
    const again = await handleFor(base, token('again'));
    expect(await send(base, 'host-c', again)).toBe(502);
    expect(await send(base, 'host-c', before)).toBe(404);

    // Per serverId (quota 2): a third bound handle replaces the serverId's least recently used.
    const q = [await handleFor(base, token('q1')), await handleFor(base, token('q2')), await handleFor(base, token('q3'))];
    for (const handle of q) expect(await send(base, 'host-d', handle)).toBe(502);
    expect(await send(base, 'host-d', q[0]!)).toBe(404);
    expect(await send(base, 'host-d', q[1]!)).toBe(502);
    expect(await send(base, 'host-d', q[2]!)).toBe(502);
  }, 120_000);

  it('refuses unauthenticated registrations with 503 when full of bound handles, while still evicting unbound handles', async () => {
    const { base } = await start(newState(), { ...BASE_VARS, PUSH_MAX_HANDLES: '2' });
    const older = await handleFor(base, token('full-1'));
    expect(await send(base, 'full-a', older)).toBe(502);
    // Second slot holds an unbound handle: a fresh registration evicts only the unbound one.
    const unbound = await handleFor(base, token('full-unbound'));
    const replacement = await handleFor(base, token('full-2'));
    expect(await send(base, 'full-a', older)).toBe(502);
    expect(await send(base, 'full-b', unbound)).toBe(404);
    // Now bind the second slot so all slots are bound.
    expect(await send(base, 'full-b', replacement)).toBe(502);
    const refused = await register(base, token('full-3'));
    expect(refused.status).toBe(503);
    expect(await refused.json()).toEqual({ error: 'full' });
    // Both bound handles remain intact.
    expect(await send(base, 'full-a', older)).toBe(502);
    expect(await send(base, 'full-b', replacement)).toBe(502);
  }, 120_000);

  it('does not bind or evict handles when rejected by the global send rate limit', async () => {
    const { base } = await start(newState(), {
      ...BASE_VARS,
      PUSH_MAX_SEND_GLOBAL: '1',
      PUSH_MAX_HANDLES_PER_SERVER_ID: '1',
      PUSH_RATE_WINDOW_MS: '600',
    });
    const h1 = await handleFor(base, token('g-nomut-1'));
    const h2 = await handleFor(base, token('g-nomut-2'));
    // Spend the 1 global send slot binding h1 to host-a.
    expect(await send(base, 'g-host-a', h1)).toBe(502);
    // While globally rate-limited, host-a tries to send to unbound h2: rejected 429.
    expect(await send(base, 'g-host-a', h2)).toBe(429);
    // Wait for the short window to pass.
    await new Promise((resolve) => setTimeout(resolve, 750));
    // h2 must still be unbound so host-b can bind it (not 403 foreign-handle).
    expect(await send(base, 'g-host-b', h2)).toBe(502);
    await new Promise((resolve) => setTimeout(resolve, 750));
    // h1 must still exist for host-a (not evicted by #makeRoomFor during the 429).
    expect(await send(base, 'g-host-a', h1)).toBe(502);
  }, 120_000);

  it('keeps its rate windows across a restart and counts only new tokens against the global registration budget', async () => {
    const state = newState();
    const vars = { ...BASE_VARS, PUSH_MAX_REGISTRATIONS_GLOBAL: '2' };
    const first = await start(state, vars);
    expect((await register(first.base, token('g1'))).status).toBe(201);
    expect((await register(first.base, token('g2'))).status).toBe(201);
    expect((await register(first.base, token('g3'))).status).toBe(429);
    await first.stop();

    // A new process (the object's memory gone) still remembers the window.
    const second = await start(state, vars);
    const limited = await register(second.base, token('g4'));
    expect(limited.status).toBe(429);
    expect(Number(limited.headers.get('retry-after'))).toBeGreaterThan(0);
    // A token the relay holds (one more host for the same phone) still registers.
    expect((await register(second.base, token('g1'))).status).toBe(201);
  }, 180_000);

  it('answers a body over the limit 413 without reading it all', async () => {
    const { base } = await start(newState(), BASE_VARS);
    const declared = await fetch(`http://${base}/push/v1/handles`, { method: 'POST', body: 'x'.repeat(9000) });
    expect(declared.status).toBe(413);
    expect(await declared.json()).toEqual({ error: 'too-large' });
    // Streamed without a length: cut off at the limit.
    let sent = 0;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        if (sent >= 64 * 1024) return controller.close();
        sent += 4096;
        controller.enqueue(new Uint8Array(4096).fill(120));
      },
    });
    const streamed = await fetch(`http://${base}/push/v1/handles`, { method: 'POST', body: stream, duplex: 'half' } as RequestInit);
    expect(streamed.status).toBe(413);
  }, 120_000);
});
