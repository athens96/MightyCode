import { describe, expect, it } from 'vitest';
import { emptyState, type StoreState } from '../../store/store';
import type { ShellModel } from '../../shell/types';
import {
  choiceWindow, chooseKeyboard, focusVerdict, inDialog, InputTracker, navigationWindow, pageCovered, pressWindow, subscribeChoices,
  TerminalKeyboard, typingIn, type FocusedElement, type FocusInputs,
} from './focus';

/** A fake element: its tag, its input type, and the selectors of the ancestors it sits in (itself included). */
function element(tagName: string, options: { type?: string; editable?: boolean; role?: string; within?: string[]; connected?: boolean } = {}): FocusedElement {
  const within = options.within ?? [];
  return {
    tagName,
    type: options.type,
    isContentEditable: options.editable ?? false,
    isConnected: options.connected ?? true,
    getAttribute: (name) => (name === 'role' ? options.role ?? null : null),
    closest: (selector) => selector.split(',').map((part) => part.trim()).some((part) => within.includes(part)) || null,
  };
}

const body = element('BODY');
const composer = element('TEXTAREA', { within: ['.composer'] });
const sidebarButton = element('BUTTON');
const tab = element('BUTTON', { role: 'tab' });

describe('typing in an element', () => {
  it('is a text field, a text area, a select or editable text', () => {
    expect(typingIn(element('INPUT'))).toBe(true);
    expect(typingIn(element('INPUT', { type: 'search' }))).toBe(true);
    expect(typingIn(composer)).toBe(true);
    expect(typingIn(element('SELECT'))).toBe(true);
    expect(typingIn(element('DIV', { editable: true }))).toBe(true);
    expect(typingIn(element('DIV', { role: 'textbox' }))).toBe(true);
    expect(typingIn(element('DIV', { role: 'combobox' }))).toBe(true);
  });

  it('is not a control, the body, a terminal’s own keyboard, a hidden pane’s field or a removed one', () => {
    expect(typingIn(null)).toBe(false);
    expect(typingIn(body)).toBe(false);
    expect(typingIn(sidebarButton)).toBe(false);
    expect(typingIn(element('INPUT', { type: 'checkbox' }))).toBe(false);
    expect(typingIn(element('INPUT', { type: 'button' }))).toBe(false);
    // Another terminal pane's xterm (its helper textarea) or a native terminal's slot: switching terminals is not typing elsewhere.
    expect(typingIn(element('TEXTAREA', { within: ['.terminal-surface'] }))).toBe(false);
    expect(typingIn(element('DIV', { within: ['[data-native-slot]'] }))).toBe(false);
    expect(typingIn(element('TEXTAREA', { within: ['[aria-hidden="true"]'] }))).toBe(false);
    expect(typingIn(element('TEXTAREA', { connected: false }))).toBe(false);
  });

  it('knows focus in a dialog, a menu or a page overlay', () => {
    expect(inDialog(element('BUTTON', { within: ['[role="dialog"]'] }))).toBe(true);
    expect(inDialog(element('INPUT', { within: ['[aria-modal="true"]'] }))).toBe(true);
    expect(inDialog(element('BUTTON', { within: ['[role="menu"]'] }))).toBe(true);
    expect(inDialog(element('DIV', { within: ['[data-page-overlay]'] }))).toBe(true);
    expect(inDialog(sidebarButton)).toBe(false);
    expect(inDialog(null)).toBe(false);
  });
});

describe('page cover', () => {
  const state = (patch: Partial<StoreState>): StoreState => ({ ...emptyState, ...patch });
  it('is any dialog, host view, frame dialog or page overlay, wherever it stands', () => {
    expect(pageCovered(state({}))).toBe(false);
    expect(pageCovered(state({ overlays: 1 }))).toBe(true);
    expect(pageCovered(state({ shell: { dialogOpen: true } as ShellModel }))).toBe(true);
    expect(pageCovered(state({ dialogs: [{ id: 1, dialog: { type: 'confirm', title: '', message: '', close: '', defaultPrimary: false } }] }))).toBe(true);
    expect(pageCovered(state({ natives: [{ id: 1, place: 'dialog', view: null }] as unknown as StoreState['natives'] }))).toBe(true);
  });
});

