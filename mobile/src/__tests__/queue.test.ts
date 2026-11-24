import { readFileSync } from 'fs';
import { resolve } from 'path';
import { createElement } from 'react';
import { Text, TextInput } from 'react-native';
import TestRenderer, { act, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer';
import type { QueuedItem } from '@/api/types';
import { QueuedList } from '@/components/queued-list';
import { resetLanguage, t } from '@/lib/i18n';
import { editedQueueWords, queueMovePlace, queueRowEdits, runNowOf, runNowReasonKey } from '@/lib/queue';

type Wire = { runNow: string; runNowReason?: string } | null;
const fixture = JSON.parse(
  readFileSync(resolve(__dirname, '../../../native/contracts/fixtures/queue-run-now.json'), 'utf8'),
) as { cases: { name: string; wire: Wire }[] };

describe('runNowOf', () => {
  it.each(fixture.cases.map((c) => [c.name, c.wire] as const))('%s', (_name, wire) => {
    const read = runNowOf({ id: 'q', text: 'x', ...(wire ?? {}) } as never);
    if (wire === null) expect(read).toBeUndefined();
    else expect(read).toEqual({ kind: wire.runNow, ...(wire.runNowReason ? { reason: wire.runNowReason } : {}) });
  });

  it('shows no button for anything else', () => {
    for (const junk of ['steer', 'agentMenu', 'hidden', '', 1, null, {}]) {
      expect(runNowOf({ id: 'q', text: 'x', runNow: junk } as never)).toBeUndefined();
    }
  });

  it('reads an unknown or missing reason as the current task', () => {
    expect(runNowOf({ id: 'q', text: 'x', runNow: 'unavailable', runNowReason: 'later' } as never)).toEqual({
      kind: 'unavailable',
      reason: 'turn',
    });
    expect(runNowOf({ id: 'q', text: 'x', runNow: 'unavailable' } as never)).toEqual({ kind: 'unavailable', reason: 'turn' });
  });

  it('words each reason with the shared keys', () => {
    expect(runNowReasonKey('turn')).toBe('queue.runNowUnavailableHelp');
    expect(runNowReasonKey('held')).toBe('queue.runNowAfterHeldHelp');
    expect(runNowReasonKey('update')).toBe('queue.runNowUpdateHelp');
    expect(runNowReasonKey('agent')).toBe('phone.queue.runNowAgentOnComputer');
  });
});

describe('the queued list', () => {
  beforeAll(() => resetLanguage('ko'));
  afterAll(() => resetLanguage());

  let renderer: ReactTestRenderer | undefined;
  afterEach(() => {
    act(() => renderer?.unmount());
    renderer = undefined;
  });

  const mount = (items: QueuedItem[], onRunNow = jest.fn(), onExplain = jest.fn()) => {
    act(() => {
      renderer = TestRenderer.create(
        createElement(QueuedList, { items, manageable: true, busy: false, onRemove: jest.fn(), onRunNow, onExplain }),
      );
    });
    return renderer!;
  };

  /** The pressable of each button labelled 바로 실행 (the innermost node that takes the press, one per button). */
  const runNowButtons = (view: ReactTestRenderer): ReactTestInstance[] => {
    const found = view.root.findAll(
      (node) =>
        typeof node.props.onPress === 'function' &&
        node.props.accessibilityRole === 'button' &&
        node.findAllByType(Text).some((text) => [text.props.children].flat(3).join('') === t('queue.runNow')),
    );
    const holds = (outer: ReactTestInstance, inner: ReactTestInstance) => {
      for (let node = inner.parent; node; node = node.parent) if (node === outer) return true;
      return false;
    };
    return found.filter((node) => !found.some((other) => other !== node && holds(node, other)));
  };

  it('offers 바로 실행 only on rows the host gave one, and none from an old host', () => {
    const view = mount([
      { id: 'a', text: 'first', runNow: 'worktree' },
      { id: 'b', text: 'second' },
      { id: 'c', text: 'third', runNow: 'here' },
    ]);
    const buttons = runNowButtons(view);
    expect(buttons).toHaveLength(2);
    expect(buttons.map((button) => button.props.accessibilityLabel)).toEqual([
      t('phone.queue.runNowLabel', { text: 'first' }),
      t('phone.queue.runNowLabel', { text: 'third' }),
    ]);
    act(() => renderer?.unmount());
    expect(runNowButtons(mount([{ id: 'old', text: 'from an old host' }]))).toHaveLength(0);
  });

  it('runs an active row', () => {
    const onRunNow = jest.fn();
    const item: QueuedItem = { id: 'a', text: 'first', runNow: 'worktree' };
    const view = mount([item], onRunNow);
    const [button] = runNowButtons(view);
    expect(button!.props.accessibilityState).toMatchObject({ disabled: false });
    expect(button!.props.accessibilityHint).toBe(t('phone.queue.runNowWorktreeHint'));
    act(() => button!.props.onPress());
    expect(onRunNow).toHaveBeenCalledWith(item);
  });

  it('dims a row that must wait, and a tap says why instead of asking the host', () => {
    const onRunNow = jest.fn();
    const onExplain = jest.fn();
    const view = mount([{ id: 'a', text: 'first', runNow: 'unavailable', runNowReason: 'agent' }], onRunNow, onExplain);
    const [button] = runNowButtons(view);
    expect(button!.props.accessibilityState).toMatchObject({ disabled: true });
    expect(button!.props.accessibilityHint).toBe(t('phone.queue.runNowAgentOnComputer'));
    act(() => button!.props.onPress());
    expect(onRunNow).not.toHaveBeenCalled();
    expect(onExplain).toHaveBeenCalledWith(t('phone.queue.runNowAgentOnComputer'));
  });
});

describe('editing and moving a queued row (MC-49, "queueEdit")', () => {
  const rows: QueuedItem[] = [
    { id: 'h', text: 'notice', held: true },
    { id: 'a', text: 'first' },
    { id: 'card', text: 'go on', locked: true },
    { id: 'b', text: 'last' },
  ];

  it('offers 고치기 and ↑/↓ only on a host that advertised it, never on a held row, and no 고치기 on a card follow-up', () => {
    expect(rows.map((row) => queueRowEdits(row, true))).toEqual([
      { edit: false, move: false },
      { edit: true, move: true },
      { edit: false, move: true },
      { edit: true, move: true },
    ]);
    for (const row of rows) expect(queueRowEdits(row, false)).toEqual({ edit: false, move: false });
  });

  it('moves a row before the row above it, or before the row two below it (the end when none), never above a held row', () => {
    expect(queueMovePlace(rows, 1, 'up')).toBeUndefined();
    expect(queueMovePlace(rows, 2, 'up')).toEqual({ before: 'a' });
    expect(queueMovePlace(rows, 1, 'down')).toEqual({ before: 'b' });
    expect(queueMovePlace(rows, 2, 'down')).toEqual({ to: 1000 });
    expect(queueMovePlace(rows, 3, 'down')).toBeUndefined();
  });

  it('refuses words none', () => {
    expect(editedQueueWords('  \n ')).toBeUndefined();
    expect(editedQueueWords(' new ')).toBe(' new ');
  });

  describe('the list', () => {
    beforeAll(() => resetLanguage('ko'));
    afterAll(() => resetLanguage());
    let renderer: ReactTestRenderer | undefined;
    afterEach(() => {
      act(() => renderer?.unmount());
      renderer = undefined;
    });
    const labelled = (view: ReactTestRenderer, label: string) =>
      view.root.findAll((node) => typeof node.props.onPress === 'function' && node.props.accessibilityRole === 'button' && String(node.props.accessibilityLabel ?? '').startsWith(label));
    const mount = (editable: boolean, onEdit = jest.fn(async () => true), onMove = jest.fn()) => {
      act(() => {
        renderer = TestRenderer.create(
          createElement(QueuedList, {
            items: rows,
            manageable: true,
            editable,
            busy: false,
            onRemove: jest.fn(),
            onRunNow: jest.fn(),
            onExplain: jest.fn(),
            onEdit,
            onMove,
          }),
        );
      });
      return renderer!;
    };

    it('draws nothing new on a host without it', () => {
      const view = mount(false);
      expect(labelled(view, t('queue.edit'))).toHaveLength(0);
      expect(labelled(view, t('queue.moveUp'))).toHaveLength(0);
    });

    it('moves a row with ↑/↓, the ends dimmed', () => {
      const onMove = jest.fn();
      const view = mount(true, undefined, onMove);
      const ups = labelled(view, t('queue.moveUp'));
      const downs = labelled(view, t('queue.moveDown'));
      expect(ups).toHaveLength(3);
      expect(ups[0]!.props.accessibilityState).toMatchObject({ disabled: true });
      act(() => downs[0]!.props.onPress());
      act(() => ups[1]!.props.onPress());
      expect(onMove.mock.calls).toEqual([
        [rows[1], { before: 'b' }],
        [rows[2], { before: 'a' }],
      ]);
    });

    it('edits a row in place, refuses words none, and closes once the host took them', async () => {
      const onEdit = jest.fn(async () => true);
      const view = mount(true, onEdit);
      const edits = labelled(view, t('queue.edit'));
      expect(edits).toHaveLength(2);
      act(() => edits[0]!.props.onPress());
      const input = view.root.findByType(TextInput);
      expect(input.props.value).toBe('first');
      act(() => input.props.onChangeText('   '));
      expect(view.root.findAll((node) => node.type === Text && [node.props.children].flat().join('') === t('queue.editEmpty'))).not.toHaveLength(0);
      act(() => view.root.findByType(TextInput).props.onChangeText('first, better'));
      // 저장 is labelled by its words alone; the innermost pressable that holds them.
      const saves = view.root.findAll(
        (node) =>
          typeof node.props.onPress === 'function' &&
          node.props.accessibilityRole === 'button' &&
          node.findAllByType(Text).some((text) => [text.props.children].flat().join('') === t('queue.editSave')),
      );
      const save = saves[saves.length - 1];
      await act(async () => {
        save!.props.onPress();
      });
      expect(onEdit).toHaveBeenCalledWith(rows[1], 'first, better');
      expect(view.root.findAllByType(TextInput)).toHaveLength(0);
    });
  });
});
