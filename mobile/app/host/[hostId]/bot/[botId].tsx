import { useMemo, useState } from 'react';
import { RefreshControl, ScrollView, StyleSheet, Text, View } from 'react-native';
import { Stack, useLocalSearchParams } from 'expo-router';
import { describeError } from '@/api/client';
import type { BotVerb, MobileBot } from '@/api/types';
import { BotsPoller } from '@/components/bots-poller';
import { ReportRow, openBotPane } from '@/components/bots';
import { StatusDot } from '@/components/status-dot';
import { BetaBadge, Button, EmptyState, ErrorBanner } from '@/components/ui';
import { useNow } from '@/hooks/use-now';
import { BOT_STATE_KEYS, botTone, reportsOf, statusLine, timeWords } from '@/lib/bots';
import { language, t } from '@/lib/i18n';
import { refreshBots, useBotsStore } from '@/store/bots';
import { useHostsStore } from '@/store/hosts';
import { useHostClient, useHostState } from '@/store/live';
import { showToast } from '@/store/toast';
import { layout, radius, spacing, toneColors, useStyles, usePalette, type Palette } from '@/theme';

/** How often the bot's screen reads its host's bots again (the tabs' list poll does not run under it). */
const REFRESH_MS = 10_000;

/** The bot's own actions: 지금 실행 (a button per job when it has several), 재우기/깨우기, 창 열기. */
function BotActions({ hostId, bot }: { hostId: string; bot: MobileBot }) {
  const styles = useStyles(makeStyles);
  const client = useHostClient(hostId);
  const [busy, setBusy] = useState<string | null>(null);

  const act = (key: string, verb: BotVerb, job: string | undefined, done: string) => {
    if (!client || busy) return;
    setBusy(key);
    client
      .botAction(bot.handle, verb, job)
      .then(() => showToast(done, 'success'))
      .catch((error: unknown) => showToast(describeError(error), 'error'))
      .finally(() => {
        setBusy(null);
        refreshBots(hostId);
      });
  };

  const ran = t('phone.bots.toast.ran', { handle: bot.handle });
  return (
    <View style={styles.actionsBlock}>
      {bot.jobNames.length <= 1 ? (
        <View style={styles.actions}>
          <Button
            compact
            tone="primary"
            label={t('automation.jobs.runNow')}
            busy={busy === 'run'}
            disabled={!client || busy !== null || bot.jobNames.length === 0}
            onPress={() => act('run', 'run', bot.jobNames[0], ran)}
          />
          {bot.asleep ? (
            <Button
              compact
              label={t('bot.menu.wake')}
              busy={busy === 'wake'}
              disabled={!client || busy !== null}
              onPress={() => act('wake', 'wake', undefined, t('phone.bots.toast.awake', { handle: bot.handle }))}
            />
          ) : (
            <Button
              compact
              label={t('bot.menu.sleep')}
              busy={busy === 'sleep'}
              disabled={!client || busy !== null}
              onPress={() => act('sleep', 'sleep', undefined, t('phone.bots.toast.asleep', { handle: bot.handle }))}
            />
          )}
          <Button compact label={t('bot.report.open')} onPress={() => openBotPane(hostId, bot.id)} />
        </View>
      ) : (
        <>
          <Text style={styles.label}>{t('phone.bots.detail.pickJob')}</Text>
          <View style={styles.actions}>
            {bot.jobNames.map((name) => (
              <Button
                key={name}
                compact
                tone="primary"
                label={t('phone.bots.detail.runJob', { job: name })}
                busy={busy === `run:${name}`}
                disabled={!client || busy !== null}
                onPress={() => act(`run:${name}`, 'run', name, ran)}
              />
            ))}
          </View>
          <View style={styles.actions}>
            <Button
              compact
              label={t(bot.asleep ? 'bot.menu.wake' : 'bot.menu.sleep')}
              busy={busy === 'sleep' || busy === 'wake'}
              disabled={!client || busy !== null}
              onPress={() =>
                bot.asleep
                  ? act('wake', 'wake', undefined, t('phone.bots.toast.awake', { handle: bot.handle }))
                  : act('sleep', 'sleep', undefined, t('phone.bots.toast.asleep', { handle: bot.handle }))
              }
            />
            <Button compact label={t('bot.report.open')} onPress={() => openBotPane(hostId, bot.id)} />
          </View>
        </>
      )}
      {bot.jobNames.length === 0 ? <Text style={styles.hint}>{t('phone.bots.detail.noJobs')}</Text> : null}
    </View>
  );
}

/**
 * One bot (MC-107): its state dot and word, @name, workspace, what it is doing and its next run, then 지금 실행, 재우기/깨우기 and
 * 창 열기, and its recent reports (opening one marks it read).
 */
