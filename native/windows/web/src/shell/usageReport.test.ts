import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import {
  computeBarHeights,
  formatTokenCount,
  formatUsd,
  parseBudgetUsd,
  UsageReportPanel,
  type UsageTotalRow,
} from './UsageReportPanel';

const services = {
  bridge: { call: () => Promise.resolve(null) },
  store: { dispatch: () => {} },
  t: translator([
    {
      'dialog.close': 'Close',
      'dialog.resize': 'Resize',
      'dialog.loading': 'Loading…',
      'usage.title': 'Usage',
      'usage.subtitle': 'Cost and tokens each finished run reported',
      'usage.range.days': '{count} days',
    },
  ]),
} as unknown as AppServices;

describe('usage formatting helpers', () => {
  it('formats USD values with two decimals and clamps non-positive or invalid values to $0.00', () => {
    expect(formatUsd(0)).toBe('$0.00');
    expect(formatUsd(1.5)).toBe('$1.50');
    expect(formatUsd(12.345)).toBe('$12.35');
    expect(formatUsd(-4)).toBe('$0.00');
    expect(formatUsd(Number.NaN)).toBe('$0.00');
  });

  it('formats token counts with US thousands separators', () => {
    expect(formatTokenCount(0)).toBe('0');
    expect(formatTokenCount(950)).toBe('950');
    expect(formatTokenCount(12_345)).toBe('12,345');
    expect(formatTokenCount(1_500_000.4)).toBe('1,500,000');
    expect(formatTokenCount(-10)).toBe('0');
  });

  it('parses daily budget inputs cleanly', () => {
    expect(parseBudgetUsd('')).toBe(0);
    expect(parseBudgetUsd('0')).toBe(0);
    expect(parseBudgetUsd('-5')).toBe(0);
    expect(parseBudgetUsd(' $12.5 ')).toBe(12.5);
    expect(parseBudgetUsd('3.456')).toBe(3.46);
  });
});

describe('computeBarHeights', () => {
  const row = (key: string, costUsd: number, tokens: number): UsageTotalRow => ({
    key,
    label: key,
    costUsd,
    tokens,
    runs: costUsd > 0 || tokens > 0 ? 1 : 0,
  });

  it('scales bars proportionally by costUsd when cost data is present', () => {
    const daily = [
      row('2026-11-20', 0, 50_000),
      row('2026-11-21', 2.5, 10_000),
      row('2026-11-22', 5.0, 20_000),
    ];
    expect(computeBarHeights(daily)).toEqual([0, 50, 100]);
  });

  it('falls back to token counts when all costUsd values are 0', () => {
    const daily = [
      row('2026-11-20', 0, 0),
      row('2026-11-21', 0, 3_000),
      row('2026-11-22', 0, 12_000),
    ];
    expect(computeBarHeights(daily)).toEqual([0, 25, 100]);
  });

  it('returns zeroes when both costUsd and tokens are 0 across all days', () => {
    expect(computeBarHeights([])).toEqual([]);
    expect(computeBarHeights([row('2026-11-21', 0, 0), row('2026-11-22', 0, 0)])).toEqual([0, 0]);
  });
});

describe('UsageReportPanel rendering', () => {
  it('renders nothing when closed and renders segmented range buttons when open', () => {
    const closed = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(UsageReportPanel, { open: false, onClose: () => {} }),
      ),
    );
    expect(closed).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(UsageReportPanel, { open: true, onClose: () => {} }),
      ),
    );
    expect(openHtml).toContain('data-testid="usage-report-panel"');
    expect(openHtml).toContain('7 days');
    expect(openHtml).toContain('30 days');
  });
});
