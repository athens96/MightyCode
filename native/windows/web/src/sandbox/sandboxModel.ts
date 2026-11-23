// MC-34: what the sandbox settings box reads from the host (sandbox.rs `sandboxGet`) and the rules it shows them by.
import type { Workspace } from '../store/models';

export interface SandboxBlock {
  pane: string;
  /** `network` (a host off the allow list) or `tool` (a request the reviewer denied). */
  kind: string;
  what: string;
  reason: string;
  at: string;
}

/** One promise of the sandbox and how far this OS keeps it (sandbox/windows.rs `guarantees`). */
export interface SandboxGuarantee {
  /** `writes`, `git`, `instructions`, `secrets`, `otherAgents`, `network`, `desktop`, `processes`. */
  key: string;
  level: 'enforced' | 'partial' | 'advisory';
}

/** MC-42: one change the Windows sandbox makes to a folder (a label or an access-list entry). */
export interface SandboxChange {
  path: string;
  /** `workspace`, `worktree`, `git`, `instructions`, `readOnly`, `agent`, `secret`, `appData`, `private`. */
  kind: string;
  /** `lowLabel`, `secretLabel`, `readOnlyLabel`, `allow`, `deny`, `denyDelete`. */
  op: string;
  agent?: string | null;
  /** Already made for this workspace. */
  made: boolean;
  exists: boolean;
}

/** MC-42: the Windows part of the box: the approval and the changes listed for it. */
export interface SandboxWindows {
  approved: boolean;
  approvedAt?: string | null;
  changes: SandboxChange[];
  error?: string | null;
  /** Every change the sandbox has made on this computer (for "undo every change"). */
  recorded: number;
}

export interface SandboxReport {
  applied: number;
  undone: number;
  failures: { path: string; reason: string }[];
}

export interface SandboxStatus {
  workspaceId: string;
  name: string;
  /** `mac` or `windows`. */
  platform?: string;
  /** The OS has a sandbox (the Mac, Windows). */
  supported: boolean;
  /** The Mac: `sandbox-exec` is present. Windows: always. */
  available: boolean;
  /** It enforces less than the Mac's (Windows). */
  experimental?: boolean;
  guarantees?: SandboxGuarantee[];
  windows?: SandboxWindows | null;
  /** The save turned the sandbox on but the changes are not approved yet: the box lists them and asks. */
  needsApproval?: boolean;
  report?: SandboxReport | null;
  ssh: boolean;
  remote: boolean;
  enabled: boolean;
  limitNetwork: boolean;
  allowedHosts: string[];
  protectInstructions: boolean;
  readOnlyPaths: string[];
  reviewer: boolean;
  reviewerAgent: string;
  reviewerModel: string;
  reviewerAgents: string[];
  blocked: SandboxBlock[];
}

/** Why the switch cannot be turned on here (a locale key), or null when it can. */
export function sandboxNotice(status: Pick<SandboxStatus, 'supported' | 'available' | 'ssh' | 'remote'>): string | null {
  if (!status.supported) return 'sandbox.settings.unsupported';
  if (status.ssh) return 'sandbox.settings.ssh';
  if (status.remote) return 'sandbox.settings.remote';
  if (!status.available) return 'sandbox.settings.unavailable';
  return null;
}

/** The non-empty, trimmed lines of a text box, each once. */
export function linesOf(text: string): string[] {
  const out: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    const item = line.trim();
    if (item && !out.includes(item)) out.push(item);
  }
  return out;
}

/** Whether a workspace's agent panes run in the sandbox (the pane badge): it is on and the folder is on this computer. */
export function sandboxed(workspace: Pick<Workspace, 'sandbox' | 'ssh' | 'remote'> | undefined): boolean {
  return Boolean(workspace?.sandbox?.enabled && !workspace.ssh && !workspace.remote);
}

/** The order the approval list groups changes in. */
export const CHANGE_KINDS = ['workspace', 'worktree', 'git', 'instructions', 'readOnly', 'agent', 'secret', 'appData', 'private'] as const;

/** The changes grouped by why they are made (in [`CHANGE_KINDS`] order), each path once with every change to it. */
export function groupChanges(changes: SandboxChange[]): { kind: string; items: { path: string; ops: string[]; made: boolean; agent?: string | null }[] }[] {
  const groups: { kind: string; items: { path: string; ops: string[]; made: boolean; agent?: string | null }[] }[] = [];
  for (const kind of CHANGE_KINDS) {
    const items: { path: string; ops: string[]; made: boolean; agent?: string | null }[] = [];
    for (const change of changes.filter((c) => c.kind === kind)) {
      const item = items.find((i) => i.path === change.path);
      if (item) {
        if (!item.ops.includes(change.op)) item.ops.push(change.op);
        item.made = item.made && change.made;
      } else {
        items.push({ path: change.path, ops: [change.op], made: change.made, agent: change.agent });
      }
    }
    if (items.length) groups.push({ kind, items });
  }
  return groups;
}

/** How many listed changes still wait (not made for this workspace). */
export function pendingChanges(windows: SandboxWindows | null | undefined): number {
  return windows ? windows.changes.filter((c) => !c.made).length : 0;
}

/** The colour of a guarantee's level. */
export function levelTone(level: string): 'ok' | 'warn' | 'err' {
  return level === 'enforced' ? 'ok' : level === 'partial' ? 'warn' : 'err';
}

/** The locale key of each change kind, operation, guarantee and level (whole keys, so every one is a literal here). */
export const KIND_KEYS: Record<string, string> = {
  workspace: 'sandbox.windows.kind.workspace',
  worktree: 'sandbox.windows.kind.worktree',
  git: 'sandbox.windows.kind.git',
  instructions: 'sandbox.windows.kind.instructions',
  readOnly: 'sandbox.windows.kind.readOnly',
  agent: 'sandbox.windows.kind.agent',
  secret: 'sandbox.windows.kind.secret',
  appData: 'sandbox.windows.kind.appData',
  private: 'sandbox.windows.kind.private',
};

export const OP_KEYS: Record<string, string> = {
  lowLabel: 'sandbox.windows.op.lowLabel',
  secretLabel: 'sandbox.windows.op.secretLabel',
  readOnlyLabel: 'sandbox.windows.op.readOnlyLabel',
  allow: 'sandbox.windows.op.allow',
  deny: 'sandbox.windows.op.deny',
  denyDelete: 'sandbox.windows.op.denyDelete',
};

export const GUARANTEE_KEYS: Record<string, string> = {
  writes: 'sandbox.enforce.writes',
  git: 'sandbox.enforce.git',
  instructions: 'sandbox.enforce.instructions',
  secrets: 'sandbox.enforce.secrets',
  otherAgents: 'sandbox.enforce.otherAgents',
  network: 'sandbox.enforce.network',
  desktop: 'sandbox.enforce.desktop',
  processes: 'sandbox.enforce.processes',
};

export const LEVEL_KEYS: Record<string, string> = {
  enforced: 'sandbox.level.enforced',
  partial: 'sandbox.level.partial',
  advisory: 'sandbox.level.advisory',
};

/** A key from one of the tables, or the name itself when the host sent one the page does not know. */
export function keyOf(table: Record<string, string>, name: string): string {
  return table[name] ?? name;
}
