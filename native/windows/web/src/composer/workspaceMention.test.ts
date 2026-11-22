import { describe, expect, it } from 'vitest';
import type { Workspace } from '../store/models';
import { applyWorkspaceMention, filterMentionWorkspaces, mentionQuery, workspaceFolderName, workspaceMentionSlug } from './workspaceMention';

const ws = (id: string, name: string, path: string, remote = false): Workspace => ({
  id,
  name,
  path,
  createdAt: '2026-11-22T00:00:00Z',
  ...(remote ? { remote: { connectionId: 'c1', workspaceId: 'r1', hostName: 'box', version: 2 } } : {}),
});

describe('workspaceMention', () => {
  it('parses @ and @workspace: tokens at start or after whitespace only', () => {
    expect(mentionQuery('@', 1)).toEqual({ start: 0, end: 1, query: '' });
    expect(mentionQuery('check @api', 10)).toEqual({ start: 6, end: 10, query: 'api' });
    expect(mentionQuery('sync @workspace:mighty-srv now', 26)).toEqual({ start: 5, end: 26, query: 'mighty-srv' });
    expect(mentionQuery('user@example.com', 16)).toBeNull();
    expect(mentionQuery('@api schema', 11)).toBeNull();
  });

  it('filters out current and remote workspaces and ranks prefix matches first', () => {
    const list = [
      ws('w-cur', 'Current App', '/repos/current-app'),
      ws('w-web', 'Admin Portal', '/repos/mighty-admin'),
      ws('w-srv', 'Mighty Server', '/repos/server-core'),
      ws('w-rem', 'Mighty Remote', '/repos/mighty-remote', true),
    ];
    const all = filterMentionWorkspaces(list, 'w-cur', '');
    expect(all.map((item) => item.id)).toEqual(['w-web', 'w-srv']);

    const ranked = filterMentionWorkspaces(list, 'w-cur', 'mighty');
    // Mighty Server matches name prefix first; Admin Portal matches folder `mighty-admin` prefix too.
    expect(ranked.map((item) => item.id)).toEqual(['w-web', 'w-srv']);

    const sub = filterMentionWorkspaces(list, 'w-cur', 'core');
    expect(sub.map((item) => item.id)).toEqual(['w-srv']);
  });

  it('inserts @workspace:<slug> cleanly with trailing space and preserves suffix', () => {
    expect(workspaceMentionSlug('Mighty Server')).toBe('Mighty-Server');
    expect(workspaceFolderName('/Users/dev/mighty-server/')).toBe('mighty-server');

    const first = applyWorkspaceMention('align DTO with @ser', 15, 19, 'Mighty Server', 1000);
    expect(first).toEqual({
      text: 'align DTO with @workspace:Mighty-Server ',
      cursor: 40,
    });

    const middle = applyWorkspaceMention('check @api please', 6, 10, 'Auth API', 1000);
    expect(middle).toEqual({
      text: 'check @workspace:Auth-API please',
      cursor: 25,
    });
  });
});
