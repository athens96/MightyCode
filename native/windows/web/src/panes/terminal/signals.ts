// A terminal's "look at me" signals (MC-26), as the page's xterm reads them: OSC 9 (iTerm2's notification; ConEmu's numbered forms
// such as the `9;4;…` progress are not), OSC 777 (`notify;title;body`), OSC 99 (kitty's, its title and body only) and the bell. The
// host makes the words safe again and decides what to mark (src-tauri attention.rs); the Mac's Ghostty reports them itself
// (mighty-terminal signals.rs is the same reading in Rust).

/** page → host: the `terminalMessage`s a signal is. */
export type TerminalSignal = { type: 'notify'; title: string; body: string } | { type: 'bell' };

/** The most characters of a signal's words kept. */
export const textLimit = 200;
/** The shortest gap between two signals of one terminal (the host keeps the same). */
export const signalGap = 1000;

const hidden = /[‪-‮⁦-⁩​﻿]/g;
// Control characters (C0, DEL, C1) and white space become one space.
// eslint-disable-next-line no-control-regex
const spaces = /[\u0000-\u001f\u007f-\u009f\s]+/g;

/** `text` safe for one line: control characters and runs of white space one space, trimmed, at most {@link textLimit} characters. */
export function sanitize(text: string): string {
  const clean = text.replace(hidden, '').replace(spaces, ' ').trim();
  const chars = Array.from(clean);
  return chars.length > textLimit ? chars.slice(0, textLimit).join('').trimEnd() : clean;
}

/** OSC 9's data (after `9;`): a notification, or null for ConEmu's numbered forms (`4;1;50` progress, `1;500` sleep, …). */
export function osc9(data: string): TerminalSignal | null {
  const first = data.split(';')[0] ?? '';
  if (/^\d{1,2}$/.test(first)) return null;
  return { type: 'notify', title: '', body: sanitize(data) };
}

/** OSC 777's data (after `777;`): `notify;title;body` (the body may hold `;`). */
export function osc777(data: string): TerminalSignal | null {
  const parts = data.split(';');
  if (parts[0] !== 'notify') return null;
  return { type: 'notify', title: sanitize(parts[1] ?? ''), body: sanitize(parts.slice(2).join(';')) };
}

function decodeBase64(text: string): string | null {
  try {
    const bytes = Uint8Array.from(atob(text.trim()), (c) => c.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch {
    return null;
  }
}

/** kitty's notifications (OSC 99) still waiting for their last chunk (`d=0`), by id. */
export class KittyNotifications {
  private pending = new Map<string, { title: string; body: string }>();

  /** OSC 99's data (after `99;`): `key=value:…;payload`; a notification once its last chunk came, else null. */
  take(data: string): TerminalSignal | null {
    const split = data.indexOf(';');
    const metadata = split < 0 ? data : data.slice(0, split);
    let payload = split < 0 ? '' : data.slice(split + 1);
    let id = '';
    let done = true;
    let part: 'title' | 'body' | null = 'title';
    let base64 = false;
    for (const pair of metadata.split(':')) {
      if (!pair) continue;
      const at = pair.indexOf('=');
      const key = at < 0 ? pair : pair.slice(0, at);
      const value = at < 0 ? '' : pair.slice(at + 1);
      if (key === 'i') id = value.replace(/[^A-Za-z0-9\-_+.]/g, '').slice(0, 64);
      else if (key === 'd') done = value !== '0';
      else if (key === 'e') base64 = value === '1';
      else if (key === 'p') part = value === 'title' || value === 'body' ? value : null;
    }
    if (base64) {
      const decoded = decodeBase64(payload);
      if (decoded === null) return null;
      payload = decoded;
    }
    if (!this.pending.has(id) && this.pending.size >= 8) this.pending.clear();
    const held = this.pending.get(id) ?? { title: '', body: '' };
    if (part && held[part].length < 4096) held[part] += payload;
    this.pending.set(id, held);
    if (!done) return null;
    this.pending.delete(id);
    if (!part && !held.title && !held.body) return null;
    return { type: 'notify', title: sanitize(held.title), body: sanitize(held.body) };
  }
}

/** At most one signal each {@link signalGap} ms: true when the one at `now` goes through. */
export function signalLimiter(): (now: number) => boolean {
  let last = -Infinity;
  return (now) => {
    if (now - last < signalGap) return false;
    last = now;
    return true;
  };
}
