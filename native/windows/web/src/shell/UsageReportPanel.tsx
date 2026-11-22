// Usage report panel: finished run cost and tokens over 7 or 30 days, daily bar chart,
// breakdowns by workspace/model/agent, and daily USD budget configuration.
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { usePageOverlay } from '../shell/overlay';
import './UsageReportPanel.css';

export interface UsageTotalRow {
  key: string;
  label: string;
  costUsd: number;
  tokens: number;
  runs: number;
}

export interface UsageReportData {
  days: number;
  totalCostUsd: number;
  totalTokens: number;
  totalRuns: number;
  todayCostUsd: number;
  budgetUsd: number;
  daily: UsageTotalRow[];
  byWorkspace: UsageTotalRow[];
  byModel: UsageTotalRow[];
  byAgent: UsageTotalRow[];
}

export interface UsageReportPanelProps {
  open: boolean;
  onClose: () => void;
}

/** Formats a USD amount with two fractional digits (`$0.00`). */
export function formatUsd(n: number): string {
  const safe = Number.isFinite(n) && n > 0 ? n : 0;
  return '$' + safe.toFixed(2);
}

/** Formats an integer token count with thousands separators (`12,345`). */
export function formatTokenCount(n: number): string {
  const safe = Number.isFinite(n) && n > 0 ? Math.round(n) : 0;
  return safe.toLocaleString('en-US');
}

/**
 * Computes normalized bar heights in `[0, 100]` for the daily chart.
 * Uses `costUsd` when any day has `costUsd > 0`, falling back to `tokens`
 * when all `costUsd === 0` so token-only providers still show activity.
 */
export function computeBarHeights(daily: UsageTotalRow[]): number[] {
  if (daily.length === 0) return [];
  const costs = daily.map((row) => (Number.isFinite(row.costUsd) && row.costUsd > 0 ? row.costUsd : 0));
  const maxCost = Math.max(...costs);
  if (maxCost > 0) {
    return costs.map((cost) => (cost / maxCost) * 100);
  }
  const tokens = daily.map((row) => (Number.isFinite(row.tokens) && row.tokens > 0 ? row.tokens : 0));
  const maxTokens = Math.max(...tokens);
  if (maxTokens > 0) {
    return tokens.map((tok) => (tok / maxTokens) * 100);
  }
  return daily.map(() => 0);
}

/** Parses user budget text into a non-negative USD number (`0` disables the budget). */
export function parseBudgetUsd(raw: string): number {
  const cleaned = raw.trim().replace(/^\$/, '').trim();
  if (cleaned.length === 0) return 0;
  const parsed = Number(cleaned);
  if (!Number.isFinite(parsed) || parsed <= 0) return 0;
  return Math.round(parsed * 100) / 100;
}

function renderInBodyPortal(node: ReactNode) {
  if (typeof document !== 'undefined' && document.body) {
    return createPortal(node, document.body);
  }
  return node;
}

