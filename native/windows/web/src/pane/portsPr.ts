// MC-26: what a pane's row, tab and worktree bar show of its ports and pull request (the host reads both: src-tauri attention.rs).
import type { PanePr } from '../shell/types';

/** The ports as a row shows them: ":3000", ":3000 +2" when compact, every port otherwise. */
export function portsText(ports: readonly number[], compact: boolean): string {
  if (ports.length === 0) return '';
  if (!compact) return ports.map((port) => ':' + port).join(' ');
  return ':' + ports[0] + (ports.length > 1 ? ' +' + (ports.length - 1) : '');
}

/** The tone a pull request reads in: its checks first (failed, pending, passed), else its state. */
export function prTone(pr: PanePr): 'err' | 'wait' | 'done' | 'run' | 'idle' {
  if (pr.state === 'merged') return 'done';
  if (pr.state === 'closed') return 'idle';
  if (pr.checks === 'failed') return 'err';
  if (pr.checks === 'pending') return 'wait';
  if (pr.checks === 'passed') return 'done';
  return pr.state === 'draft' ? 'idle' : 'run';
}

/** A web address of this computer for a port. */
export function portUrl(port: number): string {
  return 'http://localhost:' + port;
}
