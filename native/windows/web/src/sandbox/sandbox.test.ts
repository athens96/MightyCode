import { describe, expect, it } from 'vitest';
import { CHANGE_KINDS, KIND_KEYS, groupChanges, keyOf, levelTone, linesOf, pendingChanges, sandboxNotice, sandboxed } from './sandboxModel';

const status = { supported: true, available: true, ssh: false, remote: false };

describe('sandbox settings (MC-34)', () => {
  it('says why the switch cannot be turned on', () => {
    expect(sandboxNotice(status)).toBeNull();
    expect(sandboxNotice({ ...status, supported: false })).toBe('sandbox.settings.unsupported');
    expect(sandboxNotice({ ...status, ssh: true })).toBe('sandbox.settings.ssh');
    expect(sandboxNotice({ ...status, remote: true })).toBe('sandbox.settings.remote');
    expect(sandboxNotice({ ...status, available: false })).toBe('sandbox.settings.unavailable');
  });

  it('reads a text box as trimmed lines, each once', () => {
    expect(linesOf(' github.com \n\nregistry.npmjs.org\r\ngithub.com\n')).toEqual(['github.com', 'registry.npmjs.org']);
    expect(linesOf('')).toEqual([]);
  });

  it('badges only a local workspace with the sandbox on', () => {
    expect(sandboxed({ sandbox: { enabled: true } })).toBe(true);
    expect(sandboxed({ sandbox: { enabled: false } })).toBe(false);
    expect(sandboxed({ sandbox: { enabled: true }, ssh: { machine: 'm', dir: '~', host: 'h' } })).toBe(false);
    expect(sandboxed(undefined)).toBe(false);
  });
});

describe('the Windows sandbox approval (MC-42)', () => {
  const change = (path: string, kind: string, op: string, made = false) => ({ path, kind, op, made, exists: true });

  it('groups the changes by why, each path once with all its changes', () => {
    const groups = groupChanges([
      change('C:\\Users\\me\\.ssh', 'secret', 'secretLabel', true),
      change('C:\\work\\app', 'workspace', 'lowLabel'),
      change('C:\\work\\app', 'workspace', 'allow', true),
      change('C:\\work\\app\\.git\\config', 'git', 'deny'),
    ]);
    expect(groups.map((g) => g.kind)).toEqual(['workspace', 'git', 'secret']);
    expect(groups[0]?.items).toEqual([{ path: 'C:\\work\\app', ops: ['lowLabel', 'allow'], made: false, agent: undefined }]);
    expect(groups[2]?.items[0]?.made).toBe(true);
  });

  it('counts what still waits and colours the levels', () => {
    expect(pendingChanges({ approved: false, changes: [change('a', 'git', 'deny'), change('b', 'git', 'deny', true)], recorded: 1 })).toBe(1);
    expect(pendingChanges(null)).toBe(0);
    expect([levelTone('enforced'), levelTone('partial'), levelTone('advisory')]).toEqual(['ok', 'warn', 'err']);
  });

  it('has a locale key for every change kind', () => {
    for (const kind of CHANGE_KINDS) expect(keyOf(KIND_KEYS, kind)).toBe('sandbox.windows.kind.' + kind);
    expect(keyOf(KIND_KEYS, 'unknown')).toBe('unknown');
  });
});
