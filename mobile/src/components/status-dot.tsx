import { StyleSheet, View } from 'react-native';
import type { Tone } from '@/lib/status-tone';
import { headerTone, usePalette } from '@/theme';

/** The dot and the halo round it (`headerTone`): 8 pt in a ring of 3 pt. */
const DOT = 8;
const HALO = 3;

/**
 * The status dot leading a block's or result's title (option ⑨): the status fill in a halo of its
 * soft tint, on the card's own background. A mark only, hidden from screen readers: the header it
 * leads carries the status word (as its meta or its `accessibilityLabel`).
 */
export function StatusDot({ tone, testID }: { tone: Tone | undefined; testID?: string }) {
  const palette = usePalette();
  const colors = headerTone(palette, tone);
  return (
    <View
      accessible={false}
      importantForAccessibility="no-hide-descendants"
      pointerEvents="none"
      style={[styles.halo, { backgroundColor: colors.halo }]}
      testID={testID}
    >
      <View style={[styles.dot, { backgroundColor: colors.dot }]} />
    </View>
  );
}

const styles = StyleSheet.create({
  halo: {
    alignItems: 'center',
    borderRadius: (DOT + HALO * 2) / 2,
    height: DOT + HALO * 2,
    justifyContent: 'center',
    width: DOT + HALO * 2,
  },
  dot: { borderRadius: DOT / 2, height: DOT, width: DOT },
});
