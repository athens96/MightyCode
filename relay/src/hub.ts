import { createHmac } from 'node:crypto';
import type { RawData, WebSocket } from 'ws';

import type { RelayConfig } from './config.ts';
import {
  CloseCode,
  ownsServerId,
  parseHostAuthFrame,
  sendableCloseCode,
  truncateReason,
  type ControlNotice,
  type SocketParams,
  type TurnCredentialsPayload,
  type TurnRateLimited,
  type TurnUnavailable,
} from './protocol.ts';
import { pushIpKey } from './push.ts';

/** A frame held while the host data socket is still attaching. */
interface BufferedFrame {
  readonly payload: Buffer;
  readonly binary: boolean;
}

interface Connection {
  readonly serverId: string;
  readonly connectionId: string;
  readonly client: WebSocket;
  /** The client's source address as per-address caps count it (IPv6: its /64). */
  readonly ipKey: string;
  host: WebSocket | null;
  buffered: BufferedFrame[];
  bufferedBytes: number;
  attachTimer: NodeJS.Timeout | null;
  /** Running until the client sends its first frame (the E2EE `hello`). */
  helloTimer: NodeJS.Timeout | null;
  /** Whether the host half has sent the client anything (its `ready`); until then the slot is pending. */
  answered: boolean;
  closed: boolean;
}

interface ServerEntry {
  control: WebSocket | null;
  /** The address key of the current control socket, for the per-address host cap. */
  controlIp: string | null;
  readonly connections: Map<string, Connection>;
}

/** Sink for one-line connect/disconnect logs. */
export type RelayLogger = (line: string) => void;

function toBuffer(data: RawData): Buffer {
  if (Buffer.isBuffer(data)) return data;
  if (Array.isArray(data)) return Buffer.concat(data);
  return Buffer.from(data);
}

function shortId(serverId: string): string {
  return serverId.slice(0, 8);
}

/**
 * Stateless-per-process pairing engine: it matches a host control socket with client
 * data sockets and their host counterparts, then pipes frames verbatim.
 *
 * TURN credential minting: the relay holds the coturn use-auth-secret and mints
 * short-lived HMAC-SHA1 credentials on request from authenticated host control
 * sockets. The secret never appears in any frame forwarded to client data sockets.
 */
export class RelayHub {
  readonly #config: RelayConfig;
  readonly #log: RelayLogger;
  readonly #servers = new Map<string, ServerEntry>();
  readonly #alive = new Map<WebSocket, boolean>();
  /** Host sockets still owing their `host-auth` frame, per `serverId` and address key. */
  readonly #authPending = new Map<string, number>();
  #keepalive: NodeJS.Timeout | null = null;

  /** Timestamps (ms) of recent TURN mints, keyed by serverId. */
  readonly #turnRateByServerId = new Map<string, number[]>();
  /** Timestamps (ms) of recent TURN mints, keyed by IP address. */
  readonly #turnRateByIp = new Map<string, number[]>();
  /** Timestamps (ms) of recent TURN mints across all hosts (single key). */
  readonly #turnRateGlobal = new Map<string, number[]>();

  constructor(config: RelayConfig, log: RelayLogger = (line) => console.log(line)) {
    this.#config = config;
    this.#log = log;
    this.#startKeepalive();
  }

