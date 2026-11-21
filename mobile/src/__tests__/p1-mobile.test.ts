import { createElement } from 'react';
import { AddressInfo } from 'node:net';
import { AppState, Linking } from 'react-native';
import TestRenderer, { act, type ReactTestRenderer } from 'react-test-renderer';
import { WebSocket, WebSocketServer, type RawData } from 'ws';

import {
  DIRECTION_CLIENT_TO_HOST,
  DIRECTION_HOST_TO_CLIENT,
  RelayCipher,
  deriveSessionKey,
  fromBase64,
  generateHandshakeNonce,
  generateKeyPair,
  toBase64,
} from '@/api/relay/crypto';
import {
  RelayConnection,
  type NetworkChangeEvent,
  type RelaySocket,
} from '@/api/relay/transport';
import { HtmlView } from '@/components/html-view';
import { useLongPoll } from '@/hooks/use-long-poll';
import {
  CLIENT_ID_KEY,
  DEVICE_TOKEN_PREFIX,
  HOST_CLIENT_ID_PREFIX,
  HOST_INDEX_FILENAME,
  INDEX_KEY,
  PAIRING_KEY_PREFIX,
  adoptDeviceToken,
  ensureClientId,
  mintHostClientId,
  readHostIndex,
  writeHostIndex,
  writeHostSecrets,
  type HostIndexStorage,
  type SecretStore,
  type SecretStoreWriteOptions,
} from '@/lib/host-secrets';
import { resetLanguage } from '@/lib/i18n';
import { ensurePushHandle, PUSH_HANDLE_PREFIX, type PushFetch } from '@/lib/push/registry';
import { createPushSettings, PUSH_SETTINGS_KEY } from '@/lib/push/settings';
import { createWorkspaceMute, MUTED_PREFIX, type RouteFiles } from '@/lib/push/workspace-mute';
import { useHostsStore, type PairedHost } from '@/store/hosts';

const MOCK_AFTER_FIRST_UNLOCK = 1;
const mockSecureItems = new Map<string, string>();
const mockSecureSetCalls: Array<{
  key: string;
  value: string;
  options: SecretStoreWriteOptions | undefined;
}> = [];
const mockFiles = new Map<string, string>();
const mockNetworkListeners = new Set<(event: NetworkChangeEvent) => void>();
const appStateListeners = new Set<(state: string) => void>();

jest.mock('expo-secure-store', () => ({
  AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY: 1,
  getItemAsync: jest.fn(async (key: string) => mockSecureItems.get(key) ?? null),
  setItemAsync: jest.fn(
    async (key: string, value: string, options?: SecretStoreWriteOptions) => {
      mockSecureSetCalls.push({ key, value, options });
      mockSecureItems.set(key, value);
    },
  ),
  deleteItemAsync: jest.fn(async (key: string) => {
    mockSecureItems.delete(key);
  }),
}));

jest.mock('expo-file-system', () => {
  class MockFile {
    readonly uri: string;
    constructor(...parts: string[]) {
      this.uri = parts.join('/');
    }
    get exists(): boolean {
      return mockFiles.has(this.uri);
    }
    create(): void {
      mockFiles.set(this.uri, '');
    }
    write(text: string): void {
      mockFiles.set(this.uri, text);
    }
    async text(): Promise<string> {
      return mockFiles.get(this.uri) ?? '';
    }
    delete(): void {
      mockFiles.delete(this.uri);
    }
    moveSync(to: MockFile): void {
      mockFiles.set(to.uri, mockFiles.get(this.uri) ?? '');
      mockFiles.delete(this.uri);
    }
  }
  return { File: MockFile, Paths: { document: 'mock-doc' } };
});

jest.mock('expo-network', () => ({
  addNetworkStateListener: jest.fn((listener: (event: NetworkChangeEvent) => void) => {
    mockNetworkListeners.add(listener);
    return {
      remove: jest.fn(() => {
        mockNetworkListeners.delete(listener);
      }),
    };
  }),
}));

jest.mock('expo-router', () => {
  const React = require('react') as typeof import('react');
  return {
    useFocusEffect: (effect: () => void | (() => void)) => {
      React.useEffect(effect, [effect]);
    },
  };
});

jest.mock('react-native-webview', () => ({ WebView: 'WebView' }));

beforeAll(() => resetLanguage('ko'));
afterAll(() => resetLanguage());

