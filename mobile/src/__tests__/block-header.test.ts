import { createElement } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import TestRenderer, { act, type ReactTestInstance, type ReactTestRenderer } from 'react-test-renderer';
import { MightyRunList } from '@/components/mighty-blocks';
import { StatusDot } from '@/components/status-dot';
import { resetLanguage, t } from '@/lib/i18n';
import { normalizeMighty } from '@/lib/mighty';
import { darkPalette, headerTone, lightPalette, toneColors } from '@/theme';

/**
 * Option ⑨ "점 + 색 제목" (the owner's choice of block header): a block's and a result's header stays
 * on the card — no filled strip, no bar, no pill — with a status dot in a soft halo leading the title,
 * a result's title in its status's ink and the status word as the meta, in the secondary ink.
 */

beforeAll(() => resetLanguage('ko'));
afterAll(() => resetLanguage());

function textOf(node: ReactTestInstance): string {
  return node
    .findAllByType(Text)
    .map((text) => [text.props.children].flat(3).filter((part) => typeof part === 'string').join(''))
    .join('');
}

function byTestID(renderer: ReactTestRenderer, id: string): ReactTestInstance {
  const found = renderer.root.findAll((node) => node.props.testID === id && typeof node.type !== 'string');
  if (found.length === 0) throw new Error(`no node ${id}`);
  return found[0]!;
}

function runs(status: string, extra: Record<string, unknown> = {}) {
  return normalizeMighty({
    style: 'cli',
    runs: [
      {
        id: 'r1',
        input: '가이드 고쳐 줘',
        status,
        result: '가이드를 고쳤습니다.',
        blocks: [{ id: 'b1', kind: 'main', title: '요청', status, output: '가이드를 고쳤습니다.' }],
        ...extra,
      },
    ],
  })!.runs;
}

describe('headerTone', () => {
  it.each([
    ['light', lightPalette],
    ['dark', darkPalette],
  ] as const)('dots each status in its fill, haloed in its soft tint, titled in its ink (%s)', (_mode, palette) => {
    for (const tone of ['run', 'wait', 'done', 'err', 'stop'] as const) {
      const colors = toneColors(palette, tone);
      expect(headerTone(palette, tone)).toEqual({ dot: colors.fill, halo: colors.soft, title: colors.ink });
    }
    // A request, or no state at all: the neutral dot and the secondary ink.
    const neutral = { dot: palette.textFaint, halo: palette.surfaceRaised, title: palette.textMuted };
    expect(headerTone(palette, undefined)).toEqual(neutral);
    expect(headerTone(palette, 'idle')).toEqual(neutral);
  });
});

describe('a result card under option ⑨', () => {
  let renderer: ReactTestRenderer | undefined;
  afterEach(() => {
    act(() => renderer?.unmount());
    renderer = undefined;
  });

  function head(status: string, extra?: Record<string, unknown>): ReactTestInstance {
    act(() => {
      renderer = TestRenderer.create(createElement(MightyRunList, { runs: runs(status, extra) }));
    });
    return byTestID(renderer!, 'mighty-result-head-r1');
  }

  /** The heading's ink, as the test renderer draws it: in the light palette (its `useColorScheme` answers light). */
  function headingColor(node: ReactTestInstance): string {
    const heading = node.findAllByType(Text)[0]!;
    return StyleSheet.flatten(heading.props.style).color as string;
  }

  it.each([
    ['completed', 'done'],
    ['error', 'err'],
    ['stopped', 'stop'],
  ] as const)('keeps a %s header on the card, its title in the status ink and its word as the meta', (status, tone) => {
    const node = head(status);
    // No filled strip: the header draws no background of its own.
    expect(StyleSheet.flatten(node.props.style).backgroundColor).toBeUndefined();
    expect(node.findAllByType(StatusDot)[0]!.props.tone).toBe(tone);
    expect(headingColor(node)).toBe(headerTone(lightPalette, tone).title);
    const word = t(status === 'completed' ? 'session.state.completed' : status === 'error' ? 'session.state.error' : 'session.state.stopped');
    expect(textOf(node)).toContain(word);
    expect(node.props.accessibilityLabel).toContain(word);
  });

  it('draws 알림 대기 중 in the running blue, with no status word after its heading', () => {
    const node = head('completed', { awaitingNotice: { delivered: 0, total: 1, tasks: [{ title: 'CI', status: 'running' }] } });
    expect(node.findAllByType(StatusDot)[0]!.props.tone).toBe('run');
    expect(headingColor(node)).toBe(lightPalette.accent);
    expect(textOf(node)).toBe(t('graph.block.awaitingNotice'));
  });

  it('draws the result so far in the secondary ink', () => {
    const node = head('completed', { backgroundWaiting: 2 });
    expect(node.findAllByType(StatusDot)[0]!.props.tone).toBeUndefined();
    expect(headingColor(node)).toBe(lightPalette.textMuted);
  });

  it('leads the request and its blocks with a dot instead of a pill', () => {
    act(() => {
      renderer = TestRenderer.create(createElement(MightyRunList, { runs: runs('running') }));
    });
    expect(byTestID(renderer!, 'mighty-run-dot-r1').props.tone).toBe('run');
    expect(byTestID(renderer!, 'mighty-dot-b1').props.tone).toBe('run');
    // The dot is a mark only: hidden from screen readers, whose word is the meta beside it.
    const halo = renderer!.root.findAll((node) => node.type === View && node.props.testID === 'mighty-dot-b1')[0]!;
    expect(halo.props.importantForAccessibility).toBe('no-hide-descendants');
    expect(textOf(byTestID(renderer!, 'mighty-block-b1'))).toContain(t('session.state.running'));
  });
});
