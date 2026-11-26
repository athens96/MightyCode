import { useState } from 'react';
import { Linking, Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { describeError } from '@/api/client';
import type { MobileBot, MobileBotApproval, MobileBotReport } from '@/api/types';
import { ageText } from '@/components/session-card';
import { ConfirmDialog } from '@/components/sheets';
import { StatusDot } from '@/components/status-dot';
import { Button } from '@/components/ui';
import { useNow } from '@/hooks/use-now';
import {
  BOT_STATE_KEYS,
  REPORT_STATUS_KEYS,
  botTone,
  openableLink,
  reportTone,
  statusLine,
  timeWords,
} from '@/lib/bots';
import { language, t } from '@/lib/i18n';
import { refreshBots, useBotsStore } from '@/store/bots';
import { useHostClient } from '@/store/live';
import { showToast } from '@/store/toast';
import { radius, spacing, toneColors, useStyles, usePalette, type Palette } from '@/theme';

/** A rounded list's corner radius (the alerts tab's rows). */
const LIST_RADIUS = 18;

/** The small unread mark: a circular dot (the desktop's `.report-card-unread`), never a coloured edge. */
export function UnreadDot({ label }: { label?: string }) {
  const styles = useStyles(makeStyles);
  return (
    <View
      accessible={label !== undefined}
      accessibilityLabel={label}
      importantForAccessibility={label ? 'yes' : 'no-hide-descendants'}
      style={styles.unread}
    />
  );
}

/** Opens the bot's pane on its host (the existing pane screen). */
export function openBotPane(hostId: string, paneId: string): void {
  router.push(`/host/${hostId}/session/${paneId}`);
}

/**
 * One bot as a row: the state dot and word, @name with its workspace tag, the one line saying what it is doing (its next run in
 * words), and its last report's line with an unread dot. Tapping it opens the bot.
 */
export function BotRow({
  hostId,
  bot,
  first,
  last,
}: {
  hostId: string;
  bot: MobileBot;
  first: boolean;
  last: boolean;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const now = useNow(60_000);
  const tone = botTone(bot.state);
  const word = t(BOT_STATE_KEYS[bot.state]);
  const line = statusLine(bot, timeWords(bot.nextRun, new Date(now), language()), t);
  const report = bot.report;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={t('phone.bots.rowLabel', { handle: bot.handle, state: word, line })}
      onPress={() => router.push(`/host/${hostId}/bot/${encodeURIComponent(bot.id)}`)}
      style={({ pressed }) => [styles.row, first && styles.first, last && styles.last, pressed && styles.pressed]}
    >
      {first ? null : <View style={styles.rule} />}
      <View style={styles.dotWrap}>
        <StatusDot tone={tone} />
      </View>
      <View style={styles.body}>
        <View style={styles.headLine}>
          <Text numberOfLines={1} style={styles.handle}>
            @{bot.handle}
          </Text>
          {bot.kind === 'workspace' && bot.workspace ? (
            <View style={styles.tag}>
              <Text numberOfLines={1} style={styles.tagText}>
                {bot.workspace}
              </Text>
            </View>
          ) : null}
          <View style={styles.spacer} />
          <Text style={[styles.stateWord, { color: toneColors(palette, tone).ink }]}>{word}</Text>
        </View>
        {line ? (
          <Text numberOfLines={1} style={styles.line}>
            {line}
          </Text>
        ) : null}
        {report ? (
          <View style={styles.reportLine}>
            {report.unread ? <UnreadDot label={t('bot.report.unread')} /> : null}
            <Text numberOfLines={1} style={[styles.reportText, report.unread && styles.reportTextUnread]}>
              {t(REPORT_STATUS_KEYS[report.status])}
              {report.summary ? ` · ${report.summary}` : ''}
            </Text>
          </View>
        ) : null}
      </View>
    </Pressable>
  );
}

/**
 * One trigger action waiting for the person (MC-99): the event's type and subject with its title, the rule and where, what will
 * run, when; 실행 asks once more (it starts work on the computer), 건너뛰기 does not. Decided only here in the app.
 */
export function ApprovalCard({ hostId, hostName, approval }: { hostId: string; hostName: string; approval: MobileBotApproval }) {
  const styles = useStyles(makeStyles);
  const client = useHostClient(hostId);
  const drop = useBotsStore((store) => store.dropApproval);
  const [busy, setBusy] = useState<'approve' | 'skip' | null>(null);
  const [confirming, setConfirming] = useState(false);
  const now = useNow(60_000);

  const decide = (verb: 'approve' | 'skip') => {
    if (!client || busy) return;
    setBusy(verb);
    client
      .botApproval(approval.id, verb)
      .then((answer) => {
        drop(hostId, approval.id);
        showToast(
          answer.note ?? t(verb === 'approve' ? 'phone.bots.approvals.ran' : 'phone.bots.approvals.skipped'),
          'success',
        );
      })
      .catch((error: unknown) => {
        showToast(describeError(error), 'error');
        refreshBots(hostId);
      })
      .finally(() => {
        setBusy(null);
        setConfirming(false);
      });
  };

  const link = openableLink(approval.url);
  return (
    <View style={styles.card} testID="bot-approval">
      <View style={styles.cardHead}>
        <StatusDot tone="wait" />
        <Text numberOfLines={1} style={styles.cardType}>
          {approval.type || '—'}
        </Text>
        <Text numberOfLines={1} style={styles.cardSubject}>
          {approval.subject}
        </Text>
      </View>
      {approval.title ? (
        <Text
          numberOfLines={2}
          style={[styles.cardTitle, link && styles.link]}
          onPress={link ? () => void Linking.openURL(link) : undefined}
        >
          {approval.title}
        </Text>
      ) : null}
      <View style={styles.lines}>
        <Text style={styles.lineLabel}>{t('automation.approvals.rule')}</Text>
        <Text style={styles.lineValue}>{[approval.rule, approval.workspace].filter(Boolean).join(' · ')}</Text>
        <Text style={styles.lineLabel}>{t('automation.approvals.what')}</Text>
        <Text style={styles.lineValue}>{approval.what}</Text>
        <Text style={styles.lineLabel}>{t('automation.approvals.when')}</Text>
        <Text style={styles.lineValue}>{ageText(approval.at, now) ?? approval.at}</Text>
      </View>
      <View style={styles.actions}>
        <Button
          compact
          tone="primary"
          label={t('automation.approvals.run')}
          busy={busy === 'approve'}
          disabled={!client || busy !== null}
          onPress={() => setConfirming(true)}
        />
        <Button
          compact
          label={t('automation.approvals.skip')}
          busy={busy === 'skip'}
          disabled={!client || busy !== null}
          onPress={() => decide('skip')}
        />
      </View>
      <ConfirmDialog
        visible={confirming}
        title={t('phone.bots.approvals.confirmTitle', { rule: approval.rule })}
        description={t('phone.bots.approvals.confirmBody', { what: approval.what, host: hostName })}
        confirmLabel={t('automation.approvals.run')}
        busy={busy === 'approve'}
        onConfirm={() => decide('approve')}
        onCancel={() => setConfirming(false)}
      />
    </View>
  );
}

/**
 * One report in 봇 알림: the status dot and word, @name (left out on a bot's own screen), the summary and when, an unread dot. Opening
 * it marks it read and shows its links, what fired it, 다시 실행 and 창 열기.
 */
export function ReportRow({
  hostId,
  report,
  first,
  last,
  showHandle = true,
}: {
  hostId: string;
  report: MobileBotReport;
  first: boolean;
  last: boolean;
  showHandle?: boolean;
}) {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const client = useHostClient(hostId);
  const markRead = useBotsStore((store) => store.read);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const now = useNow(60_000);
  const tone = reportTone(report.status);
  const word = t(REPORT_STATUS_KEYS[report.status]);
  const age = ageText(report.at, now);

  const toggle = () => {
    const opening = !open;
    setOpen(opening);
    if (opening && !report.read && client) {
      markRead(hostId, report.id);
      client.botReport(report.id, 'read').catch(() => refreshBots(hostId));
    }
  };

  const rerun = () => {
    if (!client || busy) return;
    setBusy(true);
    client
      .botReport(report.id, 'rerun')
      .then(() => showToast(t('bot.report.rerunStarted', { handle: report.handle }), 'success'))
      .catch((error: unknown) => showToast(describeError(error), 'error'))
      .finally(() => setBusy(false));
  };

  const event = openableLink(report.eventUrl);
  return (
    <View style={[styles.row, styles.reportRow, first && styles.first, last && styles.last]}>
      {first ? null : <View style={styles.rule} />}
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={t('phone.bots.reportLabel', {
          handle: report.handle,
          status: word,
          summary: report.summary,
        })}
        onPress={toggle}
        style={({ pressed }) => [styles.reportHead, pressed && styles.pressedSoft]}
      >
        <View style={styles.dotWrap}>
          <StatusDot tone={tone} />
        </View>
        <View style={styles.body}>
          <View style={styles.headLine}>
            {showHandle ? (
              <Text numberOfLines={1} style={styles.handle}>
                @{report.handle}
              </Text>
            ) : null}
            <Text style={[styles.stateWord, { color: toneColors(palette, tone).ink }]}>{word}</Text>
            <View style={styles.spacer} />
            {report.unread ? <UnreadDot label={t('bot.report.unread')} /> : null}
            {age ? <Text style={styles.age}>{age}</Text> : null}
          </View>
          {report.summary ? (
            <Text numberOfLines={open ? undefined : 2} style={[styles.summary, report.unread && styles.reportTextUnread]}>
              {report.summary}
            </Text>
          ) : null}
          {report.repeat > 1 ? <Text style={styles.line}>{t('bot.report.repeat', { count: report.repeat })}</Text> : null}
        </View>
      </Pressable>
      {open ? (
        <View style={styles.reportBody}>
          {report.items.map((item, index) => {
            const url = openableLink(item.url);
            return (
              <Text
                key={`${index}:${item.text}`}
                style={[styles.item, url && styles.link]}
                onPress={url ? () => void Linking.openURL(url) : undefined}
                accessibilityRole={url ? 'link' : undefined}
              >
                • {item.text}
              </Text>
            );
          })}
          {report.job || report.reason ? (
            <Text style={styles.line}>
              {[report.job, report.reason ? t('bot.report.trigger', { reason: report.reason }) : undefined]
                .filter(Boolean)
                .join(' · ')}
            </Text>
          ) : null}
          <View style={styles.actions}>
            {report.rerun ? (
              <Button compact label={t('bot.report.rerun')} busy={busy} disabled={!client || busy} onPress={rerun} />
            ) : null}
            {report.pane ? (
              <Button compact label={t('bot.report.open')} onPress={() => openBotPane(hostId, report.pane!)} />
            ) : null}
            {event ? <Button compact label={t('bot.report.event')} onPress={() => void Linking.openURL(event)} /> : null}
          </View>
        </View>
      ) : null}
    </View>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    row: {
      alignItems: 'flex-start',
      backgroundColor: palette.surface,
      flexDirection: 'row',
      gap: spacing.lg,
      paddingHorizontal: spacing.lg,
      paddingVertical: spacing.md,
    },
    reportRow: { flexDirection: 'column', gap: 0, paddingHorizontal: 0, paddingVertical: 0 },
    reportHead: {
      alignItems: 'flex-start',
      flexDirection: 'row',
      gap: spacing.lg,
      paddingHorizontal: spacing.lg,
      paddingVertical: spacing.md,
    },
    reportBody: { gap: spacing.sm, paddingBottom: spacing.lg, paddingLeft: spacing.lg + 14 + spacing.lg, paddingRight: spacing.lg },
    first: { borderTopLeftRadius: LIST_RADIUS, borderTopRightRadius: LIST_RADIUS },
    last: { borderBottomLeftRadius: LIST_RADIUS, borderBottomRightRadius: LIST_RADIUS },
    pressed: { backgroundColor: palette.surfaceRaised },
    pressedSoft: { opacity: 0.75 },
    rule: {
      backgroundColor: palette.border,
      height: 1,
      left: spacing.lg + 14 + spacing.lg,
      position: 'absolute',
      right: 0,
      top: 0,
    },
    dotWrap: { paddingTop: spacing.xs },
    body: { flex: 1, gap: 1, minWidth: 0 },
    headLine: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    spacer: { flex: 1 },
    handle: { color: palette.text, flexShrink: 1, fontSize: 15, fontWeight: '700', lineHeight: 20 },
    tag: {
      backgroundColor: palette.surfaceRaised,
      borderColor: palette.border,
      borderRadius: radius.round,
      borderWidth: StyleSheet.hairlineWidth,
      flexShrink: 1,
      paddingHorizontal: spacing.md,
      paddingVertical: spacing.xs / 2,
    },
    tagText: { color: palette.textMuted, fontSize: 11.5, fontWeight: '600' },
    stateWord: { fontSize: 12.5, fontWeight: '700' },
    line: { color: palette.textMuted, fontSize: 12.5, lineHeight: 17 },
    reportLine: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    reportText: { color: palette.textMuted, flex: 1, fontSize: 12.5, lineHeight: 17 },
    reportTextUnread: { color: palette.text, fontWeight: '600' },
    unread: { backgroundColor: palette.accent, borderRadius: radius.round, height: 7, width: 7 },
    age: { color: palette.textFaint, fontSize: 12 },
    summary: { color: palette.textMuted, fontSize: 13.5, lineHeight: 19 },
    item: { color: palette.text, fontSize: 13.5, lineHeight: 19 },
    link: { color: palette.accent, textDecorationLine: 'underline' },
    actions: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing.md, marginTop: spacing.sm },
    card: {
      backgroundColor: palette.surface,
      borderColor: palette.border,
      borderRadius: LIST_RADIUS,
      borderWidth: StyleSheet.hairlineWidth,
      gap: spacing.md,
      padding: spacing.xl,
    },
    cardHead: { alignItems: 'center', flexDirection: 'row', gap: spacing.md },
    cardType: { color: palette.text, fontSize: 13.5, fontWeight: '700' },
    cardSubject: { color: palette.textMuted, flex: 1, fontSize: 13 },
    cardTitle: { color: palette.text, fontSize: 14.5, fontWeight: '600', lineHeight: 20 },
    lines: { gap: 1 },
    lineLabel: { color: palette.textFaint, fontSize: 11.5, fontWeight: '700', marginTop: spacing.sm },
    lineValue: { color: palette.text, fontSize: 13.5, lineHeight: 19 },
  });
