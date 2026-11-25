// MC-71: the new-bot wizard, the classic way (A): the steps listed at the left (done ✓, the current one marked, an earlier one a
// click away), one step at the right, and 취소 · ← 이전 · 다음 → at the bottom. 무엇을 (a template from the gallery) → 어디서 → 누가
// → 언제 → 얼마나 → 확인. The host gives what it picks from (`botWizard`) and makes the bot (`botWizardCreate`, bot_wizard.rs);
// the bot's pane opens in the 봇 tab once it is made.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { ProviderMark } from '../conversation/ProviderMark';
import { usePageOverlay } from './overlay';
import { BetaBadge } from './parts';
import {
  choicesFor, clock, createRequest, dayKeys, extraKinds, extraWords, filterTemplates, githubEvents, independent, independentHere, independentPick,
  intervalChoices, jobWords, newExtra, stepProblem, steps, weekOrder, whenWords, type ExtraJob, type ExtraKind, type GithubEventName, type Step,
  type TimeRule, type WizardChoices, type WizardModel, type WizardTemplate,
} from './botWizardRules';
import './botWizard.css';

/** The event that opens the wizard (the 봇 tab's 봇 추가, its empty list's button). */
export const openBotWizardEvent = 'mighty:open-bot-wizard';
export function openBotWizard() {
  window.dispatchEvent(new Event(openBotWizardEvent));
}

/** Each step's name in the rail and its heading. */
const stepTitles: Record<Step, string> = {
  template: 'bot.wizard.step.template',
  where: 'bot.wizard.step.where',
  who: 'bot.wizard.step.who',
  when: 'bot.wizard.step.when',
  limits: 'bot.wizard.step.limits',
  confirm: 'bot.wizard.step.confirm',
};

/** Each step's one line under its heading. */
const stepLeads: Record<Step, string> = {
  template: 'bot.wizard.lead.template',
  where: 'bot.wizard.lead.where',
  who: 'bot.wizard.lead.who',
  when: 'bot.wizard.lead.when',
  limits: 'bot.wizard.lead.limits',
  confirm: 'bot.wizard.lead.confirm',
};

/** Mounted once in the shell: shows the wizard while it is open. */
export function BotWizardHost() {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const show = () => setOpen(true);
    window.addEventListener(openBotWizardEvent, show);
    return () => window.removeEventListener(openBotWizardEvent, show);
  }, []);
  return open ? <BotWizard onClose={() => setOpen(false)} /> : null;
}

