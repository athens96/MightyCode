// MC-26: a pane's ports and pull request, as chips: plain marks in a sidebar row (the row is the button; icons only when narrow),
// buttons on the worktree bar (a port opens localhost, the PR opens its page).
import { useServices } from '../app/context';
import { Icon } from '../icons/Icon';
import type { PaneInfo } from '../shell/types';
import { portsText, portUrl, prTone } from './portsPr';
import './worktree.css';

export function PaneSignals({ info, compact, interactive }: { info: PaneInfo; compact: boolean; interactive: boolean }) {
  const { t, bridge } = useServices();
  const ports = info.ports ?? [];
  const pr = info.pr ?? null;
  if (ports.length === 0 && !pr) return null;
  const open = (url: string) => void bridge.call('openUrl', { url }).catch(() => {});
  const portsHelp = t('panePorts.help', { ports: ports.join(', ') });
  return (
    <>
      {ports.length > 0 && (interactive ? (
        ports.map((port) => (
          <button key={port} type="button" className="pane-signal pane-port link mono" title={t('panePorts.open', { port })}
            data-testid="pane-port" onClick={() => open(portUrl(port))}>
            <Icon name="network" size={10} weight={2.2} /><span>{':' + port}</span>
          </button>
        ))
      ) : (
        <span className="pane-signal pane-port mono" title={portsHelp} aria-label={portsHelp} data-testid="sidebar-ports">
          <Icon name="network" size={10} weight={2.2} />{!compact && <span>{portsText(ports, true)}</span>}
        </span>
      ))}
      {pr && (interactive ? (
        <button type="button" className={'pane-signal pane-pr link tone-text-' + prTone(pr)} title={pr.help} data-testid="pane-pr"
          onClick={() => { if (pr.url) open(pr.url); }}>
          <Icon name="git-merge" size={10} weight={2.2} /><span>{'#' + pr.number}</span>
        </button>
      ) : (
        <span className={'pane-signal pane-pr tone-text-' + prTone(pr)} title={pr.help} aria-label={pr.help} data-testid="sidebar-pr">
          <Icon name="git-merge" size={10} weight={2.2} />{!compact && <span>{'#' + pr.number}</span>}
        </span>
      ))}
    </>
  );
}
