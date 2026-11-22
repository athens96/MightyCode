import { describe, expect, it } from 'vitest';
import { KittyNotifications, osc777, osc9, sanitize, signalGap, signalLimiter, textLimit } from './signals';

describe('terminal signals (MC-26)', () => {
  it('reads OSC 9 and leaves ConEmu numbered forms alone', () => {
    expect(osc9('빌드 끝')).toEqual({ type: 'notify', title: '', body: '빌드 끝' });
    expect(osc9('3 tests failed')).toEqual({ type: 'notify', title: '', body: '3 tests failed' });
    expect(osc9('4;1;50')).toBeNull();
    expect(osc9('4;0')).toBeNull();
    expect(osc9('1;500')).toBeNull();
    expect(osc9('12')).toBeNull();
  });

  it('reads OSC 777 notify with a body that holds semicolons', () => {
    expect(osc777('notify;Build;done; all green')).toEqual({ type: 'notify', title: 'Build', body: 'done; all green' });
    expect(osc777('other;x;y')).toBeNull();
  });

  it('reads kitty OSC 99: title by default, body, base64, chunks, and ignores the rest', () => {
    const kitty = new KittyNotifications();
    expect(kitty.take(';Hello')).toEqual({ type: 'notify', title: 'Hello', body: '' });
    expect(kitty.take('i=1:p=body;World')).toEqual({ type: 'notify', title: '', body: 'World' });
    expect(kitty.take('e=1:p=body;7JWI64WV')).toEqual({ type: 'notify', title: '', body: '안녕' });
    expect(kitty.take('i=7:d=0;Deploy')).toBeNull();
    expect(kitty.take('i=7:p=body;finished')).toEqual({ type: 'notify', title: 'Deploy', body: 'finished' });
    expect(kitty.take('i=1:p=close;')).toBeNull();
    expect(kitty.take('e=1;%%%')).toBeNull();
  });

  it('makes the words safe for one line and caps them', () => {
    expect(sanitize('  a\tb\r\n\u001b[31mc\u0007  ')).toBe('a b [31mc');
    expect(sanitize('‮evil')).toBe('evil');
    expect(Array.from(sanitize('가'.repeat(300))).length).toBe(textLimit);
  });

  it('lets one signal through a second', () => {
    const allow = signalLimiter();
    expect(allow(0)).toBe(true);
    expect(allow(500)).toBe(false);
    expect(allow(signalGap)).toBe(true);
  });
});
