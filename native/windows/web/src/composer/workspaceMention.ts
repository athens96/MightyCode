import type { Workspace } from '../store/models';

export interface WorkspaceMentionQuery {
  /** Index of `@` in `draft`. */
  start: number;
  /** Caret offset at the end of the `@...` token. */
  end: number;
  /** Filter query after `@` or `@workspace:`. */
  query: string;
}

const MAX_MENTION_LENGTH = 64;

/** Converts a workspace name into a whitespace-free `@workspace:<slug>` token matching `workspace_mcp::mention_slug`. */
export function workspaceMentionSlug(name: string): string {
  return name.trim().split(/\s+/).filter(Boolean).join('-');
}

/** Basename of a workspace folder path (`/` or `\`). */
export function workspaceFolderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '');
  const split = trimmed.split(/[\\/]/);
  return split[split.length - 1] || path;
}

/**
 * Parses an active `@` or `@workspace:` mention token ending at `caret`.
 * Triggers only when `@` is at index 0 or preceded by whitespace, and no whitespace sits inside the token.
 */
export function mentionQuery(draft: string, caret: number): WorkspaceMentionQuery | null {
  const pos = Math.max(0, Math.min(caret, draft.length));
  const before = draft.slice(0, pos);
  const at = before.lastIndexOf('@');
  if (at < 0) return null;
  if (at > 0 && !/\s/.test(before[at - 1]!)) return null;
  const token = before.slice(at + 1);
  if (/\s/.test(token) || token.length > MAX_MENTION_LENGTH) return null;
  // Ignore file line references or email-like fragments when already closed, but allow `@` and `@workspace:...`.
  const query = token.toLowerCase().startsWith('workspace:') ? token.slice('workspace:'.length) : token;
  return { start: at, end: pos, query };
}

/**
 * Filters local workspaces (excluding the pane's own workspace and remote/mirror workspaces),
 * ranked by prefix match on name/slug/folder, then substring match.
 */
export function filterMentionWorkspaces(
  workspaces: readonly Workspace[],
  currentWorkspaceId: string,
  query: string,
): Workspace[] {
  const candidates = workspaces.filter(
    (ws) => !ws.remote && ws.id !== currentWorkspaceId && ws.name.trim().length > 0 && ws.path.trim().length > 0,
  );
  const needle = query.trim().toLowerCase();
  if (!needle) return candidates;
  const prefix: Workspace[] = [];
  const contains: Workspace[] = [];
  for (const ws of candidates) {
    const name = ws.name.toLowerCase();
    const slug = workspaceMentionSlug(ws.name).toLowerCase();
    const folder = workspaceFolderName(ws.path).toLowerCase();
    if (name.startsWith(needle) || slug.startsWith(needle) || folder.startsWith(needle)) {
      prefix.push(ws);
    } else if (name.includes(needle) || slug.includes(needle) || folder.includes(needle)) {
      contains.push(ws);
    }
  }
  return [...prefix, ...contains];
}

/** Replaces `[start, end)` in `draft` with `@workspace:<slug> ` and returns the updated text and caret position. */
export function applyWorkspaceMention(
  draft: string,
  start: number,
  end: number,
  workspaceName: string,
  maxLength: number,
): { text: string; cursor: number } {
  const slug = workspaceMentionSlug(workspaceName);
  const token = `@workspace:${slug}`;
  const tail = draft.slice(end);
  const needsSpace = tail.length === 0 || !/^\s/.test(tail);
  const inserted = needsSpace ? `${token} ` : token;
  const combined = (draft.slice(0, start) + inserted + tail).slice(0, maxLength);
  const cursor = Math.min(start + inserted.length, combined.length);
  return { text: combined, cursor };
}
