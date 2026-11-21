import {
  appendDictatedMobileText,
  applyMobileCodeHints,
  formatMobileVoiceSeconds,
  normalizeMobileWaveform,
  resolveSpeechDriver,
  setTestSpeechDriver,
  speechBcp47,
  type MobileSpeechEvent,
} from '@/lib/voice';

const appJson = require('../../app.json') as {
  expo: {
    ios: { infoPlist: Record<string, unknown> };
    android: { permissions: string[]; blockedPermissions: string[] };
  };
};

describe('mobile voice dictation (MC-4)', () => {
  afterEach(() => {
    setTestSpeechDriver(undefined);
  });

  it('declares Android RECORD_AUDIO and iOS microphone + speech usage descriptions', () => {
    expect(appJson.expo.android.permissions).toContain('android.permission.RECORD_AUDIO');
    expect(appJson.expo.android.blockedPermissions).not.toContain('android.permission.RECORD_AUDIO');
    expect(typeof appJson.expo.ios.infoPlist.NSMicrophoneUsageDescription).toBe('string');
    expect(typeof appJson.expo.ios.infoPlist.NSSpeechRecognitionUsageDescription).toBe('string');
  });

  it('appends dictated text with smart spacing and restores code identifiers', () => {
    expect(appendDictatedMobileText('', 'approved')).toBe('approved');
    expect(appendDictatedMobileText('after tests pass', 'please merge')).toBe('after tests pass please merge');
    expect(appendDictatedMobileText('line1\n', 'line2')).toBe('line1\nline2');

    expect(
      applyMobileCodeHints('plancard dot tsx and composer check', ['PlanCard.tsx', 'PlanCard', 'Composer.tsx']),
    ).toBe('PlanCard.tsx and composer check');
  });

  it('normalizes waveform levels, duration strings, and BCP-47 tags', () => {
    expect(normalizeMobileWaveform([0.4, 0.8, 2], 4)).toEqual([0.16, 0.4, 0.8, 1]);
    expect(formatMobileVoiceSeconds(65)).toBe('1:05');
    expect(speechBcp47('ko')).toBe('ko-KR');
    expect(speechBcp47('en')).toBe('en-US');
    expect(speechBcp47('ja')).toBe('ja-JP');
    expect(speechBcp47('zh')).toBe('zh-CN');
  });

  it('streams recording, waveform levels, transcribing, and cancellation via driver seam', async () => {
    const received: MobileSpeechEvent[] = [];
    let listenerRef: ((event: MobileSpeechEvent) => void) | null = null;
    let cancelled = false;

    setTestSpeechDriver({
      isAvailable: () => true,
      getPermissionStatus: async () => 'granted',
      requestPermission: async () => 'granted',
      start: async () => {
        listenerRef?.({ type: 'recording' });
        listenerRef?.({ type: 'level', level: 0.72 });
        listenerRef?.({ type: 'partial', text: 'partial hypothesis' });
        return true;
      },
      stop: async () => {
        listenerRef?.({ type: 'transcribing' });
      },
      cancel: async () => {
        cancelled = true;
        listenerRef?.({ type: 'cancelled' });
      },
      addListener: (cb) => {
        listenerRef = cb;
        return { remove: () => { listenerRef = null; } };
      },
    });

    const driver = resolveSpeechDriver()!;
    const sub = driver.addListener((e) => received.push(e));
    await driver.start('ko-KR', ['PlanCard']);
    await driver.stop();
    await driver.cancel();
    sub.remove();

    expect(cancelled).toBe(true);
    expect(received.map((e) => e.type)).toEqual(['recording', 'level', 'partial', 'transcribing', 'cancelled']);
  });
});
