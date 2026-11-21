/**
 * One serverId's sockets (../src/hub.ts on Cloudflare): the host control socket, client data
 * sockets and their host halves, piped frame for frame. Sockets use the hibernation API, so
 * everything that must outlive an eviction — role, deadlines, tags and attachments — is kept on
 * the sockets themselves and enforced by Durable Object Alarms (`ctx.storage.setAlarm` +
 * `alarm()`). Only the frames a client sends before its host half attaches are held in memory.
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
  /** Epoch ms when an unauthenticated host socket closes 4401 if `host-auth` has not arrived. */
  authDeadline?: number;
  /** Epoch ms when a client socket closes 4408 if it has not sent its first frame (`hello`). */
  helloDeadline?: number;
  /** Epoch ms when a client socket closes 4504 if its host half has not attached. */
  attachDeadline?: number;
  /** A client the host half has sent something (its `ready`); until then it is pending. */
  answered?: boolean;
  done?: boolean;
}

interface Pending {
  frames: Array<string | ArrayBuffer>;
  bytes: number;
}

export interface HubEnv {
  readonly RELAY_ATTACH_TIMEOUT_MS?: string;
  readonly RELAY_MAX_CONNECTIONS?: string;
  readonly RELAY_MAX_BUFFERED_FRAMES?: string;
  readonly RELAY_MAX_BUFFERED_BYTES?: string;
  readonly RELAY_MAX_PAYLOAD?: string;
  readonly RELAY_AUTH_TIMEOUT_MS?: string;
  readonly RELAY_HELLO_TIMEOUT_MS?: string;
  readonly RELAY_MAX_PENDING_PER_IP?: string;
}