beforeEach(() => {
  mockSecureItems.clear();
  mockSecureSetCalls.length = 0;
  mockFiles.clear();
  mockNetworkListeners.clear();
  appStateListeners.clear();
});

// ---------------------------------------------------------------------------
// 1. INDEX_KEY document-file storage & one-time migration from SecureStore
// ---------------------------------------------------------------------------

describe('host index file storage and SecureStore migration', () => {
  function memorySecretStore(initial: Record<string, string> = {}, accessible?: number) {
    const items = new Map<string, string>(Object.entries(initial));
    const writes: Array<{ key: string; value: string; options: SecretStoreWriteOptions | undefined }> = [];
    const deleted: string[] = [];
    const store: SecretStore = {
      ...(accessible !== undefined ? { AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY: accessible } : {}),
      getItemAsync: async (key) => items.get(key) ?? null,
      setItemAsync: async (key, value, options) => {
        writes.push({ key, value, options });
        items.set(key, value);
      },
      deleteItemAsync: async (key) => {
        deleted.push(key);
        items.delete(key);
      },
    };
    return { store, items, writes, deleted };
  }

  function memoryIndexFile(initial: string | null = null): HostIndexStorage & { content: string | null } {
    const state = { content: initial };
    return {
      get content() {
        return state.content;
      },
      set content(v: string | null) {
        state.content = v;
      },
      read: async () => state.content,
      write: async (text: string) => {
        state.content = text;
      },
    };
  }

  it('migrates legacy SecureStore INDEX_KEY to the document file once and deletes the legacy SecureStore entry', async () => {
    const legacyPayload = JSON.stringify([
      {
        id: 'mac-1',
        name: 'Mac Studio',
        serverId: 'a'.repeat(64),
        relayUrl: 'wss://relay.example.invalid',
        hostPublicKeyB64: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
        hostId: 'host-1',
        appVersion: '1.0.0',
        pairedAt: '2026-11-01T00:00:00.000Z',
      },
    ]);
    const { store, items, deleted } = memorySecretStore({ [INDEX_KEY]: legacyPayload });
    const file = memoryIndexFile();

    const firstRead = await readHostIndex(store, file);
    expect(firstRead).toBe(legacyPayload);
    expect(file.content).toBe(legacyPayload);
    expect(deleted).toEqual([INDEX_KEY]);
    expect(items.has(INDEX_KEY)).toBe(false);

    // Subsequent reads come straight from the document file.
    const secondRead = await readHostIndex(store, file);
    expect(secondRead).toBe(legacyPayload);
  });

  it('preserves the legacy SecureStore entry if writing the document file fails during migration', async () => {
    const legacyPayload = '[{"id":"mac-1"}]';
    const { store, items, deleted } = memorySecretStore({ [INDEX_KEY]: legacyPayload });
    const failingFile: HostIndexStorage = {
      read: async () => null,
      write: async () => {
        throw new Error('ENOSPC');
      },
    };

    const result = await readHostIndex(store, failingFile);
    expect(result).toBe(legacyPayload);
    expect(deleted).toEqual([]);
    expect(items.get(INDEX_KEY)).toBe(legacyPayload);
  });

  it('writes indexes larger than SecureStore 2048-byte limit to the document file without touching SecureStore', async () => {
    const manyHosts: PairedHost[] = Array.from({ length: 12 }, (_, idx) => ({
      id: `host-${idx}`,
      name: `Studio Workstation ${idx}`,
      serverId: String(idx).padStart(64, 'a'),
      relayUrl: 'wss://relay.example.invalid',
      hostPublicKeyB64: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
      hostId: `hid-${idx}`,
      appVersion: '1.1.0',
      pairedAt: '2026-11-21T00:00:00.000Z',
      directEndpoints: [`100.64.0.${idx + 1}:43139`, `[fd7a:115c:a1e0::${idx + 1}]:43139`],
    }));
    const serialized = JSON.stringify(manyHosts);
    expect(Buffer.byteLength(serialized, 'utf8')).toBeGreaterThan(2048);

    await writeHostIndex(serialized);
    expect(mockFiles.get(`mock-doc/${HOST_INDEX_FILENAME}`)).toBe(serialized);
    expect(mockSecureSetCalls.some((call) => call.key === INDEX_KEY)).toBe(false);

    // `useHostsStore.load` reads the full >2 KB index from the document file cleanly.
    useHostsStore.setState({ hosts: [], keys: {}, tokens: {}, clientIds: {}, status: {}, loaded: false });
    await useHostsStore.getState().load({ probe: false });
    expect(useHostsStore.getState().hosts).toHaveLength(12);
  });

  it('migrates legacy SecureStore INDEX_KEY end-to-end inside useHostsStore.load() and never writes INDEX_KEY back to SecureStore on addHost', async () => {
    const legacyHost: PairedHost = {
      id: 'legacy-mac',
      name: 'Legacy Mac',
      serverId: 'b'.repeat(64),
      relayUrl: 'wss://relay.example.invalid',
      hostPublicKeyB64: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
      hostId: 'legacy-host-id',
      appVersion: '1.0.0',
      pairedAt: '2026-10-01T00:00:00.000Z',
    };
    mockSecureItems.set(INDEX_KEY, JSON.stringify([legacyHost]));
    mockSecureItems.set(`${DEVICE_TOKEN_PREFIX}legacy-mac`, 'tok-legacy');

    useHostsStore.setState({ hosts: [], keys: {}, tokens: {}, clientIds: {}, status: {}, loaded: false });
    await useHostsStore.getState().load({ probe: false });

    expect(useHostsStore.getState().hosts).toEqual([legacyHost]);
    expect(mockSecureItems.has(INDEX_KEY)).toBe(false);
    expect(mockFiles.get(`mock-doc/${HOST_INDEX_FILENAME}`)).toBe(JSON.stringify([legacyHost]));

    mockSecureSetCalls.length = 0;
    await useHostsStore.getState().addHost(
      {
        name: 'New Mac',
        serverId: 'c'.repeat(64),
        relayUrl: 'wss://relay.example.invalid',
        hostPublicKeyB64: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
        pairingKey: 'pair-key-123',
      },
      { hostId: 'new-hid', appVersion: '1.1.0', deviceToken: 'tok-new' },
    );

    expect(mockSecureSetCalls.some((c) => c.key === INDEX_KEY)).toBe(false);
    const tokenWrite = mockSecureSetCalls.find((c) => c.key === `${DEVICE_TOKEN_PREFIX}${'c'.repeat(64)}`);
    expect(tokenWrite?.options).toEqual({ keychainAccessible: MOCK_AFTER_FIRST_UNLOCK });
    const persisted = JSON.parse(mockFiles.get(`mock-doc/${HOST_INDEX_FILENAME}`) ?? '[]') as PairedHost[];
    expect(persisted.map((h) => h.id)).toEqual(['legacy-mac', 'c'.repeat(64)]);
  });
});

