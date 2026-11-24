import { describe, expect, it } from 'vitest';
import { nextRunWords } from './botTime';

describe('nextRunWords (MC-64)', () => {
  const now = new Date(2026, 10, 25, 8, 0);
  it('shows the time today and the month-day and time on another day', () => {
    expect(nextRunWords(new Date(2026, 10, 25, 9, 5).toISOString(), now)).toBe('09:05');
    expect(nextRunWords(new Date(2026, 10, 26, 21, 30).toISOString(), now)).toBe('11-26 21:30');
  });
  it('shows nothing without a readable time', () => {
    expect(nextRunWords(undefined, now)).toBeUndefined();
    expect(nextRunWords('soon', now)).toBeUndefined();
  });
});
