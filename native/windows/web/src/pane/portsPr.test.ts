import { describe, expect, it } from 'vitest';
import type { PanePr } from '../shell/types';
import { portUrl, portsText, prTone } from './portsPr';

const pr = (state: PanePr['state'], checks: PanePr['checks']): PanePr => ({ number: 12, state, checks, url: 'https://x/pull/12', help: 'PR #12' });

describe("a pane's ports and pull request (MC-26)", () => {
  it('shows every port, or the first and how many more when compact', () => {
    expect(portsText([], false)).toBe('');
    expect(portsText([3000, 5173], false)).toBe(':3000 :5173');
    expect(portsText([3000, 5173, 8080], true)).toBe(':3000 +2');
    expect(portsText([3000], true)).toBe(':3000');
    expect(portUrl(5173)).toBe('http://localhost:5173');
  });

  it('reads the checks first, then the state', () => {
    expect(prTone(pr('open', 'failed'))).toBe('err');
    expect(prTone(pr('open', 'pending'))).toBe('wait');
    expect(prTone(pr('open', 'passed'))).toBe('done');
    expect(prTone(pr('open', 'none'))).toBe('run');
    expect(prTone(pr('draft', 'none'))).toBe('idle');
    expect(prTone(pr('merged', 'failed'))).toBe('done');
    expect(prTone(pr('closed', 'passed'))).toBe('idle');
  });
});
