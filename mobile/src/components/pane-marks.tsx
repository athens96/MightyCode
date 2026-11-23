import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import type { MobileSessionDetail, MobileSessionSummary } from '@/api/types';
import { t } from '@/lib/i18n';
import { forkSource, latestSandboxBlock } from '@/lib/pane-marks';
import { radius, spacing, usePalette, useStyles, type Palette } from '@/theme';

/**
 * The marks under a pane's bar: where a forked pane came from (MC-28, `갈라낸 곳: …`, a tap opens the source while the phone
 * lists it) and the sandbox (MC-34: a 샌드박스 badge on a sandboxed pane and the latest thing it refused). Nothing at all on a
 * pane without them, or from an older host.
 */
export function PaneMarks({
  hostId,
  session,
  detail,
  sessions,
}: {
  hostId: string;
  session: MobileSessionSummary | undefined;
  detail: Pick<MobileSessionDetail, 'sandboxBlocked'> | undefined;
  sessions: readonly MobileSessionSummary[];
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const fork = forkSource(session, sessions);
  const blocked = latestSandboxBlock(detail);
  const boxed = session?.sandboxed === true;
  if (!fork && !blocked && !boxed) return null;
  return (
    <View style={styles.marks}>
      {fork ? (
        <Pressable
          accessibilityRole={fork.open ? 'link' : 'text'}
          accessibilityHint={fork.open ? t('fork.strip.open') : t('fork.strip.closed')}
          disabled={!fork.open}
          onPress={() => router.push(`/host/${hostId}/session/${fork.paneId}`)}
          style={({ pressed }) => [styles.row, pressed && styles.pressed]}
        >
          <Text numberOfLines={1} style={[styles.fork, fork.open && { color: palette.accent }]}>
            {t('phone.session.forkedFrom', { title: fork.title })}
          </Text>
          {!fork.open ? <Text style={styles.muted}>{t('fork.strip.closed')}</Text> : null}
        </Pressable>
      ) : null}
      {boxed || blocked ? (
        <View style={styles.row}>
          {boxed ? (
            <View accessible accessibilityLabel={t('sandbox.badgeHelp')} style={styles.badge}>
              <Text style={styles.badgeText}>{t('sandbox.badge')}</Text>
            </View>
          ) : null}
          {blocked ? (
            <Text numberOfLines={2} style={styles.blocked}>
              {blocked}
            </Text>
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    marks: { gap: spacing.sm, marginHorizontal: spacing.xl, marginTop: spacing.md },
    row: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    pressed: { opacity: 0.7 },
    fork: { color: palette.textMuted, flexShrink: 1, fontSize: 12.5, fontWeight: '600' },
    muted: { color: palette.textFaint, fontSize: 12 },
    badge: {
      backgroundColor: palette.accentMuted,
      borderColor: palette.border,
      borderRadius: radius.round,
      borderWidth: 1,
      paddingHorizontal: spacing.md,
      paddingVertical: spacing.xs,
    },
    badgeText: { color: palette.accent, fontSize: 11, fontWeight: '700' },
    blocked: { color: palette.danger, flex: 1, fontSize: 12, lineHeight: 17 },
  });