describe('focus verdict', () => {
  const inputs = (patch: Partial<FocusInputs> = {}): FocusInputs => ({ live: true, focused: body, covered: false, navigating: false, chosen: false, ...patch });

  it('takes the keyboard for the active shown pane when nothing else holds it', () => {
    expect(focusVerdict(inputs())).toBe('take');
    expect(focusVerdict(inputs({ focused: sidebarButton }))).toBe('take');
    expect(focusVerdict(inputs({ focused: element('TEXTAREA', { within: ['.terminal-surface'] }) }))).toBe('take');
  });

  it('forgets it for a pane no longer active, arrow moves, or a field being typed in', () => {
    expect(focusVerdict(inputs({ live: false }))).toBe('drop');
    expect(focusVerdict(inputs({ navigating: true, focused: tab }))).toBe('drop');
    expect(focusVerdict(inputs({ focused: composer }))).toBe('drop');
    expect(focusVerdict(inputs({ focused: element('INPUT') }))).toBe('drop');
  });

  it('waits while a dialog, a menu or a popover is open', () => {
    expect(focusVerdict(inputs({ covered: true }))).toBe('wait');
    expect(focusVerdict(inputs({ covered: true, focused: element('INPUT', { within: ['[role="dialog"]'] }) }))).toBe('wait');
    expect(focusVerdict(inputs({ focused: element('INPUT', { within: ['[role="dialog"]'] }) }))).toBe('wait');
  });

  it('leaves a field behind when the person chose the pane, but never moves on an arrow', () => {
    expect(focusVerdict(inputs({ chosen: true, focused: element('INPUT', { type: 'search' }) }))).toBe('take');
    expect(focusVerdict(inputs({ chosen: true, navigating: true }))).toBe('drop');
    expect(focusVerdict(inputs({ chosen: true, covered: true }))).toBe('wait');
  });
});

describe('input tracker', () => {
  it('counts a plain arrow, Home/End or Page key as moving, for a moment', () => {
    const tracker = new InputTracker();
    expect(tracker.navigating(0)).toBe(false);
    tracker.key({ key: 'ArrowRight' }, 100);
    expect(tracker.navigating(100 + navigationWindow - 1)).toBe(true);
    expect(tracker.navigating(100 + navigationWindow)).toBe(false);
    for (const key of ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'Home', 'End', 'PageUp', 'PageDown']) {
      tracker.key({ key }, 0);
      expect(tracker.navigating(1)).toBe(true);
    }
  });

  it('does not count a shortcut, Enter, or typing in a terminal as moving', () => {
    const tracker = new InputTracker();
    tracker.key({ key: 'ArrowRight', metaKey: true }, 0);
    expect(tracker.navigating(1)).toBe(false);
    tracker.key({ key: 'ArrowDown', altKey: true }, 0);
    expect(tracker.navigating(1)).toBe(false);
    tracker.key({ key: 'ArrowUp', ctrlKey: true }, 0);
    expect(tracker.navigating(1)).toBe(false);
    tracker.key({ key: 'ArrowRight' }, 0);
    tracker.key({ key: 'Enter' }, 1);
    expect(tracker.navigating(2)).toBe(false);
    tracker.key({ key: 'ArrowUp', inTerminal: true }, 0);
    expect(tracker.navigating(1)).toBe(false);
  });

  it('remembers a press for a moment, and a key replaces it', () => {
    const tracker = new InputTracker();
    tracker.press(50);
    expect(tracker.pressedLately(50 + pressWindow - 1)).toBe(true);
    expect(tracker.pressedLately(50 + pressWindow)).toBe(false);
    expect(tracker.navigating(51)).toBe(false);
    tracker.key({ key: 'a' }, 60);
    expect(tracker.pressedLately(61)).toBe(false);
  });
});

