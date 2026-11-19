/**
 * One serverId's sockets (../src/hub.ts on Cloudflare): the host control socket, client data
 * sockets and their host halves, piped frame for frame. Sockets use the hibernation API, so
 * what must outlive an eviction is on the sockets themselves (tags and attachments); only the
 * frames a client sends before its host half attaches are in memory, and the attach timer
 * that is pending meanwhile keeps the object awake.
 *
 * Before a socket counts it must vouch for itself: a host socket opened without `hostToken`
 * in its query sends a `host-auth` first frame within the auth deadline, and a client sends
 * its first frame (the E2EE `hello`) within the hello deadline. One address (IPv6: its /64)
 * may hold only a few sockets here that have not got that far (client sockets the host has
 * not answered yet, host sockets owing their frame), so strangers cannot squat the slots.
 *
 * Differences from the Node relay: the runtime answers pings (the Mac and Windows hosts ping,
 * the phone's encrypted channel pings every 20 s), so the relay sends none; Workers keep no
 * per-socket send buffer to measure, so 4507 never comes; TURN is off (turn-unavailable).
 */

import { DurableObject } from 'cloudflare:workers';

import {
  CloseCode,
  ownsServerId,
  parseHostAuthFrame,
  sendableCloseCode,
  type ControlNotice,
  type TurnUnavailable,
} from '../../src/protocol.ts';
import { pushIpKey } from './ip.ts';

/** Headers the Worker sets on the request it hands a Hub (never trusted from the client). */
export const HUB_HEADERS = {
  kind: 'x-mighty-relay-kind',
  serverId: 'x-mighty-relay-server-id',
  connectionId: 'x-mighty-relay-connection-id',
  ip: 'x-mighty-relay-ip',
  /** `query` when the Worker checked the host token, `frame` when a `host-auth` frame must bring it. */
  auth: 'x-mighty-relay-auth',
} as const;

type Kind = 'control' | 'client' | 'host';

/** What a socket is, kept on it across hibernation. `done` sockets are being closed by the relay. */
interface Attachment {
  readonly kind: Kind;
  readonly connectionId: string | null;
  readonly serverId: string;
  /** The source address as per-address caps count it (IPv6: its /64). */
  readonly ip?: string;
  /** A host socket still owing its `host-auth` frame: not yet the control socket or a host half. */
  authPending?: boolean;
  /** A client the host half has sent something (its `ready`); until then it is pending. */
  answered?: boolean;
  done?: boolean;
}

interface Pending {
  frames: Array<string | ArrayBuffer>;
  timer: ReturnType<typeof setTimeout>;
}

interface HubEnv {
  readonly RELAY_ATTACH_TIMEOUT_MS?: string;
  readonly RELAY_MAX_CONNECTIONS?: string;
  readonly RELAY_MAX_BUFFERED_FRAMES?: string;
  readonly RELAY_MAX_PAYLOAD?: string;
  readonly RELAY_AUTH_TIMEOUT_MS?: string;
  readonly RELAY_HELLO_TIMEOUT_MS?: string;
  readonly RELAY_MAX_PENDING_PER_IP?: string;
}

