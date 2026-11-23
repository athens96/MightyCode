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

export interface SandboxStatus {
  workspaceId: string;
  name: string;
  /** The OS has a sandbox (the Mac). */
  supported: boolean;
  /** `sandbox-exec` is present. */
  available: boolean;
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
  if (!status.supported) return 'sandbox.settings.windows';
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
