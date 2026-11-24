// MC-7: the live preview's controls (src-tauri preview/). The pane header's globe lists the dev servers of the workspace (its main
// checkout's and each worktree's) and opens one in the preview of the pane it belongs to; the strip over a pane's composer while its
// preview is open: 요소 고르기, 스크린샷, 콘솔 오류 n, 실패한 요청 n, 닫기. What they make goes into the composer as attachments.
import { useEffect, useState } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { Icon } from '../icons/Icon';
import { useMenu, type MenuItem } from '../shell/parts';
import { firstPerPane, previewMenu, shortAddress, type PreviewMessage, type PreviewTargets } from './preview';
import './preview.css';

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The pane header's 프리뷰 button: the workspace's dev servers, found when it is pressed. */
export function PreviewButton({ paneId }: { paneId: string }) {
  const { t, bridge } = useServices();
  const menu = useMenu();
  const [looking, setLooking] = useState(false);
  const open = (target: { paneId: string; url: string; external: boolean }) =>
    void bridge.call('previewOpen', target).catch(() => {});
  const press = (button: HTMLElement) => {
    if (looking) return;
    setLooking(true);
    void bridge.call<PreviewTargets>('previewTargets', { paneId }).then(
      (targets) => {
        const rows = previewMenu(t, targets);
        const items: MenuItem[] = rows.map((row) => {
          if (row.separator) return { separator: true };
          if (row.open) { const target = row.open; return { label: row.label, action: () => open(target), testId: 'preview-server' }; }
          if (!row.disabled && targets.servers) {
            const external = !targets.browser;
            return { label: row.label, testId: 'preview-open-all', action: () => firstPerPane(targets.servers!).forEach((s) => open({ paneId: s.paneId, url: s.url, external })) };
          }
          return { label: row.label, disabled: true };
        });
        if (button.isConnected) menu.under(button, items);
      },
      (error: unknown) => { if (button.isConnected) menu.under(button, [{ label: errorText(error), disabled: true }]); },
    ).finally(() => setLooking(false));
  };
  return (
    <button type="button" className="icon-button pane-plugins-button" data-testid="pane-preview" title={t('preview.button.help')}
      aria-label={t('preview.button.name')} aria-busy={looking} onClick={(event) => press(event.currentTarget)}>
      <Icon name="globe" size={14} weight={2.2} />
    </button>
  );
}

/** The strip over the composer of a pane whose preview is open (or that has something said about it). */
export function PreviewStrip({ paneId }: { paneId: string }) {
  const { t, bridge, store } = useServices();
  const pane = useStoreSelector((state) => state.preview?.[paneId]);
  const [working, setWorking] = useState<string | null>(null);
  // After a page load the host is asked once what it shows (the pushes say every change after).
  useEffect(() => {
    void bridge.call<PreviewMessage>('previewState', { paneId }).then((message) => { if (message) store.dispatch(message); }, () => {});
  }, [bridge, store, paneId]);
  if (!pane) return null;
  const preview = pane.preview;
  const act = (name: string, method: string, args: Record<string, unknown> = {}) => {
    setWorking(name);
    void bridge.call(method, { paneId, ...args }).catch(() => {}).finally(() => setWorking(null));
  };
  return (
    <div className="preview-strip" data-testid={'pane-preview-strip-' + paneId}>
      {preview && (
        <>
          <Icon name="globe" size={12} weight={2.2} />
          <span className="preview-strip-address mono" title={t('preview.strip.help', { browser: preview.browser, url: preview.url })} data-testid="preview-address">
            {preview.title ? preview.title + ' · ' : ''}{shortAddress(preview.url)}
          </span>
          <button type="button" className={'preview-strip-button' + (preview.picking ? ' active' : '')} aria-pressed={preview.picking} data-testid="preview-pick"
            title={t('preview.strip.pickHelp')} disabled={working === 'pick'} onClick={() => act('pick', 'previewPick', { on: !preview.picking })}>
            <Icon name="mouse-pointer-click" size={11} weight={2.2} />{preview.picking ? t('preview.strip.picking') : t('preview.strip.pick')}
          </button>
          <button type="button" className="preview-strip-button" data-testid="preview-shot" disabled={working === 'shot'}
            title={preview.picked ? t('preview.strip.shotElementHelp', { selector: preview.picked.selector }) : t('preview.strip.shotPageHelp')}
            onClick={() => act('shot', 'previewScreenshot')}>
            <Icon name="image" size={11} weight={2.2} />{preview.picked ? t('preview.strip.shotElement', { tag: preview.picked.tag }) : t('preview.strip.shotPage')}
          </button>
          <button type="button" className={'preview-strip-button' + (preview.consoleErrors > 0 ? ' tone-text-err' : '')} data-testid="preview-console"
            disabled={preview.consoleErrors === 0 || working === 'console'} title={t('preview.strip.consoleHelp')}
            onClick={() => act('console', 'previewFindings', { which: 'console' })}>
            <Icon name="triangle-alert" size={11} weight={2.2} />{t('preview.strip.console', { count: String(preview.consoleErrors) })}
          </button>
          <button type="button" className={'preview-strip-button' + (preview.failedRequests > 0 ? ' tone-text-err' : '')} data-testid="preview-requests"
            disabled={preview.failedRequests === 0 || working === 'requests'} title={t('preview.strip.requestsHelp')}
            onClick={() => act('requests', 'previewFindings', { which: 'requests' })}>
            <Icon name="network" size={11} weight={2.2} />{t('preview.strip.requests', { count: String(preview.failedRequests) })}
          </button>
        </>
      )}
      {pane.note && (
        <span className="preview-strip-note tone-text-wait" data-testid="preview-note" title={pane.note}>
          {pane.note}
          <button type="button" className="preview-strip-icon" aria-label={t('preview.strip.dismiss')} title={t('preview.strip.dismiss')}
            onClick={() => act('dismiss', 'previewDismissNote')}><Icon name="x" size={10} weight={2.4} /></button>
        </span>
      )}
      <span className="spacer" />
      {preview && (
        <button type="button" className="preview-strip-icon" data-testid="preview-close" aria-label={t('preview.strip.close')} title={t('preview.strip.close')}
          onClick={() => act('close', 'previewClose')}><Icon name="x" size={12} weight={2.4} /></button>
      )}
    </div>
  );
}
