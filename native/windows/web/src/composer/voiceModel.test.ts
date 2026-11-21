import { describe, expect, it } from 'vitest';
import {
  applyWorkspaceCodeHints,
  formatVoiceDuration,
  insertDictatedText,
  isPushToTalkShortcut,
  normalizeWaveformBars,
} from './voiceModel';

describe('composer voiceModel (MC-4)', () => {
  it('matches Push-to-Talk shortcut ⇧⌘M on Mac and Ctrl+Shift+M on Windows', () => {
    expect(isPushToTalkShortcut({ key: 'M', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false }, true)).toBe(true);
    expect(isPushToTalkShortcut({ key: 'm', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false }, true)).toBe(false);
    expect(isPushToTalkShortcut({ key: 'M', metaKey: false, ctrlKey: true, shiftKey: true, altKey: false }, false)).toBe(true);
    expect(isPushToTalkShortcut({ key: 'M', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false }, false)).toBe(false);
  });

  it('normalizes waveform levels and formats duration cleanly', () => {
    const bars = normalizeWaveformBars([0.5, 0.9, 1.4, -0.2], 6);
    expect(bars).toEqual([0.14, 0.14, 0.5, 0.9, 1, 0.14]);
    expect(formatVoiceDuration(0)).toBe('0:00');
    expect(formatVoiceDuration(9)).toBe('0:09');
    expect(formatVoiceDuration(75)).toBe('1:15');
  });

  it('restores workspace code symbols and spoken file extensions', () => {
    const hints = ['FilesView.tsx', 'CodeEditor', 'Composer.tsx', 'mighty-files'];
    expect(applyWorkspaceCodeHints('composer dot tsx 와 codeeditor 를 점검해줘', hints)).toBe(
      'Composer.tsx 와 CodeEditor 를 점검해줘',
    );
    expect(applyWorkspaceCodeHints('Update filesview 점 tsx.', hints, '점')).toBe('Update FilesView.tsx.');
  });

  it('inserts dictated text at cursor with smart spacing', () => {
    expect(insertDictatedText('', '버그를 고쳐줘')).toEqual({ text: '버그를 고쳐줘', cursor: 7 });
    expect(insertDictatedText('FilesView.tsx에서', '줄 번호 확인해줘', 15, 15)).toEqual({
      text: 'FilesView.tsx에서 줄 번호 확인해줘',
      cursor: 25,
    });
    const mid = insertDictatedText('앞문장 뒷문장', '중간', 4, 4);
    expect(mid.text).toBe('앞문장 중간 뒷문장');
  });
});
