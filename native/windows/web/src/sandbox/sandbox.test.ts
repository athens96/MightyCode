import { describe, expect, it } from 'vitest';
import { linesOf, sandboxNotice, sandboxed } from './sandboxModel';

const status = { supported: true, available: true, ssh: false, remote: false };

describe('sandbox settings (MC-34)', () => {
  it('says why the switch cannot be turned on', () => {
    expect(sandboxNotice(status)).toBeNull();
    expect(sandboxNotice({ ...status, supported: false })).toBe('sandbox.settings.windows');
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
