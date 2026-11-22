import { createElement } from 'react';
import TestRenderer, { act, type ReactTestRenderer } from 'react-test-renderer';
import type { MobileSavedPrompt } from '@/api/types';
import { Composer } from '@/components/composer';
import { resetLanguage, t } from '@/lib/i18n';

const PROMPTS: MobileSavedPrompt[] = [
  { id: 'p-1', name: 'review', text: 'Review my diff carefully', scope: 'workspace' },
  { id: 'p-2', name: 'ship', text: 'Run tests and prepare PR', scope: 'all' },
];

describe('saved prompts strip in Composer', () => {
  beforeAll(() => resetLanguage('en'));
  afterAll(() => resetLanguage());

  function mount(overrides: Partial<Parameters<typeof Composer>[0]> = {}) {
    const sent: MobileSavedPrompt[] = [];
    const inserted: MobileSavedPrompt[] = [];
    let renderer: ReactTestRenderer | undefined;
    act(() => {
      renderer = TestRenderer.create(
        createElement(Composer, {
          text: '',
          onChangeText: () => undefined,
          disabled: false,
          terminal: false,
          running: false,
          sending: false,
          submitModes: true,
          commands: [],
          savedPrompts: PROMPTS,
          onSend: () => undefined,
          onSendSavedPrompt: (prompt) => sent.push(prompt),
          onInsertSavedPrompt: (prompt) => inserted.push(prompt),
          onStop: () => undefined,
          onCommand: () => undefined,
          ...overrides,
        }),
      );
    });
    return { renderer: renderer!, sent, inserted };
  }

  it('renders /prompt:<name> chips with hint and toolbar label, firing send on tap and insert on long-press', () => {
    const { renderer, sent, inserted } = mount();
    const toolbar = renderer.root.find(
      (node) => node.props.accessibilityRole === 'toolbar' && node.props.accessibilityLabel === t('phone.prompts.label'),
    );
    expect(toolbar).toBeDefined();

    const reviewChip = renderer.root.find(
      (node) => typeof node.props.onPress === 'function' && node.props.accessibilityLabel === '/prompt:review',
    );
    expect(reviewChip.props.accessibilityHint).toBe(t('phone.prompts.hint'));
    expect(reviewChip.props.disabled).toBe(false);

    act(() => reviewChip.props.onPress());
    expect(sent).toEqual([PROMPTS[0]]);
    expect(inserted).toEqual([]);

    act(() => reviewChip.props.onLongPress());
    expect(inserted).toEqual([PROMPTS[0]]);
    act(() => renderer.unmount());
  });

  it('disables chip presses while sending is in flight and hides the strip when empty or disabled', () => {
    const busy = mount({ sending: true });
    const busyChip = busy.renderer.root.find(
      (node) => typeof node.props.onPress === 'function' && node.props.accessibilityLabel === '/prompt:ship',
    );
    expect(busyChip.props.disabled).toBe(true);
    expect(busyChip.props.accessibilityState).toEqual({ busy: true, disabled: true });
    act(() => busy.renderer.unmount());

    const empty = mount({ savedPrompts: [] });
    expect(empty.renderer.root.findAll((node) => node.props.accessibilityLabel === '/prompt:review')).toHaveLength(0);
    act(() => empty.renderer.unmount());

    const disabled = mount({ disabled: true });
    expect(disabled.renderer.root.findAll((node) => node.props.accessibilityLabel === '/prompt:review')).toHaveLength(0);
    act(() => disabled.renderer.unmount());
  });
});
