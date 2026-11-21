/**
 * The Mighty Code relay on Cloudflare Workers. Same wire as the Node relay (../src/server.ts,
 * docs/relay.md): `GET /healthz`, `GET /ws?...` and `/push/v1/*`. Sockets go to the Hub of
 * their serverId (hub.ts), pushes to the one PushStore (push.ts).
 */

import { CloseCode, parseSocketParams, WS_PATH } from '../../src/protocol.ts';
import { SlidingWindowLimiter, type RateDecision } from '../../src/rate.ts';
import { ownsServerIdAsync } from './identity.ts';
import { HUB_HEADERS } from './hub.ts';
import { pushIpKey } from './ip.ts';
import { BODY_LIMIT } from './push.ts';

/** The URL a Hub sees; the client's URL (and any token in its query) is not handed on. */
const HUB_URL = 'https://hub.invalid/ws';
const DEFAULT_WS_RATE_WINDOW_MS = 60_000;
const DEFAULT_WS_MAX_UPGRADES_PER_IP = 120;
const DEFAULT_WS_MAX_UPGRADES_GLOBAL = 2_000;

export { Hub } from './hub.ts';
export { PushStore } from './push.ts';

export interface Env {
  readonly HUB: DurableObjectNamespace;
  readonly PUSH: DurableObjectNamespace;
  /** The FCM service account JSON (a secret); absent or broken leaves sending off (503). */
  readonly FCM_SERVICE_ACCOUNT?: string;
  /** Sliding window for isolate-level `/ws` upgrade limiting (ms). */
  readonly WS_RATE_WINDOW_MS?: string;
  /** Max `/ws` upgrades per IP (IPv6: /64) per window before touching a Hub DO. */
  readonly WS_MAX_UPGRADES_PER_IP?: string;
  /** Max `/ws` upgrades across all IPs per window in this isolate before touching a Hub DO. */
  readonly WS_MAX_UPGRADES_GLOBAL?: string;
}

