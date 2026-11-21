import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

jest.mock('@react-native-firebase/messaging', () => ({
  getMessaging: jest.fn(() => ({ name: 'messaging' })),
  getToken: jest.fn(async () => 'fcm-token-1'),
  onMessage: jest.fn(() => () => undefined),
  onTokenRefresh: jest.fn(() => () => undefined),
  setAutoInitEnabled: jest.fn(async () => undefined),
}));
jest.mock('react-native-notify-kit', () => ({
  __esModule: true,
  default: {
    createChannel: jest.fn(async () => 'channel'),
    displayNotification: jest.fn(async () => 'shown'),
    requestPermission: jest.fn(async () => ({ authorizationStatus: 1 })),
  },
  AndroidImportance: { DEFAULT: 3, HIGH: 4 },
  AndroidVisibility: { PRIVATE: 0, PUBLIC: 1, SECRET: -1 },
  EventType: { DISMISSED: 0, PRESS: 1, ACTION_PRESS: 2 },
}));
jest.mock('react-native', () => ({
  Platform: { OS: 'android', Version: 34 },
  PermissionsAndroid: { request: jest.fn(async () => 'granted'), PERMISSIONS: { POST_NOTIFICATIONS: 'post' } },
  AppState: { currentState: 'active', addEventListener: jest.fn(() => ({ remove: jest.fn() })) },
}));
jest.mock('expo-secure-store', () => {
  const items = new Map<string, string>();
  return {
    items,
    getItemAsync: jest.fn(async (key: string) => items.get(key) ?? null),
    setItemAsync: jest.fn(async (key: string, value: string) => void items.set(key, value)),
    deleteItemAsync: jest.fn(async (key: string) => void items.delete(key)),
  };
});
jest.mock('expo-file-system', () => {
  const files = new Map<string, string>();
  class File {
    private readonly uri: string;
    constructor(...parts: string[]) {
      this.uri = parts.join('/');
    }
    get exists(): boolean {
      return files.has(this.uri);
    }
    create(): void {
      files.set(this.uri, '');
    }
    write(text: string): void {
      files.set(this.uri, text);
    }
    async text(): Promise<string> {
      return files.get(this.uri) ?? '';
    }
    delete(): void {
      files.delete(this.uri);
    }
    moveSync(to: File): void {
      files.set(to.uri, files.get(this.uri) ?? '');
      files.delete(this.uri);
    }
  }
  return { files, File, Paths: { document: 'document' } };
});
jest.mock('@/lib/push/device-lock', () => ({ isDeviceLocked: jest.fn(() => false) }));
jest.mock('@/lib/push/host-lease', () => {
  const fake = {
    state: { hosts: [] as Array<{ id: string; relayUrl: string }>, tokens: {} as Record<string, string>, clientIds: {}, clientId: '' },
    clients: {} as Record<string, unknown>,
  };
  return {
    fake,
    loadedHosts: jest.fn(async () => fake.state),
    withHostClient: jest.fn(async (id: string, use: (client: unknown, signal: AbortSignal) => Promise<unknown>) => {
      const client = fake.clients[id];
      if (!client) throw new Error('unknown host');
      return use(client, new AbortController().signal);
    }),
  };
});

import { chacha20poly1305 } from '@noble/ciphers/chacha';
import { toBase64Url, utf8Encode } from '@/api/relay/crypto';
import type { MobileState } from '@/api/types';
import { PUSH_HANDLE_PREFIX } from '@/lib/push/registry';
import { PUSH_INFO, PUSH_VERSION, pushKey } from '@/lib/push/seal';

type Runtime = typeof import('@/lib/push/runtime');

const { seal } = JSON.parse(readFileSync(resolve(__dirname, '../../../native/contracts/fixtures/push.json'), 'utf8')) as {
  seal: {
    deviceToken: string;
    clientId: string;
    handle: string;
    plaintext: string;
    envelope: string;
    tampered: string[];
    otherHandle: string;
  };
};
// A plain stub before any module reset, so expo's lazy `fetch` global is never built here.
(globalThis as { fetch?: unknown }).fetch = jest.fn();
const SECRET = 's'.repeat(43);
const RELAY = 'wss://relay.example';

