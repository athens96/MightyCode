import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import * as Haptics from 'expo-haptics';
import { Icon } from '@/components/icons';
import { Button } from '@/components/ui';
import { language, t } from '@/lib/i18n';
import {
  MOBILE_WAVE_BARS,
  applyMobileCodeHints,
  formatMobileVoiceSeconds,
  normalizeMobileWaveform,
  openAppPermissionSettings,
  resolveSpeechDriver,
  speechBcp47,
  type MobileSpeechDriver,
} from '@/lib/voice';
import { radius, spacing, usePalette, useStyles, type Palette } from '@/theme';

export type MobileVoicePhase = 'idle' | 'recording' | 'transcribing';

const HOLD_THRESHOLD_MS = 260;

export function useVoiceDictation({
  disabled = false,
  hints = [],
  onDictated,
}: {
  disabled?: boolean;
  hints?: readonly string[];
  onDictated: (text: string) => void;
}) {
  const [phase, setPhase] = useState<MobileVoicePhase>('idle');
  const [levels, setLevels] = useState<number[]>(() => normalizeMobileWaveform([], MOBILE_WAVE_BARS));
  const [partial, setPartial] = useState('');
  const [seconds, setSeconds] = useState(0);
  const [permissionDenied, setPermissionDenied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const phaseRef = useRef<MobileVoicePhase>('idle');
  phaseRef.current = phase;
  const driverRef = useRef<MobileSpeechDriver | null>(null);
  const holdStartRef = useRef<number | null>(null);
  const hintsRef = useRef(hints);
  hintsRef.current = hints;
  const onDictatedRef = useRef(onDictated);
  onDictatedRef.current = onDictated;

  const resetVisuals = useCallback(() => {
    setLevels(normalizeMobileWaveform([], MOBILE_WAVE_BARS));
    setPartial('');
    setSeconds(0);
  }, []);

  useEffect(() => {
    const driver = resolveSpeechDriver();
    driverRef.current = driver;
    if (!driver) return undefined;
    const sub = driver.addListener((event) => {
      if (event.type === 'recording') {
        setPhase('recording');
        setPermissionDenied(false);
        setError(null);
      } else if (event.type === 'level' && typeof event.level === 'number') {
        const lvl = event.level;
        setLevels((prev) => normalizeMobileWaveform([...prev.slice(-(MOBILE_WAVE_BARS - 1)), lvl], MOBILE_WAVE_BARS));
      } else if (event.type === 'partial') {
        setPartial(applyMobileCodeHints(event.text ?? '', hintsRef.current));
      } else if (event.type === 'transcribing') {
        setPhase('transcribing');
      } else if (event.type === 'result') {
        setPhase('idle');
        resetVisuals();
        const corrected = applyMobileCodeHints(event.text ?? '', hintsRef.current);
        if (corrected.length > 0) onDictatedRef.current(corrected);
      } else if (event.type === 'cancelled') {
        setPhase('idle');
        resetVisuals();
      } else if (event.type === 'permissionDenied') {
        setPhase('idle');
        resetVisuals();
        setPermissionDenied(true);
      } else if (event.type === 'error') {
        setPhase('idle');
        resetVisuals();
        setError(t('composer.voice.errorFailed'));
      }
    });
    return () => {
      sub.remove();
      if (phaseRef.current !== 'idle') {
        void driver.cancel().catch(() => {});
      }
    };
  }, [resetVisuals]);

  useEffect(() => {
    if (phase !== 'recording') return undefined;
    setSeconds(0);
    const timer = setInterval(() => setSeconds((s) => s + 1), 1000);
    return () => clearInterval(timer);
  }, [phase]);

  const start = useCallback(async () => {
    if (disabled || phaseRef.current !== 'idle') return;
    setPermissionDenied(false);
    setError(null);
    resetVisuals();

    const driver = resolveSpeechDriver();
    driverRef.current = driver;
    if (!driver || !driver.isAvailable()) {
      setError(t('composer.voice.unavailable'));
      return;
    }

    const perm = await driver.getPermissionStatus().catch(() => 'denied' as const);
    if (perm === 'denied') {
      setPermissionDenied(true);
      return;
    }
    if (perm === 'undetermined') {
      const asked = await driver.requestPermission().catch(() => 'denied' as const);
      if (asked !== 'granted') {
        setPermissionDenied(true);
        return;
      }
    }

    void Haptics.selectionAsync().catch(() => {});
    setPhase('recording');
    const started = await driver.start(speechBcp47(language()), [...hintsRef.current]).catch(() => false);
    if (!started) {
      setPhase('idle');
    }
  }, [disabled, resetVisuals]);

  const stop = useCallback(async (cancel = false) => {
    const driver = driverRef.current;
    if (!driver || phaseRef.current === 'idle') return;
    if (cancel) {
      setPhase('idle');
      resetVisuals();
      await driver.cancel().catch(() => {});
    } else {
      setPhase('transcribing');
      await driver.stop().catch(() => {
        setPhase('idle');
        resetVisuals();
      });
    }
  }, [resetVisuals]);

  const onPressIn = useCallback(() => {
    if (disabled) return;
    if (phaseRef.current === 'idle') {
      holdStartRef.current = Date.now();
      void start();
    } else if (phaseRef.current === 'recording') {
      holdStartRef.current = null;
      void stop(false);
    } else if (phaseRef.current === 'transcribing') {
      holdStartRef.current = null;
      void stop(true);
    }
  }, [disabled, start, stop]);

  const onPressOut = useCallback(() => {
    if (holdStartRef.current === null) return;
    const held = Date.now() - holdStartRef.current;
    holdStartRef.current = null;
    if (held >= HOLD_THRESHOLD_MS && phaseRef.current === 'recording') {
      void stop(false);
    }
  }, [stop]);

  return {
    phase,
    levels,
    partial,
    seconds,
    permissionDenied,
    error,
    dismissPermission: () => setPermissionDenied(false),
    dismissError: () => setError(null),
    openSettings: () => void openAppPermissionSettings(),
    stop: () => void stop(false),
    cancel: () => void stop(true),
    onPressIn,
    onPressOut,
  };
}

export function VoiceMicButton({
  phase,
  disabled = false,
  compact = false,
  testID = 'voice-mic-button',
  onPressIn,
  onPressOut,
}: {
  phase: MobileVoicePhase;
  disabled?: boolean;
  compact?: boolean;
  testID?: string;
  onPressIn: () => void;
  onPressOut: () => void;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const active = phase === 'recording';
  const transcribing = phase === 'transcribing';
  const label = active
    ? t('composer.voice.stopAndTranscribe')
    : transcribing
      ? t('composer.voice.cancel')
      : t('composer.voice.buttonLabel');

  return (
    <Pressable
      testID={testID}
      accessibilityLabel={label}
      accessibilityRole="button"
      accessibilityState={{ disabled, selected: active, busy: transcribing }}
      disabled={disabled}
      hitSlop={6}
      onPressIn={onPressIn}
      onPressOut={onPressOut}
      style={({ pressed }) => [
        compact ? styles.micCompact : styles.mic,
        active && styles.micRecording,
        transcribing && styles.micTranscribing,
        disabled && styles.micDisabled,
        pressed && styles.pressed,
      ]}
    >
      <Icon
        name="mic"
        color={active ? palette.err : transcribing ? palette.accent : palette.textMuted}
        size={compact ? 16 : 18}
        strokeWidth={2.2}
      />
    </Pressable>
  );
}

export function VoiceDictationBar({
  phase,
  levels,
  partial,
  seconds,
  permissionDenied,
  error,
  onStop,
  onCancel,
  onOpenSettings,
  onDismissPermission,
  onDismissError,
}: {
  phase: MobileVoicePhase;
  levels: readonly number[];
  partial: string;
  seconds: number;
  permissionDenied: boolean;
  error: string | null;
  onStop: () => void;
  onCancel: () => void;
  onOpenSettings: () => void;
  onDismissPermission: () => void;
  onDismissError: () => void;
}) {
  const styles = useStyles(makeStyles);

  if (permissionDenied) {
    return (
      <View style={styles.permissionBox} testID="voice-permission-banner">
        <Text style={styles.permissionText}>{t('composer.voice.permissionGuidance')}</Text>
        <View style={styles.bannerActions}>
          <Button label={t('composer.voice.openSettings')} tone="primary" compact onPress={onOpenSettings} />
          <Button label={t('common.cancel')} tone="ghost" compact onPress={onDismissPermission} />
        </View>
      </View>
    );
  }

  if (error && phase === 'idle') {
    return (
      <View style={styles.errorBox} testID="voice-error-banner">
        <Text style={styles.errorText}>{error}</Text>
        <Button label={t('common.cancel')} tone="ghost" compact onPress={onDismissError} />
      </View>
    );
  }

  if (phase === 'idle') return null;

  return (
    <View style={[styles.bar, phase === 'recording' ? styles.barRecording : styles.barTranscribing]} testID="voice-dictation-bar">
      {phase === 'recording' ? (
        <>
          <View style={styles.dot} />
          <View style={styles.wave} testID="voice-waveform">
            {levels.map((lvl, idx) => (
              <View key={idx} style={[styles.waveBar, { height: Math.max(4, Math.round(lvl * 16)) }]} />
            ))}
          </View>
          <Text style={styles.timer}>{formatMobileVoiceSeconds(seconds)}</Text>
          <Text numberOfLines={1} style={styles.partial}>
            {partial || t('composer.voice.listening')}
          </Text>
          <Button label={t('composer.voice.cancel')} tone="ghost" compact onPress={onCancel} />
          <Button label={t('composer.voice.done')} tone="ink" compact onPress={onStop} />
        </>
      ) : (
        <>
          <Text numberOfLines={1} style={styles.partial}>
            {partial || t('composer.voice.transcribing')}
          </Text>
          <Button label={t('composer.voice.cancel')} tone="ghost" compact onPress={onCancel} />
        </>
      )}
    </View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    mic: {
      alignItems: 'center',
      backgroundColor: palette.surfaceRaised,
      borderColor: palette.border,
      borderRadius: radius.round,
      borderWidth: 1,
      height: 34,
      justifyContent: 'center',
      width: 34,
    },
    micCompact: {
      alignItems: 'center',
      backgroundColor: palette.surfaceRaised,
      borderColor: palette.border,
      borderRadius: radius.round,
      borderWidth: 1,
      height: 30,
      justifyContent: 'center',
      width: 30,
    },
    micRecording: {
      backgroundColor: palette.surface,
      borderColor: palette.err,
    },
    micTranscribing: {
      backgroundColor: palette.accentMuted,
      borderColor: palette.accent,
    },
    micDisabled: { opacity: 0.4 },
    bar: {
      alignItems: 'center',
      backgroundColor: palette.surface,
      borderColor: palette.border,
      borderRadius: radius.md,
      borderWidth: 1,
      flexDirection: 'row',
      gap: spacing.xs,
      paddingHorizontal: spacing.sm,
      paddingVertical: spacing.xs,
    },
    barRecording: { borderColor: palette.err },
    barTranscribing: { borderColor: palette.accent },
    dot: {
      backgroundColor: palette.err,
      borderRadius: radius.round,
      height: 8,
      width: 8,
    },
    wave: {
      alignItems: 'center',
      flexDirection: 'row',
      gap: spacing.xs,
      height: 18,
    },
    waveBar: {
      backgroundColor: palette.err,
      borderRadius: radius.sm,
      width: 2.5,
    },
    timer: {
      color: palette.textMuted,
      fontSize: 11,
      fontVariant: ['tabular-nums'],
    },
    partial: {
      color: palette.text,
      flex: 1,
      fontSize: 12,
    },
    permissionBox: {
      backgroundColor: palette.surface,
      borderColor: palette.wait,
      borderRadius: radius.md,
      borderWidth: 1,
      gap: spacing.xs,
      paddingHorizontal: spacing.md,
      paddingVertical: spacing.sm,
    },
    permissionText: { color: palette.text, fontSize: 12 },
    bannerActions: { alignItems: 'center', flexDirection: 'row', gap: spacing.sm },
    errorBox: {
      alignItems: 'center',
      backgroundColor: palette.surface,
      borderColor: palette.danger,
      borderRadius: radius.md,
      borderWidth: 1,
      flexDirection: 'row',
      gap: spacing.sm,
      paddingHorizontal: spacing.md,
      paddingVertical: spacing.xs,
    },
    errorText: { color: palette.danger, flex: 1, fontSize: 12 },
    pressed: { opacity: 0.65 },
  });
