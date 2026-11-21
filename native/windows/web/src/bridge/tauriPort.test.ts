import { afterEach, describe, expect, it, vi } from 'vitest';
import { Bridge } from './bridge';
import { HOST_MESSAGE, HOST_POST, tauriPort, type TauriApi } from './tauriPort';

/** A Tauri stand-in: `listen` resolves when the test says so; `invoke` records each post and can be told to fail. */
function fakeTauri() {
  const invoked: unknown[] = [];
  let handler: ((event: { payload: unknown }) => void) | null = null;
  let registered: () => void = () => {};
  let refused: (error: Error) => void = () => {};
  let failing = false;
  const api: TauriApi = {
    invoke: (command, args) => {
      expect(command).toBe(HOST_POST);
      expect(args.nonce).toBe('run-nonce');
      if (failing) return Promise.reject(new Error('ipc down'));
      invoked.push(args.message);
      return Promise.resolve(null);
    },
    listen: (event, listener) => {
      expect(event).toBe(HOST_MESSAGE);
      handler = listener;
      return new Promise((resolve, reject) => {
        registered = () => resolve(() => {});
        refused = reject;
      });
    },
  };
  return {
    api,
    invoked,
    register: () => registered(),
    refuse: (error: Error) => refused(error),
    fail: () => (failing = true),
    recover: () => (failing = false),
    emit: (payload: unknown) => handler?.({ payload }),
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('tauriPort', () => {
  it('holds every post until the listener is registered, then sends them in order', async () => {
    const tauri = fakeTauri();
    const port = tauriPort(tauri.api, () => 'run-nonce');
    port.postMessage({ kind: 'call', id: 1, method: 'getLocale' });
    port.postMessage({ kind: 'call', id: 2, method: 'ready' });
    await settle();
    expect(tauri.invoked).toEqual([]);
    tauri.register();
    await settle();
    expect(tauri.invoked).toEqual([
      { kind: 'call', id: 1, method: 'getLocale' },
      { kind: 'call', id: 2, method: 'ready' },
    ]);
    port.postMessage({ kind: 'call', id: 3, method: 'shellState' });
    await settle();
    expect(tauri.invoked).toHaveLength(3);
  });

  it('hands the host-message payloads to the page as message events', async () => {
    const tauri = fakeTauri();
    const port = tauriPort(tauri.api, () => 'run-nonce');
    const seen: unknown[] = [];
    port.addEventListener('message', (event) => seen.push(event.data));
    tauri.register();
    await settle();
    tauri.emit({ kind: 'snapshot', snapshot: { version: 1 } });
    tauri.emit({ kind: 'result', id: 2, ok: true });
    expect(seen).toEqual([{ kind: 'snapshot', snapshot: { version: 1 } }, { kind: 'result', id: 2, ok: true }]);
  });

  it('carries a Bridge call to its result, the push before it first', async () => {
    const tauri = fakeTauri();
    const bridge = new Bridge(tauriPort(tauri.api, () => 'run-nonce'));
    const pushes: unknown[] = [];
    bridge.onPush((message) => pushes.push(message.kind));
    const ready = bridge.call('ready');
    tauri.register();
    await settle();
    expect(tauri.invoked).toEqual([{ kind: 'call', id: 1, method: 'ready' }]);
    tauri.emit({ kind: 'snapshot', snapshot: {} });
    tauri.emit({ kind: 'result', id: 1, ok: true });
    await expect(ready).resolves.toBeUndefined();
    expect(pushes).toEqual(['snapshot']);
  });

  it('fails a call it cannot deliver instead of leaving it waiting', async () => {
    const tauri = fakeTauri();
    const bridge = new Bridge(tauriPort(tauri.api, () => 'run-nonce'));
    tauri.register();
    await settle();
    tauri.fail();
    await expect(bridge.call('getLocale')).rejects.toThrow(/host unreachable: Error: ipc down/);
  });

  it('keeps posting after a listener throws on a failed result', async () => {
    const tauri = fakeTauri();
    const port = tauriPort(tauri.api, () => 'run-nonce');
    const seen: unknown[] = [];
    let throwing = true;
    port.addEventListener('message', (event) => {
      seen.push(event.data);
      if (throwing) throw new Error('listener failed');
    });
    tauri.register();
    await settle();
    tauri.fail();
    port.postMessage({ kind: 'call', id: 1, method: 'getLocale' });
    await settle();
    expect(seen).toEqual([{ kind: 'result', id: 1, ok: false, error: 'host unreachable: Error: ipc down' }]);
    throwing = false;
    tauri.recover();
    port.postMessage({ kind: 'call', id: 2, method: 'ready' });
    await settle();
    expect(tauri.invoked).toEqual([{ kind: 'call', id: 2, method: 'ready' }]);
  });

  it('fails every call when the listener cannot be registered', async () => {
    const tauri = fakeTauri();
    const bridge = new Bridge(tauriPort(tauri.api, () => 'run-nonce'));
    const first = bridge.call('getLocale');
    tauri.refuse(new Error('no event permission'));
    await expect(first).rejects.toThrow(/no event permission/);
    await expect(bridge.call('ready')).rejects.toThrow(/no event permission/);
    expect(tauri.invoked).toEqual([]);
  });
});

describe('hostNonce', () => {
  const holder = globalThis as { __MIGHTY_HOST_NONCE__?: unknown };

  afterEach(() => {
    try {
      delete holder.__MIGHTY_HOST_NONCE__;
    } catch {
      // Non-configurable property in the final test case stays read-only in this isolated worker.
    }
    vi.resetModules();
  });

  it('captures __MIGHTY_HOST_NONCE__ on module load, deletes it from globalThis, and caches it', async () => {
    holder.__MIGHTY_HOST_NONCE__ = 'boot-nonce';
    vi.resetModules();
    const fresh = await import('./tauriPort');
    expect('__MIGHTY_HOST_NONCE__' in holder).toBe(false);
    expect(fresh.hostNonce()).toBe('boot-nonce');
    holder.__MIGHTY_HOST_NONCE__ = 'spoofed-nonce';
    expect(fresh.hostNonce()).toBe('boot-nonce');
  });

  it('captures and deletes __MIGHTY_HOST_NONCE__ on first access when injected after load', async () => {
    delete holder.__MIGHTY_HOST_NONCE__;
    vi.resetModules();
    const fresh = await import('./tauriPort');
    expect(fresh.hostNonce()).toBeUndefined();
    holder.__MIGHTY_HOST_NONCE__ = 'late-nonce';
    expect(fresh.hostNonce()).toBe('late-nonce');
    expect('__MIGHTY_HOST_NONCE__' in holder).toBe(false);
    holder.__MIGHTY_HOST_NONCE__ = 'other-nonce';
    expect(fresh.hostNonce()).toBe('late-nonce');
  });

  it('handles a non-configurable __MIGHTY_HOST_NONCE__ without throwing', async () => {
    delete holder.__MIGHTY_HOST_NONCE__;
    Object.defineProperty(globalThis, '__MIGHTY_HOST_NONCE__', {
      value: 'locked-nonce',
      writable: false,
      configurable: false,
    });
    vi.resetModules();
    const fresh = await import('./tauriPort');
    expect(fresh.hostNonce()).toBe('locked-nonce');
  });
});