function sealFixtureMessage(
  overrides: Record<string, unknown> = {},
  options: { handle?: string; nonceByte?: number } = {},
): string {
  const handle = options.handle ?? seal.handle;
  const nonce = new Uint8Array(12).fill(options.nonceByte ?? 0x42);
  const key = pushKey(seal.deviceToken, seal.clientId);
  const baseMessage = JSON.parse(seal.plaintext) as Record<string, unknown>;
  const plaintext = utf8Encode(JSON.stringify({ ...baseMessage, ...overrides }));
  const ciphertext = chacha20poly1305(key, nonce, utf8Encode(`${PUSH_INFO}:${handle}`)).encrypt(plaintext);
  const out = new Uint8Array(1 + nonce.length + ciphertext.length);
  out[0] = PUSH_VERSION;
  out.set(nonce, 1);
  out.set(ciphertext, 1 + nonce.length);
  return toBase64Url(out);
}

function mocks() {
  return {
    notifee: (
      jest.requireMock('react-native-notify-kit') as {
        default: {
          displayNotification: jest.Mock;
          createChannel: jest.Mock;
          requestPermission: jest.Mock;
        };
      }
    ).default,
    messaging: jest.requireMock('@react-native-firebase/messaging') as Record<string, jest.Mock>,
    store: jest.requireMock('expo-secure-store') as { items: Map<string, string> },
    lease: (jest.requireMock('@/lib/push/host-lease') as { fake: { state: Record<string, unknown> & { hosts: unknown[]; tokens: Record<string, string> }; clients: Record<string, unknown> } }).fake,
    lock: (jest.requireMock('@/lib/push/device-lock') as { isDeviceLocked: jest.Mock }).isDeviceLocked,
    permissions: (jest.requireMock('react-native') as { PermissionsAndroid: { request: jest.Mock } }).PermissionsAndroid.request,
    platform: (jest.requireMock('react-native') as { Platform: { OS: string; Version: number | string } }).Platform,
  };
}

/** Each test gets fresh module state (token, opener, retries) and a fresh keychain. */
function load(os: 'android' | 'ios' = 'android'): { runtime: Runtime; m: ReturnType<typeof mocks> } {
  jest.resetModules();
  const m = mocks();
  m.platform.OS = os;
  m.platform.Version = os === 'android' ? 34 : '18.0';
  m.store.items.clear();
  for (const fn of [
    m.notifee.displayNotification,
    m.notifee.createChannel,
    m.notifee.requestPermission,
    ...Object.values(m.messaging),
    m.permissions,
  ]) {
    fn.mockClear();
  }
  m.lock.mockReturnValue(false);
  m.lease.state.hosts = [];
  m.lease.state.tokens = {};
  m.lease.state['clientId'] = seal.clientId;
  m.lease.clients = {};
  (require('@/lib/i18n') as typeof import('@/lib/i18n')).resetLanguage('ko');
  return { runtime: require('@/lib/push/runtime') as Runtime, m };
}

/** The locale text from the registry the runtime under test was loaded from. */
const t = (key: string): string => (require('@/lib/i18n') as typeof import('@/lib/i18n')).t(key);


/** Pairing `id` holds the fixture's handle when `withHandle`, and the fixture's device token. */
function pair(m: ReturnType<typeof mocks>, id: string, withHandle: boolean, handle = seal.handle) {
  m.lease.state.hosts.push({ id, relayUrl: RELAY });
  m.lease.state.tokens[id] = seal.deviceToken;
  if (withHandle) {
    m.store.items.set(PUSH_HANDLE_PREFIX + id, JSON.stringify({ handle, secret: SECRET, relayUrl: RELAY, fcmToken: 'fcm-token-1' }));
  }
}

