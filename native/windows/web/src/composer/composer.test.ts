import { describe, expect, it } from 'vitest';
import { attachmentRefusal, byteLabel, maximumFileBytes, toBase64 } from './attachments';
import { enterAction, runSettingsEnter, secondSendWords, type EnterKey } from './keys';

const enter = (over: Partial<EnterKey> = {}): EnterKey => ({ key: 'Enter', isComposing: false, keyCode: 13, shiftKey: false, altKey: false, metaKey: false, ctrlKey: false, repeat: false, ...over });

describe('enterAction', () => {
  it('sends on a plain Enter and second-sends with Ctrl', () => {
    expect(enterAction(enter())).toBe('send');
    expect(enterAction(enter({ ctrlKey: true }))).toBe('second');
  });
  it('never sends during an IME composition', () => {
    expect(enterAction(enter({ isComposing: true }))).toBe('pass');
    expect(enterAction(enter({ keyCode: 229 }))).toBe('pass');
  });
  it('leaves Shift, Alt and the Windows key to the editor', () => {
    expect(enterAction(enter({ shiftKey: true }))).toBe('pass');
    expect(enterAction(enter({ altKey: true }))).toBe('pass');
    expect(enterAction(enter({ metaKey: true }))).toBe('pass');
    expect(enterAction(enter({ metaKey: true, ctrlKey: true }))).toBe('pass');
  });
  it('second-sends with ⌘ on the Mac, and still with Ctrl', () => {
    expect(enterAction(enter(), true)).toBe('send');
    expect(enterAction(enter({ metaKey: true }), true)).toBe('second');
    expect(enterAction(enter({ ctrlKey: true }), true)).toBe('second');
    expect(enterAction(enter({ metaKey: true, ctrlKey: true }), true)).toBe('pass');
    expect(enterAction(enter({ metaKey: true, shiftKey: true }), true)).toBe('pass');
    expect(enterAction(enter({ metaKey: true, altKey: true }), true)).toBe('pass');
    expect(enterAction(enter({ metaKey: true, isComposing: true }), true)).toBe('pass');
    expect(enterAction(enter({ metaKey: true, keyCode: 229 }), true)).toBe('pass');
    expect(enterAction(enter({ metaKey: true, repeat: true }), true)).toBe('swallow');
  });
  it('swallows a repeating Enter', () => {
    expect(enterAction(enter({ repeat: true }))).toBe('swallow');
  });
  it('passes other keys', () => {
    expect(enterAction(enter({ key: 'a' }))).toBe('pass');
  });
});

describe('secondSendWords', () => {
  it('names ⌘Enter on the Mac and leaves Windows and other words alone', () => {
    expect(secondSendWords('composer.placeholder.busyWorktree', true)).toBe('composer.placeholder.busyWorktreeMac');
    expect(secondSendWords('queue.waitingOnBackground.windowsWorktree', true)).toBe('queue.waitingOnBackground.macWorktree');
    expect(secondSendWords('queue.waitingOnBackground.windows', true)).toBe('queue.waitingOnBackground.mac');
    expect(secondSendWords('composer.placeholder.busyWorktree', false)).toBe('composer.placeholder.busyWorktree');
    expect(secondSendWords('composer.placeholder.idle', true)).toBe('composer.placeholder.idle');
  });
});

describe('runSettingsEnter', () => {
  it('applies on a plain Enter only, as the Mac Apply button (.defaultAction)', () => {
    expect(runSettingsEnter(enter())).toBe('apply');
    expect(runSettingsEnter(enter({ ctrlKey: true }))).toBe('pass');
    expect(runSettingsEnter(enter({ shiftKey: true }))).toBe('pass');
    expect(runSettingsEnter(enter({ altKey: true }))).toBe('pass');
    expect(runSettingsEnter(enter({ metaKey: true }))).toBe('pass');
  });
  it('never applies during an IME composition, and swallows a repeat', () => {
    expect(runSettingsEnter(enter({ isComposing: true }))).toBe('pass');
    expect(runSettingsEnter(enter({ keyCode: 229 }))).toBe('pass');
    expect(runSettingsEnter(enter({ repeat: true }))).toBe('swallow');
    expect(runSettingsEnter(enter({ key: 'a' }))).toBe('pass');
  });
});

describe('attachments', () => {
  it('refuses with the host keys', () => {
    expect(attachmentRefusal([{ name: 'a', size: 1 }], 8, 0)).toBe('composer.attachment.tooMany');
    expect(attachmentRefusal([{ name: 'a', size: maximumFileBytes + 1 }], 0, 0)).toBe('composer.attachment.fileTooLarge');
    expect(attachmentRefusal([{ name: 'a', size: 4_000_000 }, { name: 'b', size: 4_500_000 }], 0, 0)).toBe('attachments.error.totalTooLarge');
    expect(attachmentRefusal([{ name: 'a', size: 10 }], 0, 0)).toBeNull();
  });
  it('labels sizes as the chips do', () => {
    expect(byteLabel(999)).toBe('999 bytes');
    expect(byteLabel(1500)).toBe('2 KB');
    expect(byteLabel(2_340_000)).toBe('2.3 MB');
    expect(byteLabel(3_000_000)).toBe('3 MB');
  });
  it('encodes base64', () => {
    expect(toBase64(new TextEncoder().encode('hello').buffer as ArrayBuffer)).toBe('aGVsbG8=');
  });
});