function envInt(raw: string | undefined, fallback: number): number {
  const parsed = Number.parseInt(raw ?? '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

/** Close reasons are capped at 123 bytes by RFC 6455. */
function truncateReason(reason: string): string {
  const encoder = new TextEncoder();
  let out = reason;
  while (encoder.encode(out).byteLength > 123) out = out.slice(0, -1);
  return out;
}

function closeSocket(ws: WebSocket, code: number, reason: string): void {
  try {
    ws.close(sendableCloseCode(code), truncateReason(reason));
  } catch {
    // Already closed or closing.
  }
}

function byteLength(message: string | ArrayBuffer): number {
  return typeof message === 'string' ? new TextEncoder().encode(message).byteLength : message.byteLength;
}

const tagFor = { client: (id: string) => `c:${id}`, host: (id: string) => `h:${id}` } as const;

export class Hub extends DurableObject<HubEnv> {
  readonly #attachTimeoutMs: number;
  readonly #maxConnections: number;
  readonly #maxBufferedFrames: number;
  readonly #maxPayload: number;
  readonly #authTimeoutMs: number;
  readonly #helloTimeoutMs: number;
  readonly #maxPendingPerIp: number;
  readonly #pending = new Map<string, Pending>();
  /** Clients that have not sent their first frame yet, by connectionId. */
  readonly #helloTimers = new Map<string, ReturnType<typeof setTimeout>>();

  constructor(ctx: DurableObjectState, env: HubEnv) {
    super(ctx, env);
    this.#attachTimeoutMs = envInt(env.RELAY_ATTACH_TIMEOUT_MS, 10_000);
    this.#maxConnections = envInt(env.RELAY_MAX_CONNECTIONS, 32);
    this.#maxBufferedFrames = envInt(env.RELAY_MAX_BUFFERED_FRAMES, 64);
    this.#maxPayload = envInt(env.RELAY_MAX_PAYLOAD, 1024 * 1024);
    this.#authTimeoutMs = envInt(env.RELAY_AUTH_TIMEOUT_MS, 5_000);
    this.#helloTimeoutMs = envInt(env.RELAY_HELLO_TIMEOUT_MS, 5_000);
    this.#maxPendingPerIp = envInt(env.RELAY_MAX_PENDING_PER_IP, 8);
  }

  override async fetch(request: Request): Promise<Response> {
    const kind = request.headers.get(HUB_HEADERS.kind);
    const serverId = request.headers.get(HUB_HEADERS.serverId) ?? '';
    const connectionId = request.headers.get(HUB_HEADERS.connectionId) ?? '';
    const ip = pushIpKey(request.headers.get(HUB_HEADERS.ip) ?? '0.0.0.0');
    const byFrame = request.headers.get(HUB_HEADERS.auth) === 'frame';
    const pair = new WebSocketPair();
    const [client, server] = [pair[0], pair[1]];
    switch (kind) {
      case 'control':
        if (byFrame) this.#awaitAuth(server, 'control', serverId, null, ip);
        else this.#openControl(server, serverId, ip, false);
        break;
      case 'client-data':
        this.#openClient(server, serverId, connectionId, ip);
        break;
      case 'host-data':
        if (byFrame) this.#awaitAuth(server, 'host', serverId, connectionId, ip);
        else this.#openHost(server, serverId, connectionId, ip, false);
        break;
      default:
        server.accept();
        closeSocket(server, CloseCode.badRequest, 'bad role');
    }
    return new Response(null, { status: 101, webSocket: client });
  }

  // --- opening ---------------------------------------------------------------

  /**
   * A host socket without `hostToken` in its query: accepted with the tags it will have, but
   * left out of everything (`#live`) until its `host-auth` frame comes; 4401 after the deadline.
   */
  #awaitAuth(ws: WebSocket, kind: 'control' | 'host', serverId: string, connectionId: string | null, ip: string): void {
    const waiting = this.ctx.getWebSockets('auth').filter((other) => {
      const me = this.#attachment(other);
      return other.readyState === WebSocket.READY_STATE_OPEN && me?.authPending === true && me.done !== true && me.ip === ip;
    });
    if (waiting.length >= this.#maxPendingPerIp) return this.#refuse(ws, CloseCode.tooManyConnections, 'too many pending connections');
    const tags = kind === 'control' ? ['control', 'auth'] : ['host', tagFor.host(connectionId ?? ''), 'auth'];
    this.#accept(ws, tags, { kind, connectionId, serverId, ip, authPending: true });
    setTimeout(() => {
      const me = this.#attachment(ws);
      if (me?.authPending !== true || me.done === true) return;
      this.#retire(ws);
      closeSocket(ws, CloseCode.unauthorized, 'host-auth frame missing');
    }, this.#authTimeoutMs);
  }

  /** The first frame of a socket in `#awaitAuth`: a good `host-auth` makes it what it asked to be. */
  #authFrame(ws: WebSocket, me: Attachment, message: string | ArrayBuffer): void {
    const token = typeof message === 'string' && message.length <= 512 ? parseHostAuthFrame(message) : null;
    if (token === null || !ownsServerId(me.serverId, token)) {
      this.#retire(ws);
      closeSocket(ws, CloseCode.unauthorized, 'invalid or missing host token');
      return;
    }
    if (me.kind === 'control') this.#openControl(ws, me.serverId, me.ip ?? '', true);
    else this.#openHost(ws, me.serverId, me.connectionId ?? '', me.ip ?? '', true);
  }

  #openControl(ws: WebSocket, serverId: string, ip: string, accepted: boolean): void {
    for (const previous of this.#live('control')) {
      // The newer control socket inherits the pending and paired connections.
      this.#retire(previous);
      closeSocket(previous, CloseCode.conflict, 'replaced by newer control socket');
    }
    const attachment: Attachment = { kind: 'control', connectionId: null, serverId, ip };
    if (accepted) ws.serializeAttachment(attachment);
    else this.#accept(ws, ['control'], attachment);
    this.#log(serverId, 'connect control');
  }

  #openClient(ws: WebSocket, serverId: string, connectionId: string, ip: string): void {
    if (this.#control() === null) return this.#refuse(ws, CloseCode.notFound, 'no host');
    const clients = this.#live('client');
    if (clients.length >= this.#maxConnections) return this.#refuse(ws, CloseCode.tooManyConnections, 'too many connections');
    if (this.#live(tagFor.client(connectionId)).length > 0) return this.#refuse(ws, CloseCode.badRequest, 'duplicate connectionId');
    const unanswered = clients.filter((other) => {
      const me = this.#attachment(other);
      return me?.ip === ip && me.answered !== true;
    });
    if (unanswered.length >= this.#maxPendingPerIp) return this.#refuse(ws, CloseCode.tooManyConnections, 'too many pending connections');
    this.#accept(ws, ['client', tagFor.client(connectionId)], { kind: 'client', connectionId, serverId, ip });
    this.#pending.set(connectionId, {
      frames: [],
      timer: setTimeout(() => this.#teardown(connectionId, CloseCode.attachTimeout, 'host data socket timeout', null), this.#attachTimeoutMs),
    });
    // Phones and computers send `hello` as soon as the socket opens; a client that does not holds a slot for nothing.
    this.#helloTimers.set(connectionId, setTimeout(() => this.#teardown(connectionId, CloseCode.helloTimeout, 'hello timeout', null), this.#helloTimeoutMs));
    this.#log(serverId, `connect client ${connectionId}`);
    this.#notify({ type: 'connected', connectionId });
  }

  #openHost(ws: WebSocket, serverId: string, connectionId: string, ip: string, accepted: boolean): void {
    const reject = (code: number, reason: string): void => {
      if (!accepted) return this.#refuse(ws, code, reason);
      this.#retire(ws);
      closeSocket(ws, code, reason);
    };
    if (this.#control() === null || this.#live(tagFor.client(connectionId)).length === 0) return reject(CloseCode.notFound, 'unknown connectionId');
    if (this.#live(tagFor.host(connectionId)).length > 0) return reject(CloseCode.notFound, 'connection already attached');
    const attachment: Attachment = { kind: 'host', connectionId, serverId, ip };
    if (accepted) ws.serializeAttachment(attachment);
    else this.#accept(ws, ['host', tagFor.host(connectionId)], attachment);
    this.#log(serverId, `connect host-data ${connectionId}`);
    const pending = this.#pending.get(connectionId);
    this.#pending.delete(connectionId);
    if (pending === undefined) return;
    clearTimeout(pending.timer);
    for (const frame of pending.frames) {
      if (this.#done(ws)) break;
      ws.send(frame);
    }
  }

  // --- events ----------------------------------------------------------------

  override webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): void {
    const me = this.#attachment(ws);
    if (me === null || me.done === true) return;
    if (me.authPending === true) return this.#authFrame(ws, me, message);
    if (byteLength(message) > this.#maxPayload) {
      if (me.kind === 'control') {
        this.#retire(ws);
        closeSocket(ws, 1009, 'message too big');
        this.#controlGone(me.serverId);
      } else {
        this.#teardown(me.connectionId ?? '', 1009, 'message too big', null);
      }
      return;
    }
    switch (me.kind) {
      case 'control':
        this.#controlMessage(ws, message);
        return;
      case 'client': {
        const id = me.connectionId ?? '';
        const hello = this.#helloTimers.get(id);
        if (hello !== undefined) {
          clearTimeout(hello);
          this.#helloTimers.delete(id);
        }
        const host = this.#live(tagFor.host(id))[0];
        if (host !== undefined) {
          host.send(message);
          return;
        }
        let pending = this.#pending.get(id);
        if (pending === undefined) {
          // The object was evicted between the client's arrival and now: start the wait again.
          pending = { frames: [], timer: setTimeout(() => this.#teardown(id, CloseCode.attachTimeout, 'host data socket timeout', null), this.#attachTimeoutMs) };
          this.#pending.set(id, pending);
        }
        if (pending.frames.length >= this.#maxBufferedFrames) {
          this.#teardown(id, CloseCode.bufferOverflow, 'buffer overflow', null);
          return;
        }
        pending.frames.push(message);
        return;
      }
      case 'host': {
        const client = this.#live(tagFor.client(me.connectionId ?? ''))[0];
        if (client === undefined) return;
        const theirs = this.#attachment(client);
        // Answered by its host, the client no longer counts against its address's pending cap.
        if (theirs !== null && theirs.answered !== true) client.serializeAttachment({ ...theirs, answered: true });
        client.send(message);
        return;
      }
    }
  }

  override webSocketClose(ws: WebSocket, code: number, reason: string): void {
    const me = this.#attachment(ws);
    // A socket the relay closed has finished its handshake; any other is answered here.
    if (me === null || me.done === true) return;
    closeSocket(ws, code, reason);
    this.#retire(ws);
    if (me.authPending === true) return;
    if (me.kind === 'control') {
      this.#log(me.serverId, 'disconnect control');
      this.#controlGone(me.serverId);
      return;
    }
    this.#teardown(me.connectionId ?? '', code, reason, me.kind);
  }

  override webSocketError(ws: WebSocket): void {
    const me = this.#attachment(ws);
    if (me === null || me.done === true) return;
    this.#retire(ws);
    closeSocket(ws, CloseCode.normal, '');
    if (me.authPending === true) return;
    if (me.kind === 'control') this.#controlGone(me.serverId);
    else this.#teardown(me.connectionId ?? '', CloseCode.normal, '', me.kind);
  }

  // --- control ---------------------------------------------------------------

  /**
   * Only `turn-credentials-request` is acted on; anything else is ignored (the relay must not
   * act on E2EE frames that reach the control socket by accident). This relay has no TURN.
   */
  #controlMessage(ws: WebSocket, message: string | ArrayBuffer): void {
    let parsed: unknown;
    try {
      parsed = JSON.parse(typeof message === 'string' ? message : new TextDecoder().decode(message));
    } catch {
      return;
    }
    if (typeof parsed !== 'object' || parsed === null || (parsed as Record<string, unknown>)['type'] !== 'turn-credentials-request') return;
    const reply: TurnUnavailable = { type: 'turn-unavailable' };
    ws.send(JSON.stringify(reply));
  }

  /** The control socket left without a successor: every connection closes 4410. */
  #controlGone(_serverId: string): void {
    if (this.#control() !== null) return;
    const ids = new Set<string>();
    for (const ws of [...this.#live('client'), ...this.#live('host')]) {
      const id = this.#attachment(ws)?.connectionId;
      if (id) ids.add(id);
    }
    for (const id of this.#pending.keys()) ids.add(id);
    for (const id of ids) this.#teardown(id, CloseCode.hostOffline, 'host offline', null);
  }

  // --- shared ----------------------------------------------------------------

  /** Closes both halves of a connection with the same code and reason and tells the control socket once. */
  #teardown(connectionId: string, code: number, reason: string, initiator: 'client' | 'host' | null): void {
    const pending = this.#pending.get(connectionId);
    if (pending !== undefined) clearTimeout(pending.timer);
    this.#pending.delete(connectionId);
    const hello = this.#helloTimers.get(connectionId);
    if (hello !== undefined) clearTimeout(hello);
    this.#helloTimers.delete(connectionId);
    const halves = [...this.#live(tagFor.client(connectionId)), ...this.#live(tagFor.host(connectionId))];
    if (halves.length === 0 && initiator === null) return;
    let serverId = '';
    for (const ws of halves) {
      const me = this.#attachment(ws);
      serverId = me?.serverId ?? serverId;
      this.#retire(ws);
      if (me?.kind !== initiator) closeSocket(ws, code, reason);
    }
    if (serverId !== '') this.#log(serverId, `disconnect ${connectionId} code=${code}`);
    this.#notify({ type: 'disconnected', connectionId });
  }

  #notify(notice: ControlNotice): void {
    const control = this.#control();
    if (control === null) return;
    try {
      control.send(JSON.stringify(notice));
    } catch {
      // The control socket is going away; its close handler tears down.
    }
  }

  #accept(ws: WebSocket, tags: string[], attachment: Attachment): void {
    this.ctx.acceptWebSocket(ws, tags);
    ws.serializeAttachment(attachment);
  }

  #refuse(ws: WebSocket, code: number, reason: string): void {
    ws.accept();
    closeSocket(ws, code, reason);
  }

  #attachment(ws: WebSocket): Attachment | null {
    const value = ws.deserializeAttachment() as Attachment | null | undefined;
    return value ?? null;
  }

  #done(ws: WebSocket): boolean {
    return this.#attachment(ws)?.done === true;
  }

  /** Marks a socket as one the relay is closing, so its close event changes nothing more. */
  #retire(ws: WebSocket): void {
    const me = this.#attachment(ws);
    if (me !== null) ws.serializeAttachment({ ...me, done: true });
  }

  /** Open sockets under `tag` that the relay is not already closing and that are not still owing `host-auth`. */
  #live(tag: string): WebSocket[] {
    return this.ctx.getWebSockets(tag).filter((ws) => {
      if (ws.readyState !== WebSocket.READY_STATE_OPEN) return false;
      const me = this.#attachment(ws);
      return me?.done !== true && me?.authPending !== true;
    });
  }

  #control(): WebSocket | null {
    return this.#live('control')[0] ?? null;
  }

  #log(serverId: string, line: string): void {
    console.log(`[${serverId.slice(0, 8)}] ${line}`);
  }
}