describe('showPush', () => {
  it('shows the push on the pairing whose handle it came through, hidden on a secure lock screen', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-a', false);
    pair(m, 'pair-b', true);
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: seal.envelope } });
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(1);
    const shown = m.notifee.displayNotification.mock.calls[0][0];
    // The sealed message says hostId "host-1"; the pairing comes from the handle alone.
    expect(shown.data).toEqual({ storageId: 'pair-b', sessionId: 'pane-1', kind: 'permission', requestId: 'req-1', runId: 'run-1', buttons: 'claude' });
    expect(shown.android.visibility).toBe(0);
    expect(shown.android.actions.map((action: { pressAction: { id: string } }) => action.pressAction.id)).toEqual(['allow', 'deny']);
    expect(m.notifee.createChannel).toHaveBeenCalledWith(expect.objectContaining({ visibility: 0 }));
  });

  it('drops a push for no pairing, of another version, tampered with, or of a kind switched off', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-b', true);
    await runtime.showPush({ data: { v: '1', h: seal.otherHandle, p: seal.envelope } });
    await runtime.showPush({ data: { v: '2', h: seal.handle, p: seal.envelope } });
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: seal.tampered[0] } });
    await runtime.showPush({ data: { v: '1', h: seal.handle } });
    await runtime.showPush({});
    const { pushSettings } = require('@/lib/push/settings') as typeof import('@/lib/push/settings');
    await pushSettings.update((now) => ({ ...now, kinds: { ...now.kinds, permission: false } }));
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: seal.envelope } });
    expect(m.notifee.displayNotification).not.toHaveBeenCalled();
  });

  it('drops a push from a workspace muted on this phone, placed by its session or its one name', async () => {
    const { runtime, m } = load();
    (jest.requireMock('expo-file-system') as { files: Map<string, string> }).files.clear();
    const { withHostClient } = jest.requireMock('@/lib/push/host-lease') as { withHostClient: jest.Mock };
    withHostClient.mockClear();
    pair(m, 'pair-b', true);
    const { workspaceMute } = require('@/lib/push/workspace-mute') as typeof import('@/lib/push/workspace-mute');
    const workspaces = [
      { id: 'ws-demo', name: 'demo', path: '/demo' },
      { id: 'ws-api', name: 'api', path: '/api' },
    ];
    const pane = (workspaceId: string) => ({ id: 'pane-1', workspaceId }) as MobileState['sessions'][number];
    const pushFor = (id: string) => ({
      data: { v: '1', h: seal.handle, p: sealFixtureMessage({ id }) },
    });

    // The fixture's push is pane-1 in "demo"; the phone last saw pane-1 in "api".
    await workspaceMute.learn('pair-b', { protocol: 1, revision: 1, hostName: 'Mac', workspaces, sessions: [pane('ws-api')] });
    await workspaceMute.setMuted('pair-b', 'ws-demo', true);
    await runtime.showPush(pushFor('request:run-1:req-1'));
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(1);

    await workspaceMute.setMuted('pair-b', 'ws-api', true);
    await runtime.showPush(pushFor('request:run-1:req-2'));
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(1);

    // The pane is gone from the host's list: the name "demo" places it.
    await workspaceMute.learn('pair-b', { protocol: 1, revision: 2, hostName: 'Mac', workspaces, sessions: [] });
    await runtime.showPush(pushFor('request:run-1:req-3'));
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(1);

    await workspaceMute.setMuted('pair-b', 'ws-demo', false);
    await runtime.showPush(pushFor('request:run-1:req-4'));
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(2);
    // Muting is the phone's alone: no host was called.
    expect(withHostClient).not.toHaveBeenCalled();
  });

  it('deduplicates replayed or duplicate FCM deliveries with the same message.id per pairing', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-b', true);
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: seal.envelope } });
    // Exact duplicate FCM delivery and re-encrypted envelope with the same message.id are both dropped.
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: seal.envelope } });
    await runtime.showPush({
      data: { v: '1', h: seal.handle, p: sealFixtureMessage({}, { nonceByte: 0x99 }) },
    });
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(1);

    // Another pairing with its own handle is not blocked by pair-b's message.id namespace.
    pair(m, 'pair-c', true, seal.otherHandle);
    await runtime.showPush({
      data: { v: '1', h: seal.otherHandle, p: sealFixtureMessage({}, { handle: seal.otherHandle }) },
    });
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(2);
  });

  it('enforces 24h max age and 10m future skew when nowMs is supplied', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-b', true);
    const nowMs = Date.parse('2026-11-21T12:00:00Z');

    // Older than 24h -> dropped without burning the message.id.
    const stale = sealFixtureMessage({ id: 'req:stale', at: '2026-11-20T11:59:59Z' });
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: stale } }, { nowMs });
    expect(m.notifee.displayNotification).not.toHaveBeenCalled();

    // More than 10m in the future -> dropped.
    const future = sealFixtureMessage({ id: 'req:future', at: '2026-11-21T12:10:01Z' });
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: future } }, { nowMs });
    expect(m.notifee.displayNotification).not.toHaveBeenCalled();

    // Fresh timestamp within window -> shown once.
    const fresh = sealFixtureMessage({ id: 'req:fresh', at: '2026-11-21T11:55:00Z' });
    await runtime.showPush({ data: { v: '1', h: seal.handle, p: fresh } }, { nowMs });
    expect(m.notifee.displayNotification).toHaveBeenCalledTimes(1);
  });

  it('shows host text as written on Android and keeps plain text without HTML tags on iOS', () => {
    const android = load('android').runtime;
    expect(android.plainText('fix <Row> & <b>x</b>')).toBe('fix &lt;Row&gt; &amp; &lt;b&gt;x&lt;/b&gt;');
    expect(android.plainBody('first <a>\nsecond\r\nthird')).toBe('first &lt;a&gt;<br>second<br>third');

    const ios = load('ios').runtime;
    expect(ios.plainText('fix <Row> & <b>x</b>')).toBe('fix <Row> & <b>x</b>');
    expect(ios.plainBody('first <a> & b\nsecond\r\nthird')).toBe('first <a> & b\nsecond\nthird');
  });
});

