import { createElement } from 'react';
import { Text } from 'react-native';
import TestRenderer, { act, type ReactTestRenderer } from 'react-test-renderer';
import type { HostCredentials, MobileClient } from '@/api/client';
import { resetLanguage } from '@/lib/i18n';
import { useHostsStore, type PairedHost } from '@/store/hosts';

const mockAcquireCalls: Array<{ hostId: string; credentials: HostCredentials }> = [];
const mockRelease = jest.fn();

jest.mock('expo-secure-store', () => ({
  getItemAsync: jest.fn(async () => null),
  setItemAsync: jest.fn(async () => undefined),
  deleteItemAsync: jest.fn(async () => undefined),
}));

jest.mock('@/api/client', () => {
  const actual = jest.requireActual('@/api/client');
  return {
    ...actual,
    acquireHostClient: jest.fn((hostId: string, credentials: HostCredentials) => {
      mockAcquireCalls.push({ hostId, credentials });
      return {
        client: {} as MobileClient,
        connection: {},
        release: mockRelease,
      };
    }),
  };
});

jest.mock('expo-router', () => ({
  DarkTheme: { dark: true, colors: {} },
  DefaultTheme: { dark: false, colors: {} },
  Stack: Object.assign(() => null, { Screen: () => null }),
  ThemeProvider: ({ children }: { children: unknown }) => children,
  router: { canGoBack: () => false, back: jest.fn(), replace: jest.fn() },
}));
jest.mock('expo-status-bar', () => ({ StatusBar: () => null }));
jest.mock('react-native-gesture-handler', () => ({ GestureHandlerRootView: 'View' }));
jest.mock('react-native-safe-area-context', () => ({ SafeAreaProvider: 'View' }));
jest.mock('@/components/icons', () => ({ Icon: () => null }));
jest.mock('@/components/push-root', () => ({ PushRoot: () => null }));
jest.mock('@/components/toast-host', () => ({ ToastHost: () => null }));

import { useHostClient } from '@/store/live';
import { ErrorBoundary } from '../../app/_layout';

const baseHost: PairedHost = {
  id: 'host-direct',
  name: 'Studio',
  serverId: 'a'.repeat(64),
  relayUrl: 'wss://relay.example.invalid',
  hostPublicKeyB64: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8',
  hostId: 'host-1',
  appVersion: '1.0.0',
  pairedAt: '2026-11-01T00:00:00.000Z',
};

function HookHarness({ hostId }: { hostId: string | undefined }) {
  useHostClient(hostId);
  return null;
}

let mounted: ReactTestRenderer | undefined;

beforeAll(() => resetLanguage('ko'));
afterAll(() => resetLanguage());

beforeEach(() => {
  mockAcquireCalls.length = 0;
  mockRelease.mockClear();
  useHostsStore.setState({ hosts: [], keys: {}, tokens: {}, clientIds: {}, clientId: 'client-1', status: {} });
});

afterEach(() => {
  act(() => mounted?.unmount());
  mounted = undefined;
});

describe('useHostClient directEndpoints propagation', () => {
  it('passes directEndpoints from the host store into HostCredentials and updates when they change', () => {
    useHostsStore.setState({
      hosts: [{ ...baseHost, directEndpoints: ['100.101.102.103:43139'] }],
      tokens: { [baseHost.id]: 'device-token-1' },
      clientId: 'client-1',
    });

    act(() => {
      mounted = TestRenderer.create(createElement(HookHarness, { hostId: baseHost.id }));
    });

    expect(mockAcquireCalls).toHaveLength(1);
    expect(mockAcquireCalls[0]?.credentials).toMatchObject({
      serverId: baseHost.serverId,
      relayUrl: baseHost.relayUrl,
      hostPublicKeyB64: baseHost.hostPublicKeyB64,
      deviceToken: 'device-token-1',
      clientId: 'client-1',
      directEndpoints: ['100.101.102.103:43139'],
    });

    // Updating directEndpoints propagates new credentials into acquireHostClient.
    act(() => {
      useHostsStore.setState({
        hosts: [{ ...baseHost, directEndpoints: ['100.101.102.104:43139', '[fd7a:115c:a1e0::1]:43139'] }],
      });
    });

    expect(mockAcquireCalls).toHaveLength(2);
    expect(mockAcquireCalls[1]?.credentials.directEndpoints).toEqual([
      '100.101.102.104:43139',
      '[fd7a:115c:a1e0::1]:43139',
    ]);
  });

  it('omits directEndpoints when the host has none', () => {
    useHostsStore.setState({
      hosts: [baseHost],
      tokens: { [baseHost.id]: 'device-token-1' },
      clientId: 'client-1',
    });

    act(() => {
      mounted = TestRenderer.create(createElement(HookHarness, { hostId: baseHost.id }));
    });

    expect(mockAcquireCalls).toHaveLength(1);
    expect(mockAcquireCalls[0]?.credentials.directEndpoints).toBeUndefined();
  });
});

describe('RootLayout ErrorBoundary', () => {
  it('renders a recoverable error view and calls retry when pressed', () => {
    const retry = jest.fn(async () => undefined);
    act(() => {
      mounted = TestRenderer.create(
        createElement(ErrorBoundary, { error: new Error('render exploded'), retry }),
      );
    });

    const texts = mounted!.root
      .findAllByType(Text)
      .map((node) => [node.props.children].flat(3).filter((p) => typeof p === 'string').join(''));
    expect(texts).toContain('알 수 없는 오류');
    expect(texts).toContain('render exploded');
    expect(texts).toContain('다시 시도');

    const retryButton = mounted!.root.find(
      (node) => node.props.accessibilityRole === 'button' && typeof node.props.onPress === 'function',
    );
    act(() => {
      retryButton.props.onPress();
    });
    expect(retry).toHaveBeenCalledTimes(1);
  });
});
