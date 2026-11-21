import { createElement } from 'react';
import { Text, TextInput } from 'react-native';
import TestRenderer, { act, type ReactTestRenderer } from 'react-test-renderer';
import { toBase64 } from '@/api/relay/crypto';
import { resetLanguage } from '@/lib/i18n';

let mockSearchParams: Record<string, string | string[] | undefined> = {};

jest.mock('expo-router', () => ({
  router: { dismissTo: jest.fn(), push: jest.fn() },
  useLocalSearchParams: () => mockSearchParams,
}));
jest.mock('expo-camera', () => ({
  CameraView: 'CameraView',
  useCameraPermissions: () => [{ granted: true }, jest.fn()],
}));
jest.mock('expo-haptics', () => ({
  selectionAsync: jest.fn(async () => undefined),
}));
jest.mock('expo-secure-store', () => ({
  getItemAsync: jest.fn(async () => null),
  setItemAsync: jest.fn(async () => undefined),
  deleteItemAsync: jest.fn(async () => undefined),
}));

import PairScreen from '../../app/pair';

const publicKeyUrl = toBase64(Uint8Array.from({ length: 32 }, (_, index) => index))
  .replace(/\+/g, '-')
  .replace(/\//g, '_')
  .replace(/=+$/, '');

let mounted: ReactTestRenderer | undefined;

beforeAll(() => resetLanguage('ko'));
afterAll(() => resetLanguage());

beforeEach(() => {
  mockSearchParams = {};
});

afterEach(() => {
  act(() => mounted?.unmount());
  mounted = undefined;
});

function shownTexts(view: ReactTestRenderer): string[] {
  return view.root
    .findAllByType(Text)
    .map((node) => [node.props.children].flat(3).filter((part) => typeof part === 'string').join(''));
}

describe('PairScreen deep links', () => {
  it('stays in QR scanner mode when opened without deep-link search params', () => {
    act(() => {
      mounted = TestRenderer.create(createElement(PairScreen));
    });
    expect(mounted!.root.findAllByType(TextInput)).toHaveLength(0);
  });

  it('pre-fills manual mode from mightycode://pair search params without error when valid', () => {
    mockSearchParams = {
      v: '2',
      sid: 'mac-studio.local',
      pk: publicKeyUrl,
      relay: 'wss://relay.example.com:8787',
      key: 'pairing-key-32bytes',
      name: 'Studio',
      direct: '100.101.102.103:43139',
    };

    act(() => {
      mounted = TestRenderer.create(createElement(PairScreen));
    });

    const inputs = mounted!.root.findAllByType(TextInput);
    expect(inputs).toHaveLength(1);
    expect(inputs[0]?.props.value).toContain('mightycode://pair?');
    expect(inputs[0]?.props.value).toContain('sid=mac-studio.local');
    expect(inputs[0]?.props.value).toContain('direct=100.101.102.103%3A43139');
    expect(shownTexts(mounted!)).not.toContain('지원하지 않는 페어링 버전입니다 (v=3).');
  });

  it('shows a validation error immediately when opened with an invalid deep link', () => {
    mockSearchParams = { v: '3', sid: 'mac-studio.local' };

    act(() => {
      mounted = TestRenderer.create(createElement(PairScreen));
    });

    const inputs = mounted!.root.findAllByType(TextInput);
    expect(inputs).toHaveLength(1);
    expect(inputs[0]?.props.value).toBe('mightycode://pair?v=3&sid=mac-studio.local');
    expect(shownTexts(mounted!)).toContain('지원하지 않는 페어링 버전입니다 (v=3).');
  });
});
