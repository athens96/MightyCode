import { useEffect, useRef, useState } from 'react';
import { Animated, Easing, StyleSheet, Text, TextInput, View } from 'react-native';
import type { QueuedItem, QueuePlace } from '@/api/types';
import { Button } from '@/components/ui';
import { t } from '@/lib/i18n';
import { editedQueueWords, queueMovePlace, queueRowEdits, runNowOf, runNowReasonKey } from '@/lib/queue';
import { monoText, motion, radius, spacing, useStyles, usePalette, type Palette } from '@/theme';

/** How long a queued send's flash waits for its row to be listed. */
const FLASH_WAIT_MS = 5000;

/**
 * The pane's queue. Removing an item and 바로 실행 are offered only on a host that advertised
 * "queue"; otherwise the list reads exactly as it did before. 바로 실행 sits on each row the host
 * sent a `runNow` for (an old host sends none, so it shows none): the host runs the row in a new
 * worktree child or in this pane. A row that must wait is dimmed, and a tap says why. On a host
 * that also advertised "queueEdit" (MC-49) a row's words are edited in place (저장 or 취소; words
 * none are refused) and ↑/↓ move it; a held row does neither, an allowed card's follow-up only moves.
 */
export function QueuedList({
  items,
  manageable,
  busy,
  flash = 0,
  onRemove,
  onRunNow,
  onExplain,
  editable = false,
  onEdit,
  onMove,
}: {
  items: QueuedItem[];
  manageable: boolean;
  /** MC-49: the host advertised "queueEdit". */
  editable?: boolean;
  /** 저장 on an edited row: true when the host took the words (the editor closes), false to keep it open. */
  onEdit?: (item: QueuedItem, text: string) => Promise<boolean>;
  /** ↑/↓ on a row: its new place. */
  onMove?: (item: QueuedItem, place: QueuePlace) => void;
  busy: boolean;
  /**
   * Bumped when a send from this phone was queued (natural submit B3): the list washes in
   * the run tint and fades back over `motion.highlight`. A colour change, not movement, so
   * Reduce Motion keeps it. 0 never flashes.
   */
  flash?: number;
  onRemove: (itemId: string) => void;
  /** 바로 실행 on a row the host can run now. */
  onRunNow: (item: QueuedItem) => void;
  /** A tap on a dimmed 바로 실행: the words that say why it must wait. */
  onExplain: (text: string) => void;
}) {
  const styles = useStyles(makeStyles);
  const palette = usePalette();
  const wash = useRef(new Animated.Value(0)).current;
  const [editing, setEditing] = useState<{ id: string; text: string } | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const save = async (item: QueuedItem) => {
    const words = editing ? editedQueueWords(editing.text) : undefined;
    if (!editing || words === undefined || !onEdit || saving) return;
    if (words === item.text) {
      setEditing(undefined);
      return;
    }
    setSaving(true);
    try {
      if (await onEdit(item, words)) setEditing(undefined);
    } finally {
      setSaving(false);
    }
  };
  // The answer that said "queued" can come before the poll that lists the item, so a flash
  // waits for a row to show it on, for a few seconds at most.
  const owed = useRef<number | undefined>(undefined);
  const running = useRef<Animated.CompositeAnimation | undefined>(undefined);
  useEffect(() => {
    if (flash !== 0) owed.current = Date.now();
  }, [flash]);
  useEffect(() => {
    const since = owed.current;
    if (since === undefined || items.length === 0) return;
    owed.current = undefined;
    if (Date.now() - since > FLASH_WAIT_MS) return;
    running.current?.stop();
    wash.setValue(1);
    running.current = Animated.timing(wash, {
      toValue: 0,
      duration: motion.highlight,
      easing: Easing.bezier(...motion.easing),
      useNativeDriver: false,
    });
    running.current.start();
  }, [flash, items.length, wash]);
  useEffect(() => () => running.current?.stop(), []);
  if (items.length === 0) return null;
  const backgroundColor = wash.interpolate({ inputRange: [0, 1], outputRange: [palette.surface, palette.runSoft] });
  return (
    <Animated.View style={[styles.wrap, { backgroundColor }]}>
      <View style={styles.header}>
        <Text style={styles.title}>{t('phone.queue.title', { count: items.length })}</Text>
      </View>
      {items.map((item, index) => {
        const runNow = runNowOf(item);
        const edits = queueRowEdits(item, manageable && editable);
        if (editing?.id === item.id) {
          const empty = editedQueueWords(editing.text) === undefined;
          return (
            <View key={item.id} style={styles.editing}>
              <TextInput
                autoFocus
                multiline
                value={editing.text}
                onChangeText={(text) => setEditing({ id: item.id, text })}
                accessibilityLabel={t('queue.edit')}
                placeholderTextColor={palette.textFaint}
                style={styles.input}
                editable={!saving}
              />
              {empty ? <Text style={styles.refusal}>{t('queue.editEmpty')}</Text> : null}
              <View style={styles.row}>
                <View style={styles.spacer} />
                <Button label={t('common.cancel')} tone="ghost" compact disabled={saving} onPress={() => setEditing(undefined)} />
                <Button label={t('queue.editSave')} tone="neutral" compact disabled={empty} busy={saving} onPress={() => void save(item)} />
              </View>
            </View>
          );
        }
        const up = edits.move ? queueMovePlace(items, index, 'up') : undefined;
        const down = edits.move ? queueMovePlace(items, index, 'down') : undefined;
        return (
          <View key={item.id} style={styles.row}>
            <Text numberOfLines={2} style={styles.text}>
              · {item.text}
            </Text>
            {edits.edit && onEdit ? (
              <Button
                label={t('queue.edit')}
                accessibilityLabel={`${t('queue.edit')}: ${item.text.slice(0, 30)}`}
                tone="ghost"
                compact
                busy={busy}
                onPress={() => setEditing({ id: item.id, text: item.text })}
              />
            ) : null}
            {edits.move && onMove ? (
              <Button
                label="↑"
                accessibilityLabel={`${t('queue.moveUp')}: ${item.text.slice(0, 30)}`}
                tone="ghost"
                compact
                busy={busy}
                disabled={!up}
                onPress={() => up && onMove(item, up)}
              />
            ) : null}
            {edits.move && onMove ? (
              <Button
                label="↓"
                accessibilityLabel={`${t('queue.moveDown')}: ${item.text.slice(0, 30)}`}
                tone="ghost"
                compact
                busy={busy}
                disabled={!down}
                onPress={() => down && onMove(item, down)}
              />
            ) : null}
            {manageable && runNow ? (
              <Button
                label={t('queue.runNow')}
                accessibilityLabel={t('phone.queue.runNowLabel', { text: item.text.slice(0, 30) })}
                accessibilityHint={
                  runNow.kind === 'unavailable'
                    ? t(runNowReasonKey(runNow.reason))
                    : t(runNow.kind === 'worktree' ? 'phone.queue.runNowWorktreeHint' : 'queue.runNowHereHelp')
                }
                tone="neutral"
                compact
                dimmed={runNow.kind === 'unavailable'}
                busy={busy && runNow.kind !== 'unavailable'}
                onPress={() => (runNow.kind === 'unavailable' ? onExplain(t(runNowReasonKey(runNow.reason))) : onRunNow(item))}
              />
            ) : null}
            {manageable && item.held !== true ? (
              <Button
                label={t('phone.queue.remove')}
                accessibilityLabel={t('phone.queue.removeLabel', { text: item.text.slice(0, 30) })}
                tone="ghost"
                compact
                busy={busy}
                onPress={() => onRemove(item.id)}
              />
            ) : null}
          </View>
        );
      })}
    </Animated.View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    wrap: {
      backgroundColor: palette.surface,
      borderRadius: radius.lg,
      gap: spacing.xs,
      padding: spacing.md,
    },
    header: { alignItems: 'center', flexDirection: 'row', gap: spacing.sm },
    title: { color: palette.textMuted, flex: 1, fontSize: 13, fontWeight: '600' },
    row: { alignItems: 'center', flexDirection: 'row', gap: spacing.sm },
    text: { ...monoText, color: palette.textFaint, flex: 1 },
    editing: { gap: spacing.xs },
    spacer: { flex: 1 },
    input: {
      ...monoText,
      backgroundColor: palette.surfaceRaised,
      borderColor: palette.border,
      borderRadius: radius.md,
      borderWidth: StyleSheet.hairlineWidth,
      color: palette.text,
      maxHeight: 160,
      minHeight: 44,
      paddingHorizontal: spacing.md,
      paddingVertical: spacing.sm,
    },
    refusal: { color: palette.danger, fontSize: 12 },
  });