// ---------------------------------------------------------------------------
// 2. Keychain accessibility: AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY on secret writes
// ---------------------------------------------------------------------------

describe('SecureStore keychainAccessible: AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY', () => {
  function recordingStore(accessible?: number) {
    const items = new Map<string, string>();
    const calls: Array<{ key: string; options: SecretStoreWriteOptions | undefined }> = [];
    const store: SecretStore = {
      ...(accessible !== undefined ? { AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY: accessible } : {}),
      getItemAsync: async (key) => items.get(key) ?? null,
      setItemAsync: async (key, value, options) => {
        calls.push({ key, options });
        items.set(key, value);
      },
      deleteItemAsync: async (key) => {
        items.delete(key);
      },
    };
    return { store, calls };
  }

  it('passes AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY across host-secrets, push registry, push settings, and workspace mute', async () => {
    const { store, calls } = recordingStore(MOCK_AFTER_FIRST_UNLOCK);

    await ensureClientId(store, () => 'cid-1');
    await mintHostClientId(store, 'mac-1', () => 'cid-host-1');
    await writeHostSecrets(store, 'mac-1', { pairingKey: 'pk-1' });
    await adoptDeviceToken(store, 'mac-1', 'dt-1');

    const handle = 'h'.repeat(43);
    const secret = 's'.repeat(43);
    const fakeFetch: PushFetch = async () => ({
      status: 201,
      json: async () => ({ handle, secret }),
    });
    await ensurePushHandle({ store, fetch: fakeFetch, now: () => 1_000 }, 'mac-1', 'wss://relay.example', 'fcm-tok');

    const settings = createPushSettings(store);
    await settings.ready;
    await settings.update((prev) => ({ ...prev, crashReports: false }));

    const files: RouteFiles = {
      read: async () => null,
      write: async () => undefined,
      remove: async () => undefined,
    };
    const mute = createWorkspaceMute({ store, files });
    await mute.setMuted('mac-1', 'ws-1', true);

    const expectedKeys = [
      CLIENT_ID_KEY,
      `${HOST_CLIENT_ID_PREFIX}mac-1`,
      `${PAIRING_KEY_PREFIX}mac-1`,
      `${DEVICE_TOKEN_PREFIX}mac-1`,
      `${PUSH_HANDLE_PREFIX}mac-1`,
      PUSH_SETTINGS_KEY,
      `${MUTED_PREFIX}mac-1`,
    ];
    expect(calls.map((c) => c.key)).toEqual(expectedKeys);
    for (const call of calls) {
      expect(call.options).toEqual({ keychainAccessible: MOCK_AFTER_FIRST_UNLOCK });
    }
  });

  it('omits options cleanly when AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY is unavailable on a mocked store', async () => {
    const { store, calls } = recordingStore(undefined);
    await writeHostSecrets(store, 'mac-unmocked', { deviceToken: 'tok' });
    expect(calls).toEqual([{ key: `${DEVICE_TOKEN_PREFIX}mac-unmocked`, options: undefined }]);
  });
});

