import { describe, expect, it } from 'vitest';
import type { MightyGraphRun } from '../store/models';
import { atDraft, onFirstLine, onLastLine, requestHistory, walkDown, walkUp } from './history';

const run = (input: string, noticeRun = false) => ({ id: input, input, status: 'completed', rootEntries: [], agents: [], resultEntries: [], noticeRun }) as MightyGraphRun;

describe('MC-106 request history', () => {
  it('lists what the person sent, oldest first, without notice replies or repeats in a row', () => {
    expect(requestHistory([run('a'), run('  '), run('b'), run('b'), run('notice', true), run('a')])).toEqual(['a', 'b', 'a']);
    expect(requestHistory(undefined)).toEqual([]);
  });

  it('knows the first and last line of the caret', () => {
    expect(onFirstLine('one\ntwo', 2)).toBe(true);
    expect(onFirstLine('one\ntwo', 5)).toBe(false);
    expect(onLastLine('one\ntwo', 5)).toBe(true);
    expect(onLastLine('one\ntwo', 2)).toBe(false);
  });

  it('walks up through earlier requests and back down to the words being typed', () => {
    const history = ['first', 'second', 'third'];
    const up1 = walkUp(atDraft, history, 'typing');
    expect(up1?.text).toBe('third');
    const up2 = walkUp(up1!.walk, history, up1!.text);
    expect(up2?.text).toBe('second');
    const up3 = walkUp(up2!.walk, history, up2!.text);
    expect(up3?.text).toBe('first');
    expect(walkUp(up3!.walk, history, up3!.text)).toBeNull();
    const down1 = walkDown(up3!.walk, history);
    expect(down1?.text).toBe('second');
    const down2 = walkDown(down1!.walk, history);
    expect(down2?.text).toBe('third');
    const back = walkDown(down2!.walk, history);
    expect(back).toEqual({ walk: atDraft, text: 'typing' });
    expect(walkDown(back!.walk, history)).toBeNull();
  });

  it('does nothing without history', () => {
    expect(walkUp(atDraft, [], 'x')).toBeNull();
  });
});
