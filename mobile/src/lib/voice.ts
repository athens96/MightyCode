import { requireOptionalNativeModule } from 'expo';
import { Linking } from 'react-native';
import { t } from '@/lib/i18n';

export const MOBILE_WAVE_BARS = 12;

export type MobileSpeechPermission = 'granted' | 'denied' | 'undetermined';

export interface MobileSpeechEvent {
  type: 'recording' | 'level' | 'partial' | 'transcribing' | 'result' | 'cancelled' | 'permissionDenied' | 'error';
  level?: number;
  text?: string;
  message?: string;
}

export interface SpeechSubscription {
  remove(): void;
}

export interface MobileSpeechDriver {
  isAvailable(): boolean;
  getPermissionStatus(): Promise<MobileSpeechPermission>;
  requestPermission(): Promise<'granted' | 'denied'>;
  start(locale: string, hints: string[]): Promise<boolean>;
  stop(): Promise<void>;
  cancel(): Promise<void>;
  addListener(listener: (event: MobileSpeechEvent) => void): SpeechSubscription;
}

interface RawMightySpeechModule {
  isAvailable(): boolean;
  getPermissionStatus(): Promise<MobileSpeechPermission>;
  requestPermission(): Promise<'granted' | 'denied'>;
  start(locale: string, hints: string[]): Promise<boolean>;
  stop(): Promise<void>;
  cancel(): Promise<void>;
  addListener?(eventName: 'onSpeechEvent', listener: (event: MobileSpeechEvent) => void): SpeechSubscription;
}

let overrideDriver: MobileSpeechDriver | null | undefined;

/** Test seam so Jest suites can drive recording, waveform, cancellation and permission flows deterministically. */
export function setTestSpeechDriver(driver: MobileSpeechDriver | null | undefined): void {
  overrideDriver = driver;
}

export function resolveSpeechDriver(): MobileSpeechDriver | null {
  if (overrideDriver !== undefined) return overrideDriver;
  const raw = requireOptionalNativeModule<RawMightySpeechModule>('MightySpeech');
  if (!raw) return null;
  return {
    isAvailable: () => {
      try {
        return raw.isAvailable() === true;
      } catch {
        return false;
      }
    },
    getPermissionStatus: () => raw.getPermissionStatus(),
    requestPermission: () => raw.requestPermission(),
    start: (locale, hints) => raw.start(locale, hints),
    stop: () => raw.stop(),
    cancel: () => raw.cancel(),
    addListener: (listener) => {
      if (typeof raw.addListener === 'function') {
        return raw.addListener('onSpeechEvent', listener);
      }
      return { remove: () => undefined };
    },
  };
}

export function speechBcp47(lang: string): string {
  if (lang.startsWith('en')) return 'en-US';
  if (lang.startsWith('ja')) return 'ja-JP';
  if (lang.startsWith('zh')) return 'zh-CN';
  return 'ko-KR';
}

export function normalizeMobileWaveform(levels: readonly number[], count = MOBILE_WAVE_BARS): number[] {
  const out = new Array<number>(count).fill(0.16);
  const start = Math.max(0, levels.length - count);
  const offset = count - (levels.length - start);
  for (let i = start; i < levels.length; i++) {
    const value = levels[i] ?? 0;
    out[offset + (i - start)] = Number.isFinite(value) ? Math.min(1, Math.max(0.16, value)) : 0.16;
  }
  return out;
}

export function formatMobileVoiceSeconds(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const mins = Math.floor(whole / 60);
  const secs = whole % 60;
  return `${mins}:${secs < 10 ? '0' : ''}${secs}`;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function applyMobileCodeHints(
  transcript: string,
  hints: readonly string[] = [],
  spokenDot = t('composer.voice.spokenDot'),
): string {
  const trimmed = transcript.trim();
  if (!trimmed) return '';
  const dotWord = spokenDot.trim();
  const dotGroup = dotWord && dotWord.toLowerCase() !== 'dot' ? `(?:dot|${escapeRegExp(dotWord)})` : 'dot';
  let text = trimmed.replace(
    new RegExp(`\\s+${dotGroup}\\s+(tsx|ts|jsx|js|rs|py|json|md|css|html|toml|yaml|yml)\\b`, 'gi'),
    '.$1',
  );
  if (hints.length === 0) return text;
  const map = new Map<string, string>();
  for (const h of hints) {
    if (h.length >= 4 && ((/[A-Z]/.test(h) && /[a-z]/.test(h)) || /[._-]/.test(h)) && !map.has(h.toLowerCase())) {
      map.set(h.toLowerCase(), h);
    }
  }
  if (map.size === 0) return text;
  text = text.replace(/\b[A-Za-z_][A-Za-z0-9_.-]{2,}\b/g, (word) => map.get(word.toLowerCase()) ?? word);
  return text;
}

export function appendDictatedMobileText(current: string, dictated: string): string {
  const clean = dictated.trim();
  if (!clean) return current;
  if (current.length === 0) return clean;
  return /\s$/.test(current) ? `${current}${clean}` : `${current} ${clean}`;
}

export async function openAppPermissionSettings(): Promise<void> {
  try {
    await Linking.openSettings();
  } catch {
    // Ignore when platform cannot open system settings.
  }
}
