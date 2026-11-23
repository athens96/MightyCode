import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { describeError, type MobileClient } from '@/api/client';
import type { MergeChecks } from '@/api/types';
import { BottomSheet, DialogButton } from '@/components/sheets';
import { t } from '@/lib/i18n';
import { checksSections, warningLine, type ChecksTone } from '@/lib/merge-checks';
import { radius, spacing, toneColors, usePalette, useStyles, type Palette } from '@/theme';

/**
 * MC-27, 병합 전 점검 for a worktree pane: the desktop's 점검 tab read now (`GET /m1/sessions/{id}/checks`, "mergeChecks") and
 * drawn read-only — local changes and how far from its base, the pull request, each CI check, the unresolved review threads and
 * the remaining checklist, each part on its own ("확인할 수 없음" and why). Read when the sheet opens and on 다시 읽기; nothing
 * runs on a timer here.
 */
export function MergeChecksSheet({
  visible,
  sessionId,
  client,
  onClose,
}: {
  visible: boolean;
  sessionId: string;
  client: MobileClient | undefined;
  onClose: () => void;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const [model, setModel] = useState<MergeChecks | undefined>(undefined);
  const [error, setError] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const asking = useRef<AbortController | undefined>(undefined);

  const load = useCallback(() => {
    if (!client) return;
    asking.current?.abort();
    const controller = new AbortController();
    asking.current = controller;
    setLoading(true);
    setError(undefined);
    client
      .mergeChecks(sessionId, controller.signal)
      .then((answer) => {
        if (!controller.signal.aborted) setModel(answer);
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted) setError(describeError(caught));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
  }, [client, sessionId]);

  useEffect(() => {
    if (!visible) return undefined;
    load();
    return () => asking.current?.abort();
  }, [visible, load]);

  const tint = (tone: ChecksTone) => toneColors(palette, tone === 'wait' ? 'wait' : tone === 'err' ? 'err' : 'done').ink;
  const warnings = Array.isArray(model?.warnings) ? model.warnings : [];

  return (
    <BottomSheet
      visible={visible}
      onClose={onClose}
      title={t('phone.checks.title')}
      {...(warnings.length > 0 ? { note: warnings.map(warningLine).join(' · ') } : {})}
      footer={
        <>
          <DialogButton label={t('compare.view.refresh')} busy={loading} disabled={!client} onPress={load} />
          <DialogButton label={t('common.close')} onPress={onClose} />
        </>
      }
    >
      {model ? (
        checksSections(model).map((section) => (
          <View key={section.id} style={styles.section}>
            <View style={styles.head}>
              <View style={[styles.dot, { backgroundColor: tint(section.tone) }]} />
              <Text accessibilityRole="header" style={styles.title}>
                {section.title}
              </Text>
            </View>
            {section.lines.map((line, index) => (
              <Text key={index} numberOfLines={index === 0 ? 2 : 3} style={[styles.line, index === 0 && styles.lead]}>
                {line}
              </Text>
            ))}
          </View>
        ))
      ) : error ? (
        <Text style={styles.error}>{error}</Text>
      ) : (
        <View style={styles.loading}>
          <ActivityIndicator color={palette.textFaint} size="small" />
          <Text style={styles.line}>{t('phone.workspaces.loading')}</Text>
        </View>
      )}
      {model && error ? <Text style={styles.error}>{error}</Text> : null}
    </BottomSheet>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    section: {
      borderColor: palette.border,
      borderRadius: radius.md,
      borderWidth: 1,
      gap: spacing.xs,
      marginBottom: spacing.lg,
      paddingHorizontal: spacing.xl,
      paddingVertical: spacing.lg,
    },
    head: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    dot: { borderRadius: radius.round, height: 8, width: 8 },
    title: { color: palette.text, fontSize: 14, fontWeight: '700' },
    line: { color: palette.textMuted, fontSize: 12.5, lineHeight: 18 },
    lead: { color: palette.text },
    error: { color: palette.danger, fontSize: 13, lineHeight: 18 },
    loading: { alignItems: 'center', flexDirection: 'row', gap: spacing.lg, paddingVertical: spacing.xl },
  });
