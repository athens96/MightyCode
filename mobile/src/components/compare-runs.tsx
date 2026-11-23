import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { describeError, type MobileClient } from '@/api/client';
import type { CompareColumn, CompareView, MergeWarning, MobileCompareRun, MobileSessionSummary } from '@/api/types';
import { Icon } from '@/components/icons';
import { ConfirmDialog, FullScreenPanel } from '@/components/sheets';
import { Button } from '@/components/ui';
import {
  changesWords,
  columnAgent,
  compareRequestLine,
  elapsedWords,
  phaseCounts,
  phaseTone,
  phaseWord,
  summaryColumn,
  testWords,
  usd,
} from '@/lib/compare';
import { t } from '@/lib/i18n';
import { mergeConfirmText, parseWarnings } from '@/lib/merge-checks';
import { showToast } from '@/store/toast';
import { radius, spacing, toneColors, usePalette, useStyles, type Palette } from '@/theme';

/** The most compare runs the pane screen lists (newest first); the desktop keeps the rest. */
const RUNS_SHOWN = 3;

/**
 * MC-29: the compare runs a pane started, as cards under its bar — `비교 실행 · n개`, the request's first line and the children's
 * states — each opening the compare view. Children's states come from their panes' sidebar states until the view is read.
 */
export function CompareRunsCards({
  hostId,
  sessionId,
  runs,
  sessions,
  client,
  canAct,
}: {
  hostId: string;
  sessionId: string;
  runs: MobileCompareRun[];
  sessions: readonly MobileSessionSummary[];
  client: MobileClient | undefined;
  /** The host has the compare route ("compare"): the view is read and its actions offered. */
  canAct: boolean;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useState<MobileCompareRun | undefined>(undefined);
  if (runs.length === 0) return null;
  return (
    <View style={styles.cards}>
      {runs.slice(0, RUNS_SHOWN).map((run) => {
        const columns = run.children.map((child) => summaryColumn(child, sessions));
        const counts = phaseCounts(columns);
        const line = compareRequestLine(run);
        return (
          <Pressable
            key={run.id}
            accessibilityRole="button"
            accessibilityHint={t('compare.card.open')}
            onPress={() => setOpen(run)}
            style={({ pressed }) => [styles.card, pressed && styles.pressed]}
          >
            <View style={styles.cardHead}>
              <Text style={styles.cardTitle}>{t('compare.card.title', { count: run.children.length })}</Text>
              {counts ? (
                <Text numberOfLines={1} style={styles.cardCounts}>
                  {counts}
                </Text>
              ) : null}
              <Icon name="chevronRight" color={palette.textMuted} size={14} />
            </View>
            {line ? (
              <Text numberOfLines={1} style={styles.cardLine}>
                {line}
              </Text>
            ) : null}
          </Pressable>
        );
      })}
      {open ? (
        <ComparePanel
          hostId={hostId}
          sessionId={sessionId}
          run={open}
          sessions={sessions}
          client={client}
          canAct={canAct}
          onClose={() => setOpen(undefined)}
        />
      ) : null}
    </View>
  );
}

type Pending =
  | { kind: 'merge'; column: CompareColumn; warnings: MergeWarning[] }
  | { kind: 'discard'; column: CompareColumn }
  | { kind: 'rest'; merged: CompareColumn; rest: { id: string; label: string }[] };

/**
 * The compare view: one card per child with its agent and model, state, changes, time, cost and test result; 창 보기 opens the
 * child's pane, 병합 (a reported child only) goes through the pre-merge checks — what still warns is asked once more — and after
 * it the other children still open are offered for discard, all at once; 폐기 asks first. The host does the git work.
 */
function ComparePanel({
  hostId,
  sessionId,
  run,
  sessions,
  client,
  canAct,
  onClose,
}: {
  hostId: string;
  sessionId: string;
  run: MobileCompareRun;
  sessions: readonly MobileSessionSummary[];
  client: MobileClient | undefined;
  canAct: boolean;
  onClose: () => void;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const [view, setView] = useState<CompareView | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [working, setWorking] = useState<string | undefined>(undefined);
  const [pending, setPending] = useState<Pending | undefined>(undefined);
  const asking = useRef<AbortController | undefined>(undefined);

  const load = useCallback(() => {
    if (!client || !canAct) return;
    asking.current?.abort();
    const controller = new AbortController();
    asking.current = controller;
    setLoading(true);
    setError(undefined);
    client
      .compareView(sessionId, run.id, controller.signal)
      .then((answer) => {
        if (!controller.signal.aborted) setView(answer);
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted) setError(describeError(caught));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
  }, [client, canAct, sessionId, run.id]);

  useEffect(() => {
    load();
    return () => asking.current?.abort();
  }, [load]);

  const columns: CompareColumn[] =
    view && !view.gone && Array.isArray(view.children)
      ? view.children
      : run.children.map((child) => summaryColumn(child, sessions));
  const testChosen = Boolean(view?.test ?? run.test);

  const act = async (input: { action: 'merge' | 'discard' | 'discardRest'; column?: CompareColumn; ids?: string[]; confirmed?: boolean }) => {
    if (!client || working) return;
    setWorking(input.column?.id ?? input.action);
    try {
      const result = await client.compareAction(sessionId, run.id, {
        action: input.action,
        ...(input.column ? { childId: input.column.id } : {}),
        ...(input.ids ? { ids: input.ids } : {}),
        ...(input.confirmed ? { confirmed: true } : {}),
      });
      const warnings = parseWarnings(result.confirm?.warnings);
      if (input.action === 'merge' && input.column && !result.done && result.confirm && warnings.length > 0) {
        setPending({ kind: 'merge', column: input.column, warnings });
        return;
      }
      if (result.message) showToast(result.message, result.done ? 'success' : 'error');
      const rest = Array.isArray(result.rest) ? result.rest.filter((entry) => typeof entry?.id === 'string') : [];
      if (input.action === 'merge' && input.column && result.done && rest.length > 0) {
        setPending({ kind: 'rest', merged: input.column, rest });
      }
    } catch (caught) {
      showToast(describeError(caught), 'error');
    } finally {
      setWorking(undefined);
      load();
    }
  };

  const confirmText = (): { title: string; description: string; confirm: string; cancel?: string; destructive: boolean } => {
    if (!pending) return { title: '', description: '', confirm: '', destructive: false };
    if (pending.kind === 'merge') {
      return {
        title: t('git.checks.merge.title'),
        description: mergeConfirmText(pending.warnings),
        confirm: t('git.checks.merge.confirm'),
        destructive: false,
      };
    }
    if (pending.kind === 'discard') {
      return {
        title: t('phone.compare.discardTitle'),
        description: t('phone.compare.discardDetail', { agent: columnAgent(pending.column) }),
        confirm: t('compare.action.discard'),
        destructive: true,
      };
    }
    return {
      title: t('compare.rest.title'),
      description: t('compare.rest.message', {
        agent: pending.merged.agentLabel || pending.merged.agent,
        count: pending.rest.length,
        names: pending.rest.map((entry) => entry.label).join(', '),
      }),
      confirm: t('compare.rest.confirm'),
      cancel: t('compare.rest.later'),
      destructive: true,
    };
  };
  const words = confirmText();

  const head = [
    view?.baseShort ? t('compare.view.base', { base: view.baseShort }) + (view.baseBranch ? ` · ${view.baseBranch}` : '') : undefined,
    view?.testName ? t('compare.view.testName', { name: view.testName }) : undefined,
  ].filter((part): part is string => part !== undefined);

  return (
    <FullScreenPanel
      visible
      title={t('compare.view.title')}
      onClose={onClose}
      actions={
        canAct ? (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('compare.view.refresh')}
            disabled={loading}
            hitSlop={8}
            onPress={load}
            style={styles.refresh}
          >
            {loading ? <ActivityIndicator color={palette.textFaint} size="small" /> : <Text style={styles.refreshText}>↻</Text>}
          </Pressable>
        ) : undefined
      }
    >
      <ScrollView contentContainerStyle={styles.body}>
        <Text numberOfLines={3} style={styles.request}>
          {compareRequestLine(view?.request ? { request: view.request } : run)}
        </Text>
        {head.length > 0 ? <Text style={styles.meta}>{head.join(' · ')}</Text> : null}
        {view?.gone ? <Text style={styles.error}>{t('compare.view.gone')}</Text> : null}
        {error ? <Text style={styles.error}>{error}</Text> : null}
        {!canAct ? <Text style={styles.meta}>{t('phone.compare.desktopOnly')}</Text> : null}
        {columns.map((column) => {
          const tone = toneColors(palette, phaseTone(String(column.phase)));
          const listed = sessions.some((session) => session.id === column.id);
          return (
            <View key={column.id} style={styles.column}>
              <View style={styles.columnHead}>
                <Text numberOfLines={1} style={styles.agent}>
                  {columnAgent(column)}
                </Text>
                <View style={[styles.phase, { backgroundColor: tone.soft }]}>
                  <Text style={[styles.phaseText, { color: tone.ink }]}>{phaseWord(String(column.phase))}</Text>
                </View>
              </View>
              {column.failure ? (
                <Text numberOfLines={2} style={styles.error}>
                  {column.failure}
                </Text>
              ) : null}
              <Facts
                rows={[
                  [t('compare.view.changes'), changesWords(column)],
                  [t('compare.view.elapsed'), elapsedWords(column.elapsed)],
                  [t('compare.view.cost'), usd(column.costUsd)],
                  [t('compare.view.test'), testWords(column.test?.state, column.test?.exitCode, testChosen)],
                ]}
              />
              <View style={styles.buttons}>
                <Button
                  label={t('compare.action.open')}
                  compact
                  disabled={!listed}
                  onPress={() => {
                    onClose();
                    router.push(`/host/${hostId}/session/${column.id}`);
                  }}
                />
                {canAct ? (
                  <Button
                    label={t('compare.action.merge')}
                    compact
                    tone="primary"
                    dimmed={column.canMerge !== true}
                    busy={working === column.id && pending === undefined}
                    disabled={working !== undefined}
                    onPress={() =>
                      column.canMerge === true
                        ? void act({ action: 'merge', column })
                        : showToast(t('compare.action.mergeHelp'))
                    }
                  />
                ) : null}
                {canAct && column.canDiscard === true ? (
                  <Button
                    label={t('compare.action.discard')}
                    compact
                    tone="danger"
                    disabled={working !== undefined}
                    onPress={() => setPending({ kind: 'discard', column })}
                  />
                ) : null}
              </View>
            </View>
          );
        })}
      </ScrollView>
      <ConfirmDialog
        visible={pending !== undefined}
        title={words.title}
        description={words.description}
        confirmLabel={words.confirm}
        {...(words.cancel ? { cancelLabel: words.cancel } : {})}
        destructive={words.destructive}
        busy={working !== undefined}
        onConfirm={() => {
          const chosen = pending;
          setPending(undefined);
          if (!chosen) return;
          if (chosen.kind === 'merge') void act({ action: 'merge', column: chosen.column, confirmed: true });
          else if (chosen.kind === 'discard') void act({ action: 'discard', column: chosen.column });
          else void act({ action: 'discardRest', ids: chosen.rest.map((entry) => entry.id) });
        }}
        onCancel={() => setPending(undefined)}
      />
    </FullScreenPanel>
  );
}

function Facts({ rows }: { rows: [string, string][] }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.facts}>
      {rows.map(([label, value]) => (
        <View key={label} style={styles.fact}>
          <Text style={styles.factLabel}>{label}</Text>
          <Text numberOfLines={1} style={styles.factValue}>
            {value}
          </Text>
        </View>
      ))}
    </View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    cards: { gap: spacing.md, marginHorizontal: spacing.xl, marginTop: spacing.md },
    card: {
      backgroundColor: palette.surface,
      borderColor: palette.border,
      borderRadius: radius.md,
      borderWidth: 1,
      gap: spacing.xs,
      paddingHorizontal: spacing.xl,
      paddingVertical: spacing.lg,
    },
    pressed: { backgroundColor: palette.surfaceRaised },
    cardHead: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    cardTitle: { color: palette.text, fontSize: 13, fontWeight: '700' },
    cardCounts: { color: palette.textMuted, flex: 1, fontSize: 12 },
    cardLine: { color: palette.textMuted, fontSize: 12 },
    refresh: { alignItems: 'center', justifyContent: 'center', paddingHorizontal: spacing.lg },
    refreshText: { color: palette.accent, fontSize: 20, fontWeight: '600' },
    body: { gap: spacing.lg, padding: spacing.xl },
    request: { color: palette.text, fontSize: 14, fontWeight: '600', lineHeight: 20 },
    meta: { color: palette.textMuted, fontSize: 12 },
    error: { color: palette.danger, fontSize: 12.5, lineHeight: 18 },
    column: {
      backgroundColor: palette.surface,
      borderColor: palette.border,
      borderRadius: radius.md,
      borderWidth: 1,
      gap: spacing.md,
      paddingHorizontal: spacing.xl,
      paddingVertical: spacing.lg,
    },
    columnHead: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    agent: { color: palette.text, flex: 1, fontSize: 14, fontWeight: '700' },
    phase: { borderRadius: radius.round, paddingHorizontal: spacing.md, paddingVertical: spacing.xs },
    phaseText: { fontSize: 11.5, fontWeight: '700' },
    facts: { gap: spacing.xs },
    fact: { flexDirection: 'row', gap: spacing.lg },
    factLabel: { color: palette.textMuted, fontSize: 12, minWidth: 64 },
    factValue: { color: palette.text, flex: 1, fontSize: 12, fontVariant: ['tabular-nums'] },
    buttons: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing.lg },
  });
