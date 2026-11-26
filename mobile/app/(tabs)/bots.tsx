import { useEffect, useMemo, useState } from 'react';
import { RefreshControl, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { ApprovalCard, BotRow, ReportRow } from '@/components/bots';
import { BetaBadge, EmptyState, ErrorBanner, ScreenTitle, SegmentedControl } from '@/components/ui';
import { groupByKind } from '@/lib/bots';
import { t } from '@/lib/i18n';
import { refreshBots, useBotsStore, type HostBots } from '@/store/bots';
import { useDashboardStore } from '@/store/dashboard';
import { useHostsStore, type PairedHost } from '@/store/hosts';
import { useLiveStore } from '@/store/live';
import { layout, spacing, useStyles, type Palette } from '@/theme';

type BotsView = 'bots' | 'reports';

const KIND_KEYS = {
  independent: 'bot.sidebar.otherBots',
  workspace: 'bot.sidebar.workspaceBots',
} as const;

/** One host's name over its part when several computers are paired (the 현황 tab's way). */
function HostHeader({ name }: { name: string }) {
  const styles = useStyles(makeStyles);
  return (
    <Text accessibilityRole="header" numberOfLines={1} style={styles.hostName}>
      {name}
    </Text>
  );
}

/** What stands in for a host's bots while there are none to draw: the update note, the error, loading or empty. */
function HostNote({ hostId, entry, name }: { hostId: string; entry: HostBots | undefined; name: string }) {
  const styles = useStyles(makeStyles);
  // The pane list's poll knows first when the computer cannot be reached.
  const pollError = useDashboardStore((store) => store.polls[hostId]?.error);
  if (entry?.unsupported) return <Text style={styles.note}>{t('phone.bots.update', { host: name })}</Text>;
  if (entry?.error && !entry.data) return <ErrorBanner message={entry.error} />;
  if (pollError && !entry?.data) return <ErrorBanner message={pollError} />;
  if (!entry?.data) return <Text style={styles.note}>{t('phone.bots.loading')}</Text>;
  return null;
}

/** One host's bots: 독립 봇 then 워크스페이스 봇, each in state order. */
function HostBotsSection({ host, name, several }: { host: PairedHost; name: string; several: boolean }) {
  const styles = useStyles(makeStyles);
  const entry = useBotsStore((store) => store.hosts[host.id]);
  const groups = useMemo(() => groupByKind(entry?.data?.bots ?? []), [entry?.data?.bots]);
  return (
    <View style={styles.section}>
      {several ? <HostHeader name={name} /> : null}
      <HostNote hostId={host.id} entry={entry} name={name} />
      {entry?.data && entry.error ? <ErrorBanner message={entry.error} /> : null}
      {entry?.data && groups.length === 0 ? <Text style={styles.note}>{t('phone.bots.empty.host')}</Text> : null}
      {groups.map((group) => (
        <View key={group.kind} style={styles.group}>
          <Text accessibilityRole="header" style={styles.groupTitle}>
            {t(KIND_KEYS[group.kind])}
          </Text>
          <View>
            {group.bots.map((bot, index) => (
              <BotRow key={bot.id} hostId={host.id} bot={bot} first={index === 0} last={index === group.bots.length - 1} />
            ))}
          </View>
        </View>
      ))}
    </View>
  );
}

/** One host's reports, the newest first; opening one marks it read. */
function HostReportsSection({ host, name, several }: { host: PairedHost; name: string; several: boolean }) {
  const styles = useStyles(makeStyles);
  const entry = useBotsStore((store) => store.hosts[host.id]);
  const reports = entry?.data?.reports ?? [];
  return (
    <View style={styles.section}>
      {several ? <HostHeader name={name} /> : null}
      <HostNote hostId={host.id} entry={entry} name={name} />
      {entry?.data && reports.length === 0 ? <Text style={styles.note}>{t('bot.report.empty')}</Text> : null}
      <View>
        {reports.map((report, index) => (
          <ReportRow key={report.id} hostId={host.id} report={report} first={index === 0} last={index === reports.length - 1} />
        ))}
      </View>
    </View>
  );
}

/**
 * "봇" (MC-107, beta): every paired computer's bots at once, as the desktop's 봇 tab lists them — the approvals waiting on top
 * (실행 / 건너뛰기), then per computer 독립 봇 and 워크스페이스 봇 with each bot's state, line and last report; 봇 알림 lists the
 * reports. A bot opens its own screen (its reports, 지금 실행, 재우기/깨우기, 창 열기). The tab badge counts the approvals and the
 * bots waiting.
 */
export default function BotsTab() {
  const styles = useStyles(makeStyles);
  const insets = useSafeAreaInsets();
  const params = useLocalSearchParams<{ view?: string }>();
  const hosts = useHostsStore((store) => store.hosts);
  const loaded = useHostsStore((store) => store.loaded);
  const states = useLiveStore((store) => store.states);
  const byHost = useBotsStore((store) => store.hosts);
  const [view, setView] = useState<BotsView>(params.view === 'reports' ? 'reports' : 'bots');

  // A push about a report opens 봇 알림; one about an approval, the bots with 승인 대기 on top.
  useEffect(() => {
    if (params.view === 'reports' || params.view === 'bots') setView(params.view);
  }, [params.view]);

  const names = useMemo(
    () => Object.fromEntries(hosts.map((host) => [host.id, states[host.id]?.hostName || host.name])),
    [hosts, states],
  );
  const approvals = useMemo(
    () => hosts.flatMap((host) => (byHost[host.id]?.data?.approvals ?? []).map((approval) => ({ host, approval }))),
    [byHost, hosts],
  );
  const unread = useMemo(() => hosts.reduce((total, host) => total + (byHost[host.id]?.data?.unread ?? 0), 0), [byHost, hosts]);
  const refreshing = hosts.some((host) => byHost[host.id]?.loading === true);
  const several = hosts.length > 1;

  return (
    <View style={styles.screen}>
      <ScrollView
        contentContainerStyle={[styles.content, { paddingTop: insets.top + spacing.sm, paddingBottom: spacing.xxl }]}
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={() => hosts.forEach((host) => refreshBots(host.id))} />
        }
      >
        <View style={styles.titleRow}>
          <ScreenTitle title={t('phone.bots.title')} />
          <BetaBadge />
        </View>
        {hosts.length > 0 ? (
          <SegmentedControl
            options={[
              { id: 'bots', label: t('phone.bots.view.bots') },
              {
                id: 'reports',
                label: unread > 0 ? t('phone.bots.view.reportsCount', { count: unread }) : t('phone.bots.view.reports'),
              },
            ]}
            value={view}
            onChange={setView}
          />
        ) : null}

        {view === 'bots' && approvals.length > 0 ? (
          <View style={styles.section} testID="bot-approvals">
            <Text accessibilityRole="header" style={styles.groupTitle}>
              {t('automation.approvals.title')}
            </Text>
            {approvals.map(({ host, approval }) => (
              <View key={`${host.id}:${approval.id}`} style={styles.approval}>
                {several ? <Text style={styles.approvalHost}>{names[host.id]}</Text> : null}
                <ApprovalCard hostId={host.id} hostName={names[host.id] ?? host.name} approval={approval} />
              </View>
            ))}
          </View>
        ) : null}

        {hosts.length === 0 ? (
          loaded ? (
            <EmptyState
              title={t('phone.hosts.empty.title')}
              description={t('phone.bots.empty.description')}
              icon="bots"
              action={{ label: t('phone.hosts.addHost'), onPress: () => router.push('/pair') }}
            />
          ) : null
        ) : view === 'bots' ? (
          hosts.map((host) => <HostBotsSection key={host.id} host={host} name={names[host.id] ?? host.name} several={several} />)
        ) : (
          hosts.map((host) => (
            <HostReportsSection key={host.id} host={host} name={names[host.id] ?? host.name} several={several} />
          ))
        )}
      </ScrollView>
    </View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    screen: { backgroundColor: palette.background, flex: 1 },
    content: { gap: spacing.lg, paddingHorizontal: layout.gutter },
    titleRow: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    section: { gap: spacing.md, marginTop: spacing.sm },
    group: { gap: spacing.sm },
    groupTitle: {
      color: palette.textFaint,
      fontSize: 12,
      fontWeight: '700',
      letterSpacing: 0.3,
      paddingHorizontal: spacing.xs,
      paddingTop: spacing.md,
    },
    hostName: { color: palette.text, fontSize: 18, fontWeight: '700', paddingHorizontal: spacing.xs },
    note: { color: palette.textMuted, fontSize: 13.5, lineHeight: 19, paddingHorizontal: spacing.xs },
    approval: { gap: spacing.sm },
    approvalHost: { color: palette.textMuted, fontSize: 12.5, fontWeight: '600', paddingHorizontal: spacing.xs },
  });
