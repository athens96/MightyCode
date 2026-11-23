import { useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { describeError, type MobileClient } from '@/api/client';
import type { MergeWarning, MobilePaneWorktree, PaneWorktreeAction } from '@/api/types';
import { Icon } from '@/components/icons';
import { MergeChecksSheet } from '@/components/merge-checks-sheet';
import { BottomSheet, ConfirmDialog, SheetRow } from '@/components/sheets';
import { Button } from '@/components/ui';
import { t } from '@/lib/i18n';
import { mergeConfirmText, parseWarnings } from '@/lib/merge-checks';
import {
  menuOffers,
  primaryOffers,
  worktreeActions,
  worktreeCounts,
  worktreeLabel,
  worktreeMarker,
  worktreePortBlock,
  worktreeSetup,
  type WorktreeOffer,
} from '@/lib/pane-worktree';
import { showToast } from '@/store/toast';
import { radius, spacing, usePalette, useStyles, type Palette } from '@/theme';

/**
 * The Mac's worktree bar for a pane that works in its own worktree (`MobilePaneWorktree`): its branch → base, PORT, the
 * counts and setup state the Mac read, the main merge button (커밋하고 병합 while there are changes, 충돌 해결 요청 after
 * a conflict) and a menu with the other merge, undo and discard. Every action is the Mac's: the phone sends it and shows
 * the Mac's answer, which the Mac also wrote in the pane. A discard asks first; the Mac refuses it while children branch
 * from the pane. Opening the pane asks the Mac to read the counts again (`refresh`), and so does each settled run.
 *
 * From the Rust host (0.6.0+) it also shows the worktree's port block and the run actions running in it with the address
 * each printed (MC-25; a tap copies the address, which is the computer's `localhost`), offers 점검 (MC-27, "mergeChecks")
 * and, when a merge comes back with what the pre-merge checks still warn about (`confirm`), lists it and asks once more
 * before sending the merge again with `confirmed` — never blocking it.
 */
export function PaneWorktreeBar({
  sessionId,
  worktree,
  client,
  running,
  canCheck = false,
  onDiscarded,
}: {
  sessionId: string;
  worktree: MobilePaneWorktree;
  client: MobileClient | undefined;
  /** The pane's run: counts are read again when it settles. */
  running: boolean;
  /** The host reads the pre-merge checks for the phone ("mergeChecks"). */
  canCheck?: boolean;
  /** The pane is gone after a discard. */
  onDiscarded: () => void;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const [sending, setSending] = useState<PaneWorktreeAction | undefined>(undefined);
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [checksOpen, setChecksOpen] = useState(false);
  /** A merge the host sent back with what still warns: asked once more before it goes again with `confirmed`. */
  const [warned, setWarned] = useState<{ action: PaneWorktreeAction; warnings: MergeWarning[] } | undefined>(undefined);
  const sendingRef = useRef(false);

  // The Mac reads the counts every 20 s while its own bar is on screen; a pane opened only here asks once on open and
  // after each run.
  useEffect(() => {
    if (!client || running) return;
    const controller = new AbortController();
    client.paneWorktree(sessionId, 'refresh', controller.signal).catch(() => undefined);
    return () => controller.abort();
  }, [client, sessionId, running]);

  const perform = useCallback(
    async (action: PaneWorktreeAction, confirmed = false) => {
      if (!client || sendingRef.current) return;
      sendingRef.current = true;
      setSending(action);
      try {
        const result = await client.paneWorktree(sessionId, action, undefined, confirmed);
        const warnings = parseWarnings(result.confirm?.warnings);
        if (!result.done && result.confirm && warnings.length > 0) {
          setWarned({ action, warnings });
          return;
        }
        if (result.message) showToast(result.message, result.done ? 'success' : 'error');
        if (action === 'discard' && result.done) onDiscarded();
      } catch (error) {
        showToast(describeError(error), 'error');
      } finally {
        sendingRef.current = false;
        setSending(undefined);
      }
    },
    [client, sessionId, onDiscarded],
  );

  const choose = (offer: WorktreeOffer) => {
    setMenuOpen(false);
    if (offer.disabledReason) {
      showToast(offer.disabledReason, 'error');
      return;
    }
    if (offer.confirm) setConfirming(true);
    else void perform(offer.action);
  };

  const busy = worktree.busy === true || sending !== undefined;
  const counts = worktreeCounts(worktree);
  const setup = worktreeSetup(worktree);
  const block = worktreePortBlock(worktree);
  const actions = worktreeActions(worktree);
  const copy = (url: string) => {
    void Clipboard.setStringAsync(url).then(() => showToast(t('phone.worktree.addressCopied', { url })));
  };

  return (
    <View accessible={false} style={styles.bar}>
      <View accessible accessibilityLabel={worktreeMarker(worktree)} style={styles.head}>
        <Icon name="branch" color={palette.accent} size={14} />
        <Text numberOfLines={1} ellipsizeMode="middle" style={styles.branch}>
          {worktreeLabel(worktree)}
        </Text>
        <Text style={styles.port}>PORT {worktree.port}</Text>
        {block ? (
          <Text
            accessibilityLabel={t('phone.worktree.portBlock', { range: block })}
            numberOfLines={1}
            style={styles.port}
          >
            {block}
          </Text>
        ) : null}
      </View>
      {counts || setup ? (
        <Text numberOfLines={1} style={styles.counts}>
          {counts}
          {counts && setup ? ' · ' : ''}
          {setup ? <Text style={setup.failed ? styles.failed : undefined}>{setup.text}</Text> : null}
        </Text>
      ) : null}
      {actions.length > 0 ? (
        <View style={styles.runs}>
          {actions.map((action) => (
            <Pressable
              key={action.name}
              accessibilityRole="button"
              accessibilityLabel={t('paneWorktree.bar.actionRunning', { name: action.name })}
              accessibilityHint={action.url}
              disabled={!action.url}
              onPress={() => action.url && copy(action.url)}
              style={({ pressed }) => [styles.run, pressed && styles.pressed]}
            >
              <Text style={styles.runMark}>▶</Text>
              <Text numberOfLines={1} style={styles.runName}>
                {action.name}
              </Text>
              {action.url ? (
                <Text numberOfLines={1} ellipsizeMode="middle" style={styles.runUrl}>
                  {action.url}
                </Text>
              ) : null}
            </Pressable>
          ))}
        </View>
      ) : null}
      <View style={styles.actions}>
        {primaryOffers(worktree).map((offer) => (
          <Button
            key={offer.action}
            label={offer.label}
            compact
            tone={offer.action === 'resolveConflict' ? 'neutral' : 'primary'}
            busy={sending === offer.action}
            disabled={busy || !client}
            onPress={() => choose(offer)}
          />
        ))}
        {canCheck ? (
          <Button label={t('git.checks.tab')} compact tone="neutral" disabled={!client} onPress={() => setChecksOpen(true)} />
        ) : null}
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('paneWorktree.bar.more')}
          disabled={busy || !client}
          hitSlop={6}
          onPress={() => setMenuOpen(true)}
          style={({ pressed }) => [styles.more, pressed && styles.pressed, (busy || !client) && styles.dim]}
        >
          <Text style={styles.moreText}>⋯</Text>
        </Pressable>
      </View>

      <BottomSheet visible={menuOpen} onClose={() => setMenuOpen(false)} title={t('paneWorktree.bar.more')} inset={false}>
        {menuOffers(worktree).map((offer) => (
          <SheetRow
            key={offer.action}
            label={offer.label}
            {...(offer.disabledReason ? { caption: offer.disabledReason } : {})}
            danger={offer.action === 'discard'}
            disabled={offer.disabledReason !== undefined}
            onPress={() => choose(offer)}
          />
        ))}
      </BottomSheet>

      <ConfirmDialog
        visible={confirming}
        title={t('paneWorktree.discard.title')}
        description={t('paneWorktree.discard.detail', { branch: worktree.branch })}
        confirmLabel={t('paneWorktree.discard.confirm')}
        destructive
        busy={sending === 'discard'}
        onConfirm={() => {
          setConfirming(false);
          void perform('discard');
        }}
        onCancel={() => setConfirming(false)}
      />

      <ConfirmDialog
        visible={warned !== undefined}
        title={t('git.checks.merge.title')}
        description={warned ? mergeConfirmText(warned.warnings) : ''}
        confirmLabel={t('git.checks.merge.confirm')}
        busy={warned !== undefined && sending === warned.action}
        onConfirm={() => {
          const again = warned;
          setWarned(undefined);
          if (again) void perform(again.action, true);
        }}
        onCancel={() => setWarned(undefined)}
      />

      {canCheck ? (
        <MergeChecksSheet visible={checksOpen} sessionId={sessionId} client={client} onClose={() => setChecksOpen(false)} />
      ) : null}
    </View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    bar: {
      backgroundColor: palette.accentMuted,
      borderColor: palette.border,
      borderRadius: radius.md,
      borderWidth: 1,
      gap: spacing.sm,
      marginHorizontal: spacing.xl,
      marginTop: spacing.md,
      paddingHorizontal: spacing.xl,
      paddingVertical: spacing.lg,
    },
    head: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    branch: { color: palette.text, flexShrink: 1, fontSize: 13, fontWeight: '600' },
    port: { color: palette.textMuted, fontSize: 12, fontVariant: ['tabular-nums'] },
    counts: { color: palette.textMuted, fontSize: 12 },
    runs: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing.md },
    run: {
      alignItems: 'center',
      borderColor: palette.border,
      borderRadius: radius.round,
      borderWidth: 1,
      flexDirection: 'row',
      gap: spacing.sm,
      maxWidth: '100%',
      paddingHorizontal: spacing.lg,
      paddingVertical: spacing.xs,
    },
    runMark: { color: palette.success, fontSize: 9 },
    runName: { color: palette.text, fontSize: 12, fontWeight: '600' },
    runUrl: { color: palette.textMuted, flexShrink: 1, fontSize: 11.5 },
    failed: { color: palette.danger },
    actions: { alignItems: 'center', flexDirection: 'row', flexWrap: 'wrap', gap: spacing.lg },
    more: {
      alignItems: 'center',
      borderColor: palette.border,
      borderRadius: radius.sm,
      borderWidth: 1,
      justifyContent: 'center',
      paddingHorizontal: spacing.lg,
      paddingVertical: spacing.xs,
    },
    moreText: { color: palette.text, fontSize: 16, fontWeight: '700' },
    pressed: { backgroundColor: palette.surfaceRaised },
    dim: { opacity: 0.5 },
  });
