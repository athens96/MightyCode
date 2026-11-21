import { useCallback, useEffect, useRef, useState } from 'react';
import { useServices } from '../app/context';
import { Icon } from '../icons/Icon';
import { listenPane } from '../panes/channel';
import {
  PTT_HOLD_THRESHOLD_MS,
  WAVEFORM_BAR_COUNT,
  applyWorkspaceCodeHints,
  formatVoiceDuration,
  isPushToTalkShortcut,
  normalizeWaveformBars,
} from './voiceModel';

export type VoicePhase = 'idle' | 'recording' | 'transcribing';

interface VoiceStatusResponse {
  nativeAvailable: boolean;
  engine: string;
  permission: string;
}

interface BrowserSpeechRecognitionInstance {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  onresult: ((event: { resultIndex: number; results: ArrayLike<{ isFinal: boolean; 0: { transcript: string } }> }) => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
  abort(): void;
}

type BrowserSpeechRecognitionCtor = new () => BrowserSpeechRecognitionInstance;

function webSpeechConstructor(): BrowserSpeechRecognitionCtor | null {
  if (typeof window === 'undefined') return null;
  const win = window as unknown as {
    SpeechRecognition?: BrowserSpeechRecognitionCtor;
    webkitSpeechRecognition?: BrowserSpeechRecognitionCtor;
  };
  return win.SpeechRecognition ?? win.webkitSpeechRecognition ?? null;
}

function bcp47ForAppLanguage(lang: string): string {
  if (lang.startsWith('en')) return 'en-US';
  if (lang.startsWith('ja')) return 'ja-JP';
  if (lang.startsWith('zh')) return 'zh-CN';
  return 'ko-KR';
}

function activeAppLanguage(preference: string | undefined): string {
  if (preference && preference !== 'system') return preference;
  if (typeof document !== 'undefined' && document.documentElement.lang) return document.documentElement.lang;
  if (typeof navigator !== 'undefined' && navigator.language) return navigator.language;
  return 'ko';
}

export function useComposerVoice({
  paneId,
  mac,
  disabled,
  onDictated,
}: {
  paneId: string;
  mac: boolean;
  disabled: boolean;
  onDictated: (text: string) => void;
}) {
  const { bridge, store, t } = useServices();
  const [phase, setPhase] = useState<VoicePhase>('idle');
  const [levels, setLevels] = useState<number[]>(() => normalizeWaveformBars([], WAVEFORM_BAR_COUNT));
  const [partial, setPartial] = useState('');
  const [seconds, setSeconds] = useState(0);
  const [permissionDenied, setPermissionDenied] = useState(false);
  const [voiceError, setVoiceError] = useState<string | null>(null);

  const phaseRef = useRef<VoicePhase>('idle');
  phaseRef.current = phase;
  const modeRef = useRef<'native' | 'web' | null>(null);
  const hintsRef = useRef<string[]>([]);
  const webRecRef = useRef<BrowserSpeechRecognitionInstance | null>(null);
  const webFinalRef = useRef('');
  const webCancelledRef = useRef(false);
  const webAudioCleanupRef = useRef<(() => void) | null>(null);
  const holdStartRef = useRef<number | null>(null);
  const keyHoldActiveRef = useRef(false);
  const onDictatedRef = useRef(onDictated);
  onDictatedRef.current = onDictated;

  const resetVisuals = useCallback(() => {
    setLevels(normalizeWaveformBars([], WAVEFORM_BAR_COUNT));
    setPartial('');
    setSeconds(0);
  }, []);

  const stopWebAudioMeter = useCallback(() => {
    if (webAudioCleanupRef.current) {
      webAudioCleanupRef.current();
      webAudioCleanupRef.current = null;
    }
  }, []);

  const pushLevel = useCallback((level: number) => {
    setLevels((prev) => normalizeWaveformBars([...prev.slice(-(WAVEFORM_BAR_COUNT - 1)), level], WAVEFORM_BAR_COUNT));
  }, []);

  // Listen to native `voice` pushes from `src-tauri/src/voice.rs`.
  useEffect(() => listenPane('voice', paneId, (msg) => {
    if (msg.state === 'recording') {
      setPhase('recording');
      setPermissionDenied(false);
      setVoiceError(null);
    } else if (msg.state === 'level' && typeof msg.level === 'number') {
      pushLevel(msg.level);
    } else if (msg.state === 'partial') {
      setPartial(msg.text ?? '');
    } else if (msg.state === 'transcribing') {
      setPhase('transcribing');
    } else if (msg.state === 'result') {
      setPhase('idle');
      resetVisuals();
      const text = (msg.text ?? '').trim();
      if (text.length > 0) onDictatedRef.current(text);
    } else if (msg.state === 'cancelled') {
      setPhase('idle');
      resetVisuals();
    } else if (msg.state === 'permissionDenied') {
      setPhase('idle');
      resetVisuals();
      setPermissionDenied(true);
    } else if (msg.state === 'error') {
      setPhase('idle');
      resetVisuals();
      setVoiceError(msg.error ?? t('composer.voice.errorFailed'));
    }
  }), [paneId, pushLevel, resetVisuals, t]);

  // Recording elapsed-seconds clock.
  useEffect(() => {
    if (phase !== 'recording') return undefined;
    setSeconds(0);
    const interval = setInterval(() => setSeconds((s) => s + 1), 1000);
    return () => clearInterval(interval);
  }, [phase]);

  // Cleanup any live session if the pane unmounts.
  useEffect(() => () => {
    stopWebAudioMeter();
    if (webRecRef.current) {
      webCancelledRef.current = true;
      try { webRecRef.current.abort(); } catch { /* ignore */ }
      webRecRef.current = null;
    }
    if (modeRef.current === 'native' && phaseRef.current !== 'idle') {
      void bridge.call('voiceStop', { paneId, cancel: true }).catch(() => {});
    }
  }, [bridge, paneId, stopWebAudioMeter]);

  const startWebSpeech = useCallback(async (bcp47: string, hints: string[]) => {
    const Ctor = webSpeechConstructor();
    if (!Ctor) {
      setVoiceError(t('composer.voice.unavailable'));
      return false;
    }
    webCancelledRef.current = false;
    webFinalRef.current = '';
    const rec = new Ctor();
    rec.lang = bcp47;
    rec.continuous = true;
    rec.interimResults = true;
    rec.onresult = (event) => {
      let interim = '';
      for (let i = event.resultIndex; i < event.results.length; i++) {
        const item = event.results[i];
        if (!item) continue;
        const transcript = item[0]?.transcript ?? '';
        if (item.isFinal) webFinalRef.current += ' ' + transcript;
        else interim += transcript;
      }
      const preview = applyWorkspaceCodeHints((webFinalRef.current + ' ' + interim).trim(), hints, t('composer.voice.spokenDot'));
      setPartial(preview);
    };
    rec.onerror = (event) => {
      stopWebAudioMeter();
      webRecRef.current = null;
      setPhase('idle');
      resetVisuals();
      if (event.error === 'not-allowed' || event.error === 'service-not-allowed') {
        setPermissionDenied(true);
      } else if (event.error !== 'aborted' && event.error !== 'no-speech') {
        setVoiceError(t('composer.voice.errorFailed'));
      }
    };
    rec.onend = () => {
      stopWebAudioMeter();
      webRecRef.current = null;
      if (webCancelledRef.current) {
        setPhase('idle');
        resetVisuals();
        return;
      }
      const corrected = applyWorkspaceCodeHints(webFinalRef.current, hints, t('composer.voice.spokenDot'));
      setPhase('idle');
      resetVisuals();
      if (corrected.length > 0) onDictatedRef.current(corrected);
    };

    try {
      rec.start();
    } catch {
      setVoiceError(t('composer.voice.errorFailed'));
      return false;
    }
    webRecRef.current = rec;
    modeRef.current = 'web';
    setPhase('recording');

    // Optional live WebAudio microphone level meter for the waveform bars.
    if (typeof navigator !== 'undefined' && navigator.mediaDevices?.getUserMedia) {
      navigator.mediaDevices.getUserMedia({ audio: true }).then((stream) => {
        if (phaseRef.current !== 'recording' || webCancelledRef.current) {
          stream.getTracks().forEach((tr) => tr.stop());
          return;
        }
        const AudioCtx = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
        if (!AudioCtx) {
          stream.getTracks().forEach((tr) => tr.stop());
          return;
        }
        const ctx = new AudioCtx();
        const source = ctx.createMediaStreamSource(stream);
        const analyser = ctx.createAnalyser();
        analyser.fftSize = 256;
        source.connect(analyser);
        const buffer = new Uint8Array(analyser.frequencyBinCount);
        const timer = setInterval(() => {
          analyser.getByteTimeDomainData(buffer);
          let sum = 0;
          for (let i = 0; i < buffer.length; i++) {
            const norm = ((buffer[i] ?? 128) - 128) / 128;
            sum += norm * norm;
          }
          const rms = Math.sqrt(sum / buffer.length);
          pushLevel(Math.min(1, Math.max(0.14, rms * 4.2)));
        }, 70);
        webAudioCleanupRef.current = () => {
          clearInterval(timer);
          stream.getTracks().forEach((tr) => tr.stop());
          void ctx.close().catch(() => {});
        };
      }, () => {
        // Permission error is reported by SpeechRecognition.onerror.
      });
    }
    return true;
  }, [pushLevel, resetVisuals, stopWebAudioMeter, t]);

  const startVoice = useCallback(async () => {
    if (disabled || phaseRef.current !== 'idle') return;
    setPermissionDenied(false);
    setVoiceError(null);
    resetVisuals();

    const bcp47 = bcp47ForAppLanguage(activeAppLanguage(store.getState().snapshot?.languagePreference));
    let hints = hintsRef.current;
    try {
      const hintReply = await bridge.call<{ hints?: string[] }>('voiceHints', { paneId });
      if (hintReply?.hints) {
        hints = hintReply.hints;
        hintsRef.current = hints;
      }
    } catch {
      // Optional hints call.
    }

    try {
      const status = await bridge.call<VoiceStatusResponse>('voiceStatus');
      if (status?.nativeAvailable) {
        modeRef.current = 'native';
        setPhase('recording');
        const started = await bridge.call<{ started: boolean }>('voiceStart', { paneId, language: bcp47, hints });
        if (!started?.started) {
          setPhase('idle');
          await startWebSpeech(bcp47, hints);
        }
        return;
      }
    } catch {
      // Host without voiceStatus falls back to Web Speech.
    }
    await startWebSpeech(bcp47, hints);
  }, [disabled, resetVisuals, store, bridge, paneId, startWebSpeech]);

  const stopVoice = useCallback((cancel = false) => {
    if (phaseRef.current === 'idle') return;
    if (modeRef.current === 'native') {
      if (cancel) {
        setPhase('idle');
        resetVisuals();
      } else {
        setPhase('transcribing');
      }
      void bridge.call('voiceStop', { paneId, cancel }).catch(() => {
        setPhase('idle');
        resetVisuals();
      });
      return;
    }
    stopWebAudioMeter();
    const rec = webRecRef.current;
    if (!rec) {
      setPhase('idle');
      resetVisuals();
      return;
    }
    if (cancel) {
      webCancelledRef.current = true;
      webRecRef.current = null;
      try { rec.abort(); } catch { /* ignore */ }
      setPhase('idle');
      resetVisuals();
    } else {
      setPhase('transcribing');
      try { rec.stop(); } catch {
        setPhase('idle');
        resetVisuals();
      }
    }
  }, [bridge, paneId, resetVisuals, stopWebAudioMeter]);

  const openPermissionSettings = useCallback(() => {
    void bridge.call('voiceOpenSettings').catch(() => {});
  }, [bridge]);

  // Global Push-to-Talk shortcut (`⇧⌘M` on Mac / `Ctrl+Shift+M` on Windows) while this composer's pane is mounted & focused.
  const handleKeyDown = useCallback((event: {
    key: string;
    code?: string;
    metaKey: boolean;
    ctrlKey: boolean;
    shiftKey: boolean;
    altKey: boolean;
    repeat: boolean;
    preventDefault(): void;
  }): boolean => {
    if (event.key === 'Escape' && phaseRef.current !== 'idle') {
      event.preventDefault();
      stopVoice(true);
      return true;
    }
    if (!isPushToTalkShortcut(event, mac)) return false;
    event.preventDefault();
    if (event.repeat) return true;
    if (phaseRef.current === 'idle') {
      keyHoldActiveRef.current = true;
      holdStartRef.current = Date.now();
      void startVoice();
    } else if (phaseRef.current === 'recording') {
      stopVoice(false);
    }
    return true;
  }, [mac, startVoice, stopVoice]);

  useEffect(() => {
    const onWindowKeyUp = (event: globalThis.KeyboardEvent) => {
      if (!keyHoldActiveRef.current) return;
      const releasedTrigger = event.key.toLowerCase() === 'm' || event.code === 'KeyM' || (!event.metaKey && !event.ctrlKey);
      if (!releasedTrigger) return;
      keyHoldActiveRef.current = false;
      const heldMs = Date.now() - (holdStartRef.current ?? Date.now());
      holdStartRef.current = null;
      // Hold >= threshold acts as Push-to-Talk (release to transcribe); a quick tap toggles hands-free dictation!
      if (heldMs >= PTT_HOLD_THRESHOLD_MS && phaseRef.current === 'recording') {
        stopVoice(false);
      }
    };
    window.addEventListener('keyup', onWindowKeyUp);
    return () => window.removeEventListener('keyup', onWindowKeyUp);
  }, [stopVoice]);

  const onMicPointerDown = useCallback(() => {
    if (disabled) return;
    if (phaseRef.current === 'idle') {
      holdStartRef.current = Date.now();
      void startVoice();
    } else if (phaseRef.current === 'recording') {
      holdStartRef.current = null;
      stopVoice(false);
    } else if (phaseRef.current === 'transcribing') {
      holdStartRef.current = null;
      stopVoice(true);
    }
  }, [disabled, startVoice, stopVoice]);

  const onMicPointerUp = useCallback(() => {
    if (holdStartRef.current === null) return;
    const heldMs = Date.now() - holdStartRef.current;
    holdStartRef.current = null;
    if (heldMs >= PTT_HOLD_THRESHOLD_MS && phaseRef.current === 'recording') {
      stopVoice(false);
    }
  }, [stopVoice]);

  return {
    phase,
    levels,
    partial,
    seconds,
    permissionDenied,
    voiceError,
    dismissPermission: () => setPermissionDenied(false),
    dismissError: () => setVoiceError(null),
    openPermissionSettings,
    stopVoice,
    handleKeyDown,
    onMicPointerDown,
    onMicPointerUp,
  };
}

export function VoiceMicButton({
  phase,
  mac,
  disabled,
  onPointerDown,
  onPointerUp,
}: {
  phase: VoicePhase;
  mac: boolean;
  disabled?: boolean;
  onPointerDown: () => void;
  onPointerUp: () => void;
}) {
  const { t } = useServices();
  const label = phase === 'recording'
    ? t('composer.voice.stopAndTranscribe')
    : phase === 'transcribing'
      ? t('composer.voice.cancel')
      : t('composer.voice.buttonLabel');
  const tip = phase === 'recording'
    ? t('composer.voice.stopAndTranscribe')
    : t(mac ? 'composer.voice.tooltipMac' : 'composer.voice.tooltipWin');

  return (
    <button
      type="button"
      className={'composer-mic' + (phase === 'recording' ? ' recording' : phase === 'transcribing' ? ' transcribing' : '')}
      data-testid="composer-mic"
      aria-label={label}
      aria-pressed={phase === 'recording'}
      title={tip}
      disabled={disabled}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        onPointerDown();
      }}
      onPointerUp={onPointerUp}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          onPointerDown();
        }
      }}
    >
      <svg className="composer-mic-svg" viewBox="0 0 24 24" width={14} height={14} aria-hidden="true">
        <rect x="9" y="2" width="6" height="12" rx="3" />
        <path d="M5 10a7 7 0 0 0 14 0M12 17v4M8 21h8" />
      </svg>
    </button>
  );
}

