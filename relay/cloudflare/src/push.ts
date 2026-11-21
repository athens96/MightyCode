/**
 * Push handles and sends (../src/push.ts and ../src/push-store.ts on Cloudflare, docs/relay.md
 * "푸시 알림"): one object for every handle, kept in its SQLite storage. The rate windows are
 * kept there too (`SqlWindowLimiter`), so an eviction or a deploy does not start them over.
 */

import { DurableObject } from 'cloudflare:workers';

import { HOST_TOKEN_PATTERN, SERVER_ID_PATTERN } from '../../src/protocol.ts';
import type { RateDecision } from '../../src/rate.ts';
import type { PushSender } from '../../src/fcm-core.ts';
import { workerFcmSender } from './fcm.ts';
import { HUB_HEADERS } from './hub.ts';
import { newSecret, ownsServerIdAsync, sameDigest, sha256HexSync } from './identity.ts';
import { pushIpKey } from './ip.ts';

/** Same patterns as ../src/push-store.ts and ../src/push.ts. */
export const HANDLE_PATTERN = /^[A-Za-z0-9_-]{43}$/;
export const FCM_TOKEN_PATTERN = /^[\x21-\x7e]{20,4096}$/;
export const PUSH_PAYLOAD_PATTERN = /^[A-Za-z0-9_-]{24,3072}$/;
const PUSH_PAYLOAD_MAX = 3072;
/** The largest push request body read; the Worker refuses a larger one before it reaches the store. */
export const BODY_LIMIT = 8 * 1024;
const DAY_MS = 86_400_000;
const RETRY_AFTER_MAX_SECS = 3600;
const HANDLE_PATH = /^\/push\/v1\/handles\/([A-Za-z0-9_-]{43})$/;

interface PushEnv {
  readonly FCM_SERVICE_ACCOUNT?: string;
  readonly PUSH_RATE_WINDOW_MS?: string;
  readonly PUSH_MAX_SEND_PER_SERVER_ID?: string;
  readonly PUSH_MAX_SEND_PER_IP?: string;
  readonly PUSH_MAX_SEND_GLOBAL?: string;
  readonly PUSH_MAX_HANDLE_OPS_PER_IP?: string;
  readonly PUSH_MAX_REGISTRATIONS_GLOBAL?: string;
  readonly PUSH_MAX_REGISTRATIONS_PER_IP_DAY?: string;
  readonly PUSH_MAX_HANDLES_PER_TOKEN?: string;
  readonly PUSH_MAX_HANDLES_PER_SERVER_ID?: string;
  readonly PUSH_MAX_HANDLES?: string;
  readonly PUSH_HANDLE_IDLE_DAYS?: string;
}

