import { createClient, type RelayChannel } from '@/api/client';
import type { LogEntry, MobileBodyMark, MobileSessionDetail } from '@/api/types';
import { applyBodyDelta, bodySinceOf, bodyVerified, idsHash, resolveBody } from '@/lib/body-delta';

const entry = (id: string, text: string): LogEntry => ({ id, kind: 'assistant', text, timestamp: '2026-11-24T00:00:00Z' });

const markOf = (window: LogEntry[], revision: string, since?: string): MobileBodyMark => ({
  revision,
  kind: since === undefined ? 'full' : 'delta',
  ...(since === undefined ? {} : { since }),
  count: window.length,
  hash: idsHash(window.map((e) => e.id)),
});

const detail = (entries: LogEntry[], body?: MobileBodyMark): MobileSessionDetail =>
  ({ protocol: 1, revision: 1, session: {} as never, entries, permissions: [], queued: [], ...(body ? { body } : {}) }) as MobileSessionDetail;

describe('idsHash (MC-53)', () => {
  it('is the host\'s FNV-1a over code points, each id ended by a line feed', () => {
    // The same values as native/tauri/crates/mighty-remote/src/body_delta.rs `the_ids_hash_is_fnv1a_over_scalar_values`.
    expect(idsHash([])).toBe('811c9dc5');
    expect(idsHash(['e1', 'e2'])).toBe('2c436f46');
    expect(idsHash(['한글-✓'])).toBe('7a09a9ad');
    expect(idsHash(['😀'])).toBe('05f7060f');
    expect(idsHash(['ab'])).not.toBe(idsHash(['a', 'b']));
  });
});

describe('applyBodyDelta (MC-53)', () => {
  /** The host's rule (`body_delta.rs` `changed`): the entries whose id is new or whose words changed, in window order. */
  const deltaOf = (held: LogEntry[], window: LogEntry[]) => {
    const before = new Map(held.map((e) => [e.id, e.text]));
    return window.filter((e) => before.get(e.id) !== e.text);
  };

  it('lays deltas over the held window into the window whole, change after change', () => {
    const WINDOW = 20;
    let seed = 7;
    const roll = (n: number) => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed % n;
    };
    const logs = Array.from({ length: 30 }, (_, i) => entry(`e${i}`, `text ${i}`));
    const windowOf = () => logs.slice(Math.max(0, logs.length - WINDOW));
    let held = windowOf();
    let next = 30;
    for (let step = 0; step < 300; step += 1) {
      const pick = roll(4);
      if (pick === 0) {
        for (let i = 0; i <= roll(3); i += 1) logs.push(entry(`e${next}`, `new ${next++}`));
      } else if (pick === 1) {
        const last = logs[logs.length - 1];
        if (last) logs[logs.length - 1] = { ...last, text: `${last.text} more` };
      } else if (pick === 2) {
        const at = logs.length - 1 - roll(WINDOW);
        const old = logs[at];
        if (old) logs[at] = { ...old, text: `edit ${step}` };
      }
      const window = windowOf();
      const mark = markOf(window, `r${step}`);
      held = applyBodyDelta(held, deltaOf(held, window), mark.count);
      expect(held).toEqual(window);
      expect(bodyVerified(held, mark)).toBe(true);
    }
  });

  it('keeps the newest count and replaces in place', () => {
    const held = [entry('a', '1'), entry('b', '2'), entry('c', '3')];
    expect(applyBodyDelta(held, [entry('b', '2!'), entry('d', '4')], 3)).toEqual([entry('b', '2!'), entry('c', '3'), entry('d', '4')]);
    expect(applyBodyDelta(held, [], 0)).toEqual([]);
  });
});

describe('resolveBody (MC-53)', () => {
  const held = [entry('a', '1'), entry('b', '2')];
  const heldDetail = detail(held, markOf(held, 'r1'));

  it('lays a delta built on the held window over it and holds the new window', () => {
    const window = [entry('a', '1'), entry('b', '2 more'), entry('c', '3')];
    const resolved = resolveBody(heldDetail, detail([entry('b', '2 more'), entry('c', '3')], markOf(window, 'r2', 'r1')));
    expect(resolved?.entries).toEqual(window);
    expect(resolved?.body).toEqual({ revision: 'r2', kind: 'full', count: 3, hash: idsHash(['a', 'b', 'c']) });
    expect(bodySinceOf(resolved)).toBe('r2');
  });

  it('asks again whole when a delta does not lay over what is held', () => {
    const window = [entry('a', '1'), entry('b', '2'), entry('c', '3')];
    expect(resolveBody(heldDetail, detail([entry('c', '3')], markOf(window, 'r2', 'other')))).toBeUndefined();
    expect(resolveBody(undefined, detail([entry('c', '3')], markOf(window, 'r2', 'r1')))).toBeUndefined();
    const wrongHash = { ...markOf(window, 'r2', 'r1'), hash: '00000000' };
    expect(resolveBody(heldDetail, detail([entry('c', '3')], wrongHash))).toBeUndefined();
    const wrongCount = { ...markOf(window, 'r2', 'r1'), count: 2 };
    expect(resolveBody(heldDetail, detail([entry('c', '3')], wrongCount))).toBeUndefined();
  });

  it('takes a whole answer as it came, and an unmarked one from an older host', () => {
    const whole = detail(held, markOf(held, 'r9'));
    expect(resolveBody(heldDetail, whole)).toBe(whole);
    const unmarked = detail(held);
    expect(resolveBody(heldDetail, unmarked)).toBe(unmarked);
    expect(bodySinceOf(unmarked)).toBe('');
    const misdescribed = detail(held, { ...markOf(held, 'r9'), count: 5 });
    expect(resolveBody(undefined, misdescribed)?.body).toBeUndefined();
  });
});

describe('bodySince on the wire (MC-53)', () => {
  it('is sent only when given, empty for none held, and never anything but a revision', async () => {
    const calls: string[] = [];
    const channel: RelayChannel = {
      request: (_method, path) => {
        calls.push(path);
        return Promise.resolve({ status: 200, body: { protocol: 1, ok: true } });
      },
      onNotify: () => () => undefined,
    };
    const client = createClient(channel);
    await client.session('s1', { since: 2, wait: 10 });
    await client.session('s1', { since: 2, wait: 10, bodySince: '' });
    await client.session('s1', { since: 2, wait: 10, bodySince: '0a1b' });
    await client.session('s1', { since: 2, wait: 10, bodySince: '../x' });
    expect(calls).toEqual([
      '/m1/sessions/s1?since=2&wait=10',
      '/m1/sessions/s1?since=2&wait=10&bodySince=',
      '/m1/sessions/s1?since=2&wait=10&bodySince=0a1b',
      '/m1/sessions/s1?since=2&wait=10',
    ]);
  });
});