describe('a terminal pane’s keyboard', () => {
  /** The keyboard over a world the test sets: what has focus, a cover, arrow moves, whether the terminal is ready, the clock. */
  function harness() {
    const world = { focused: body as FocusedElement | null, covered: false, navigating: false, ready: true, now: 0, taken: 0 };
    const keyboard = new TerminalKeyboard({
      read: () => ({ focused: world.focused, covered: world.covered, navigating: world.navigating }),
      take: () => {
        if (!world.ready) return false;
        world.taken += 1;
        return true;
      },
      now: () => world.now,
    });
    return { world, keyboard };
  }

  it('takes the keyboard once when the pane becomes the active one (a click, a shortcut, the attention jump, a new pane)', () => {
    const { world, keyboard } = harness();
    keyboard.observe(false, false, true);
    expect(world.taken).toBe(0);
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(1);
    // Another render of the same active pane asks nothing more.
    keyboard.observe(true, false, true);
    keyboard.attempt();
    expect(world.taken).toBe(1);
    expect(keyboard.waiting).toBe(false);
    // Away and back: once more.
    keyboard.observe(false, false, true);
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(2);
  });

  it('waits for the terminal to be ready (xterm mounted, the native view’s slot shown), then takes it once', () => {
    const { world, keyboard } = harness();
    world.ready = false;
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(0);
    expect(keyboard.waiting).toBe(true);
    world.ready = true;
    expect(keyboard.attempt()).toBe(true);
    expect(keyboard.attempt()).toBe(false);
    expect(world.taken).toBe(1);
  });

  it('never takes it from a field the person is typing in', () => {
    const { world, keyboard } = harness();
    world.focused = composer;
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(0);
    expect(keyboard.waiting).toBe(false);
    // The request is forgotten: the terminal becoming ready later takes nothing.
    world.focused = body;
    keyboard.attempt();
    expect(world.taken).toBe(0);
  });

  it('never takes it while a dialog or popover is open, and takes it when that goes', () => {
    const { world, keyboard } = harness();
    world.covered = true;
    keyboard.observe(true, true, true);
    expect(world.taken).toBe(0);
    expect(keyboard.waiting).toBe(true);
    world.covered = false;
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(1);
  });

  it('does not take it for a pane selected by arrowing through the tabs', () => {
    const { world, keyboard } = harness();
    world.navigating = true;
    world.focused = tab;
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(0);
    expect(keyboard.waiting).toBe(false);
  });

  it('takes it when the person chooses the pane by its row or tab, even already active and from a field they typed in', () => {
    const { world, keyboard } = harness();
    world.focused = element('INPUT', { type: 'search' });
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(0);
    keyboard.choose();
    expect(world.taken).toBe(1);
    // A choice made before the pane shows (a sidebar row) is used when it becomes the active one.
    keyboard.observe(false, false, true);
    keyboard.choose();
    expect(world.taken).toBe(1);
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(2);
  });

  it('lets a choice leave a field behind only for a moment', () => {
    const { world, keyboard } = harness();
    world.ready = false;
    keyboard.observe(true, false, true);
    keyboard.choose();
    world.now = choiceWindow;
    world.focused = composer;
    world.ready = true;
    keyboard.attempt();
    expect(world.taken).toBe(0);
  });

  it('takes it when the window comes back to the active pane, not when a press in the page brought it', () => {
    const { world, keyboard } = harness();
    keyboard.windowFocused(false);
    expect(world.taken).toBe(0);
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(1);
    keyboard.windowFocused(true);
    expect(world.taken).toBe(1);
    keyboard.windowFocused(false);
    expect(world.taken).toBe(2);
    world.focused = composer;
    keyboard.windowFocused(false);
    expect(world.taken).toBe(2);
  });

  it('gives a native terminal the keyboard back after a cover that took it, not one the page held', () => {
    const { world, keyboard } = harness();
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(1);
    // The terminal held the keyboard (the page did not) when a dialog came over it.
    world.covered = true;
    keyboard.observe(true, true, false);
    world.covered = false;
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(2);
    // The page held it (a click opened the popover): it stays where the page puts it.
    world.covered = true;
    keyboard.observe(true, true, true);
    world.covered = false;
    keyboard.observe(true, false, true);
    expect(world.taken).toBe(2);
  });

  it('forgets a waiting request when the pane stops being the active one', () => {
    const { world, keyboard } = harness();
    world.covered = true;
    keyboard.observe(true, true, true);
    expect(keyboard.waiting).toBe(true);
    keyboard.observe(false, true, true);
    world.covered = false;
    keyboard.observe(false, false, true);
    keyboard.attempt();
    expect(world.taken).toBe(0);
  });

  it('hears the choices of its own pane only', () => {
    const heard: string[] = [];
    const stop = subscribeChoices((id) => heard.push(id));
    chooseKeyboard('a');
    chooseKeyboard('b');
    stop();
    chooseKeyboard('c');
    expect(heard).toEqual(['a', 'b']);
  });
});