const permissionNotification = {
  id: 'n-1',
  title: 'title',
  body: 'body',
  data: { storageId: 'pair-b', sessionId: 'pane-1', kind: 'permission', requestId: 'req-1', runId: 'run-1', buttons: 'claude' },
  android: { channelId: 'attention', actions: [{ title: 'allow', pressAction: { id: 'allow' } }] },
};

describe('handleNotificationEvent', () => {
  it('opens the session on a tap, once, and later when the app is not up yet', async () => {
    const { runtime } = load();
    const open = jest.fn();
    await runtime.handleNotificationEvent({ type: 1, detail: { notification: permissionNotification } } as never, open);
    expect(open).toHaveBeenCalledWith('/host/pair-b/session/pane-1');
    // The same tap again as the app's initial notification.
    runtime.openNotification(permissionNotification as never, open);
    expect(open).toHaveBeenCalledTimes(1);

    const later = jest.fn();
    await runtime.handleNotificationEvent({ type: 1, detail: { notification: { ...permissionNotification, id: 'n-2' } } } as never);
    expect(later).not.toHaveBeenCalled();
    runtime.setPushOpener(later);
    expect(later).toHaveBeenCalledWith('/host/pair-b/session/pane-1');
  });

  it('answers a button on the notification pairing and says so without buttons', async () => {
    const { runtime, m } = load();
    const respondPermission = jest.fn(async () => ({ ok: true }));
    m.lease.clients['pair-b'] = { respondPermission };
    await runtime.handleNotificationEvent({ type: 2, detail: { notification: permissionNotification, pressAction: { id: 'allow' } } } as never);
    expect(respondPermission).toHaveBeenCalledWith('pane-1', { requestId: 'req-1', runId: 'run-1', allow: true }, expect.any(AbortSignal));
    const update = m.notifee.displayNotification.mock.calls[0][0];
    expect(update.id).toBe('n-1');
    expect(update.body).toBe(t('phone.push.outcome.allowed'));
    expect(update.android).toEqual(expect.objectContaining({ actions: [], onlyAlertOnce: true, visibility: 0 }));
  });

  it('sends nothing from a locked phone and keeps the buttons', async () => {
    const { runtime, m } = load();
    const respondPermission = jest.fn();
    m.lease.clients['pair-b'] = { respondPermission };
    m.lock.mockReturnValue(true);
    await runtime.handleNotificationEvent({ type: 2, detail: { notification: permissionNotification, pressAction: { id: 'allow' } } } as never);
    expect(respondPermission).not.toHaveBeenCalled();
    const update = m.notifee.displayNotification.mock.calls[0][0];
    expect(update.body).toBe(t('phone.push.outcome.locked'));
    expect(update.android.actions).toHaveLength(1);
  });

  it('reads a refused answer as handled only when that request and run are no longer pending', async () => {
    for (const [pending, outcome] of [
      [[{ id: 'req-1', runId: 'run-1' }], 'phone.push.outcome.failed'],
      [[{ id: 'req-1', runId: 'run-2' }], 'phone.push.outcome.already'],
      [[], 'phone.push.outcome.already'],
    ] as const) {
      const { runtime, m } = load();
      const { ApiError } = require('@/api/client') as typeof import('@/api/client');
      const refused = async () => {
        throw new ApiError(409, 'stale');
      };
      m.lease.clients['pair-b'] = { respondPermission: refused, session: async () => ({ permissions: pending }) };
      await runtime.handleNotificationEvent({ type: 2, detail: { notification: permissionNotification, pressAction: { id: 'deny' } } } as never);
      expect(m.notifee.displayNotification.mock.calls[0][0].body).toBe(t(outcome));
    }
  });
});