// ---------------------------------------------------------------------------
// 3. HtmlView security hardening: javaScriptEnabled={false}
// ---------------------------------------------------------------------------

describe('HtmlView security hardening', () => {
  let mounted: ReactTestRenderer | undefined;

  afterEach(() => {
    act(() => mounted?.unmount());
    mounted = undefined;
  });

  it('disables JavaScript on WebView and blocks external/unsafe navigations cleanly', () => {
    const openUrlSpy = jest.spyOn(Linking, 'openURL').mockResolvedValue(true as never);
    try {
      act(() => {
        mounted = TestRenderer.create(createElement(HtmlView, { html: '<script>alert(1)</script><p>hi</p>' }));
      });
      const webView = mounted!.root.find((node) => (node.type as unknown) === 'WebView');
      expect(webView.props.javaScriptEnabled).toBe(false);
      expect(webView.props.allowFileAccess).toBe(false);
      expect(webView.props.allowFileAccessFromFileURLs).toBe(false);
      expect(webView.props.allowUniversalAccessFromFileURLs).toBe(false);

      const allow = webView.props.onShouldStartLoadWithRequest as (req: {
        url: string;
        isTopFrame?: boolean;
      }) => boolean;
      expect(allow({ url: 'about:blank', isTopFrame: true })).toBe(true);
      expect(allow({ url: 'about:blank#section', isTopFrame: true })).toBe(true);
      expect(allow({ url: 'javascript:alert(1)', isTopFrame: true })).toBe(false);
      expect(allow({ url: 'file:///etc/passwd', isTopFrame: true })).toBe(false);
      expect(allow({ url: 'https://example.com/subframe', isTopFrame: false })).toBe(false);
      expect(openUrlSpy).not.toHaveBeenCalled();

      // Top-frame http(s) links open externally in browser and are blocked inside the WebView.
      expect(allow({ url: 'https://example.com/docs', isTopFrame: true })).toBe(false);
      expect(openUrlSpy).toHaveBeenCalledWith('https://example.com/docs');
    } finally {
      openUrlSpy.mockRestore();
    }
  });
});

// ---------------------------------------------------------------------------
// 4. AppState background pause in useLongPoll
// ---------------------------------------------------------------------------

