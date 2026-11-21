/** Pure state & formatting helpers for Desktop Voice Mode / Push-to-Talk (`MC-4`). */

export const WAVEFORM_BAR_COUNT = 16;
export const PTT_HOLD_THRESHOLD_MS = 260;

export interface ShortcutsKeyLike {
  key: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

/** `⇧⌘M` on macOS, `Ctrl+Shift+M` on Windows/Linux. */
export function isPushToTalkShortcut(event: ShortcutsKeyLike, mac: boolean): boolean {
  const primary = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!primary || !event.shiftKey || event.altKey) return false;
  return event.key.toLowerCase() === 'm' || event.code === 'KeyM';
}

/** Formats elapsed seconds as `0:04` or `1:23`. */
export function formatVoiceDuration(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const mins = Math.floor(whole / 60);
  const secs = whole % 60;
  return `${mins}:${secs < 10 ? '0' : ''}${secs}`;
}

/** Pads or trims rolling audio levels (`0..1`) into `count` normalized bar heights (`0.14..1.0`). */
export function normalizeWaveformBars(levels: readonly number[], count = WAVEFORM_BAR_COUNT): number[] {
  const out = new Array<number>(count).fill(0.14);
  const start = Math.max(0, levels.length - count);
  const offset = count - (levels.length - start);
  for (let i = start; i < levels.length; i++) {
    const raw = levels[i] ?? 0;
    const clamped = Number.isFinite(raw) ? Math.min(1, Math.max(0.14, raw)) : 0.14;
    out[offset + (i - start)] = clamped;
  }
  return out;
}

function isDistinctiveCodeHint(term: string): boolean {
  if (term.length < 4) return false;
  const hasMixed = /[A-Z]/.test(term) && /[a-z]/.test(term);
  const hasPunct = /[._-]/.test(term);
  return hasMixed || hasPunct;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Applies spoken extension normalization (`dot tsx`, localized spoken dot) and exact workspace symbol casing on the web client. */
export function applyWorkspaceCodeHints(transcript: string, hints: readonly string[], spokenDot = 'dot'): string {
  const trimmed = transcript.trim();
  if (!trimmed) return '';
  const dotWord = spokenDot.trim();
  const dotGroup = dotWord && dotWord.toLowerCase() !== 'dot' ? `(?:dot|${escapeRegExp(dotWord)})` : 'dot';
  let text = trimmed.replace(
    new RegExp(`\\s+${dotGroup}\\s+(tsx|ts|jsx|js|rs|py|json|md|css|html|toml|yaml|yml)\\b`, 'gi'),
    '.$1',
  );

  const map = new Map<string, string>();
  for (const h of hints) {
    if (isDistinctiveCodeHint(h) && !map.has(h.toLowerCase())) {
      map.set(h.toLowerCase(), h);
    }
  }
  if (map.size === 0) return text;

  text = text.replace(/\b[A-Za-z_][A-Za-z0-9_.-]{2,}\b/g, (word) => {
    const exact = map.get(word.toLowerCase());
    if (exact) return exact;
    const withoutDot = word.endsWith('.') ? word.slice(0, -1) : null;
    if (withoutDot) {
      const base = map.get(withoutDot.toLowerCase());
      if (base) return base + '.';
    }
    return word;
  });
  return text;
}

/** Inserts `dictated` at `[selectionStart, selectionEnd]` (or appends at the end) with clean spacing around existing prose. */
export function insertDictatedText(
  currentDraft: string,
  dictated: string,
  selectionStart?: number | null,
  selectionEnd?: number | null,
  maxLength = 100_000,
): { text: string; cursor: number } {
  const clean = dictated.trim();
  if (!clean) {
    const pos = selectionEnd ?? currentDraft.length;
    return { text: currentDraft, cursor: pos };
  }
  const start = selectionStart === null || selectionStart === undefined
    ? currentDraft.length
    : Math.max(0, Math.min(selectionStart, currentDraft.length));
  const end = selectionEnd === null || selectionEnd === undefined
    ? start
    : Math.max(start, Math.min(selectionEnd, currentDraft.length));

  const before = currentDraft.slice(0, start);
  const after = currentDraft.slice(end);
  const needsLeadSpace = before.length > 0 && !/\s$/.test(before);
  const needsTrailSpace = after.length > 0 && !/^\s/.test(after) && !/^[.,!?;:)\]]/.test(after);
  const piece = (needsLeadSpace ? ' ' : '') + clean + (needsTrailSpace ? ' ' : '');
  const merged = (before + piece + after).slice(0, maxLength);
  const cursor = Math.min(merged.length, before.length + (needsLeadSpace ? 1 : 0) + clean.length);
  return { text: merged, cursor };
}