describe('syncing with the hosts', () => {
  const pushInfo = { capabilities: ['push'] };

  beforeEach(() => {
    (globalThis as { fetch?: unknown }).fetch = jest.fn(async () => ({ status: 201, json: async () => ({ handle: seal.handle, secret: SECRET }) }));
  });

  it('does not ask FCM for a token until a paired host can push', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-a', false);
    m.lease.clients['pair-a'] = { info: async () => ({ capabilities: [] }), registerPush: jest.fn() };
    expect(await runtime.syncAllHosts()).toBe(true);
    expect(m.messaging['getToken']).not.toHaveBeenCalled();
    expect(m.messaging['setAutoInitEnabled']).not.toHaveBeenCalled();
  });

  it('registers the handle and kinds with a host that can push, turning FCM on first', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-a', false);
    const registerPush = jest.fn(async () => ({ ok: true }));
    const info = jest.fn(async () => pushInfo);
    m.lease.clients['pair-a'] = { info, registerPush };
    expect(await runtime.syncAllHosts()).toBe(true);
    expect(m.messaging['setAutoInitEnabled']).toHaveBeenCalledWith(expect.anything(), true);
    expect(registerPush).toHaveBeenCalledWith(
      { handle: seal.handle, kinds: ['completed', 'failed', 'permission', 'question', 'plan', 'signin'] },
      expect.any(AbortSignal),
    );
    expect(info).toHaveBeenCalledTimes(1);
  });

  it('reports a relay that is busy as not done', async () => {
    const { runtime, m } = load();
    pair(m, 'pair-a', false);
    (globalThis as { fetch?: unknown }).fetch = jest.fn(async () => ({ status: 503, json: async () => ({}) }));
    m.lease.clients['pair-a'] = { info: async () => pushInfo, registerPush: jest.fn() };
    expect(await runtime.syncAllHosts()).toBe(false);
  });

  it('sends one sync for quick changes and retries a failed one a bounded number of times', async () => {
    jest.useFakeTimers();
    try {
      const { runtime, m } = load();
      pair(m, 'pair-a', false);
      const info = jest.fn(async () => pushInfo);
      (globalThis as { fetch?: unknown }).fetch = jest.fn(async () => ({ status: 429, json: async () => ({}) }));
      m.lease.clients['pair-a'] = { info, registerPush: jest.fn() };
      runtime.requestSync();
      runtime.requestSync();
      runtime.requestSync();
      await jest.advanceTimersByTimeAsync(runtime.SYNC_DEBOUNCE_MS);
      expect(info).toHaveBeenCalledTimes(1);
      let wait = runtime.SYNC_RETRY_MS;
      for (let attempt = 0; attempt < runtime.SYNC_RETRIES; attempt += 1) {
        await jest.advanceTimersByTimeAsync(wait - 1);
        expect(info).toHaveBeenCalledTimes(1 + attempt);
        await jest.advanceTimersByTimeAsync(1);
        expect(info).toHaveBeenCalledTimes(2 + attempt);
        wait *= 2;
      }
      await jest.advanceTimersByTimeAsync(wait * 4);
      expect(info).toHaveBeenCalledTimes(1 + runtime.SYNC_RETRIES);
    } finally {
      jest.useRealTimers();
    }
  });
});

describe('startPush', () => {
  it('asks for the Android 13 notification permission only once', async () => {
    const { runtime, m } = load('android');
    (await runtime.startPush())();
    (await runtime.startPush())();
    expect(m.permissions).toHaveBeenCalledTimes(1);
    expect(m.notifee.requestPermission).not.toHaveBeenCalled();
  });

  it('asks for the iOS notification permission only once via NotifyKit', async () => {
    const { runtime, m } = load('ios');
    (await runtime.startPush())();
    (await runtime.startPush())();
    expect(m.notifee.requestPermission).toHaveBeenCalledTimes(1);
    expect(m.permissions).not.toHaveBeenCalled();
  });

  it('listens for pushes before the permission prompt is up', async () => {
    const { runtime, m } = load();
    let listening = false;
    m.permissions.mockImplementationOnce(async () => {
      listening = m.messaging['onMessage']!.mock.calls.length === 1 && m.messaging['onTokenRefresh']!.mock.calls.length === 1;
      return 'granted';
    });
    (await runtime.startPush())();
    expect(m.permissions).toHaveBeenCalledTimes(1);
    expect(listening).toBe(true);
  });
});