describe('useLongPoll AppState background pause and resume', () => {
  let mounted: ReactTestRenderer | undefined;
  let origCurrentState: string;
  let addListenerSpy: jest.SpyInstance;

  beforeEach(() => {
    origCurrentState = AppState.currentState;
    Object.defineProperty(AppState, 'currentState', { value: 'active', configurable: true, writable: true });
    addListenerSpy = jest.spyOn(AppState, 'addEventListener').mockImplementation((_type, handler) => {
      const fn = handler as (state: string) => void;
      appStateListeners.add(fn);
      return {
        remove: () => {
          appStateListeners.delete(fn);
        },
      } as ReturnType<typeof AppState.addEventListener>;
    });
  });

  afterEach(() => {
    act(() => mounted?.unmount());
    mounted = undefined;
    addListenerSpy.mockRestore();
    Object.defineProperty(AppState, 'currentState', {
      value: origCurrentState,
      configurable: true,
      writable: true,
    });
  });

  function emitAppState(next: 'active' | 'background' | 'inactive') {
    Object.defineProperty(AppState, 'currentState', { value: next, configurable: true, writable: true });
    for (const listener of [...appStateListeners]) listener(next);
  }

  it('aborts in-flight long-poll when moving to background and resumes immediately on return to active', async () => {
    const signals: AbortSignal[] = [];
    const sinces: Array<number | undefined> = [];
    let resolvePending: ((value: { revision: number }) => void) | undefined;

    const fetchPage = jest.fn(
      (since: number | undefined, signal: AbortSignal) =>
        new Promise<{ revision: number }>((resolve, reject) => {
          sinces.push(since);
          signals.push(signal);
          resolvePending = resolve;
          signal.addEventListener('abort', () => {
            const err = new Error('aborted');
            err.name = 'AbortError';
            reject(err);
          });
        }),
    );

    function Harness() {
      useLongPoll({
        enabled: true,
        fetchPage,
        revisionOf: (d) => d.revision,
        onData: () => undefined,
      });
      return null;
    }

    await act(async () => {
      mounted = TestRenderer.create(createElement(Harness));
    });
    expect(fetchPage).toHaveBeenCalledTimes(1);
    expect(sinces[0]).toBeUndefined();
    expect(signals[0]?.aborted).toBe(false);

    // Move to background: in-flight request is aborted and no follow-up poll starts.
    await act(async () => {
      emitAppState('background');
    });
    expect(signals[0]?.aborted).toBe(true);
    expect(fetchPage).toHaveBeenCalledTimes(1);

    // Return to active: immediately resumes with a fresh full read (`since === undefined`).
    await act(async () => {
      emitAppState('active');
    });
    expect(fetchPage).toHaveBeenCalledTimes(2);
    expect(sinces[1]).toBeUndefined();
    expect(signals[1]?.aborted).toBe(false);

    // Resolve the second poll cleanly before unmount.
    await act(async () => {
      resolvePending?.({ revision: 5 });
      await Promise.resolve();
    });
  });
});

// ---------------------------------------------------------------------------
// 5. RelayConnection expo-network recovery & retrying getter
// ---------------------------------------------------------------------------

