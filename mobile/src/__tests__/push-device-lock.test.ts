import { requireOptionalNativeModule } from 'expo';
import { AppState, Platform } from 'react-native';

import { isDeviceLocked } from '@/lib/push/device-lock';

// Prevent expo's lazy global `fetch` getter from loading native modules during teardown.
(globalThis as { fetch?: unknown }).fetch = jest.fn();

jest.mock('expo', () => ({ requireOptionalNativeModule: jest.fn() }));
jest.mock('react-native', () => ({
  Platform: { OS: 'android' },
  AppState: { currentState: 'active' },
}));

const lookup = requireOptionalNativeModule as jest.MockedFunction<typeof requireOptionalNativeModule>;
const platform = Platform as { OS: string };
const appState = AppState as { currentState: string | undefined };

describe('isDeviceLocked', () => {
  beforeEach(() => {
    platform.OS = 'android';
    appState.currentState = 'active';
  });

  afterEach(() => lookup.mockReset());

  it('counts Android as locked when the native module is missing', () => {
    lookup.mockReturnValue(null);
    expect(isDeviceLocked()).toBe(true);
    expect(lookup).toHaveBeenCalledWith('MightyDeviceLock');
  });

  it('uses AppState.currentState on iOS when the Android native module is absent', () => {
    platform.OS = 'ios';
    lookup.mockReturnValue(null);

    appState.currentState = 'active';
    expect(isDeviceLocked()).toBe(false);

    appState.currentState = 'background';
    expect(isDeviceLocked()).toBe(true);

    appState.currentState = 'inactive';
    expect(isDeviceLocked()).toBe(true);

    appState.currentState = undefined;
    expect(isDeviceLocked()).toBe(true);
  });

  it('counts the phone as locked when the native check throws', () => {
    lookup.mockReturnValue({ isDeviceLocked: () => { throw new Error('no keyguard'); } });
    expect(isDeviceLocked()).toBe(true);
  });

  it('counts anything but a plain false as locked', () => {
    lookup.mockReturnValue({ isDeviceLocked: () => undefined as unknown as boolean });
    expect(isDeviceLocked()).toBe(true);
    lookup.mockReturnValue({ isDeviceLocked: () => 0 as unknown as boolean });
    expect(isDeviceLocked()).toBe(true);
  });

  it('follows the native answer', () => {
    lookup.mockReturnValue({ isDeviceLocked: () => false });
    expect(isDeviceLocked()).toBe(false);
    lookup.mockReturnValue({ isDeviceLocked: () => true });
    expect(isDeviceLocked()).toBe(true);
  });
});
