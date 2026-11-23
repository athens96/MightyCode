// MC-33: the bar over an SSH machine's pane: the machine and folder, then each dev-server port the pane's output named as 로컬로 연결
// (an `ssh -N -L` the host opens over the machine's connection) or, once forwarded, the local address that opens in the browser with
// its stop button. Drawn with the worktree bar's styles.
import { useState } from 'react';
import { useServices } from '../app/context';
import { Icon } from '../icons/Icon';
import type { PaneSsh } from '../shell/types';
import { portUrl } from './portsPr';
import './worktree.css';

export function SshBar({ paneId, ssh }: { paneId: string; ssh: PaneSsh }) {
  const { t, bridge } = useServices();
  const [busy, setBusy] = useState<number | null>(null);
  const forward = (port: number) => {
    setBusy(port);
    void bridge.call('sshForward', { paneId, port }).catch(() => {}).finally(() => setBusy(null));
  };
  const stop = (port: number) => void bridge.call('sshForwardClose', { paneId, port }).catch(() => {});
  const open = (port: number) => void bridge.call('openUrl', { url: portUrl(port) }).catch(() => {});
  const place = ssh.host + ':' + ssh.dir;
  return (
    <div className="worktree-bar" data-testid={'pane-ssh-bar-' + paneId}>
      <Icon name="network" size={12} weight={2.2} />
      <span className="worktree-bar-branch mono" title={t('ssh.bar.help', { place })}>{place}</span>
      {ssh.lost && <span className="worktree-bar-count tone-text-wait">{t('ssh.bar.lost')}</span>}
      {ssh.ports.map((port) => port.local ? (
        <span key={port.remote} className="worktree-bar-port mono">
          <button type="button" className="link" data-testid="pane-ssh-forwarded" title={t('ssh.bar.open', { local: port.local, remote: port.remote })}
            onClick={() => open(port.local!)}>{t('ssh.bar.forwarded', { remote: port.remote, local: port.local })}</button>
          {' '}
          <button type="button" className="link" data-testid="pane-ssh-forward-stop" title={t('ssh.bar.stop')} aria-label={t('ssh.bar.stop')}
            onClick={() => stop(port.remote)}><Icon name="x" size={10} weight={2.4} /></button>
        </span>
      ) : (
        <button key={port.remote} type="button" className="worktree-bar-port link mono" data-testid="pane-ssh-forward" disabled={busy === port.remote}
          title={t('ssh.bar.forwardHelp', { port: port.remote })} onClick={() => forward(port.remote)}>{t('ssh.bar.forward', { port: port.remote })}</button>
      ))}
    </div>
  );
}
