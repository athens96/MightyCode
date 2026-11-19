import { describe, expect, it } from 'vitest';
import { logText } from './log';
import type { LogEntry } from '../store/models';

const t = (key: string) => key;
const entry = (kind: LogEntry['kind'], text: string, provider?: string): LogEntry => ({ id: text, kind, text, timestamp: '2026-11-19T00:00:00Z', provider });

describe('the pane menu copy of the log', () => {
  it('writes each entry as [role] text, a blank line between, as the Mac did', () => {
    const logs = [entry('user', 'hi'), entry('assistant', 'hello'), entry('assistant', 'from codex', 'codex'), entry('output', 'ls'), entry('error', 'boom'), entry('system', 'note')];
    expect(logText(logs, 'claude', t)).toBe(
      '[pane.log.roleUser] hi\n\n[Claude] hello\n\n[Codex] from codex\n\n[composer.sessionInfo.output] ls\n\n[session.state.error] boom\n\n[pane.log.roleSystem] note');
  });
  it('is empty for an empty log', () => {
    expect(logText([], 'claude', t)).toBe('');
  });
});
