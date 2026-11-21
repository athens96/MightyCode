import { PUSH_HANDLE_PREFIX } from '@/lib/push/registry';
import { MUTED_PREFIX, workspaceMute } from '@/lib/push/workspace-mute';
import { useHostsStore, type PairedHost } from '@/store/hosts';
import { useLiveStore } from '@/store/live';

const mockItems = new Map<string, string>();
jest.mock('expo-secure-store', () => ({
  getItemAsync: jest.fn(async (key: string) => mockItems.get(key) ?? null),
  setItemAsync: jest.fn(async (key: string, value: string) => void mockItems.set(key, value)),
  deleteItemAsync: jest.fn(async (key: string) => void mockItems.delete(key)),
}));

const HANDLE = 'h'.repeat(43);
const SECRET = 's'.repeat(43);

const host: PairedHost = {
  id: 'host-a',
  name: 'Mac',
  serverId: 'a'.repeat(64),
  relayUrl: 'wss://relay.example.invalid',
  hostPublicKeyB64: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
  hostId: 'host-id',
  appVersion: '1',
  pairedAt: '2026-10-23T00:00:00.000Z',
};

describe('removing a pairing', () => {
  const realFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  it('removes the push record on the phone without waiting for the relay, and deletes the handle there', async () => {
    mockItems.set(
      `${PUSH_HANDLE_PREFIX}host-a`,
      JSON.stringify({ handle: HANDLE, secret: SECRET, relayUrl: host.relayUrl, fcmToken: 't' }),
    );
    let answer: (value: Response) => void = () => undefined;
    const fetchMock = jest.fn(
      (_url: string, _init?: RequestInit) =>
        new Promise<Response>((resolve) => {
          answer = resolve;
        }),
    );
    globalThis.fetch = fetchMock as unknown as typeof fetch;
    useHostsStore.setState({ hosts: [host] });

    await useHostsStore.getState().removeHost('host-a');
    await new Promise((resolve) => setImmediate(resolve));

    expect(useHostsStore.getState().hosts).toEqual([]);
    expect(mockItems.has(`${PUSH_HANDLE_PREFIX}host-a`)).toBe(false);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe(`https://relay.example.invalid/push/v1/handles/${HANDLE}`);
    expect(init?.method).toBe('DELETE');
    expect((init?.headers as Record<string, string>)['authorization']).toBe(`Bearer ${SECRET}`);
    answer({ status: 204 } as Response);
  });

  it('forgets the pairing’s muted workspaces with it', async () => {
    mockItems.set(`${MUTED_PREFIX}host-a`, '["ws-a"]');
    mockItems.set(`${MUTED_PREFIX}host-b`, '["ws-a"]');
    globalThis.fetch = jest.fn(async () => ({ status: 204 }) as Response) as unknown as typeof fetch;
    useHostsStore.setState({ hosts: [host, { ...host, id: 'host-b' }] });
    expect(await workspaceMute.load('host-a')).toEqual(['ws-a']);

    await useHostsStore.getState().removeHost('host-a');
    await new Promise((resolve) => setImmediate(resolve));

    expect(mockItems.has(`${MUTED_PREFIX}host-a`)).toBe(false);
    expect(workspaceMute.mutedNow('host-a')).toEqual([]);
    expect(mockItems.get(`${MUTED_PREFIX}host-b`)).toBe('["ws-a"]');
  });

  it('clears cached live store data for the removed host without touching other hosts', async () => {
    globalThis.fetch = jest.fn(async () => ({ status: 204 }) as Response) as unknown as typeof fetch;
    useHostsStore.setState({ hosts: [host, { ...host, id: 'host-b' }] });
    const state = {
      protocol: 1 as const,
      revision: 1,
      hostName: 'Mac',
      workspaces: [],
      sessions: [],
    };
    const detail = {
      protocol: 1 as const,
      revision: 1,
      session: {
        id: 'sess-1',
        workspaceId: 'ws-1',
        title: 'Pane',
        kind: 'claude' as const,
        provider: 'claude' as const,
        model: 'sonnet',
        status: 'idle' as const,
        revision: 1,
        updatedAt: '2026-11-01T00:00:00.000Z',
        pendingPermissions: 0,
        pendingQuestions: 0,
        queued: 0,
        terminal: false,
      },
      entries: [],
      hasMoreEntries: false,
      permissions: [],
      queued: [],
    };
    useLiveStore.getState().applyState('host-a', state, true);
    useLiveStore.getState().applyState('host-b', state, true);
    useLiveStore.getState().applyDetail('host-a', 'sess-1', detail, true);
    useLiveStore.getState().applyDetail('host-b', 'sess-1', detail, true);
    useLiveStore.getState().setCapabilities('host-a', ['queue'], { platform: 'darwin', appVersion: '1.0' });
    useLiveStore.getState().setCapabilities('host-b', ['queue'], { platform: 'darwin', appVersion: '1.0' });
    useLiveStore.getState().setCommands('host-a', 'sess-1', []);
    useLiveStore.getState().setCommands('host-b', 'sess-1', []);

    await useHostsStore.getState().removeHost('host-a');

    const live = useLiveStore.getState();
    expect(live.states['host-a']).toBeUndefined();
    expect(live.capabilities['host-a']).toBeUndefined();
    expect(live.hostIdentity['host-a']).toBeUndefined();
    expect(Object.keys(live.details).some((k) => k.startsWith('host-a'))).toBe(false);
    expect(Object.keys(live.detailReceivedAt).some((k) => k.startsWith('host-a'))).toBe(false);
    expect(Object.keys(live.commands).some((k) => k.startsWith('host-a'))).toBe(false);
    // Other hosts remain intact.
    expect(live.states['host-b']).toBeDefined();
    expect(live.capabilities['host-b']).toEqual(['queue']);
    expect(Object.keys(live.details).some((k) => k.startsWith('host-b'))).toBe(true);
  });
});

describe('learning where a push belongs', () => {
  it('keeps each applied state of a paired host, and nothing of a host no longer paired', () => {
    const learn = jest.spyOn(workspaceMute, 'learn').mockResolvedValue(undefined);
    useHostsStore.setState({ hosts: [host] });
    const state = { protocol: 1, revision: 3, hostName: 'Mac', workspaces: [{ id: 'ws-a', name: 'demo', path: '/demo' }], sessions: [] };
    useLiveStore.getState().applyState('host-a', state, true);
    expect(learn).toHaveBeenCalledWith('host-a', state);
    // The same revision again changes nothing and is not learned twice.
    useLiveStore.getState().applyState('host-a', { ...state }, false);
    expect(learn).toHaveBeenCalledTimes(1);
    useLiveStore.getState().applyState('host-gone', state, true);
    expect(learn).toHaveBeenCalledTimes(1);
    learn.mockRestore();
  });
});
