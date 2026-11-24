// MC-48: a result card's verification receipt (src-tauri verify/mod.rs): pass or fail and after how many rounds, each command with its
// time, the browser's counts and its screenshots, and the pane's 자동 수리 switch, 다시 검증 and 중지.
import { memo, useState } from 'react';
import { useServices, useStoreSelector } from '../app/context';
import { browserLine, duration, receiptTitle, receiptTone, screenshots, stopWords, type VerifyReceipt } from './verify';

export const VerifyReceiptCard = memo(function VerifyReceiptCard({ paneId, receipt }: { paneId: string; receipt: VerifyReceipt }) {
  const { bridge, t } = useServices();
  const pane = useStoreSelector((state) => state.verify?.[paneId]);
  const [shot, setShot] = useState<{ name: string; url: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const autoRepair = pane?.autoRepair ?? true;
  const running = pane?.running ?? false;
  const tone = receiptTone(receipt);
  const stop = stopWords(t, receipt);
  const browser = browserLine(t, receipt);
  const shots = screenshots(receipt);
  const call = (method: string, args: Record<string, unknown>) => {
    setError(null);
    bridge.call(method, args).catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  };
  const openShot = (name: string) => {
    if (shot?.name === name) { setShot(null); return; }
    bridge.call<{ url: string }>('verifyScreenshot', { paneId, file: name }).then(
      (answer) => setShot({ name, url: answer.url }),
      (e: unknown) => setError(e instanceof Error ? e.message : String(e)),
    );
  };
  return (
    <div className={'mighty-verify tone-' + tone} data-testid="mighty-verify" data-status={receipt.status}>
      <div className="mighty-verify-head">
        <span className={'mighty-verify-mark tone-' + tone} aria-hidden="true">{tone === 'done' ? '✓' : tone === 'fail' ? '✕' : '…'}</span>
        <span className="mighty-verify-title" data-testid="mighty-verify-title">{receiptTitle(t, receipt)}</span>
        {receipt.manual && <span className="mighty-verify-meta">{t('verify.card.manual')}</span>}
        <span className="mighty-verify-spacer" />
        <label className="mighty-verify-toggle" title={t('verify.card.autoRepairHelp')}>
          <input type="checkbox" checked={autoRepair} data-testid="mighty-verify-auto" onChange={(e) => call('verifySetAutoRepair', { paneId, on: e.target.checked })} />
          {t('verify.card.autoRepair')}
        </label>
        {running
          ? <button type="button" className="mighty-verify-button" data-testid="mighty-verify-stop" onClick={() => call('verifyStop', { paneId })}>{t('verify.card.stop')}</button>
          : <button type="button" className="mighty-verify-button" data-testid="mighty-verify-run" onClick={() => call('verifyRun', { paneId })}>{t('verify.card.runAgain')}</button>}
      </div>
      {stop && <div className="mighty-verify-reason" data-testid="mighty-verify-reason">{stop}</div>}
      <ul className="mighty-verify-commands">
        {receipt.commands.map((command) => (
          <li key={command.name} className={'mighty-verify-command ' + (command.passed ? 'tone-done' : 'tone-fail')} title={command.command}>
            <span className="mighty-verify-mark-small">{command.passed ? '✓' : '✕'}</span>
            <span className="mighty-verify-name">{command.name}</span>
            <span className="mighty-verify-meta">
              {command.passed ? duration(command.durationMs)
                : command.timedOut ? t('verify.card.timedOut')
                  : command.error ? command.error
                    : t('verify.card.exitCode', { code: String(command.exitCode ?? '?') })}
            </span>
          </li>
        ))}
      </ul>
      {browser && <div className="mighty-verify-browser" data-testid="mighty-verify-browser">{browser}</div>}
      {shots.length > 0 && (
        <div className="mighty-verify-shots">
          {shots.map((name, index) => (
            <button key={name} type="button" className={'mighty-verify-button' + (shot?.name === name ? ' on' : '')} data-testid="mighty-verify-shot" onClick={() => openShot(name)}>
              {t('verify.card.screenshot', { n: String(index + 1) })}
            </button>
          ))}
        </div>
      )}
      {shot && <img className="mighty-verify-image" src={shot.url} alt={t('verify.card.screenshotAlt')} />}
      {error && <div className="mighty-verify-error" role="alert">{error}</div>}
    </div>
  );
});

/** A result with no receipt yet in a workspace with an approved `verify`: 검증 실행. */
export function VerifyRunButton({ paneId }: { paneId: string }) {
  const { bridge, t } = useServices();
  const [error, setError] = useState<string | null>(null);
  return (
    <div className="mighty-verify" data-testid="mighty-verify-offer">
      <div className="mighty-verify-head">
        <span className="mighty-verify-title">{t('verify.card.offer')}</span>
        <span className="mighty-verify-spacer" />
        <button type="button" className="mighty-verify-button" data-testid="mighty-verify-run"
          onClick={() => { setError(null); bridge.call('verifyRun', { paneId }).catch((e: unknown) => setError(e instanceof Error ? e.message : String(e))); }}>
          {t('verify.card.run')}
        </button>
      </div>
      {error && <div className="mighty-verify-error" role="alert">{error}</div>}
    </div>
  );
}
