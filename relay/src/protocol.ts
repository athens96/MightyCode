/** Wire-level constants and query validation for `GET /ws`. */

import { createHash, timingSafeEqual } from 'node:crypto';

export const WS_PATH = '/ws';
export const PROTOCOL_VERSION = '1';

export const SERVER_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;
export const CONNECTION_ID_PATTERN = /^[A-Za-z0-9-]{8,64}$/;
export const HOST_TOKEN_PATTERN = /^[A-Fa-f0-9]{32,64}$/;

/** Close codes used by the relay (see docs/relay.md, "릴레이 와이어"). */
export const CloseCode = {
  /** Normal closure. */
  normal: 1000,
  /** Malformed or missing query parameters. */
  badRequest: 4400,
  /** Host socket without a hostToken whose SHA-256 is the serverId. */
  unauthorized: 4401,
  /** No host control socket, or unknown/expired connectionId. */
  notFound: 4404,
  /** Superseded by a newer host control socket. */
  conflict: 4409,
  /** Host control socket went away. */
  hostOffline: 4410,
  /** A client data socket sent nothing (no `hello`) within the hello deadline. */
  helloTimeout: 4408,
  /** Client sent more frames than the relay buffers while the host attaches. */
  bufferOverflow: 4413,
  /**
   * Per-serverId connection cap, per-address cap on sockets not yet answered or
   * authenticated, or the relay's host caps (Node relay) reached.
   */
  tooManyConnections: 4429,
  /** Host data socket did not attach in time. */
  attachTimeout: 4504,
  /** Forwarding a frame would exceed the per-socket byte buffer bound. */
  socketBufferOverflow: 4507,
} as const;

export type CloseCodeValue = (typeof CloseCode)[keyof typeof CloseCode];

export type SocketRole = 'server' | 'client';

/** Host control socket, host data socket, or client data socket. */
export type SocketKind = 'control' | 'host-data' | 'client-data';

/** A validated `GET /ws` query. */
export interface SocketParams {
  readonly kind: SocketKind;
  readonly serverId: string;
  /** Present for every kind except `control`. */
  readonly connectionId: string | null;
  /**
   * The host secret carried in the query by `control` and `host-data` sockets (always
   * null for `client-data`). Null on a host socket means the token comes in the first
   * frame instead (`parseHostAuthFrame`), which keeps it out of URLs and their logs.
   * See `ownsServerId`.
   */
  readonly hostToken: string | null;
}

/** JSON notifications the relay pushes to the host control socket. */
export interface ControlNotice {
  readonly type: 'connected' | 'disconnected';
  readonly connectionId: string;
}

/** Host → relay: request TURN credentials on the control socket (plaintext, not E2EE). */
export interface TurnCredentialsRequest {
  readonly type: 'turn-credentials-request';
}

/**
 * Relay → host: short-lived HMAC TURN credentials minted from the relay's
 * coturn use-auth-secret. The secret itself never leaves the relay process.
 */
export interface TurnCredentialsPayload {
  readonly type: 'turn-credentials';
  readonly username: string;
  readonly password: string;
  readonly ttl: number;
  readonly uris: string[];
}

/** Relay → host: request was received but rate-limited. */
export interface TurnRateLimited {
  readonly type: 'turn-rate-limited';
  readonly retryAfterSecs: number;
}

/** Relay → host: TURN credentials not available (secret not configured). */
export interface TurnUnavailable {
  readonly type: 'turn-unavailable';
}

/**
 * Host → relay, the first frame of a host socket opened without `hostToken` in its
 * query: `{"type":"host-auth","hostToken":"<hex>"}` as a text frame. Nothing answers
 * it; a wrong, malformed or missing frame closes the socket 4401.
 */
export interface HostAuthFrame {
  readonly type: 'host-auth';
  readonly hostToken: string;
}

/** The token of a well-formed `host-auth` frame, or null for anything else. */
export function parseHostAuthFrame(frame: string): string | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(frame);
  } catch {
    return null;
  }
  if (typeof parsed !== 'object' || parsed === null) return null;
  const record = parsed as Record<string, unknown>;
  const token = record['hostToken'];
  if (record['type'] !== 'host-auth' || typeof token !== 'string' || !HOST_TOKEN_PATTERN.test(token)) return null;
  return token;
}

export type ParseResult =
  | { readonly ok: true; readonly params: SocketParams }
  | { readonly ok: false; readonly reason: string };

/** Validates the `GET /ws` query string; failures must be answered with close code 4400. */
export function parseSocketParams(query: URLSearchParams): ParseResult {
  const version = query.get('v');
  if (version !== PROTOCOL_VERSION) return { ok: false, reason: 'bad v' };

  const serverId = query.get('serverId');
  if (serverId === null || !SERVER_ID_PATTERN.test(serverId)) {
    return { ok: false, reason: 'bad serverId' };
  }

  const role = query.get('role');
  if (role !== 'server' && role !== 'client') return { ok: false, reason: 'bad role' };

  const connectionId = query.get('connectionId');
  if (connectionId !== null && !CONNECTION_ID_PATTERN.test(connectionId)) {
    return { ok: false, reason: 'bad connectionId' };
  }

  if (role === 'client') {
    if (connectionId === null) return { ok: false, reason: 'missing connectionId' };
    return { ok: true, params: { kind: 'client-data', serverId, connectionId, hostToken: null } };
  }

  const hostToken = query.get('hostToken');
  if (hostToken !== null && !HOST_TOKEN_PATTERN.test(hostToken)) {
    return { ok: false, reason: 'bad hostToken' };
  }
  if (connectionId === null) {
    return { ok: true, params: { kind: 'control', serverId, connectionId: null, hostToken } };
  }
  return { ok: true, params: { kind: 'host-data', serverId, connectionId, hostToken } };
}

/**
 * A host socket speaks for `serverId` only when `serverId` is the lowercase hex
 * SHA-256 of the UTF-8 `hostToken`. Nothing is remembered between connections.
 */
export function ownsServerId(serverId: string, hostToken: string | null): boolean {
  if (hostToken === null) return false;
  const expected = new TextEncoder().encode(createHash('sha256').update(hostToken, 'utf8').digest('hex'));
  const actual = new TextEncoder().encode(serverId);
  return expected.byteLength === actual.byteLength && timingSafeEqual(expected, actual);
}

/**
 * WebSocket close codes are restricted on the wire; anything a peer cannot legally
 * send back (1005/1006 and friends) collapses to a normal closure.
 */
export function sendableCloseCode(code: number): number {
  if (code === 1000 || (code >= 3000 && code <= 4999)) return code;
  if (code >= 1001 && code <= 1014 && code !== 1004 && code !== 1005 && code !== 1006) return code;
  return CloseCode.normal;
}

/** Close reasons are capped at 123 bytes by RFC 6455. */
export function truncateReason(reason: string): string {
  let out = reason;
  while (Buffer.byteLength(out, 'utf8') > 123) out = out.slice(0, -1);
  return out;
}
