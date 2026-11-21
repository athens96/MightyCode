/**
 * Test harnesses for the Cloudflare Worker relay:
 * - `startWorker`: runs the full Worker under `wrangler dev` (local workerd) for wire parity tests.
 * - `createHubHarness` / `MockDurableObjectState`: drives `Hub` directly with persisted WebSocket
 *   attachments, DO storage alarms (`setAlarm`/`getAlarm`/`deleteAlarm`) and simulated hibernation.
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { join } from 'node:path';

export interface LocalWorker {
  readonly base: string;
  /** Stops the process; the persisted state stays for another `startWorker` on the same folder. */
  stop(): Promise<void>;
}

/** `wrangler dev` on a free-ish port with `vars` and Durable Object state in `state`, once /healthz answers. */
export async function startWorker(state: string, vars: Record<string, string>): Promise<LocalWorker> {
  const port = 8799 + Math.floor(Math.random() * 400);
  const base = `127.0.0.1:${port}`;
  const varArgs = Object.entries(vars).flatMap(([key, value]) => ['--var', `${key}:${value}`]);
  const worker: ChildProcess = spawn(
    'npx',
    ['wrangler', 'dev', '--ip', '127.0.0.1', '--port', String(port), '--persist-to', state, '--show-interactive-dev-session=false', ...varArgs],
    { cwd: join(import.meta.dirname, '..'), env: { ...process.env, WRANGLER_SEND_METRICS: 'false', CI: '1' }, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let output = '';
  worker.stdout?.on('data', (chunk: Buffer) => { output += chunk.toString(); });
  worker.stderr?.on('data', (chunk: Buffer) => { output += chunk.toString(); });
  const deadline = Date.now() + 90_000;
  for (;;) {
    try {
      if ((await fetch(`http://${base}/healthz`)).status === 200) break;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline || worker.exitCode !== null) {
      worker.kill('SIGTERM');
      throw new Error(`wrangler dev did not start:\n${output}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
  return {
    base,
    stop: () =>
      new Promise((resolve) => {
        if (worker.exitCode !== null) return resolve();
        worker.once('exit', () => resolve());
        worker.kill('SIGTERM');
        setTimeout(() => { worker.kill('SIGKILL'); resolve(); }, 5000);
      }),
  };
}

// --- Durable Object alarm & hibernation unit harness -------------------------

/** Minimal Cloudflare hibernatable WebSocket double with structured-cloned attachments. */
export class MockWebSocket {
  readyState = 1;
  accepted = false;
  readonly sent: Array<string | ArrayBuffer> = [];
  closed: { readonly code: number | undefined; readonly reason: string | undefined } | null = null;
  peer: MockWebSocket | null = null;
  #attachment: unknown = null;

  accept(): void {
    this.accepted = true;
  }

  send(data: string | ArrayBuffer): void {
    if (this.readyState !== 1) throw new Error('WebSocket is not open');
    this.sent.push(data);
  }

  close(code?: number, reason?: string): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.closed = { code, reason };
  }

  serializeAttachment(value: unknown): void {
    this.#attachment = value === undefined || value === null ? null : structuredClone(value);
  }

  deserializeAttachment(): unknown {
    return this.#attachment === null ? null : structuredClone(this.#attachment);
  }
}

/** Durable Object storage double implementing `getAlarm`, `setAlarm`, and `deleteAlarm`. */
export class MockDurableObjectStorage {
  alarmTime: number | null = null;
  readonly setAlarmCalls: number[] = [];
  deleteAlarmCalls = 0;

  async getAlarm(): Promise<number | null> {
    return this.alarmTime;
  }

  async setAlarm(scheduledTime: number | Date): Promise<void> {
    const ms = scheduledTime instanceof Date ? scheduledTime.getTime() : scheduledTime;
    this.alarmTime = ms;
    this.setAlarmCalls.push(ms);
  }

  async deleteAlarm(): Promise<void> {
    this.alarmTime = null;
    this.deleteAlarmCalls += 1;
  }
}

/** Durable Object state double supporting WebSocket hibernation tags, attachments, and alarms. */
export class MockDurableObjectState {
  readonly storage = new MockDurableObjectStorage();
  readonly #sockets = new Map<MockWebSocket, string[]>();

  acceptWebSocket(ws: WebSocket, tags: string[] = []): void {
    const mock = ws as unknown as MockWebSocket;
    mock.accept();
    this.#sockets.set(mock, [...tags]);
  }

  getWebSockets(tag?: string): WebSocket[] {
    const result: WebSocket[] = [];
    for (const [ws, tags] of this.#sockets) {
      if (tag === undefined || tags.includes(tag)) {
        result.push(ws as unknown as WebSocket);
      }
    }
    return result;
  }

  getTags(ws: WebSocket | MockWebSocket): string[] {
    return [...(this.#sockets.get(ws as unknown as MockWebSocket) ?? [])];
  }

  /**
   * Fires the currently scheduled alarm against `target` (e.g. a fresh `Hub` instance after
   * simulated hibernation). Mirrors Cloudflare's runtime by clearing the stored alarm before
   * invoking `target.alarm()`. Returns false if no alarm was scheduled.
   */
  async fireAlarm(target: { alarm(): Promise<void> }): Promise<boolean> {
    if (this.storage.alarmTime === null) return false;
    this.storage.alarmTime = null;
    await target.alarm();
    return true;
  }
}

/**
 * Installs Workers-specific globals (`WebSocketPair`, `WebSocket.READY_STATE_OPEN`, 101 `Response`)
 * for driving `Hub` in Node Vitest, returning a restore function.
 */
export function installWorkersGlobals(): () => void {
  const g = globalThis as Record<string, unknown>;
  const origPair = g['WebSocketPair'];
  const origResponse = globalThis.Response;
  const origWebSocket = g['WebSocket'] as Record<string, unknown> | undefined;
  const origReadyStateOpen = origWebSocket?.['READY_STATE_OPEN'];

  class MockWebSocketPair {
    0: MockWebSocket;
    1: MockWebSocket;
    constructor() {
      const client = new MockWebSocket();
      const server = new MockWebSocket();
      client.peer = server;
      server.peer = client;
      this[0] = client;
      this[1] = server;
    }
  }

  class WorkersResponse extends origResponse {
    override readonly webSocket: WebSocket | null;
    constructor(body?: BodyInit | null, init?: ResponseInit & { webSocket?: WebSocket | null }) {
      if (init?.status === 101) {
        super(null, { status: 200 });
        Object.defineProperty(this, 'status', { value: 101, configurable: true });
        this.webSocket = init.webSocket ?? null;
      } else {
        super(body, init);
        this.webSocket = null;
      }
    }
  }

  g['WebSocketPair'] = MockWebSocketPair;
  g['Response'] = WorkersResponse;
  if (origWebSocket !== undefined) {
    origWebSocket['READY_STATE_OPEN'] = 1;
  } else {
    g['WebSocket'] = { READY_STATE_OPEN: 1 };
  }

  return () => {
    if (origPair === undefined) delete g['WebSocketPair'];
    else g['WebSocketPair'] = origPair;
    g['Response'] = origResponse;
    if (origWebSocket !== undefined) {
      if (origReadyStateOpen === undefined) delete origWebSocket['READY_STATE_OPEN'];
      else origWebSocket['READY_STATE_OPEN'] = origReadyStateOpen;
    } else {
      delete g['WebSocket'];
    }
  };
}
