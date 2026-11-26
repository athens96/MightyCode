import { useCallback, useEffect, useMemo } from 'react';
import { AppState } from 'react-native';
import { router, useFocusEffect } from 'expo-router';
import { peekHostConnection } from '@/api/client';
import { appForeground } from '@/api/relay/foreground';
import { Tabs } from 'expo-router/js-tabs';
import { BotsPoller } from '@/components/bots-poller';
import { HostPoller } from '@/components/host-poller';
import { Icon } from '@/components/icons';
import { waitingCount } from '@/lib/bots';
import { actionCount, buildAttentionList } from '@/lib/dashboard';
import { t } from '@/lib/i18n';
import { needsOnboarding } from '@/lib/onboarding';
import { retryUnreachableHosts, unreachableHostIds } from '@/lib/offline-retry';
import { useBotsStore } from '@/store/bots';
import { useHostsStore } from '@/store/hosts';
import { useLiveStore } from '@/store/live';
import { usePalette } from '@/theme';

/**
 * The bottom tab bar — 현황 · 알림 · 봇 · 호스트 — under the root stack. A pane, its
 * files and the pairing screens are pushed above the tabs, so the bar never sits under
 * the composer. Every paired host's pane list is polled here while the tabs show.
 */
export default function TabsLayout() {
  const palette = usePalette();
  const hosts = useHostsStore((store) => store.hosts);
  const loaded = useHostsStore((store) => store.loaded);
  const states = useLiveStore((store) => store.states);
  const badge = useMemo(() => actionCount(buildAttentionList(hosts, states)), [hosts, states]);
  // MC-107: the 봇 tab counts the approvals and the bots waiting for the person, on every paired computer.
  const bots = useBotsStore((store) => store.hosts);
  const botsBadge = useMemo(() => hosts.reduce((total, host) => total + waitingCount(bots[host.id]?.data), 0), [bots, hosts]);

  const refreshAll = useHostsStore((store) => store.refreshAll);

  // A phone with no Mac yet opens the connection guide over the tabs, so pairing can
  // come back down to them (`pair.tsx` returns to "/" before opening the host).
  useEffect(() => {
    if (loaded && needsOnboarding(hosts.length)) {
      router.push('/connect');
    }
  }, [loaded, hosts.length]);

  // Every tab shows a host's live dot, so reachability is kept fresh for all of them:
  // what each host's dot says was true when the app went away, so ask again when it
  // comes back, and a host that dropped off for a moment (the Mac changed networks or
  // slept) comes back by itself instead of waiting for a pull.
  useFocusEffect(
    useCallback(() => appForeground.subscribe(() => void refreshAll()), [refreshAll]),
  );
  useFocusEffect(
    useCallback(
      () =>
        retryUnreachableHosts({
          isActive: () => AppState.currentState === 'active',
          unreachable: () => {
            const state = useHostsStore.getState();
            // Skip hosts that already have an active pooled RelayConnection retrying on its own
            // so we never open duplicate unpooled `probeHost()` WebSockets alongside HostPoller.
            return unreachableHostIds(state.hosts, state.status).filter(
              (id) => !peekHostConnection(id)?.retrying,
            );
          },
          refresh: (id) => useHostsStore.getState().refreshReachability(id, { quiet: true }),
          subscribe: (listener) => {
            const unsubscribeStore = useHostsStore.subscribe(listener);
            const appStateSub = AppState.addEventListener('change', listener);
            return () => {
              unsubscribeStore();
              appStateSub.remove();
            };
          },
        }),
      [],
    ),
  );

  return (
    <>
      {hosts.map((host) => (
        <HostPoller key={host.id} hostId={host.id} />
      ))}
      {hosts.map((host) => (
        <BotsPoller key={`bots:${host.id}`} hostId={host.id} />
      ))}
      <Tabs
        screenOptions={{
          headerShown: false,
          sceneStyle: { backgroundColor: palette.background },
          tabBarActiveTintColor: palette.accent,
          tabBarInactiveTintColor: palette.textFaint,
          tabBarLabelStyle: { fontSize: 10.5, fontWeight: '700' },
          tabBarStyle: {
            backgroundColor: palette.surface,
            borderTopColor: palette.border,
          },
          tabBarBadgeStyle: {
            backgroundColor: palette.wait,
            color: palette.onBadge,
            fontSize: 10,
            fontWeight: '800',
          },
        }}
      >
        <Tabs.Screen
          name="index"
          options={{
            title: t('phone.tabs.dashboard'),
            tabBarIcon: ({ color }) => <Icon name="dashboard" color={color} />,
          }}
        />
        <Tabs.Screen
          name="alerts"
          options={{
            title: t('phone.tabs.alerts'),
            tabBarIcon: ({ color }) => <Icon name="alerts" color={color} />,
            ...(badge > 0
              ? {
                  tabBarBadge: badge,
                  tabBarAccessibilityLabel: t('phone.tabs.alertsLabel', { count: badge }),
                }
              : {}),
          }}
        />
        <Tabs.Screen
          name="bots"
          options={{
            title: t('phone.tabs.bots'),
            tabBarIcon: ({ color }) => <Icon name="bots" color={color} />,
            ...(botsBadge > 0
              ? {
                  tabBarBadge: botsBadge,
                  tabBarAccessibilityLabel: t('phone.tabs.botsLabel', { count: botsBadge }),
                }
              : {}),
          }}
        />
        <Tabs.Screen
          name="hosts"
          options={{
            title: t('phone.tabs.hosts'),
            tabBarIcon: ({ color }) => <Icon name="hosts" color={color} />,
          }}
        />
      </Tabs>
    </>
  );
}
