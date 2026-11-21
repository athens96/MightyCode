import { requireOptionalNativeModule } from 'expo';
import { AppState, Platform } from 'react-native';

interface MightyDeviceLockModule {
  isDeviceLocked(): boolean;
}

/**
 * Locked unless the native module says otherwise on Android, or the app is active on iOS
 * where the Android keyguard native module is absent.
 */
export function isDeviceLocked(): boolean {
  const native = requireOptionalNativeModule<MightyDeviceLockModule>('MightyDeviceLock');
  if (!native) {
    if (Platform.OS === 'ios') return AppState.currentState !== 'active';
    return true;
  }
  try {
    // Only a plain `false` unlocks: an undefined or odd answer from the bridge counts as locked.
    return native.isDeviceLocked() !== false;
  } catch {
    return true;
  }
}