const WS_OPEN = typeof WebSocket !== 'undefined' && typeof WebSocket.READY_STATE_OPEN === 'number'
  ? WebSocket.READY_STATE_OPEN
  : 1;

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
  readonly #maxBufferedBytes: number;
  readonly #maxPayload: number;
  readonly #authTimeoutMs: number;
  readonly #helloTimeoutMs: number;
  readonly #maxPendingPerIp: number;
  readonly #pending = new Map<string, Pending>();

  constructor(ctx: DurableObjectState, env: HubEnv) {
    super(ctx, env);
    this.#attachTimeoutMs = envInt(env.RELAY_ATTACH_TIMEOUT_MS, 10_000);
    this.#maxConnections = envInt(env.RELAY_MAX_CONNECTIONS, 32);
    this.#maxBufferedFrames = envInt(env.RELAY_MAX_BUFFERED_FRAMES, 64);
    this.#maxBufferedBytes = envInt(env.RELAY_MAX_BUFFERED_BYTES, 2 * 1024 * 1024);
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
    let alarmDirty = false;
    switch (kind) {
      case 'control':
        if (byFrame) alarmDirty = this.#awaitAuth(server, 'control', serverId, null, ip);
        else this.#openControl(server, serverId, ip, false);
        break;
      case 'client-data':
        alarmDirty = this.#openClient(server, serverId, connectionId, ip);
        break;
      case 'host-data':
        if (byFrame) alarmDirty = this.#awaitAuth(server, 'host', serverId, connectionId, ip);
        else alarmDirty = this.#openHost(server, serverId, connectionId, ip, false);
        break;
      default:
        server.accept();
        closeSocket(server, CloseCode.badRequest, 'bad role');
    }
    if (alarmDirty) await this.#syncAlarm();
    return new Response(null, { status: 101, webSocket: client });
  }

  // --- opening ---------------------------------------------------------------

  /**
   * A host socket without `hostToken` in its query: accepted with only its permanent role tags
   * (no permanent `'auth'` tag), and left out of `#live` via `authPending` in its attachment until
   * its `host-auth` frame arrives; `alarm()` closes it 4401 once `authDeadline` passes.
   */
  #awaitAuth(ws: WebSocket, kind: 'control' | 'host', serverId: string, connectionId: string | null, ip: string): boolean {
    const waiting = this.ctx.getWebSockets().filter((other) => {
      const me = this.#attachment(other);
      return other.readyState === WS_OPEN && me?.authPending === true && me.done !== true && me.ip === ip;
    });
    if (waiting.length >= this.#maxPendingPerIp) {
      this.#refuse(ws, CloseCode.tooManyConnections, 'too many pending connections');
      return false;
    }
    const tags = kind === 'control' ? ['control'] : ['host', tagFor.host(connectionId ?? '')];
    this.#accept(ws, tags, {
      kind,
      connectionId,
      serverId,
      ip,
      authPending: true,
      authDeadline: Date.now() + this.#authTimeoutMs,
    });
    return true;
  }

  /** The first frame of a socket in `#awaitAuth`: a good `host-auth` makes it what it asked to be. */
  #authFrame(ws: WebSocket, me: Attachment, message: string | ArrayBuffer): Promise<void> {
    const token = typeof message === 'string' && message.length <= 512 ? parseHostAuthFrame(message) : null;
    if (token === null || !ownsServerId(me.serverId, token)) {
      this.#retire(ws);
      closeSocket(ws, CloseCode.unauthorized, 'invalid or missing host token');
      return this.#syncAlarm();
    }
    if (me.kind === 'control') this.#openControl(ws, me.serverId, me.ip ?? '', true);
    else this.#openHost(ws, me.serverId, me.connectionId ?? '', me.ip ?? '', true);
    return this.#syncAlarm();
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

  #openClient(ws: WebSocket, serverId: string, connectionId: string, ip: string): boolean {
    if (this.#control() === null) {
      this.#refuse(ws, CloseCode.notFound, 'no host');
      return false;
    }
    const clients = this.#live('client');
    if (clients.length >= this.#maxConnections) {
      this.#refuse(ws, CloseCode.tooManyConnections, 'too many connections');
      return false;
    }
    if (this.#live(tagFor.client(connectionId)).length > 0) {
      this.#refuse(ws, CloseCode.badRequest, 'duplicate connectionId');
      return false;
    }
    const unanswered = clients.filter((other) => {
      const me = this.#attachment(other);
      return me?.ip === ip && me.answered !== true;
    });
    if (unanswered.length >= this.#maxPendingPerIp) {
      this.#refuse(ws, CloseCode.tooManyConnections, 'too many pending connections');
      return false;
    }
    const now = Date.now();
    // Phones and computers send `hello` as soon as the socket opens; both deadlines live on
    // the attachment so DO hibernation never drops either timeout.
    this.#accept(ws, ['client', tagFor.client(connectionId)], {
      kind: 'client',
      connectionId,
      serverId,
      ip,
      helloDeadline: now + this.#helloTimeoutMs,
      attachDeadline: now + this.#attachTimeoutMs,
    });
    this.#pending.set(connectionId, { frames: [], bytes: 0 });
    this.#log(serverId, `connect client ${connectionId}`);
    this.#notify({ type: 'connected', connectionId });
    return true;
  }

  #openHost(ws: WebSocket, serverId: string, connectionId: string, ip: string, accepted: boolean): boolean {
    const reject = (code: number, reason: string): boolean => {
      if (!accepted) {
        this.#refuse(ws, code, reason);
        return false;
      }
      this.#retire(ws);
      closeSocket(ws, code, reason);
      return true;
    };
    const client = this.#live(tagFor.client(connectionId))[0];
    if (this.#control() === null || client === undefined) return reject(CloseCode.notFound, 'unknown connectionId');
    if (this.#live(tagFor.host(connectionId)).length > 0) return reject(CloseCode.notFound, 'connection already attached');
    const attachment: Attachment = { kind: 'host', connectionId, serverId, ip };
    if (accepted) ws.serializeAttachment(attachment);
    else this.#accept(ws, ['host', tagFor.host(connectionId)], attachment);
    const clientAttachment = this.#attachment(client);
    let clearedAttach = false;
    if (clientAttachment !== null && clientAttachment.attachDeadline !== undefined) {
      const { attachDeadline: _, ...rest } = clientAttachment;
      client.serializeAttachment(rest);
      clearedAttach = true;
    }
    this.#log(serverId, `connect host-data ${connectionId}`);
    const pending = this.#pending.get(connectionId);
    this.#pending.delete(connectionId);
    if (pending !== undefined) {
      for (const frame of pending.frames) {
        if (this.#done(ws)) break;
        ws.send(frame);
      }
    }
    return accepted || clearedAttach;
  }

  // --- events ----------------------------------------------------------------

  override webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): void | Promise<void> {
    const me = this.#attachment(ws);
    if (me === null || me.done === true) return;
    if (me.authPending === true) return this.#authFrame(ws, me, message);
    const size = byteLength(message);
    if (size > this.#maxPayload) {
      if (me.kind === 'control') {
        this.#retire(ws);
        closeSocket(ws, 1009, 'message too big');
        this.#controlGone(me.serverId);
      } else {
        this.#teardown(me.connectionId ?? '', 1009, 'message too big', null);
      }
      return this.#syncAlarm();
    }
    switch (me.kind) {
      case 'control':
        this.#controlMessage(ws, message);
        return;
      case 'client': {
        const id = me.connectionId ?? '';
        let alarmDirty = false;
        if (me.helloDeadline !== undefined) {
          const { helloDeadline: _, ...rest } = me;
          ws.serializeAttachment(rest);
          alarmDirty = true;
        }
        const host = this.#live(tagFor.host(id))[0];
        if (host !== undefined) {
          host.send(message);
          if (alarmDirty) return this.#syncAlarm();
          return;
        }
        let pending = this.#pending.get(id);
        if (pending === undefined) {
          // The object hibernated between the client's arrival and now; attachDeadline stays on the attachment.
          pending = { frames: [], bytes: 0 };
          this.#pending.set(id, pending);
        }
        if (pending.frames.length >= this.#maxBufferedFrames || pending.bytes + size > this.#maxBufferedBytes) {
          this.#teardown(id, CloseCode.bufferOverflow, 'buffer overflow', null);
          return this.#syncAlarm();
        }
        pending.frames.push(message);
        pending.bytes += size;
        if (alarmDirty) return this.#syncAlarm();
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

  override async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    const me = this.#attachment(ws);
    // A socket the relay closed has finished its handshake; any other is answered here.
    if (me === null || me.done === true) return;
    closeSocket(ws, code, reason);
    this.#retire(ws);
    if (me.authPending === true) {
      await this.#syncAlarm();
      return;
    }
    if (me.kind === 'control') {
      this.#log(me.serverId, 'disconnect control');
      this.#controlGone(me.serverId);
      await this.#syncAlarm();
      return;
    }
    this.#teardown(me.connectionId ?? '', code, reason, me.kind);
    await this.#syncAlarm();
  }

  override async webSocketError(ws: WebSocket): Promise<void> {
    const me = this.#attachment(ws);
    if (me === null || me.done === true) return;
    this.#retire(ws);
    closeSocket(ws, CloseCode.normal, '');
    if (me.authPending === true) {
      await this.#syncAlarm();
      return;
    }
    if (me.kind === 'control') this.#controlGone(me.serverId);
    else this.#teardown(me.connectionId ?? '', CloseCode.normal, '', me.kind);
    await this.#syncAlarm();
  }

  /**
   * Enforces every persisted socket deadline (host-auth, client hello, host attach) even across
   * DO hibernation/eviction, then schedules the next alarm if any pending deadlines remain.
   */
  override async alarm(): Promise<void> {
    const now = Date.now();
    for (const ws of this.ctx.getWebSockets()) {
      if (ws.readyState !== WS_OPEN) continue;
      const me = this.#attachment(ws);
      if (me === null || me.done === true) continue;
      if (me.authPending === true) {
        if (me.authDeadline !== undefined && me.authDeadline <= now) {
          this.#retire(ws);
          closeSocket(ws, CloseCode.unauthorized, 'host-auth frame missing');
        }
        continue;
      }
      if (me.kind !== 'client') continue;
      const helloExpired = me.helloDeadline !== undefined && me.helloDeadline <= now;
      const attachExpired = me.attachDeadline !== undefined && me.attachDeadline <= now;
      if (!helloExpired && !attachExpired) continue;
      if (helloExpired && (!attachExpired || (me.helloDeadline as number) < (me.attachDeadline as number))) {
        this.#teardown(me.connectionId ?? '', CloseCode.helloTimeout, 'hello timeout', null);
      } else {
        this.#teardown(me.connectionId ?? '', CloseCode.attachTimeout, 'host data socket timeout', null);
      }
    }
    await this.#syncAlarm();
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
    this.#pending.delete(connectionId);
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

  /** Earliest pending deadline across all live open sockets, or null when none are waiting. */
  #nextDeadline(): number | null {
    let earliest: number | null = null;
    for (const ws of this.ctx.getWebSockets()) {
      if (ws.readyState !== WS_OPEN) continue;
      const me = this.#attachment(ws);
      if (me === null || me.done === true) continue;
      for (const deadline of [me.authDeadline, me.helloDeadline, me.attachDeadline]) {
        if (deadline !== undefined && (earliest === null || deadline < earliest)) {
          earliest = deadline;
        }
      }
    }
    return earliest;
  }

  /** Keeps the DO alarm aligned with the earliest persisted deadline, or deletes it when none remain. */
  async #syncAlarm(): Promise<void> {
    const current = await this.ctx.storage.getAlarm();
    const earliest = this.#nextDeadline();
    if (earliest === null) {
      if (current !== null) await this.ctx.storage.deleteAlarm();
      return;
    }
    if (current !== earliest) {
      await this.ctx.storage.setAlarm(earliest);
    }
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

  /** Marks a socket as one the relay is closing, stripping deadlines so its close event changes nothing more. */
  #retire(ws: WebSocket): void {
    const me = this.#attachment(ws);
    if (me !== null) {
      const { authDeadline: _a, helloDeadline: _h, attachDeadline: _t, ...rest } = me;
      ws.serializeAttachment({ ...rest, done: true });
    }
  }

  /** Open sockets under `tag` that the relay is not already closing and that are not still owing `host-auth`. */
  #live(tag: string): WebSocket[] {
    return this.ctx.getWebSockets(tag).filter((ws) => {
      if (ws.readyState !== WS_OPEN) return false;
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
