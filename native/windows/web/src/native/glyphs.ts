// The Segoe Fluent Icons / MDL2 code points the host's forms name (mighty-native FontIcon, `settings_symbol`), each as the Lucide
// icon the page draws in its place. The Mac has no Segoe icon font, so a code point drawn as text shows as an empty box there; both
// OSes draw the Lucide icon, so the forms look the same on each.
import type { IconName } from '../icons/Icon';

const segoe: Readonly<Record<number, IconName>> = {
  0xe70d: 'chevron-down',
  0xf13c: 'circle-check',
  0xf136: 'circle',
  0xe9d5: 'list-checks',
  0xe8a7: 'external-link',
  0xe823: 'clock',
  0xe740: 'maximize-2',
  0xe73d: 'clipboard-check',
  0xe739: 'square',
  0xe70f: 'pencil',
  0xe712: 'ellipsis',
  0xe713: 'settings',
  0xe721: 'search',
  0xe72c: 'refresh-cw',
  0xe738: 'minus',
  0xe74d: 'trash',
  0xe756: 'square-terminal',
  0xe76c: 'chevron-right',
  0xe774: 'earth',
  0xe77b: 'square-user-round',
  0xe783: 'circle-x',
  0xe790: 'palette',
  0xe7b8: 'package',
  0xe7ba: 'triangle-alert',
  0xe897: 'circle-question-mark',
  0xe8b7: 'folder',
  0xe8c8: 'copy',
  0xe8ea: 'smartphone',
  0xe90f: 'wrench',
  0xe943: 'code-xml',
  0xe946: 'info',
  0xe950: 'brain',
  0xe968: 'globe',
  0xe977: 'monitor',
  0xeb90: 'circle-x',
  0xec61: 'circle-check',
};

/** The Lucide icon for a glyph from the Segoe private-use range; `dot` for one this table does not know yet; null for any other
 *  text (an emoji, a letter), which is drawn as it is. */
export function glyphIcon(glyph: string): IconName | null {
  const code = glyph.codePointAt(0);
  if (code === undefined || glyph.length > 2) return null;
  const known = segoe[code];
  if (known) return known;
  return code >= 0xe000 && code <= 0xf8ff ? 'dot' : null;
}
