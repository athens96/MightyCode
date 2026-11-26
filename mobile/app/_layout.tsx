import { useEffect, useMemo } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import {
  DarkTheme,
  DefaultTheme,
  Stack,
  ThemeProvider,
  router,
  type ErrorBoundaryProps,
} from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { GestureHandlerRootView } from 'react-native-gesture-handler';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import { Icon } from '@/components/icons';
import { PushRoot } from '@/components/push-root';
import { ToastHost } from '@/components/toast-host';
import { installCryptoPolyfill } from '@/api/relay/random';
import { useHostsStore } from '@/store/hosts';
import { t } from '@/lib/i18n';
import {
  darkPalette,
  dialog,
  headingFontFamily,
  radius,
  spacing,
  useStyles,
  usePalette,
  type Palette,
} from '@/theme';

/** A modal panel's ✕ on the left of its head (the popup standard), since iOS shows no grabber. */
function CloseButton() {
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      accessibilityLabel={t('dialog.close')}
      accessibilityRole="button"
      onPress={() => (router.canGoBack() ? router.back() : router.replace('/'))}
      style={({ pressed }) => [styles.close, pressed && styles.pressed]}
    >
      <Icon name="close" color={palette.text} size={dialog.closeGlyph} />
    </Pressable>
  );
}

// `@noble/*` reads `globalThis.crypto.getRandomValues`, which React Native lacks.
installCryptoPolyfill();

/** Root Expo Router error boundary: catches unhandled render crashes with a recoverable retry screen. */
export function ErrorBoundary({ error, retry }: ErrorBoundaryProps) {
  const styles = useStyles(makeStyles);
  const detail = error.message.trim();
  return (
    <View style={styles.errorScreen}>
      <View style={styles.errorCard}>
        <Text accessibilityRole="header" style={styles.errorTitle}>
          {t('phone.error.unknown')}
        </Text>
        {detail.length > 0 ? (
          <Text numberOfLines={6} style={styles.errorMessage}>
            {detail}
          </Text>
        ) : null}
        <Pressable
          accessibilityRole="button"
          onPress={() => void retry()}
          style={({ pressed }) => [styles.retryButton, pressed && styles.pressed]}
        >
          <Text style={styles.retryLabel}>{t('phone.files.retry')}</Text>
        </Pressable>
      </View>
    </View>
  );
}

export default function RootLayout() {
  const load = useHostsStore((state) => state.load);
  const palette = usePalette();
  const styles = useStyles(makeStyles);
  const dark = palette === darkPalette;

  // Navigation carries its own colours, so it follows the system scheme with the app.
  const navigationTheme = useMemo(() => {
    const base = dark ? DarkTheme : DefaultTheme;
    return {
      ...base,
      dark,
      colors: {
        ...base.colors,
        primary: palette.accent,
        background: palette.background,
        card: palette.background,
        text: palette.text,
        border: palette.border,
        notification: palette.accent,
      },
    };
  }, [dark, palette]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <SafeAreaProvider>
      <ThemeProvider value={navigationTheme}>
        {/* Gestures that run beside native scrolling (the chat's pull-to-refresh) need this root. */}
        <GestureHandlerRootView style={styles.root}>
          <StatusBar style={dark ? 'light' : 'dark'} />
          <Stack
            screenOptions={{
              // The header is the grey page itself, with no rule under it; the cards below
              // carry the colour. Its title is bold Avenir Next, a size under the screen's own.
              headerStyle: { backgroundColor: palette.background },
              headerShadowVisible: false,
              headerTintColor: palette.text,
              headerTitleStyle: {
                color: palette.text,
                fontFamily: headingFontFamily,
                fontSize: 16,
                fontWeight: '700',
              },
              contentStyle: { backgroundColor: palette.background },
            }}
          >
            {/* The tab bar (dashboard · alerts · bots · hosts) is the root; everything else stacks on it. */}
            <Stack.Screen name="(tabs)" options={{ headerShown: false, title: t('phone.tabs.dashboard') }} />
            {/* The guide draws its own large title, so its head carries only the ✕. */}
            <Stack.Screen
              name="connect"
              options={{ title: '', presentation: 'modal', headerLeft: () => <CloseButton /> }}
            />
            <Stack.Screen
              name="pair"
              options={{ title: t('phone.pair.title'), presentation: 'modal', headerLeft: () => <CloseButton /> }}
            />
            <Stack.Screen name="settings" options={{ title: t('phone.settings.title') }} />
            <Stack.Screen name="host/[hostId]/index" options={{ title: t('phone.workspaces.title') }} />
            <Stack.Screen name="host/[hostId]/session/[sessionId]" options={{ title: t('phone.session.title') }} />
            <Stack.Screen name="host/[hostId]/bot/[botId]" options={{ title: t('phone.bots.title') }} />
            {/* The only screen that may turn sideways; it unlocks landscape itself. */}
            <Stack.Screen
              name="host/[hostId]/screen"
              options={{ title: t('phone.screenShare.title') }}
            />
            <Stack.Screen
              name="host/[hostId]/workspace/[workspaceId]/files"
              options={{ title: t('phone.files.title') }}
            />
            <Stack.Screen
              name="host/[hostId]/workspace/[workspaceId]/file"
              options={{ title: t('phone.files.title') }}
            />
            <Stack.Screen
              name="host/[hostId]/workspace/[workspaceId]/workmap"
              options={{ title: t('workMap.title') }}
            />
          </Stack>
          <PushRoot />
          <ToastHost />
        </GestureHandlerRootView>
      </ThemeProvider>
    </SafeAreaProvider>
  );
}

const makeStyles = (palette: Palette) =>
  StyleSheet.create({
    root: { backgroundColor: palette.background, flex: 1 },
    close: {
      alignItems: 'center',
      borderRadius: radius.round,
      height: dialog.closeHit,
      justifyContent: 'center',
      width: dialog.closeHit,
    },
    pressed: { opacity: 0.6 },
    errorScreen: {
      alignItems: 'center',
      backgroundColor: palette.background,
      flex: 1,
      justifyContent: 'center',
      padding: spacing.xxl,
    },
    errorCard: {
      backgroundColor: palette.surface,
      borderColor: palette.border,
      borderRadius: radius.card,
      borderWidth: StyleSheet.hairlineWidth,
      gap: spacing.lg,
      maxWidth: dialog.alertWidth,
      padding: dialog.padding,
      width: '100%',
    },
    errorTitle: {
      color: palette.text,
      fontFamily: headingFontFamily,
      fontSize: 18,
      fontWeight: '700',
    },
    errorMessage: {
      color: palette.textMuted,
      fontSize: 14,
      lineHeight: 20,
    },
    retryButton: {
      alignItems: 'center',
      backgroundColor: palette.accent,
      borderRadius: radius.md,
      justifyContent: 'center',
      minHeight: dialog.buttonHeight,
      paddingHorizontal: spacing.xl,
      paddingVertical: spacing.md,
    },
    retryLabel: {
      color: palette.onAccent,
      fontSize: 15,
      fontWeight: '700',
    },
  });