export default function BotScreen() {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const { hostId, botId } = useLocalSearchParams<{ hostId: string; botId: string }>();
  const host = useHostsStore((state) => state.hosts.find((entry) => entry.id === hostId));
  const state = useHostState(hostId);
  const entry = useBotsStore((store) => (hostId ? store.hosts[hostId] : undefined));
  const bot = entry?.data?.bots.find((b) => b.id === botId);
  const reports = useMemo(() => (bot ? reportsOf(entry?.data?.reports ?? [], bot.handle) : []), [bot, entry?.data?.reports]);
  const now = useNow(60_000);
  const hostName = state?.hostName || host?.name || '';

  if (!hostId || !host) {
    return <EmptyState title={t('phone.workspaces.hostNotFound.title')} icon="hosts" />;
  }

  const tone = bot ? botTone(bot.state) : 'idle';
  const next = bot ? timeWords(bot.nextRun, new Date(now), language()) : undefined;
  return (
    <>
      <Stack.Screen options={{ title: bot ? `@${bot.handle}` : t('phone.bots.title') }} />
      <BotsPoller hostId={hostId} everyMs={REFRESH_MS} />
      <ScrollView
        style={styles.screen}
        contentContainerStyle={styles.content}
        refreshControl={<RefreshControl refreshing={entry?.loading === true} onRefresh={() => refreshBots(hostId)} />}
      >
        {entry?.unsupported ? (
          <Text style={styles.note}>{t('phone.bots.update', { host: hostName })}</Text>
        ) : !bot ? (
          entry?.data ? (
            <EmptyState title={t('phone.bots.detail.gone')} icon="bots" />
          ) : entry?.error ? (
            <ErrorBanner message={entry.error} />
          ) : (
            <Text style={styles.note}>{t('phone.bots.loading')}</Text>
          )
        ) : (
          <>
            <View style={styles.card}>
              <View style={styles.head}>
                <StatusDot tone={tone} />
                <Text numberOfLines={1} style={styles.handle}>
                  @{bot.handle}
                </Text>
                <BetaBadge />
                <View style={styles.spacer} />
                <Text style={[styles.stateWord, { color: toneColors(palette, tone).ink }]}>
                  {t(BOT_STATE_KEYS[bot.state])}
                </Text>
              </View>
              <Text style={styles.meta}>
                {[
                  bot.kind === 'workspace' ? bot.workspace : t('bot.sidebar.independent'),
                  hostName,
                  bot.jobs > 0 ? t('bot.sidebar.jobs', { count: bot.jobs }) : undefined,
                ]
                  .filter(Boolean)
                  .join(' · ')}
              </Text>
              <Text style={styles.line}>{statusLine(bot, next, t)}</Text>
              {next && bot.state !== 'idle' ? <Text style={styles.meta}>{t('bot.sidebar.nextRun', { at: next })}</Text> : null}
              {entry?.error ? <ErrorBanner message={entry.error} /> : null}
              <BotActions hostId={hostId} bot={bot} />
            </View>
            <Text accessibilityRole="header" style={styles.groupTitle}>
              {t('phone.bots.detail.reports')}
            </Text>
            {reports.length === 0 ? <Text style={styles.note}>{t('phone.bots.detail.noReports')}</Text> : null}
            <View>
              {reports.map((report, index) => (
                <ReportRow
                  key={report.id}
                  hostId={hostId}
                  report={report}
                  first={index === 0}
                  last={index === reports.length - 1}
                  showHandle={false}
                />
              ))}
            </View>
          </>
        )}
      </ScrollView>
    </>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    screen: { backgroundColor: palette.background, flex: 1 },
    content: { gap: spacing.lg, padding: layout.gutter, paddingBottom: spacing.xxl * 2 },
    card: {
      backgroundColor: palette.surface,
      borderColor: palette.border,
      borderRadius: radius.card,
      borderWidth: StyleSheet.hairlineWidth,
      gap: spacing.md,
      padding: spacing.xl,
    },
    head: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    handle: { color: palette.text, flexShrink: 1, fontSize: 18, fontWeight: '700' },
    spacer: { flex: 1 },
    stateWord: { fontSize: 13, fontWeight: '700' },
    meta: { color: palette.textMuted, fontSize: 12.5, lineHeight: 17 },
    line: { color: palette.text, fontSize: 14, lineHeight: 20 },
    actionsBlock: { gap: spacing.md, marginTop: spacing.sm },
    actions: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing.md },
    label: { color: palette.textFaint, fontSize: 12, fontWeight: '700' },
    hint: { color: palette.textMuted, fontSize: 12.5, lineHeight: 17 },
    groupTitle: {
      color: palette.textFaint,
      fontSize: 12,
      fontWeight: '700',
      letterSpacing: 0.3,
      paddingHorizontal: spacing.xs,
      paddingTop: spacing.md,
    },
    note: { color: palette.textMuted, fontSize: 13.5, lineHeight: 19, paddingHorizontal: spacing.xs },
  });