function envInt(raw: string | undefined, fallback: number): number {
  const parsed = Number.parseInt(raw ?? '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function reply(status: number, body?: unknown, retryAfterSecs?: number): Response {
  const headers: Record<string, string> = { 'cache-control': 'no-store' };
  if (retryAfterSecs !== undefined) headers['retry-after'] = String(retryAfterSecs);
  if (body === undefined) return new Response(null, { status, headers });
  headers['content-type'] = 'application/json; charset=utf-8';
  return new Response(JSON.stringify(body), { status, headers });
}

function failure(status: number, code: string, retryAfterSecs?: number): Response {
  return reply(status, { error: code }, retryAfterSecs);
}

function clampRetryAfter(secs: number): number {
  return Number.isFinite(secs) ? Math.min(RETRY_AFTER_MAX_SECS, Math.max(1, Math.ceil(secs))) : 60;
}

type Body = { readonly ok: true; readonly value: Record<string, unknown> } | { readonly ok: false; readonly status: 400 | 413 };

async function readBody(request: Request): Promise<Body> {
  const declared = Number(request.headers.get('content-length') ?? '0');
  if (declared > BODY_LIMIT) return { ok: false, status: 413 };
  const bytes = await request.arrayBuffer();
  if (bytes.byteLength > BODY_LIMIT) return { ok: false, status: 413 };
  try {
    const parsed: unknown = JSON.parse(new TextDecoder().decode(bytes));
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return { ok: false, status: 400 };
    return { ok: true, value: parsed as Record<string, unknown> };
  } catch {
    return { ok: false, status: 400 };
  }
}

function bearer(request: Request): string | null {
  const header = request.headers.get('authorization');
  if (header === null || !header.startsWith('Bearer ')) return null;
  return header.slice('Bearer '.length).trim();
}

/**
 * `SlidingWindowLimiter` (../src/rate.ts) over the object's SQLite storage: one row per
 * counted event, so the windows outlive evictions and deploys. Rows older than the window
 * are deleted by `sweep`.
 */
export class SqlWindowLimiter {
  readonly #sql: SqlStorage;
  readonly #bucket: string;
  readonly #windowMs: number;
  readonly #limit: number;

  constructor(sql: SqlStorage, bucket: string, windowMs: number, limit: number) {
    this.#sql = sql;
    this.#bucket = bucket;
    this.#windowMs = windowMs;
    this.#limit = limit;
  }

  /** Whether one more event fits for `key`; nothing is recorded. */
  check(key: string, now: number): RateDecision {
    const row = this.#sql
      .exec<{ n: number; oldest: number | null }>(
        'SELECT COUNT(*) AS n, MIN(at) AS oldest FROM rate_events WHERE bucket = ? AND key = ? AND at > ?',
        this.#bucket, key, now - this.#windowMs,
      )
      .one();
    if (Number(row.n) < this.#limit) return { ok: true };
    const oldest = Number(row.oldest ?? now);
    return { ok: false, retryAfterSecs: Math.max(1, Math.ceil((oldest + this.#windowMs - now) / 1000)) };
  }

  record(key: string, now: number): void {
    this.#sql.exec('INSERT INTO rate_events (bucket, key, at) VALUES (?, ?, ?)', this.#bucket, key, now);
  }

  sweep(now: number): void {
    this.#sql.exec('DELETE FROM rate_events WHERE bucket = ? AND at <= ?', this.#bucket, now - this.#windowMs);
  }
}

const SWEEP_EVERY_MS = 60_000;

interface HandleRow {
  readonly handle: string;
  readonly secret_hash: string;
  readonly fcm_token: string;
  readonly server_id: string | null;
  readonly used_at: number;
  [key: string]: SqlStorageValue;
}

export class PushStore extends DurableObject<PushEnv> {
  readonly #sql: SqlStorage;
  readonly #sender: PushSender | null;
  readonly #maxHandles: number;
  readonly #maxPerToken: number;
  readonly #maxPerServerId: number;
  readonly #idleMs: number;
  readonly #handleOpsByIp: SqlWindowLimiter;
  readonly #sendByServerId: SqlWindowLimiter;
  readonly #sendByIp: SqlWindowLimiter;
  readonly #sendGlobal: SqlWindowLimiter;
  readonly #registrationsGlobal: SqlWindowLimiter;
  readonly #registrationsByIpDay: SqlWindowLimiter;
  #sweptAt = 0;

  constructor(ctx: DurableObjectState, env: PushEnv) {
    super(ctx, env);
    this.#sql = ctx.storage.sql;
    this.#sql.exec(`CREATE TABLE IF NOT EXISTS handles (
      handle TEXT PRIMARY KEY,
      secret_hash TEXT NOT NULL,
      fcm_token TEXT NOT NULL,
      server_id TEXT,
      created_at INTEGER NOT NULL,
      used_at INTEGER NOT NULL
    )`);
    this.#sql.exec('CREATE INDEX IF NOT EXISTS handles_token ON handles (fcm_token)');
    this.#sql.exec('CREATE INDEX IF NOT EXISTS handles_server ON handles (server_id, used_at)');
    this.#sql.exec('CREATE INDEX IF NOT EXISTS handles_used ON handles (used_at)');
    this.#sql.exec('CREATE TABLE IF NOT EXISTS rate_events (bucket TEXT NOT NULL, key TEXT NOT NULL, at INTEGER NOT NULL)');
    this.#sql.exec('CREATE INDEX IF NOT EXISTS rate_events_lookup ON rate_events (bucket, key, at)');
    this.#sender = workerFcmSender(env.FCM_SERVICE_ACCOUNT, (line) => console.log(line));
    this.#maxHandles = envInt(env.PUSH_MAX_HANDLES, 20_000);
    this.#maxPerToken = envInt(env.PUSH_MAX_HANDLES_PER_TOKEN, 16);
    this.#maxPerServerId = envInt(env.PUSH_MAX_HANDLES_PER_SERVER_ID, 32);
    this.#idleMs = envInt(env.PUSH_HANDLE_IDLE_DAYS, 120) * DAY_MS;
    const window = envInt(env.PUSH_RATE_WINDOW_MS, 600_000);
    const sql = this.#sql;
    this.#handleOpsByIp = new SqlWindowLimiter(sql, 'handle-ops-ip', window, envInt(env.PUSH_MAX_HANDLE_OPS_PER_IP, 30));
    this.#sendByServerId = new SqlWindowLimiter(sql, 'send-server', window, envInt(env.PUSH_MAX_SEND_PER_SERVER_ID, 120));
    this.#sendByIp = new SqlWindowLimiter(sql, 'send-ip', window, envInt(env.PUSH_MAX_SEND_PER_IP, 240));
    this.#sendGlobal = new SqlWindowLimiter(sql, 'send-global', window, envInt(env.PUSH_MAX_SEND_GLOBAL, 3000));
    this.#registrationsGlobal = new SqlWindowLimiter(sql, 'register-global', window, envInt(env.PUSH_MAX_REGISTRATIONS_GLOBAL, 300));
    this.#registrationsByIpDay = new SqlWindowLimiter(sql, 'register-ip-day', DAY_MS, envInt(env.PUSH_MAX_REGISTRATIONS_PER_IP_DAY, 100));
  }

  override async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname;
    const ip = pushIpKey(request.headers.get(HUB_HEADERS.ip) ?? '0.0.0.0');
    const now = Date.now();
    this.#sweep(now);
    if (request.method === 'POST' && path === '/push/v1/handles') return this.#register(request, ip, now);
    if (request.method === 'POST' && path === '/push/v1/send') return this.#send(request, ip, now);
    const managed = HANDLE_PATH.exec(path);
    if ((request.method === 'PUT' || request.method === 'DELETE') && managed !== null) return this.#manage(request, ip, managed[1] ?? '', now);
    return reply(404, { error: 'not-found' });
  }

  #sweep(now: number): void {
    if (now - this.#sweptAt < SWEEP_EVERY_MS) return;
    this.#sweptAt = now;
    const limiters = [this.#handleOpsByIp, this.#sendByServerId, this.#sendByIp, this.#sendGlobal, this.#registrationsGlobal, this.#registrationsByIpDay];
    for (const limiter of limiters) limiter.sweep(now);
  }

  #take(checks: ReadonlyArray<readonly [SqlWindowLimiter, string]>, now: number): RateDecision {
    for (const [limiter, key] of checks) {
      const decision = limiter.check(key, now);
      if (!decision.ok) return decision;
    }
    for (const [limiter, key] of checks) limiter.record(key, now);
    return { ok: true };
  }

  async #register(request: Request, ip: string, now: number): Promise<Response> {
    const body = await readBody(request);
    if (!body.ok) return failure(body.status, body.status === 413 ? 'too-large' : 'bad-request');
    const token = body.value['fcmToken'];
    if (typeof token !== 'string' || !FCM_TOKEN_PATTERN.test(token)) return failure(400, 'bad-request');
    // A token the relay already holds (a phone pairing one more host, or registering again) adds no
    // new phone, so only new tokens spend the shared budget a flood would exhaust.
    const known = this.#sql.exec('SELECT 1 FROM handles WHERE fcm_token = ? LIMIT 1', token).toArray().length > 0;
    const checks: Array<readonly [SqlWindowLimiter, string]> = [[this.#handleOpsByIp, ip], [this.#registrationsByIpDay, ip]];
    if (!known) checks.push([this.#registrationsGlobal, '*']);
    const rate = this.#take(checks, now);
    if (!rate.ok) return failure(429, 'rate-limited', rate.retryAfterSecs);
    this.#sql.exec('DELETE FROM handles WHERE used_at < ?', now - this.#idleMs);
    // One token keeps at most maxPerToken handles (one per paired host); the newest replaces the least recently used.
    const mine = this.#sql.exec<{ handle: string }>('SELECT handle FROM handles WHERE fcm_token = ? ORDER BY used_at ASC', token).toArray();
    while (mine.length >= this.#maxPerToken) this.#sql.exec('DELETE FROM handles WHERE handle = ?', mine.shift()?.handle ?? '');
    const count = this.#count();
    if (count >= this.#maxHandles) {
      console.log('[push] handle store full');
      // Only evict handles no host has bound yet; unauthenticated registrations cannot displace active pairings.
      const needed = count - this.#maxHandles + 1;
      const victims = this.#sql
        .exec<{ handle: string }>(
          'SELECT handle FROM handles WHERE server_id IS NULL ORDER BY used_at ASC LIMIT ?',
          needed,
        )
        .toArray();
      if (victims.length < needed) return failure(503, 'full');
      for (const victim of victims) {
        this.#sql.exec('DELETE FROM handles WHERE handle = ?', victim.handle);
      }
    }
    const handle = newSecret();
    const secret = newSecret();
    this.#sql.exec(
      'INSERT INTO handles (handle, secret_hash, fcm_token, server_id, created_at, used_at) VALUES (?, ?, ?, NULL, ?, ?)',
      handle, sha256HexSync(secret), token, now, now,
    );
    return reply(201, { handle, secret });
  }

  async #manage(request: Request, ip: string, handle: string, now: number): Promise<Response> {
    const secret = bearer(request) ?? '';
    const isPut = request.method === 'PUT';
    const body = isPut ? await readBody(request) : null;
    if (body !== null && !body.ok) return failure(body.status, body.status === 413 ? 'too-large' : 'bad-request');
    const token = body?.ok === true ? body.value['fcmToken'] : undefined;
    if (isPut && (typeof token !== 'string' || !FCM_TOKEN_PATTERN.test(token))) return failure(400, 'bad-request');
    const rate = this.#take([[this.#handleOpsByIp, ip]], now);
    if (!rate.ok) return failure(429, 'rate-limited', rate.retryAfterSecs);
    const row = this.#row(handle);
    if (row === null || !HANDLE_PATTERN.test(secret) || !sameDigest(row.secret_hash, sha256HexSync(secret))) return failure(404, 'unknown-handle');
    if (isPut) this.#sql.exec('UPDATE handles SET fcm_token = ?, used_at = ? WHERE handle = ?', token as string, now, handle);
    else this.#sql.exec('DELETE FROM handles WHERE handle = ?', handle);
    return reply(204);
  }

  async #send(request: Request, ip: string, now: number): Promise<Response> {
    const body = await readBody(request);
    if (!body.ok) return failure(body.status, body.status === 413 ? 'too-large' : 'bad-request');
    const { serverId, handle, payload } = body.value;
    const hostToken = bearer(request);
    if (typeof serverId !== 'string' || !SERVER_ID_PATTERN.test(serverId)) return failure(400, 'bad-request');
    if (hostToken === null || !HOST_TOKEN_PATTERN.test(hostToken) || !(await ownsServerIdAsync(serverId, hostToken))) return failure(401, 'unauthorized');
    if (typeof handle !== 'string' || !HANDLE_PATTERN.test(handle)) return failure(400, 'bad-request');
    if (typeof payload === 'string' && payload.length > PUSH_PAYLOAD_MAX) return failure(413, 'too-large');
    if (typeof payload !== 'string' || !PUSH_PAYLOAD_PATTERN.test(payload)) return failure(400, 'bad-request');
    if (this.#sender === null) return failure(503, 'push-unavailable');
    const rate = this.#take([[this.#sendByServerId, serverId], [this.#sendByIp, ip]], now);
    if (!rate.ok) return failure(429, 'rate-limited', rate.retryAfterSecs);
    const row = this.#row(handle);
    if (row === null) return failure(404, 'unknown-handle');
    if (row.server_id !== null && row.server_id !== serverId) return failure(403, 'foreign-handle');
    // Only a send that would reach FCM spends the shared budget, so guessed or foreign handles cannot use it up.
    // Check global rate limit BEFORE mutating bindings or running #makeRoomFor eviction.
    const global = this.#take([[this.#sendGlobal, '*']], now);
    if (!global.ok) return failure(429, 'rate-limited', global.retryAfterSecs);
    if (row.server_id === null) this.#makeRoomFor(row, serverId);
    // usedAt only feeds the idle sweep: written on first use and then at most daily.
    if (row.server_id === null || now - row.used_at > DAY_MS) this.#sql.exec('UPDATE handles SET server_id = ?, used_at = ? WHERE handle = ?', serverId, now, handle);
    const short = serverId.slice(0, 8);
    const result = await this.#sender.send(row.fcm_token, { v: '1', h: handle, p: payload });
    switch (result.kind) {
      case 'sent':
        console.log(`[${short}] push sent`);
        return reply(202, { ok: true });
      case 'invalid-token':
        this.#sql.exec('DELETE FROM handles WHERE handle = ?', handle);
        console.log(`[${short}] push token gone; handle dropped`);
        return failure(410, 'token-gone');
      case 'retry':
        return failure(502, 'fcm-failed', clampRetryAfter(result.retryAfterSecs));
      case 'failed':
        // The FCM error code (for example SENDER_ID_MISMATCH: a key from another Firebase project) names the setup problem.
        console.log(`[${short}] push failed: fcm ${result.status}${result.code === undefined ? '' : ` ${result.code}`}`);
        return reply(502, result.code === undefined ? { error: 'fcm-failed' } : { error: 'fcm-failed', fcmError: result.code }, 30);
    }
  }

  /**
   * Before `row` is first bound to `serverId`: an older handle of the same token bound to the
   * same serverId goes (the phone registered again), and the serverId's least recently used
   * handles go until it is under its quota.
   */
  #makeRoomFor(row: HandleRow, serverId: string): void {
    this.#sql.exec('DELETE FROM handles WHERE server_id = ? AND fcm_token = ? AND handle != ?', serverId, row.fcm_token, row.handle);
    const bound = this.#sql
      .exec<{ handle: string }>('SELECT handle FROM handles WHERE server_id = ? AND handle != ? ORDER BY used_at ASC', serverId, row.handle)
      .toArray();
    while (bound.length >= this.#maxPerServerId) this.#sql.exec('DELETE FROM handles WHERE handle = ?', bound.shift()?.handle ?? '');
  }

  #row(handle: string): HandleRow | null {
    return this.#sql.exec<HandleRow>('SELECT handle, secret_hash, fcm_token, server_id, used_at FROM handles WHERE handle = ?', handle).toArray()[0] ?? null;
  }

  #count(): number {
    return Number(this.#sql.exec<{ n: number }>('SELECT COUNT(*) AS n FROM handles').one().n);
  }
}