function BotWizard({ onClose }: { onClose: () => void }) {
  const { t, bridge, store } = useServices();
  usePageOverlay(true);
  const [model, setModel] = useState<WizardModel | null>(null);
  const [step, setStep] = useState<Step>('template');
  const [category, setCategory] = useState('all');
  const [query, setQuery] = useState('');
  const [choices, setChoices] = useState<WizardChoices | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void bridge.call<WizardModel>('botWizard').then((value) => {
      setModel(value);
      const first = value.templates[0];
      if (first) setChoices(choicesFor(value, first));
    }, (reason: unknown) => setError(String(reason)));
  }, [bridge]);

  const template = useMemo(() => model?.templates.find((tp) => tp.id === choices?.template), [model, choices?.template]);
  const shown = useMemo(() => (model ? filterTemplates(model, category, query) : []), [model, category, query]);
  const set = useCallback((patch: Partial<WizardChoices>) => setChoices((c) => (c ? { ...c, ...patch } : c)), []);
  const pick = (tp: WizardTemplate) => { if (model) setChoices((c) => choicesFor(model, tp, c ?? undefined)); setError(null); };

  const index = steps.indexOf(step);
  const problem = template && choices ? stepProblem(step, template, choices) : null;
  const go = (next: Step) => { setError(null); setStep(next); };

  const create = async () => {
    if (!choices) return;
    setBusy(true);
    setError(null);
    try {
      const answer = await bridge.call<{ paneId?: string; handle?: string; error?: string; cancelled?: boolean }>('botWizardCreate', createRequest(choices));
      if (answer.cancelled) return;
      if (answer.error || !answer.paneId) { setError(answer.error ?? t('bot.wizard.notMade')); return; }
      onClose();
      // The new bot's pane, shown in the 봇 tab.
      store.dispatch({ kind: 'choose', paneId: answer.paneId });
      void bridge.call('selectSession', { id: answer.paneId }).catch(() => {});
      window.dispatchEvent(new CustomEvent<string>('mighty:show-bot', { detail: answer.paneId }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const another = () => void bridge.call<WizardModel>('botWizard').then((value) => set({ name: value.name }), () => {});

  const agent = model?.agents.find((a) => a.id === choices?.agent);
  const whereLabel = (where: string) => where === independentHere ? t('bot.wizard.where.independent') : where === independentPick ? t('bot.wizard.where.pick')
    : t('bot.wizard.where.workspace', { name: model?.workspaces.find((w) => w.id === where)?.name ?? where });
  const permissionLabel = (value: string) => agent?.permissions.find((p) => p.value === value)?.label ?? t('bot.wizard.permission.default');
  const budgetLabel = (budget: number) => (budget > 0 ? t('bot.wizard.budget.amount', { amount: budget }) : t('bot.wizard.budget.none'));

  const page = (s: Step, c: WizardChoices, tp: WizardTemplate, m: WizardModel) => {
    switch (s) {
      case 'template':
        return (
          <>
            <div className="bot-wizard-filter">
              <input className="dialog-field bot-wizard-search" type="search" value={query} placeholder={t('bot.wizard.search')} aria-label={t('bot.wizard.search')}
                onChange={(e) => setQuery(e.target.value)} />
              <div className="bot-wizard-chips" role="tablist" aria-label={t('bot.wizard.categories')}>
                {[{ id: 'all', label: t('bot.wizard.category.all') }, ...m.categories].map((cat) => (
                  <button key={cat.id} type="button" role="tab" aria-selected={category === cat.id} className={'bot-wizard-chip' + (category === cat.id ? ' on' : '')} onClick={() => setCategory(cat.id)}>
                    {cat.label}
                  </button>
                ))}
              </div>
            </div>
            {shown.length === 0 ? <p className="bot-wizard-muted">{t('bot.wizard.noResults')}</p> : (
              <div className="bot-wizard-cards" role="listbox" aria-label={t('bot.new.template')}>
                {shown.map((item) => (
                  <button key={item.id} type="button" role="option" aria-selected={item.id === tp.id} className={'bot-wizard-card' + (item.id === tp.id ? ' on' : '')}
                    data-testid={'bot-template-' + item.id} onClick={() => pick(item)} onDoubleClick={() => { pick(item); go('where'); }}>
                    <span className="bot-wizard-card-icon" aria-hidden="true">{item.icon}</span>
                    <b>{item.label}</b>
                    <span>{item.what}</span>
                  </button>
                ))}
              </div>
            )}
            <div className="bot-wizard-detail">
              <div><b>{t('bot.wizard.detail.what')}</b>{tp.what}</div>
              <div><b>{t('bot.wizard.detail.when')}</b>{tp.jobs.length > 0 ? tp.jobs.map((j) => j.when).join(' · ') : t('bot.wizard.detail.onCall')}</div>
            </div>
          </>
        );
      case 'where':
        return (
          <>
            {[...m.workspaces.map((w) => w.id), independentHere, independentPick].map((where) => (
              <label key={where} className="bot-wizard-job">
                <input type="radio" name="bot-wizard-where" checked={c.where === where} onChange={() => set({ where })} />
                <span>{whereLabel(where)}</span>
              </label>
            ))}
            {tp.needs === 'repo' && (
              <>
                <label className="bot-wizard-label" htmlFor="bot-wizard-repo">{t('bot.wizard.repo')}</label>
                <input id="bot-wizard-repo" className="dialog-field" value={c.repo} placeholder="owner/name" onChange={(e) => set({ repo: e.target.value })} data-autofocus />
                <p className="bot-wizard-hint">{t(independent(c.where) ? 'bot.wizard.repo.needed' : 'bot.wizard.repo.workspaceHint')}</p>
              </>
            )}
            {tp.needs === 'command' && (
              <>
                <label className="bot-wizard-label" htmlFor="bot-wizard-command">{t('bot.wizard.command')}</label>
                <input id="bot-wizard-command" className="dialog-field mono" value={c.command} placeholder={tp.commandHint} onChange={(e) => set({ command: e.target.value })} data-autofocus />
              </>
            )}
          </>
        );
      case 'who':
        return (
          <>
            <div className="bot-wizard-agents" role="radiogroup" aria-label={t('bot.new.agent')}>
              {m.agents.map((a) => (
                <button key={a.id} type="button" role="radio" aria-checked={a.id === c.agent} className={'bot-wizard-agent' + (a.id === c.agent ? ' on' : '')}
                  onClick={() => set({ agent: a.id, permission: '', model: a.id === c.agent ? c.model : '' })}>
                  <ProviderMark provider={a.id} size={16} />
                  <span>{a.label}</span>
                  {a.available === false && <small>{t('bot.wizard.agentMissing')}</small>}
                </button>
              ))}
            </div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-name">{t('bot.wizard.name')}</label>
            <div className="bot-wizard-row">
              <input id="bot-wizard-name" className="dialog-field" value={c.name} placeholder={t('bot.new.nameHint')} onChange={(e) => set({ name: e.target.value })} />
              <button type="button" className="btn" title={t('bot.wizard.another')} aria-label={t('bot.wizard.another')} onClick={another}>🎲</button>
            </div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-model">{t('bot.wizard.model')}</label>
            {/* MC-71: the agent's models, as its model picker lists them; a name is typed when it lists none. */}
            {agent && agent.models.length > 0 ? (
              <select id="bot-wizard-model" className="dialog-field" value={c.model} onChange={(e) => set({ model: e.target.value })}>
                <option value="">{t('bot.wizard.model.default')}</option>
                {agent.models.map((mo) => <option key={mo.value} value={mo.value}>{mo.label}</option>)}
                {c.model && !agent.models.some((mo) => mo.value === c.model) && <option value={c.model}>{c.model}</option>}
              </select>
            ) : (
              <input id="bot-wizard-model" className="dialog-field" value={c.model} placeholder={t('bot.wizard.modelHint')} onChange={(e) => set({ model: e.target.value })} />
            )}
            <label className="bot-wizard-label" htmlFor="bot-wizard-instructions">{t('bot.wizard.instructions')}</label>
            <textarea id="bot-wizard-instructions" className="dialog-field bot-wizard-textarea" value={c.instructions} placeholder={t('bot.dialog.instructionsHint')}
              onChange={(e) => set({ instructions: e.target.value })} />
          </>
        );
      case 'when': {
        // MC-79: the template's jobs (on or off, their time and request changed here) and the jobs the person adds.
        const setExtra = (i: number, next: ExtraJob) => set({ extra: c.extra.map((job, k) => (k === i ? next : job)) });
        return (
          <>
            {tp.jobs.length === 0 && c.extra.length === 0 && <p className="bot-wizard-muted">{t('bot.wizard.noJobs')}</p>}
            {tp.jobs.map((_, i) => {
              const rule = c.times[i] ?? null;
              const on = c.jobs[i] !== false;
              return (
                <div key={i} className="bot-wizard-job-block">
                  <label className="bot-wizard-job">
                    <input type="checkbox" checked={on} onChange={(e) => set({ jobs: c.jobs.map((v, k) => (k === i ? e.target.checked : v)) })} />
                    <span>{jobWords(tp, c, i, t)}</span>
                  </label>
                  {/* MC-71: a schedule's time, changed here: the time (or the interval and hours) and the days. */}
                  {rule && on && <TimeEditor rule={rule} onChange={(next) => set({ times: c.times.map((r, k) => (k === i ? next : r)) })} />}
                  {on && <PromptField value={c.prompts[i] ?? ''} template={tp.prompt} onChange={(value) => set({ prompts: c.prompts.map((p, k) => (k === i ? value : p)) })} />}
                </div>
              );
            })}
            {c.extra.map((job, i) => (
              <div key={'extra-' + i} className="bot-wizard-job-block bot-wizard-extra" data-testid="bot-wizard-extra">
                <div className="bot-wizard-extra-head">
                  <span>{extraWords(job, t)}</span>
                  <button type="button" className="icon-button" title={t('bot.wizard.job.remove')} aria-label={t('bot.wizard.job.remove')}
                    onClick={() => set({ extra: c.extra.filter((_, k) => k !== i) })}><Icon name="x" size={12} /></button>
                </div>
                <ExtraEditor job={job} bots={m.bots} onChange={(next) => setExtra(i, next)} />
                <PromptField value={job.prompt} template={tp.prompt} open={tp.prompt === ''} onChange={(prompt) => setExtra(i, { ...job, prompt })} />
              </div>
            ))}
            <div className="bot-wizard-add" role="group" aria-label={t('bot.wizard.add.title')}>
              <span>{t('bot.wizard.add.title')}</span>
              {extraKinds.map((kind) => (
                <button key={kind} type="button" className="bot-wizard-chip" data-testid={'bot-wizard-add-' + kind} onClick={() => set({ extra: [...c.extra, newExtra(kind)] })}>
                  + {t(addKeys[kind])}
                </button>
              ))}
            </div>
          </>
        );
      }
      case 'limits':
        return (
          <>
            <span className="bot-wizard-label">{t('bot.wizard.permission')}</span>
            <div className="bot-wizard-chips" role="radiogroup" aria-label={t('bot.wizard.permission')}>
              {[{ value: '', label: t('bot.wizard.permission.default') }, ...(agent?.permissions ?? [])].map((p) => (
                <button key={p.value || 'default'} type="button" role="radio" aria-checked={c.permission === p.value} className={'bot-wizard-chip' + (c.permission === p.value ? ' on' : '')}
                  onClick={() => set({ permission: p.value })}>{p.label}</button>
              ))}
            </div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-budget">{t('bot.wizard.budget')}</label>
            <input id="bot-wizard-budget" type="range" min={0} max={20} step={1} value={c.budget} onChange={(e) => set({ budget: Number(e.target.value) })} className="bot-wizard-range" />
            <p className="bot-wizard-hint">{budgetLabel(c.budget)}</p>
          </>
        );
      case 'confirm':
        return (
          <>
            <dl className="bot-wizard-summary" data-testid="bot-wizard-summary">
              <div><dt>{t('bot.wizard.summary.bot')}</dt><dd>@{c.name.replace(/^@/, '') || m.name} · {agent?.label ?? c.agent}{c.model ? ' · ' + c.model : ''}</dd></div>
              <div><dt>{t('bot.wizard.summary.task')}</dt><dd>{tp.icon} {tp.label}</dd></div>
              <div><dt>{t('bot.wizard.summary.where')}</dt><dd>{whereLabel(c.where)}{c.repo.trim() ? ' · ' + c.repo.trim() : ''}</dd></div>
              <div><dt>{t('bot.wizard.summary.when')}</dt><dd>{whenWords(tp, c, t).join(' · ') || t('bot.wizard.detail.onCall')}</dd></div>
              <div><dt>{t('bot.wizard.summary.limits')}</dt><dd>{permissionLabel(c.permission)} · {budgetLabel(c.budget)}</dd></div>
            </dl>
            <label className="bot-wizard-job">
              <input type="checkbox" checked={c.greet} onChange={(e) => set({ greet: e.target.checked })} />
              <span>{t('bot.wizard.greet')}</span>
            </label>
          </>
        );
    }
  };

  const body = !model || !choices || !template ? <DialogLoading rows={5} /> : (
    <div className="bot-wizard" data-testid="bot-wizard">
      <nav className="bot-wizard-rail" aria-label={t('bot.new.title')}>
        <div className="bot-wizard-rail-icon" aria-hidden="true">{template.icon}</div>
        <ol>
          {steps.map((s, i) => (
            <li key={s} className={i < index ? 'done' : i === index ? 'current' : ''} aria-current={i === index ? 'step' : undefined}>
              <button type="button" disabled={i > index || busy} onClick={() => go(s)}>
                <span className="bot-wizard-step-dot">{i < index ? <Icon name="check" size={10} weight={3} /> : i + 1}</span>
                <span>{t(stepTitles[s])}</span>
              </button>
            </li>
          ))}
        </ol>
      </nav>
      <section className="bot-wizard-page" aria-labelledby="bot-wizard-heading">
        <h3 id="bot-wizard-heading">{t(stepTitles[step])}</h3>
        <p className="bot-wizard-lead">{t(stepLeads[step])}</p>
        {page(step, choices, template, model)}
      </section>
    </div>
  );

  const shownError = error ?? (problem ? t(problem) : null);
  const last = index === steps.length - 1;
  const buttons = (
    <>
      <button type="button" className="btn" disabled={busy} onClick={onClose}>{t('bot.dialog.cancel')}</button>
      <button type="button" className="btn" disabled={busy || index === 0} data-testid="bot-wizard-back" onClick={() => go(steps[index - 1] ?? 'template')}>← {t('bot.wizard.back')}</button>
      {last
        ? <button type="button" className="btn primary" disabled={busy || !choices} data-testid="bot-wizard-create" onClick={() => void create()}>{busy ? t('bot.wizard.creating') : t('bot.new.create')}</button>
        : <button type="button" className="btn primary" disabled={!choices || problem !== null} data-testid="bot-wizard-next" onClick={() => go(steps[index + 1] ?? 'confirm')}>{t('bot.wizard.next')} →</button>}
    </>
  );

  return createPortal(
    <DialogShell
      kind="panel"
      panel="botWizard"
      width={780}
      height={560}
      minWidth={620}
      minHeight={460}
      label={t('bot.new.title')}
      title={<>{t('bot.new.title')} <BetaBadge /></>}
      icon={<Icon name="bot" size={18} weight={1.8} />}
      busy={busy}
      onCancel={onClose}
      testId="bot-wizard-panel"
      bodyClassName="bot-wizard-body"
      backdrop={{ 'data-page-overlay': '' }}
      status={shownError ? <span className="tone-text-err" role="alert">{shownError}</span> : undefined}
      buttons={buttons}
    >
      {body}
    </DialogShell>,
    document.body,
  );
}

/** MC-71: a schedule's time: the clock time (or every N minutes between two hours) and the days, Monday first. */
function TimeEditor({ rule, onChange }: { rule: TimeRule; onChange: (rule: TimeRule) => void }) {
  const { t } = useServices();
  const hours = Array.from({ length: 24 }, (_, h) => h);
  const toggle = (day: number) => onChange({ ...rule, days: rule.days.includes(day) ? rule.days.filter((d) => d !== day) : [...rule.days, day].sort((a, b) => a - b) });
  // MC-79: a time of day, or every N minutes between two hours.
  const switchTo = (kind: TimeRule['kind']) => {
    if (kind === rule.kind) return;
    onChange(kind === 'at' ? { kind, hour: 9, minute: 0, days: rule.days } : { kind, minutes: 30, from: 9, to: 18, days: rule.days });
  };
  return (
    <div className="bot-wizard-time" data-testid="bot-wizard-time">
      <div className="bot-wizard-chips" role="radiogroup" aria-label={t('bot.wizard.time.kind')}>
        {(['at', 'every'] as const).map((kind) => (
          <button key={kind} type="button" role="radio" aria-checked={rule.kind === kind} className={'bot-wizard-chip' + (rule.kind === kind ? ' on' : '')} onClick={() => switchTo(kind)}>
            {t(kind === 'at' ? 'bot.wizard.time.kindAt' : 'bot.wizard.time.kindEvery')}
          </button>
        ))}
      </div>
      {rule.kind === 'at' ? (
        <label className="bot-wizard-time-field">
          <span>{t('bot.wizard.time.time')}</span>
          <input type="time" className="dialog-field" value={clock(rule.hour, rule.minute)} step={300}
            onChange={(e) => {
              const [h, m] = e.target.value.split(':').map(Number);
              if (Number.isInteger(h) && Number.isInteger(m)) onChange({ ...rule, hour: h ?? 0, minute: m ?? 0 });
            }} />
        </label>
      ) : (
        <div className="bot-wizard-time-field">
          <span>{t('bot.wizard.time.interval')}</span>
          <select className="dialog-field" value={rule.minutes} aria-label={t('bot.wizard.time.interval')} onChange={(e) => onChange({ ...rule, minutes: Number(e.target.value) })}>
            {[...new Set([...intervalChoices, rule.minutes])].sort((a, b) => a - b).map((n) => <option key={n} value={n}>{t('bot.wizard.time.minutes', { minutes: n })}</option>)}
          </select>
          <select className="dialog-field" value={rule.from} aria-label={t('bot.wizard.time.from')} onChange={(e) => { const from = Number(e.target.value); onChange({ ...rule, from, to: Math.max(from, rule.to) }); }}>
            {hours.map((h) => <option key={h} value={h}>{t('bot.wizard.time.hour', { hour: h })}</option>)}
          </select>
          <span>–</span>
          <select className="dialog-field" value={rule.to} aria-label={t('bot.wizard.time.to')} onChange={(e) => { const to = Number(e.target.value); onChange({ ...rule, to, from: Math.min(rule.from, to) }); }}>
            {hours.map((h) => <option key={h} value={h}>{t('bot.wizard.time.hour', { hour: h })}</option>)}
          </select>
        </div>
      )}
      <div className="bot-wizard-days" role="group" aria-label={t('bot.wizard.time.days')}>
        {weekOrder.map((day) => (
          <button key={day} type="button" aria-pressed={rule.days.includes(day)} className={'bot-wizard-day' + (rule.days.includes(day) ? ' on' : '')} onClick={() => toggle(day)}>
            {t(dayKeys[day])}
          </button>
        ))}
      </div>
    </div>
  );
}

const addKeys: Record<ExtraKind, string> = {
  schedule: 'bot.wizard.add.schedule',
  github: 'bot.wizard.add.github',
  bot: 'bot.wizard.add.bot',
  event: 'bot.wizard.add.event',
};

/** MC-79: what an added job watches: its time, its GitHub event and repository, the bot whose request ending wakes it, or the event. */
function ExtraEditor({ job, bots, onChange }: { job: ExtraJob; bots: string[]; onChange: (job: ExtraJob) => void }) {
  const { t } = useServices();
  switch (job.kind) {
    case 'schedule':
      return <TimeEditor rule={job.rule} onChange={(rule) => onChange({ ...job, rule })} />;
    case 'github':
      return (
        <div className="bot-wizard-time-field">
          <select className="dialog-field" value={job.event} aria-label={t('bot.wizard.github.event')} onChange={(e) => onChange({ ...job, event: e.target.value as GithubEventName })}>
            {githubEvents.map((event) => <option key={event.value} value={event.value}>{t(event.key)}</option>)}
          </select>
          <input className="dialog-field" value={job.repo} aria-label={t('bot.wizard.github.repo')} placeholder={t('bot.wizard.github.repoHint')}
            onChange={(e) => onChange({ ...job, repo: e.target.value })} />
        </div>
      );
    case 'bot':
      return (
        <div className="bot-wizard-time-field">
          <input className="dialog-field" list="bot-wizard-bots" value={job.bot} aria-label={t('bot.wizard.bot.name')} placeholder={t('bot.wizard.bot.hint')}
            onChange={(e) => onChange({ ...job, bot: e.target.value })} />
          <datalist id="bot-wizard-bots">
            <option value="*">{t('bot.wizard.bot.any')}</option>
            {bots.map((bot) => <option key={bot} value={'@' + bot} />)}
          </datalist>
        </div>
      );
    case 'event':
      return (
        <div className="bot-wizard-time-field">
          <input className="dialog-field" value={job.name} aria-label={t('bot.wizard.event.name')} placeholder={t('bot.wizard.event.hint')}
            onChange={(e) => onChange({ ...job, name: e.target.value })} />
        </div>
      );
  }
}

/** MC-79: the request a job sends, the template's unless the person writes another (folded until opened, or when there is none). */
function PromptField({ value, template, open = false, onChange }: { value: string; template: string; open?: boolean; onChange: (value: string) => void }) {
  const { t } = useServices();
  return (
    <details className="bot-wizard-prompt" open={open || value.trim() !== ''}>
      <summary>{t('bot.wizard.job.prompt')}</summary>
      <textarea className="dialog-field bot-wizard-textarea" value={value} aria-label={t('bot.wizard.job.prompt')}
        placeholder={template || t('bot.wizard.job.promptNeeded')} onChange={(e) => onChange(e.target.value)} />
      {template !== '' && <small className="bot-wizard-hint">{t('bot.wizard.job.promptHint')}</small>}
    </details>
  );
}