function envInt(raw: string | undefined, fallback: number): number {
  const parsed = Number.parseInt(raw ?? '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

/**
 * Isolate-level per-IP + global sliding-window limiter for `/ws` upgrades, run before
 * `env.HUB.get(idFromName(serverId))` so a flood of random serverIds cannot fan out
 * Durable Objects unboundedly.
 */
export class WsUpgradeLimiter {
  readonly #byIp: SlidingWindowLimiter;
  readonly #global: SlidingWindowLimiter;
  readonly #sweepEveryMs: number;
  #sweptAt = 0;

  constructor(windowMs: number, maxPerIp: number, maxGlobal: number) {
    this.#byIp = new SlidingWindowLimiter(windowMs, maxPerIp);
    this.#global = new SlidingWindowLimiter(windowMs, maxGlobal);
    this.#sweepEveryMs = Math.min(windowMs, 60_000);
  }

  take(ipKey: string, now = Date.now()): RateDecision {
    if (now - this.#sweptAt >= this.#sweepEveryMs) {
      this.#sweptAt = now;
      this.#byIp.sweep(now);
      this.#global.sweep(now);
    }
    const perIp = this.#byIp.check(ipKey, now);
    if (!perIp.ok) return perIp;
    const global = this.#global.check('*', now);
    if (!global.ok) return global;
    this.#byIp.record(ipKey, now);
    this.#global.record('*', now);
    return { ok: true };
  }
}

let wsUpgradeLimiter: { readonly key: string; readonly limiter: WsUpgradeLimiter } | null = null;

function getWsUpgradeLimiter(env: Env): WsUpgradeLimiter {
  const windowMs = envInt(env.WS_RATE_WINDOW_MS, DEFAULT_WS_RATE_WINDOW_MS);
  const maxPerIp = envInt(env.WS_MAX_UPGRADES_PER_IP, DEFAULT_WS_MAX_UPGRADES_PER_IP);
  const maxGlobal = envInt(env.WS_MAX_UPGRADES_GLOBAL, DEFAULT_WS_MAX_UPGRADES_GLOBAL);
  const key = `${windowMs}:${maxPerIp}:${maxGlobal}`;
  if (wsUpgradeLimiter === null || wsUpgradeLimiter.key !== key) {
    wsUpgradeLimiter = { key, limiter: new WsUpgradeLimiter(windowMs, maxPerIp, maxGlobal) };
  }
  return wsUpgradeLimiter.limiter;
}

/** Resets the isolate-level `/ws` upgrade limiter (tests). */
export function resetWsUpgradeLimiter(): void {
  wsUpgradeLimiter = null;
}

function text(status: number, body: string): Response {
  return new Response(body, { status, headers: { 'content-type': 'text/plain; charset=utf-8' } });
}

/** Accepts the upgrade only to close it at once with `code`, as the Node relay does for a bad query. */
function refuseSocket(code: number, reason: string): Response {
  const pair = new WebSocketPair();
  const [client, server] = [pair[0], pair[1]];
  server.accept();
  server.close(code, reason);
  return new Response(null, { status: 101, webSocket: client });
}

/**
 * The body when it is at most `limit` bytes (declared or streamed), else null: reading stops
 * at the limit instead of taking in whatever a client sends.
 */
async function boundedBody(request: Request, limit: number): Promise<ArrayBuffer | null> {
  if (Number(request.headers.get('content-length') ?? '0') > limit) return null;
  if (request.body === null) return new ArrayBuffer(0);
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const out = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out.buffer;
}

/** The address per-IP limits count: Cloudflare's view of the client. */
export function clientIp(request: Request): string {
  return request.headers.get('cf-connecting-ip') ?? '0.0.0.0';
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const upgrade = request.headers.get('upgrade')?.toLowerCase() === 'websocket';

    if (upgrade) {
      if (url.pathname !== WS_PATH) return text(404, 'not found');
      const parsed = parseSocketParams(url.searchParams);
      if (!parsed.ok) return refuseSocket(CloseCode.badRequest, parsed.reason);
      const { params } = parsed;
      // A host socket with `hostToken` in its query (the form hosts in the field use) is checked
      // here; one without it proves itself with a `host-auth` first frame in its Hub.
      const byFrame = params.kind !== 'client-data' && params.hostToken === null;
      if (params.kind !== 'client-data' && !byFrame && !(await ownsServerIdAsync(params.serverId, params.hostToken))) {
        return refuseSocket(CloseCode.unauthorized, 'invalid or missing host token');
      }
      const sourceIp = clientIp(request);
      const rate = getWsUpgradeLimiter(env).take(pushIpKey(sourceIp));
      if (!rate.ok) {
        return new Response('rate limited', {
          status: 429,
          headers: {
            'content-type': 'text/plain; charset=utf-8',
            'cache-control': 'no-store',
            'retry-after': String(rate.retryAfterSecs),
          },
        });
      }
      const headers = new Headers(request.headers);
      headers.set(HUB_HEADERS.kind, params.kind);
      headers.set(HUB_HEADERS.serverId, params.serverId);
      headers.set(HUB_HEADERS.connectionId, params.connectionId ?? '');
      headers.set(HUB_HEADERS.ip, sourceIp);
      headers.set(HUB_HEADERS.auth, byFrame ? 'frame' : 'query');
      const hub = env.HUB.get(env.HUB.idFromName(params.serverId));
      // The Hub gets no query: a host token in the URL goes no further than this check.
      return hub.fetch(new Request(HUB_URL, { method: 'GET', headers }));
    }

    if (url.pathname.startsWith('/push/v1/')) {
      const headers = new Headers(request.headers);
      headers.set(HUB_HEADERS.ip, clientIp(request));
      const store = env.PUSH.get(env.PUSH.idFromName('push'));
      const init: RequestInit = { method: request.method, headers };
      if (request.method !== 'GET' && request.method !== 'HEAD') {
        const body = await boundedBody(request, BODY_LIMIT);
        if (body === null) {
          return new Response(JSON.stringify({ error: 'too-large' }), {
            status: 413,
            headers: { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' },
          });
        }
        init.body = body;
      }
      return store.fetch(new Request(request.url, init));
    }

    if (request.method === 'GET' && url.pathname === '/healthz' && url.search === '') return text(200, 'ok');
    return text(404, 'not found');
  },
} satisfies ExportedHandler<Env>;
