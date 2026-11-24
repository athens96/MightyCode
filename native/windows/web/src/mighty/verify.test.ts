// MC-48: the verification receipt on a result card: which result shows it, its words, the store's copy, and the card as drawn.
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import ko from '@locales/ko.json';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import type { MightyGraphRun } from '../store/models';
import { emptyState, reduce, Store } from '../store/store';
import { blockExtras } from './extras';
import { makeLayout } from './layout';
import { applyVerify, browserLine, duration, receiptForRun, receiptTitle, screenshots, stopWords, type VerifyMessage, type VerifyReceipt } from './verify';
import { VerifyReceiptCard, VerifyRunButton } from './VerifyReceipt';
import cardSource from './VerifyReceipt.tsx?raw';

const t = translator([ko as Record<string, string>]);

function receipt(overrides: Partial<VerifyReceipt> = {}): VerifyReceipt {
  return {
    id: 'v1', status: 'failed', rounds: 3, repairs: 2, maxRounds: 2, stopReason: 'maxRounds', configHash: 'abc', startedAt: '', runIds: ['run-1', 'run-2'],
    commands: [
      { name: 'typecheck', command: 'npx tsc --noEmit', passed: true, durationMs: 4200 },
      { name: 'test', command: 'npm test', passed: false, exitCode: 1, durationMs: 65_000 },
    ],
    browser: { browser: 'Google Chrome', pages: [{ url: 'http://localhost:4100/', screenshot: 'v1-3-1.png' }] },
    browserCounts: { pages: 1, consoleErrors: 2, exceptions: 1, failedRequests: 3, unnamedControls: 0, pageErrors: 0 },
    ...overrides,
  };
}

const message = (receipts: VerifyReceipt[], extra: Partial<VerifyMessage> = {}): VerifyMessage =>
  ({ kind: 'verify', paneId: 'p1', autoRepair: true, running: false, receipts, ...extra });

describe('the verification receipt', () => {
  it('belongs to the result of the last request it followed', () => {
    const pane = applyVerify(undefined, message([receipt()]));
    expect(receiptForRun(pane, 'run-2')?.id).toBe('v1');
    expect(receiptForRun(pane, 'run-1')).toBeUndefined();
    expect(receiptForRun(undefined, 'run-2')).toBeUndefined();
  });

  it('says how it ended, why, and what the browser saw', () => {
    expect(receiptTitle(t, receipt())).toBe('검증 실패 (3회 시도)');
    expect(receiptTitle(t, receipt({ status: 'passed', rounds: 1 }))).toBe('검증 통과');
    expect(receiptTitle(t, receipt({ status: 'passed', rounds: 2 }))).toBe('검증 통과 (2회차)');
    expect(receiptTitle(t, receipt({ status: 'repairing', repairs: 1 }))).toBe('수리 요청 중 (1/2)');
    expect(stopWords(t, receipt())).toBe('자동 수리 횟수 상한에 도달했습니다.');
    expect(stopWords(t, receipt({ status: 'passed' }))).toBe('');
    expect(browserLine(t, receipt())).toContain('콘솔 오류 2');
    expect(browserLine(t, receipt({ browser: { pages: [], unavailable: '브라우저 없음' } }))).toBe('브라우저 점검: 브라우저 없음');
    expect(browserLine(t, receipt({ browser: undefined }))).toBeNull();
    expect(screenshots(receipt())).toEqual(['v1-3-1.png']);
    expect([duration(400), duration(12_300), duration(125_000)]).toEqual(['0.4s', '12s', '2m 05s']);
  });

  it('is kept by the store per pane, a push keeping what verifyState said', () => {
    let state = reduce(emptyState, { ...message([]), configured: true, approved: true });
    expect(state.verify?.p1?.configured).toBe(true);
    state = reduce(state, message([receipt()], { running: true }));
    expect(state.verify?.p1).toMatchObject({ configured: true, approved: true, running: true });
    expect(state.verify?.p1?.receipts).toHaveLength(1);
  });

  it('goes on the result block, and an approved verify offers 검증 실행 on the newest result without one', () => {
    const at = '2026-11-24T00:00:00Z';
    const runs: MightyGraphRun[] = ['run-1', 'run-2'].map((id) => ({
      id, input: 'Request', status: 'completed', finalOutput: 'Answer', agents: [],
      rootEntries: [{ id: id + '-a', kind: 'assistant', text: 'Working', timestamp: at }],
      resultEntries: [{ id: id + '-r', kind: 'assistant', text: 'Answer', timestamp: at }],
    }));
    const layout = makeLayout(runs, '', true, new Set(), { splitsExecution: true, viewport: { w: 1200, h: 800 }, zoom: 1 });
    const result = (runId: string) => layout.nodes.find((n) => n.kind === 'result' && n.id.includes(runId))!.id;
    const verify = { ...applyVerify(undefined, message([receipt({ runIds: ['run-1'] })])), configured: true, approved: true };
    const extras = blockExtras({ layout, runs, inputs: null, draft: '', planHistory: undefined, resultSize: undefined, planText: undefined, latest: result('run-2'), verify }, new Map());
    expect(extras.get(result('run-1'))?.verify?.id).toBe('v1');
    expect(extras.get(result('run-2'))?.verify).toBeUndefined();
    expect(extras.get(result('run-2'))?.verifyOffer).toBe(true);
    const unapproved = blockExtras({ layout, runs, inputs: null, draft: '', planHistory: undefined, resultSize: undefined, planText: undefined, latest: result('run-2'), verify: { ...verify, approved: false } }, new Map());
    expect(unapproved.get(result('run-2'))?.verifyOffer).toBeUndefined();
  });

  it('draws each command, the browser counts, the switch and the buttons', () => {
    const store = new Store(reduce(emptyState, message([receipt()], { autoRepair: false })));
    const services = { bridge: { call: () => Promise.resolve(null) }, store, t } as unknown as AppServices;
    const html = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(VerifyReceiptCard, { paneId: 'p1', receipt: receipt() })));
    expect(html).toContain('data-status="failed"');
    expect(html).toContain('검증 실패 (3회 시도)');
    expect(html).toContain('자동 수리 횟수 상한에 도달했습니다.');
    expect(html).toContain('typecheck');
    expect(html).toContain('4.2s');
    expect(html).toContain('종료 코드 1');
    expect(html).toContain('콘솔 오류 2');
    expect(html).toContain('스크린샷 1');
    expect(html).toContain('data-testid="mighty-verify-run"');
    expect(html).not.toContain('checked=""');

    const running = new Store(reduce(emptyState, message([receipt({ status: 'running' })], { running: true })));
    const live = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: { ...services, store: running } }, createElement(VerifyReceiptCard, { paneId: 'p1', receipt: receipt({ status: 'running' }) })));
    expect(live).toContain('data-testid="mighty-verify-stop"');
    expect(live).toContain('검증 중…');

    const offer = renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, createElement(VerifyRunButton, { paneId: 'p1' })));
    expect(offer).toContain('검증 실행');
  });

  it('keeps every word in the locales and no colour literal in the card', () => {
    // Comments may name the Korean buttons; code may not hold their words.
    const code = cardSource.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
    expect(code).not.toMatch(/[가-힣]/);
    expect(code).not.toMatch(/#[0-9a-fA-F]{3,6}\b/);
  });
});