  /**
   * Entry point for an accepted WebSocket with an already validated query. A host socket
   * proves its serverId with `hostToken` in the query (the form hosts in the field use) or,
   * without it, with a `host-auth` first frame (docs/relay.md "릴레이 와이어").
   */
  handleSocket(ws: WebSocket, params: SocketParams, ip: string): void {
    this.#track(ws);
    const ipKey = pushIpKey(ip);
    if (params.kind === 'client-data') {
      this.#handleClientData(ws, params.serverId, params.connectionId ?? '', ipKey);
      return;
    }
    if (params.hostToken === null) {
      this.#awaitHostAuth(ws, params, ipKey);
      return;
    }
    if (!ownsServerId(params.serverId, params.hostToken)) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.unauthorized, 'invalid or missing host token');
      return;
    }
    this.#openHost(ws, params, ipKey);
  }

  #openHost(ws: WebSocket, params: SocketParams, ipKey: string): void {
    if (params.kind === 'control') this.#handleControl(ws, params.serverId, ipKey);
    else this.#handleHostData(ws, params.serverId, params.connectionId ?? '');
  }

  /**
   * A host socket without `hostToken` in its query: its first frame must be `host-auth`
   * within `authTimeoutMs`, or it closes 4401. Until then it is nothing to the hub; one
   * address may keep only `maxPendingPerIp` of these per serverId.
   */
  #awaitHostAuth(ws: WebSocket, params: SocketParams, ipKey: string): void {
    const key = `${params.serverId} ${ipKey}`;
    const waiting = this.#authPending.get(key) ?? 0;
    if (waiting >= this.#config.maxPendingPerIp) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.tooManyConnections, 'too many pending connections');
      return;
    }
    this.#authPending.set(key, waiting + 1);
    let settled = false;
    const settle = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      ws.off('message', onFirst);
      ws.off('close', settle);
      const left = (this.#authPending.get(key) ?? 1) - 1;
      if (left <= 0) this.#authPending.delete(key);
      else this.#authPending.set(key, left);
    };
    const onFirst = (data: RawData, isBinary: boolean): void => {
      settle();
      const frame = toBuffer(data);
      const token = isBinary || frame.length > 512 ? null : parseHostAuthFrame(frame.toString('utf8'));
      if (token === null || !ownsServerId(params.serverId, token)) {
        this.#untrack(ws);
        closeSocket(ws, CloseCode.unauthorized, 'invalid or missing host token');
        return;
      }
      this.#openHost(ws, params, ipKey);
    };
    const timer = setTimeout(() => {
      settle();
      this.#untrack(ws);
      closeSocket(ws, CloseCode.unauthorized, 'host-auth frame missing');
    }, this.#config.authTimeoutMs);
    timer.unref?.();
    ws.on('message', onFirst);
    ws.on('close', settle);
    ws.on('error', () => closeSocket(ws, CloseCode.normal, ''));
  }

  /** Rejects a socket that failed query validation. */
  reject(ws: WebSocket, code: number, reason: string): void {
    closeSocket(ws, code, reason);
  }

  /** Stops the keepalive timer; call when the HTTP server shuts down. */
  dispose(): void {
    if (this.#keepalive !== null) {
      clearInterval(this.#keepalive);
      this.#keepalive = null;
    }
    this.#alive.clear();
    this.#authPending.clear();
    this.#turnRateByServerId.clear();
    this.#turnRateByIp.clear();
    this.#turnRateGlobal.clear();
  }

  // --- control socket ------------------------------------------------------

  #handleControl(ws: WebSocket, serverId: string, ipKey: string): void {
    // A host replacing its own control socket was already counted; only a new one meets the caps.
    if ((this.#servers.get(serverId)?.control ?? null) === null) {
      let hosts = 0;
      let fromAddress = 0;
      for (const other of this.#servers.values()) {
        if (other.control === null) continue;
        hosts += 1;
        if (other.controlIp === ipKey) fromAddress += 1;
      }
      if (hosts >= this.#config.maxServers || fromAddress >= this.#config.maxServersPerIp) {
        this.#untrack(ws);
        closeSocket(ws, CloseCode.tooManyConnections, 'too many hosts');
        if (hosts >= this.#config.maxServers) this.#log('[relay] host cap reached');
        return;
      }
    }
    let entry = this.#servers.get(serverId);
    if (entry === undefined) {
      entry = { control: null, controlIp: null, connections: new Map() };
      this.#servers.set(serverId, entry);
    }

    const previous = entry.control;
    entry.control = ws;
    entry.controlIp = ipKey;
    if (previous !== null && previous !== ws) {
      // The newer control socket inherits the pending/paired connections.
      closeSocket(previous, CloseCode.conflict, 'replaced by newer control socket');
    }
    this.#log(`[${shortId(serverId)}] connect control`);

    ws.on('message', (data: RawData) => {
      this.#handleControlMessage(ws, serverId, ipKey, toBuffer(data));
    });
    ws.on('close', () => {
      this.#untrack(ws);
      this.#log(`[${shortId(serverId)}] disconnect control`);
      const current = this.#servers.get(serverId);
      if (current === undefined || current.control !== ws) return;
      current.control = null;
      current.controlIp = null;
      for (const connection of [...current.connections.values()]) {
        this.#teardown(connection, CloseCode.hostOffline, 'host offline', null);
      }
      if (current.connections.size === 0) this.#servers.delete(serverId);
    });
    ws.on('error', () => closeSocket(ws, CloseCode.normal, ''));
  }

  /**
   * Handles a message on the host control socket.
   * Only `turn-credentials-request` is acted upon; anything else is ignored
   * (the relay must not read or act on E2EE application frames that might
   * occasionally be routed through the control socket by accident).
   */
  #handleControlMessage(ws: WebSocket, serverId: string, ipKey: string, data: Buffer): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(data.toString('utf8'));
    } catch {
      return;
    }
    if (
      typeof parsed !== 'object' ||
      parsed === null ||
      (parsed as Record<string, unknown>)['type'] !== 'turn-credentials-request'
    ) {
      return;
    }
    this.#handleTurnRequest(ws, serverId, ipKey);
  }

  // --- TURN credential minting --------------------------------------------

  #handleTurnRequest(ws: WebSocket, serverId: string, ipKey: string): void {
    const secret = this.#config.turnSecret;
    if (!secret) {
      const msg: TurnUnavailable = { type: 'turn-unavailable' };
      ws.send(JSON.stringify(msg));
      return;
    }

    const now = Date.now();

    const perServer = this.#rateSlot(
      this.#turnRateByServerId,
      serverId,
      this.#config.turnMaxPerServerId,
      now,
    );
    if (!perServer.ok) {
      const msg: TurnRateLimited = {
        type: 'turn-rate-limited',
        retryAfterSecs: perServer.retryAfterSecs,
      };
      ws.send(JSON.stringify(msg));
      return;
    }

    const perIp = this.#rateSlot(this.#turnRateByIp, ipKey, this.#config.turnMaxPerIp, now);
    if (!perIp.ok) {
      const msg: TurnRateLimited = {
        type: 'turn-rate-limited',
        retryAfterSecs: perIp.retryAfterSecs,
      };
      ws.send(JSON.stringify(msg));
      return;
    }

    const global = this.#rateSlot(this.#turnRateGlobal, '*', this.#config.turnMaxGlobal, now);
    if (!global.ok) {
      const msg: TurnRateLimited = {
        type: 'turn-rate-limited',
        retryAfterSecs: global.retryAfterSecs,
      };
      ws.send(JSON.stringify(msg));
      this.#log(`[${shortId(serverId)}] turn-credentials refused: global mint cap reached`);
      return;
    }

    // Mint HMAC-SHA1 TURN credentials (coturn REST API format).
    const ttl = this.#config.turnCredentialTtlSecs;
    const expiry = Math.floor(now / 1000) + ttl;
    const username = `${expiry}:${serverId.slice(0, 16)}`;
    const password = createHmac('sha1', secret).update(username, 'utf8').digest('base64');

    // Record the mint.
    perServer.times.push(now);
    this.#turnRateByServerId.set(serverId, perServer.times);
    perIp.times.push(now);
    this.#turnRateByIp.set(ipKey, perIp.times);
    global.times.push(now);
    this.#turnRateGlobal.set('*', global.times);

    const uris = [`turn:${this.#config.turnHost}:${this.#config.turnPort}`];
    const msg: TurnCredentialsPayload = { type: 'turn-credentials', username, password, ttl, uris };
    ws.send(JSON.stringify(msg));
    this.#log(`[${shortId(serverId)}] turn-credentials minted`);
  }

  /**
   * Prunes mints older than the rate window out of `bucket[key]`, then reports whether
   * one more mint fits under `limit`. Buckets left empty are dropped so the maps cannot
   * grow without bound as hosts and source IPs come and go.
   */
  #rateSlot(
    bucket: Map<string, number[]>,
    key: string,
    limit: number,
    now: number,
  ):
    | { readonly ok: true; readonly times: number[] }
    | { readonly ok: false; readonly retryAfterSecs: number } {
    const window = this.#config.turnRateWindowMs;
    const times = (bucket.get(key) ?? []).filter((t) => now - t < window);
    if (times.length === 0) bucket.delete(key);
    else bucket.set(key, times);
    if (times.length < limit) return { ok: true, times };
    const oldest = times[0] ?? now;
    return { ok: false, retryAfterSecs: Math.max(1, Math.ceil((oldest + window - now) / 1000)) };
  }

  /** Drops rate-limit buckets whose mints have all aged out of the window. */
  #sweepTurnRates(now: number): void {
    const window = this.#config.turnRateWindowMs;
    for (const bucket of [this.#turnRateByServerId, this.#turnRateByIp, this.#turnRateGlobal]) {
      for (const [key, times] of bucket) {
        const live = times.filter((t) => now - t < window);
        if (live.length === 0) bucket.delete(key);
        else if (live.length !== times.length) bucket.set(key, live);
      }
    }
  }

  // --- client data socket --------------------------------------------------

  #handleClientData(ws: WebSocket, serverId: string, connectionId: string, ipKey: string): void {
    const entry = this.#servers.get(serverId);
    if (entry === undefined || entry.control === null) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.notFound, 'no host');
      return;
    }
    if (entry.connections.size >= this.#config.maxConnectionsPerServer) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.tooManyConnections, 'too many connections');
      return;
    }
    if (entry.connections.has(connectionId)) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.badRequest, 'duplicate connectionId');
      return;
    }
    let pending = 0;
    for (const other of entry.connections.values()) {
      if (other.ipKey === ipKey && !other.answered) pending += 1;
    }
    if (pending >= this.#config.maxPendingPerIp) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.tooManyConnections, 'too many pending connections');
      return;
    }

    const connection: Connection = {
      serverId,
      connectionId,
      client: ws,
      ipKey,
      host: null,
      buffered: [],
      bufferedBytes: 0,
      attachTimer: null,
      helloTimer: null,
      answered: false,
      closed: false,
    };
    entry.connections.set(connectionId, connection);
    connection.attachTimer = setTimeout(() => {
      this.#teardown(connection, CloseCode.attachTimeout, 'host data socket timeout', null);
    }, this.#config.attachTimeoutMs);
    connection.attachTimer.unref?.();
    // Phones and computers send `hello` as soon as the socket opens; a client that does not holds a slot for nothing.
    connection.helloTimer = setTimeout(() => {
      this.#teardown(connection, CloseCode.helloTimeout, 'hello timeout', null);
    }, this.#config.helloTimeoutMs);
    connection.helloTimer.unref?.();

    this.#log(`[${shortId(serverId)}] connect client ${connectionId}`);
    this.#notify(entry, { type: 'connected', connectionId });

    ws.on('message', (data: RawData, isBinary: boolean) => {
      const payload = toBuffer(data);
      if (connection.helloTimer !== null) {
        clearTimeout(connection.helloTimer);
        connection.helloTimer = null;
      }
      if (connection.host !== null) {
        this.#forwardOrClose(connection, connection.host, payload, isBinary);
        return;
      }
      if (
        connection.buffered.length >= this.#config.maxBufferedFrames ||
        connection.bufferedBytes + payload.byteLength > this.#config.maxBufferedBytes
      ) {
        this.#teardown(connection, CloseCode.bufferOverflow, 'buffer overflow', null);
        return;
      }
      connection.buffered.push({ payload, binary: isBinary });
      connection.bufferedBytes += payload.byteLength;
    });
    ws.on('close', (code: number, reason: Buffer) => {
      this.#untrack(ws);
      this.#teardown(connection, code, reason.toString('utf8'), 'client');
    });
    ws.on('error', () => closeSocket(ws, CloseCode.normal, ''));
  }

  // --- host data socket ----------------------------------------------------

  #handleHostData(ws: WebSocket, serverId: string, connectionId: string): void {
    const entry = this.#servers.get(serverId);
    const connection = entry?.connections.get(connectionId);
    if (entry === undefined || entry.control === null || connection === undefined) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.notFound, 'unknown connectionId');
      return;
    }
    if (connection.host !== null || connection.closed) {
      this.#untrack(ws);
      closeSocket(ws, CloseCode.notFound, 'connection already attached');
      return;
    }

    connection.host = ws;
    if (connection.attachTimer !== null) {
      clearTimeout(connection.attachTimer);
      connection.attachTimer = null;
    }
    this.#log(`[${shortId(serverId)}] connect host-data ${connectionId}`);

    const pending = connection.buffered;
    connection.buffered = [];
    connection.bufferedBytes = 0;
    for (const frame of pending) {
      if (connection.closed) break;
      this.#forwardOrClose(connection, ws, frame.payload, frame.binary);
    }

    ws.on('message', (data: RawData, isBinary: boolean) => {
      connection.answered = true;
      this.#forwardOrClose(connection, connection.client, toBuffer(data), isBinary);
    });
    ws.on('close', (code: number, reason: Buffer) => {
      this.#untrack(ws);
      this.#teardown(connection, code, reason.toString('utf8'), 'host');
    });
    ws.on('error', () => closeSocket(ws, CloseCode.normal, ''));
  }

  // --- shared --------------------------------------------------------------

  /**
   * Closes both halves of a connection with the same code/reason, drops it from the
   * server entry, and notifies the control socket exactly once.
   */
  #teardown(
    connection: Connection,
    code: number,
    reason: string,
    initiator: 'client' | 'host' | null,
  ): void {
    if (connection.closed) return;
    connection.closed = true;

    if (connection.attachTimer !== null) {
      clearTimeout(connection.attachTimer);
      connection.attachTimer = null;
    }
    if (connection.helloTimer !== null) {
      clearTimeout(connection.helloTimer);
      connection.helloTimer = null;
    }
    connection.buffered = [];
    connection.bufferedBytes = 0;

    const entry = this.#servers.get(connection.serverId);
    if (entry?.connections.get(connection.connectionId) === connection) {
      entry.connections.delete(connection.connectionId);
      if (entry.control === null && entry.connections.size === 0) {
        this.#servers.delete(connection.serverId);
      }
    }

    if (initiator !== 'client') closeSocket(connection.client, code, reason);
    if (initiator !== 'host' && connection.host !== null) {
      closeSocket(connection.host, code, reason);
    }

    this.#log(
      `[${shortId(connection.serverId)}] disconnect ${connection.connectionId} code=${code}`,
    );
    if (entry !== undefined) {
      this.#notify(entry, { type: 'disconnected', connectionId: connection.connectionId });
    }
  }

  #forwardOrClose(connection: Connection, target: WebSocket, payload: Buffer, binary: boolean): void {
    if (target.readyState !== target.OPEN) return;
    if (target.bufferedAmount + payload.byteLength > this.#config.maxSocketBufferedBytes) {
      this.#teardown(connection, CloseCode.socketBufferOverflow, 'socket buffer overflow', null);
      return;
    }
    target.send(payload, { binary });
  }

  #notify(entry: ServerEntry, notice: ControlNotice): void {
    const control = entry.control;
    if (control === null || control.readyState !== control.OPEN) return;
    control.send(JSON.stringify(notice));
  }

  #track(ws: WebSocket): void {
    this.#alive.set(ws, true);
    ws.on('pong', () => {
      if (this.#alive.has(ws)) this.#alive.set(ws, true);
    });
    ws.on('close', () => this.#untrack(ws));
  }

  #untrack(ws: WebSocket): void {
    this.#alive.delete(ws);
  }

  #startKeepalive(): void {
    this.#keepalive = setInterval(() => {
      this.#sweepTurnRates(Date.now());
      for (const [ws, alive] of this.#alive) {
        if (!alive) {
          this.#alive.delete(ws);
          ws.terminate();
          continue;
        }
        this.#alive.set(ws, false);
        if (ws.readyState === ws.OPEN) ws.ping();
      }
    }, this.#config.pingIntervalMs);
    this.#keepalive.unref?.();
  }
}

function closeSocket(ws: WebSocket, code: number, reason: string): void {
  if (ws.readyState === ws.CLOSED || ws.readyState === ws.CLOSING) return;
  ws.close(sendableCloseCode(code), truncateReason(reason));
}