function BreakdownColumn({
  title,
  rows,
  emptyLabel,
  removedLabel,
  rowHelp,
}: {
  title: string;
  rows: UsageTotalRow[];
  emptyLabel: string;
  removedLabel: string;
  rowHelp: (row: UsageTotalRow) => string;
}) {
  const top = rows.slice(0, 8);
  return (
    <div className="usage-breakdown-col">
      <h4 className="usage-breakdown-title">{title}</h4>
      {top.length === 0 ? (
        <div className="usage-breakdown-empty">{emptyLabel}</div>
      ) : (
        <div className="usage-breakdown-list">
          {top.map((row) => {
            const displayLabel = row.key === '__removed__' ? removedLabel : row.label || row.key;
            const help = rowHelp(row);
            return (
              <div key={row.key} className="usage-breakdown-row" title={help}>
                <div className="usage-breakdown-main">
                  <span className="usage-breakdown-name">{displayLabel}</span>
                  <span className="usage-breakdown-cost mono">{formatUsd(row.costUsd)}</span>
                </div>
                <div className="usage-breakdown-sub">{help}</div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function UsageReportPanel({ open, onClose }: UsageReportPanelProps) {
  const { bridge, t } = useServices();
  usePageOverlay(open);

  const [days, setDays] = useState<7 | 30>(7);
  const [data, setData] = useState<UsageReportData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [budgetText, setBudgetText] = useState('');
  const [savingBudget, setSavingBudget] = useState(false);
  const [budgetSaved, setBudgetSaved] = useState(false);
  const initializedBudgetOnOpen = useRef(false);
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!open) {
      initializedBudgetOnOpen.current = false;
      setBudgetSaved(false);
      if (savedTimerRef.current) {
        clearTimeout(savedTimerRef.current);
        savedTimerRef.current = null;
      }
      return undefined;
    }
    let live = true;
    setLoading(true);
    setError(null);
    void bridge
      .call<UsageReportData>('usageReport', { days })
      .then(
        (res) => {
          if (!live || !res) return;
          setData(res);
          setLoading(false);
          if (!initializedBudgetOnOpen.current) {
            initializedBudgetOnOpen.current = true;
            setBudgetText(res.budgetUsd > 0 ? res.budgetUsd.toFixed(2) : '');
          }
        },
        (reason: unknown) => {
          if (!live) return;
          setLoading(false);
          setError(reason instanceof Error ? reason.message : String(reason));
        },
      );
    return () => {
      live = false;
    };
  }, [open, days, bridge]);

  const barHeights = useMemo(() => computeBarHeights(data?.daily ?? []), [data]);

  const applyBudget = useCallback(
    async (event?: FormEvent) => {
      event?.preventDefault();
      if (savingBudget) return;
      const parsed = parseBudgetUsd(budgetText);
      setSavingBudget(true);
      setError(null);
      try {
        const res = await bridge.call<{ budgetUsd: number }>('usageSetBudget', { budgetUsd: parsed });
        const applied = res?.budgetUsd ?? parsed;
        setBudgetText(applied > 0 ? applied.toFixed(2) : '');
        setData((prev) => (prev ? { ...prev, budgetUsd: applied } : prev));
        setBudgetSaved(true);
        if (savedTimerRef.current) clearTimeout(savedTimerRef.current);
        savedTimerRef.current = setTimeout(() => {
          setBudgetSaved(false);
        }, 2000);
      } catch (reason: unknown) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setSavingBudget(false);
      }
    },
    [bridge, budgetText, savingBudget],
  );

  if (!open) return null;

  const rangeTools = (
    <div className="usage-range-segmented" role="group" aria-label={t('usage.title')}>
      <button
        type="button"
        className={'usage-range-btn' + (days === 7 ? ' active' : '')}
        data-testid="usage-range-7"
        aria-pressed={days === 7}
        onClick={() => setDays(7)}
      >
        {t('usage.range.days', { count: 7 })}
      </button>
      <button
        type="button"
        className={'usage-range-btn' + (days === 30 ? ' active' : '')}
        data-testid="usage-range-30"
        aria-pressed={days === 30}
        onClick={() => setDays(30)}
      >
        {t('usage.range.days', { count: 30 })}
      </button>
    </div>
  );

  const dialog = (
    <DialogShell
      kind="panel"
      panel="usageReport"
      width={780}
      height={560}
      minWidth={560}
      minHeight={420}
      label={t('usage.title')}
      title={t('usage.title')}
      subtitle={t('usage.subtitle')}
      icon={<Icon name="square-kanban" size={18} weight={1.8} />}
      tools={rangeTools}
      onCancel={onClose}
      testId="usage-report-panel"
      closeTestId="usage-report-close"
      bodyClassName="usage-report-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={error ? <span className="tone-text-err">{error}</span> : undefined}
    >
      {loading && !data ? (
        <DialogLoading rows={4} />
      ) : data ? (
        <div className="usage-report-content">
          <div className="usage-headline-bar">
            <div className="usage-headline-left">
              <span className="usage-total-cost" data-testid="usage-total-cost">
                {formatUsd(data.totalCostUsd)}
              </span>
              <span className="usage-total-meta">
                {t('usage.headline', {
                  tokens: formatTokenCount(data.totalTokens),
                  runs: data.totalRuns,
                })}
              </span>
            </div>
            <span className="usage-today-pill" data-testid="usage-today-pill">
              {t('usage.today', { cost: formatUsd(data.todayCostUsd) })}
            </span>
          </div>

          <div
            className="usage-chart"
            role="img"
            aria-label={t('usage.chartLabel')}
            data-testid="usage-chart"
          >
            <div className="usage-chart-bars">
              {data.daily.map((day, index) => {
                const height = barHeights[index] ?? 0;
                const tooltip = t('usage.tooltip', {
                  day: day.key,
                  cost: formatUsd(day.costUsd),
                  tokens: formatTokenCount(day.tokens),
                  runs: day.runs,
                });
                return (
                  <div key={day.key} className="usage-chart-col" title={tooltip}>
                    <div className="usage-chart-track">
                      <div
                        className={'usage-chart-bar' + (height > 0 ? ' non-zero' : '')}
                        style={{ height: `${height}%` }}
                      />
                    </div>
                    <span className="usage-chart-date mono">{day.key.slice(5)}</span>
                  </div>
                );
              })}
            </div>
          </div>

          <div className="usage-breakdowns">
            <BreakdownColumn
              title={t('usage.byWorkspace')}
              rows={data.byWorkspace}
              emptyLabel={t('usage.empty')}
              removedLabel={t('usage.removedWorkspace')}
              rowHelp={(row) =>
                t('usage.rowHelp', {
                  tokens: formatTokenCount(row.tokens),
                  runs: row.runs,
                })
              }
            />
            <BreakdownColumn
              title={t('usage.byModel')}
              rows={data.byModel}
              emptyLabel={t('usage.empty')}
              removedLabel={t('usage.removedWorkspace')}
              rowHelp={(row) =>
                t('usage.rowHelp', {
                  tokens: formatTokenCount(row.tokens),
                  runs: row.runs,
                })
              }
            />
            <BreakdownColumn
              title={t('usage.byAgent')}
              rows={data.byAgent}
              emptyLabel={t('usage.empty')}
              removedLabel={t('usage.removedWorkspace')}
              rowHelp={(row) =>
                t('usage.rowHelp', {
                  tokens: formatTokenCount(row.tokens),
                  runs: row.runs,
                })
              }
            />
          </div>

          <form
            className="usage-budget-footer"
            onSubmit={(event) => {
              void applyBudget(event);
            }}
          >
            <label className="usage-budget-label" htmlFor="usage-budget-input">
              {t('usage.budget.label')}
            </label>
            <input
              id="usage-budget-input"
              type="number"
              step="0.5"
              min="0"
              placeholder="0.00"
              className="dialog-field usage-budget-input mono"
              data-testid="usage-budget-input"
              value={budgetText}
              disabled={savingBudget}
              onChange={(event) => {
                setBudgetText(event.target.value);
                if (budgetSaved) setBudgetSaved(false);
              }}
            />
            <button
              type="submit"
              className="btn"
              data-testid="usage-budget-save"
              disabled={savingBudget}
            >
              {t('savedPrompts.save')}
            </button>
            {budgetSaved && (
              <span className="usage-budget-saved" data-testid="usage-budget-saved">
                {t('usage.budget.saved')}
              </span>
            )}
            <span className="usage-budget-note">{t('usage.budget.note')}</span>
          </form>
        </div>
      ) : null}
    </DialogShell>
  );

  return renderInBodyPortal(dialog);
}
