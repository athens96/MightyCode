/**
 * Isolate-level `/ws` upgrade rate limiter tests: verifies random `serverId` fan-out is
 * blocked with HTTP 429 BEFORE calling `env.HUB.idFromName` / `env.HUB.get`.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('cloudflare:workers', () => ({
  DurableObject: class DurableObject {},
}));

import { ownsServerIdAsync, sha256Hex, sha256HexSync } from '../src/identity.ts';
import worker, { resetWsUpgradeLimiter, WsUpgradeLimiter, type Env } from '../src/worker.ts';

function wsRequest(serverId: string, ip: string, connectionId = 'conn-00000001'): Request {
  const params = new URLSearchParams({ v: '1', serverId, role: 'client', connectionId });
  return new Request(`https://relay.invalid/ws?${params.toString()}`, {
    method: 'GET',
    headers: { upgrade: 'websocket', 'cf-connecting-ip': ip },
  });
}

function makeFakeEnv(overrides: Partial<Env> = {}) {
  const hubFetch = vi.fn(async () => new Response('ok', { status: 200 }));
  const idFromName = vi.fn((name: string) => ({ name }) as unknown as DurableObjectId);
  const get = vi.fn(() => ({ fetch: hubFetch }) as unknown as DurableObjectStub);
  const env: Env = {
    HUB: { idFromName, get } as unknown as DurableObjectNamespace,
    PUSH: { idFromName: vi.fn(), get: vi.fn() } as unknown as DurableObjectNamespace,
    ...overrides,
  };
  return { env, idFromName, get, hubFetch };
}

afterEach(() => {
  resetWsUpgradeLimiter();
});

describe('CF Worker /ws DO fan-out rate limit', () => {
  it('blocks random serverId flood per IP with 429 before instantiating Hub DOs', async () => {
    const { env, idFromName, get } = makeFakeEnv({
      WS_RATE_WINDOW_MS: '60000',
      WS_MAX_UPGRADES_PER_IP: '3',
      WS_MAX_UPGRADES_GLOBAL: '100',
    });

    for (let i = 0; i < 3; i += 1) {
      const res = await worker.fetch(wsRequest(`random-server-${i}`, '198.51.100.10'), env);
      expect(res.status).toBe(200);
    }
    expect(idFromName).toHaveBeenCalledTimes(3);
    expect(get).toHaveBeenCalledTimes(3);

    // 4th upgrade from the same IP to another random serverId must be blocked BEFORE touching HUB.
    const blocked = await worker.fetch(wsRequest('random-server-999', '198.51.100.10'), env);
    expect(blocked.status).toBe(429);
    expect(Number(blocked.headers.get('retry-after'))).toBeGreaterThan(0);
    expect(blocked.headers.get('cache-control')).toBe('no-store');
    expect(idFromName).toHaveBeenCalledTimes(3);
    expect(get).toHaveBeenCalledTimes(3);

    // A different IP is unaffected and still reaches its Hub DO.
    const other = await worker.fetch(wsRequest('random-server-other', '198.51.100.11'), env);
    expect(other.status).toBe(200);
    expect(idFromName).toHaveBeenCalledTimes(4);
    expect(get).toHaveBeenCalledTimes(4);
  });

  it('groups IPv6 upgrades by /64 so address rotation cannot bypass per-IP fan-out limit', async () => {
    const { env, idFromName } = makeFakeEnv({
      WS_RATE_WINDOW_MS: '60000',
      WS_MAX_UPGRADES_PER_IP: '2',
      WS_MAX_UPGRADES_GLOBAL: '100',
    });

    expect((await worker.fetch(wsRequest('srv-v6-1', '2001:db8:1:2::1'), env)).status).toBe(200);
    expect((await worker.fetch(wsRequest('srv-v6-2', '2001:db8:1:2:abcd::2'), env)).status).toBe(200);
    const blocked = await worker.fetch(wsRequest('srv-v6-3', '2001:db8:1:2:ffff::3'), env);
    expect(blocked.status).toBe(429);
    expect(idFromName).toHaveBeenCalledTimes(2);

    // Another /64 prefix has its own budget.
    expect((await worker.fetch(wsRequest('srv-v6-4', '2001:db8:1:3::1'), env)).status).toBe(200);
    expect(idFromName).toHaveBeenCalledTimes(3);
  });

  it('enforces the global upgrade cap across distinct IPs before touching Hub DOs', async () => {
    const { env, idFromName, get } = makeFakeEnv({
      WS_RATE_WINDOW_MS: '60000',
      WS_MAX_UPGRADES_PER_IP: '10',
      WS_MAX_UPGRADES_GLOBAL: '2',
    });

    expect((await worker.fetch(wsRequest('srv-g-1', '203.0.113.1'), env)).status).toBe(200);
    expect((await worker.fetch(wsRequest('srv-g-2', '203.0.113.2'), env)).status).toBe(200);
    const blocked = await worker.fetch(wsRequest('srv-g-3', '203.0.113.3'), env);
    expect(blocked.status).toBe(429);
    expect(idFromName).toHaveBeenCalledTimes(2);
    expect(get).toHaveBeenCalledTimes(2);
  });

  it('recovers after the sliding window expires', () => {
    const limiter = new WsUpgradeLimiter(10_000, 2, 10);
    expect(limiter.take('198.51.100.1', 0)).toEqual({ ok: true });
    expect(limiter.take('198.51.100.1', 1_000)).toEqual({ ok: true });
    expect(limiter.take('198.51.100.1', 2_000)).toEqual({ ok: false, retryAfterSecs: 8 });
    expect(limiter.take('198.51.100.1', 10_000)).toEqual({ ok: true });
  });
});

describe('identity helpers', () => {
  it('matches sync and async SHA-256 digests and verifies ownership in constant time', async () => {
    const token = 'a'.repeat(64);
    const digest = await sha256Hex(token);
    expect(sha256HexSync(token)).toBe(digest);
    expect(await ownsServerIdAsync(digest, token)).toBe(true);
    expect(await ownsServerIdAsync(digest, 'b'.repeat(64))).toBe(false);
    expect(await ownsServerIdAsync(digest, null)).toBe(false);
  });
});