export function VoiceStatusBar({
  phase,
  levels,
  partial,
  seconds,
  permissionDenied,
  voiceError,
  onStop,
  onCancel,
  onOpenSettings,
  onDismissPermission,
  onDismissError,
}: {
  phase: VoicePhase;
  levels: readonly number[];
  partial: string;
  seconds: number;
  permissionDenied: boolean;
  voiceError: string | null;
  onStop: () => void;
  onCancel: () => void;
  onOpenSettings: () => void;
  onDismissPermission: () => void;
  onDismissError: () => void;
}) {
  const { t } = useServices();

  if (permissionDenied) {
    return (
      <div className="composer-voice-permission" data-testid="voice-permission-banner" role="alert">
        <Icon name="triangle-alert" size={13} weight={2} />
        <span>{t('composer.voice.permissionGuidance')}</span>
        <button type="button" className="composer-text-button" onClick={onOpenSettings}>
          {t('composer.voice.openSettings')}
        </button>
        <button type="button" className="composer-icon-button" aria-label={t('settings.closeButton')} onClick={onDismissPermission}>
          ×
        </button>
      </div>
    );
  }

  if (voiceError && phase === 'idle') {
    return (
      <div className="composer-voice-error" data-testid="voice-error-banner" role="alert">
        <span>{voiceError}</span>
        <button type="button" className="composer-icon-button" aria-label={t('settings.closeButton')} onClick={onDismissError}>
          ×
        </button>
      </div>
    );
  }

  if (phase === 'idle') return null;

  return (
    <div className={'composer-voice-bar ' + phase} data-testid="composer-voice-bar" role="status" aria-live="polite">
      {phase === 'recording' ? (
        <>
          <span className="composer-voice-dot" aria-hidden="true" />
          <div className="composer-voice-wave" data-testid="voice-waveform" aria-hidden="true">
            {levels.map((lvl, idx) => (
              <span key={idx} className="composer-voice-wave-bar" style={{ height: `${Math.round(lvl * 16)}px` }} />
            ))}
          </div>
          <span className="composer-voice-timer">{formatVoiceDuration(seconds)}</span>
          <span className="composer-voice-partial">{partial || t('composer.voice.listening')}</span>
          <button type="button" className="composer-icon-button" data-testid="voice-cancel" onClick={onCancel}>
            {t('composer.voice.cancel')}
          </button>
          <button type="button" className="composer-text-button" data-testid="voice-finish" onClick={onStop}>
            {t('composer.voice.done')}
          </button>
        </>
      ) : (
        <>
          <span className="guided-spinner" aria-hidden="true" />
          <span className="composer-voice-partial">{partial || t('composer.voice.transcribing')}</span>
          <button type="button" className="composer-text-button" data-testid="voice-cancel" onClick={onCancel}>
            {t('composer.voice.cancel')}
          </button>
        </>
      )}
    </div>
  );
}
