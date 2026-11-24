// MC-53 ("bodyDelta"): a session detail's entries as only what changed since the window the phone holds.
//
// A host that lists "bodyDelta" takes `bodySince` on `GET /m1/sessions/{id}`: the revision of the entry window the phone holds
// (empty: none yet). Its answer marks the window (`body`: revision, kind, count, the ids' hash); with `kind: "delta"` the
// `entries` are only the changed and the new ones, laid over the held window here — an entry held is replaced where it is, a new
// one goes last, the newest `count` are kept — and checked against the count and the hash. A delta that does not lay over what is
// held is read again whole. The host's side is `mighty_remote::body_delta` (native/tauri/crates/mighty-remote/src/body_delta.rs).
import type { LogEntry, MobileBodyMark, MobileSessionDetail } from '@/api/types';

/**
 * The hash of a window's ids: FNV-1a (32 bits) over each id's code points, each id followed by a line feed, as 8 lower-case hex
 * digits — the host's `ids_hash`.
 */
export function idsHash(ids: Iterable<string>): string {
  let hash = 0x811c9dc5;
  const mix = (value: number) => {
    hash ^= value;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  };
  for (const id of ids) {
    for (const ch of id) mix(ch.codePointAt(0) ?? 0);
    mix(0x0a);
  }
  return (hash >>> 0).toString(16).padStart(8, '0');
}

/** A delta's entries laid over the held window: replaced in place or added last, the newest `count` kept. */
export function applyBodyDelta(held: readonly LogEntry[], entries: readonly LogEntry[], count: number): LogEntry[] {
  const out = [...held];
  const at = new Map(out.map((entry, index) => [entry.id, index]));
  for (const entry of entries) {
    const index = at.get(entry.id);
    if (index === undefined) {
      at.set(entry.id, out.length);
      out.push(entry);
    } else {
      out[index] = entry;
    }
  }
  return out.slice(Math.max(0, out.length - Math.max(0, count)));
}

/** Whether a window is the one a mark describes (its count and its ids). */
export function bodyVerified(entries: readonly LogEntry[], mark: Pick<MobileBodyMark, 'count' | 'hash'>): boolean {
  return entries.length === mark.count && idsHash(entries.map((entry) => entry.id)) === mark.hash;
}

/** `bodySince` for the next ask: the revision of the window held, empty when none is (or it came unmarked). */
export function bodySinceOf(held: Pick<MobileSessionDetail, 'body'> | undefined): string {
  return held?.body?.revision ?? '';
}

/**
 * The answer as a whole detail: a delta laid over the held detail's window, a whole answer as it came (its mark dropped when it
 * does not describe its entries, so the next ask holds nothing). Undefined when a delta does not lay over what is held (built on
 * another window, or the count or the hash do not match): read the detail again whole.
 */
export function resolveBody(
  held: MobileSessionDetail | undefined,
  incoming: MobileSessionDetail,
): MobileSessionDetail | undefined {
  const mark = incoming.body;
  if (!mark) return incoming;
  if (mark.kind !== 'delta') {
    return bodyVerified(incoming.entries, mark) ? incoming : { ...incoming, body: undefined };
  }
  if (!held?.body || held.body.revision !== mark.since) return undefined;
  const entries = applyBodyDelta(held.entries, incoming.entries, mark.count);
  if (!bodyVerified(entries, mark)) return undefined;
  return { ...incoming, entries, body: { revision: mark.revision, kind: 'full', count: mark.count, hash: mark.hash } };
}