describe('RelayConnection expo-network recovery', () => {
  jest.setTimeout(15_000);

  const connections: RelayConnection[] = [];
  const servers: Array<() => Promise<void>> = [];

  afterEach(async () => {
    for (const conn of connections.splice(0)) conn.close();
    for (const stop of servers.splice(0)) await stop();
  });

  async function startFakeRelay() {
    const hostKeys = generateKeyPair();
    const server = new WebSocketServer({ port: 0 });
    let connects = 0;
    let ignorePings = false;
    let rejectWithCode: number | undefined;
    const pings: number[] = [];

    server.on('connection', (socket) => {
      connects += 1;
      if (rejectWithCode !== undefined) {
        socket.close(rejectWithCode, 'offline');
        return;
      }
      let cipher: RelayCipher | undefined;
      socket.on('message', (raw: RawData, isBinary: boolean) => {
        if (!isBinary) {
          const hello = JSON.parse(raw.toString()) as { clientKey: string; nonce: string };
          const serverNonce = generateHandshakeNonce();
          cipher = new RelayCipher(
            deriveSessionKey({
              secretKey: hostKeys.secretKey,
              peerPublicKey: fromBase64(hello.clientKey),
              clientNonce: fromBase64(hello.nonce),
              serverNonce,
            }),
            DIRECTION_HOST_TO_CLIENT,
            DIRECTION_CLIENT_TO_HOST,
          );
          socket.send(
            JSON.stringify({
              type: 'ready',
              v: 1,
              serverKey: toBase64(hostKeys.publicKey),
              nonce: toBase64(serverNonce),
            }),
          );
          return;
        }
        if (!cipher) return;
        const msg = cipher.openJson(new Uint8Array(raw as Buffer)) as { type?: string };
        if (msg.type === 'auth') {
          socket.send(
            cipher.sealJson({
              type: 'auth_ok',
              hostName: 'Studio',
              hostId: 'host-1',
              appVersion: '1.1.0',
            }),
          );
        } else if (msg.type === 'ping') {
          pings.push(Date.now());
          if (!ignorePings) socket.send(cipher.sealJson({ type: 'pong' }));
        }
      });
    });

    await new Promise<void>((resolve) => server.once('listening', resolve));
    const { port } = server.address() as AddressInfo;
    servers.push(
      () =>
        new Promise<void>((resolve) => {
          for (const ws of server.clients) ws.terminate();
          server.close(() => resolve());
        }),
    );

    return {
      url: `ws://127.0.0.1:${port}`,
      hostPublicKeyB64: toBase64(hostKeys.publicKey),
      get connects() {
        return connects;
      },
      pings,
      setIgnorePings(value: boolean) {
        ignorePings = value;
      },
      setRejectWithCode(code: number | undefined) {
        rejectWithCode = code;
      },
    };
  }

  function nodeSocketFactory(url: string): RelaySocket {
    return new WebSocket(url) as unknown as RelaySocket;
  }

  async function waitFor(predicate: () => boolean, timeoutMs = 4_000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (!predicate()) {
      if (Date.now() > deadline) throw new Error('condition not met in time');
      await new Promise((resolve) => setTimeout(resolve, 15));
    }
  }

  it('probes and redials a dead tunnel promptly on expo-network transition while foregrounded', async () => {
    const fake = await startFakeRelay();
    let foreground = true;

    const conn = new RelayConnection(
      {
        serverId: 'srv-net-1',
        relayUrl: fake.url,
        hostPublicKeyB64: fake.hostPublicKeyB64,
        pairingKey: 'pk-1',
      },
      {
        createSocket: nodeSocketFactory,
        autoReconnect: true,
        probeTimeoutMs: 120,
        isForeground: () => foreground,
      },
    );
    connections.push(conn);
    await conn.ready();
    expect(conn.retrying).toBe(true);
    expect(mockNetworkListeners.size).toBe(1);

    // Freeze the old path so pings go unanswered, then simulate Wi-Fi -> Cellular transition.
    fake.setIgnorePings(true);
    // While backgrounded, a network transition does NOT probe.
    foreground = false;
    for (const listener of mockNetworkListeners) {
      listener({ type: 'WIFI', isConnected: true, isInternetReachable: true });
    }
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(fake.pings).toHaveLength(0);

    // Foregrounded transition WIFI -> CELLULAR triggers immediate probe, drops dead socket in 120ms, and redials.
    foreground = true;
    const started = Date.now();
    for (const listener of mockNetworkListeners) {
      listener({ type: 'CELLULAR', isConnected: true, isInternetReachable: true });
    }
    await waitFor(() => fake.connects === 2 && conn.state === 'ready');
    expect(fake.pings.length).toBeGreaterThanOrEqual(1);
    expect(Date.now() - started).toBeLessThan(2_000);

    conn.close();
    expect(conn.retrying).toBe(false);
    expect(mockNetworkListeners.size).toBe(0);
  });

  it('cancels reconnect backoff immediately when expo-network reports connectivity restored', async () => {
    const fake = await startFakeRelay();
    fake.setRejectWithCode(4404);

    const conn = new RelayConnection(
      {
        serverId: 'srv-net-2',
        relayUrl: fake.url,
        hostPublicKeyB64: fake.hostPublicKeyB64,
        pairingKey: 'pk-1',
      },
      {
        createSocket: nodeSocketFactory,
        autoReconnect: true,
        isForeground: () => true,
      },
    );
    connections.push(conn);

    // First attempt closes 4404 -> enters backoff (1.5s+), while `retrying` stays true.
    await waitFor(() => conn.state === 'closed' && fake.connects === 1);
    expect(conn.retrying).toBe(true);

    // Host comes back and network transitions NONE -> WIFI: reconnects right away without waiting 1.5s.
    fake.setRejectWithCode(undefined);
    const started = Date.now();
    for (const listener of mockNetworkListeners) {
      listener({ type: 'NONE', isConnected: false, isInternetReachable: false });
    }
    expect(fake.connects).toBe(1);
    for (const listener of mockNetworkListeners) {
      listener({ type: 'WIFI', isConnected: true, isInternetReachable: true });
    }

    await waitFor(() => conn.state === 'ready' && fake.connects === 2);
    expect(Date.now() - started).toBeLessThan(1_000);
  });
});
