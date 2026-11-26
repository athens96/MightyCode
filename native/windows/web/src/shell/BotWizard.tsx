// MC-71: the new-bot wizard, the classic way (A): the steps listed at the left (done ✓, the current one marked, an earlier one a
// click away), one step at the right, and 취소 · ← 이전 · 다음 → at the bottom. 무엇을 (a template from the gallery) → 어디서 → 누가
// → 언제 → 얼마나 → 확인. The host gives what it picks from (`botWizard`) and makes the bot (`botWizardCreate`, bot_wizard.rs);
// the bot's pane opens in the 봇 tab once it is made. MC-99: the 언제 step shows a trigger's rule (its conditions, what it does and
// its policy) and lets the person change it; a MightyWork recipe asks for the project key in 어디서. MC-101: 지침과 요청 right
// after the gallery: the instructions and the request every job sends, filled in and editable, an example a chip away.
// MC-104: the 언제 step made simpler: a sentence on top saying what the bot will do, then one card per job (the template's and the
// added ones alike: what it watches, ×, its time or its conditions as plain chips, 고급 folded with its own request and the raw
// condition rows); what a trigger does and its policy moved to 얼마나 맡길까요, beside the permission and the budget.
import { type ReactNode, useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { ProviderMark } from '../conversation/ProviderMark';
import { usePageOverlay } from './overlay';
import { BetaBadge } from './parts';
import {
  actionKinds, actionNoteKeys, approves, approvesByDefault, choicesFor, clock, conditionChips, conditionPresets, createRequest, dayKeys, doKeys, exampleOn,
  extraKinds, extraLine, extraWatch, extraWords, filterTemplates, githubEvents, independent, independentHere, independentPick, intervalChoices, jobLine,
  jobWatch, jobWords, mightyworkStages, moreExtraKinds, newExtra, opWords, ownWords, ruleOps, stageKeys, stepProblem, steps, summaryWords, triggerRules,
  watchFields, weekOrder, withExample, withoutChip, withPreset, withTriggerRule,
  type ActionKindName, type ExtraJob, type ExtraKind, type GithubEventName, type RuleCondition, type RuleOp, type Step, type TimeRule, type Watch,
  type WizardChoices, type WizardExample, type WizardModel, type WizardRule, type WizardTemplate,
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
  prompt: 'bot.wizard.step.prompt',
  where: 'bot.wizard.step.where',
  who: 'bot.wizard.step.who',
  when: 'bot.wizard.step.when',
  limits: 'bot.wizard.step.limits',
  confirm: 'bot.wizard.step.confirm',
};

/** Each step's one line under its heading. */
const stepLeads: Record<Step, string> = {
  template: 'bot.wizard.lead.template',
  prompt: 'bot.wizard.lead.prompt',
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
                    data-testid={'bot-template-' + item.id} onClick={() => pick(item)} onDoubleClick={() => { pick(item); go('prompt'); }}>
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
              {/* MC-101: the examples the next step offers. */}
              {tp.examples.length > 0 && <div className="bot-wizard-detail-wide"><b>{t('bot.wizard.examples')}</b>{tp.examples.map((e) => e.label).join(' · ')}</div>}
            </div>
          </>
        );
      case 'prompt':
        return <PromptStep template={tp} choices={c} onChange={set} />;
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
            {/* MC-99: the MightyWork project a recipe watches. */}
            {tp.needs === 'project' && (
              <>
                <label className="bot-wizard-label" htmlFor="bot-wizard-project">{t('bot.wizard.project')}</label>
                <input id="bot-wizard-project" className="dialog-field mono" value={c.project} placeholder="MC" onChange={(e) => set({ project: e.target.value })} data-autofocus />
                <p className="bot-wizard-hint">{t('bot.wizard.project.hint')}</p>
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
          </>
        );
      case 'when':
        return <WhenStep template={tp} choices={c} bots={m.bots} onChange={set} />;
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
            <TriggerActions template={tp} choices={c} onChange={(next) => setChoices(next)} />
          </>
        );
      case 'confirm':
        return (
          <>
            <dl className="bot-wizard-summary" data-testid="bot-wizard-summary">
              <div><dt>{t('bot.wizard.summary.bot')}</dt><dd>@{c.name.replace(/^@/, '') || m.name} · {agent?.label ?? c.agent}{c.model ? ' · ' + c.model : ''}</dd></div>
              <div><dt>{t('bot.wizard.summary.task')}</dt><dd>{tp.icon} {tp.label}</dd></div>
              <div><dt>{t('bot.wizard.summary.where')}</dt><dd>{whereLabel(c.where)}{c.repo.trim() ? ' · ' + c.repo.trim() : ''}</dd></div>
              {/* MC-104: the 언제 step's sentence. */}
              <div><dt>{t('bot.wizard.summary.when')}</dt><dd>{summaryWords(tp, c, t) || t('bot.wizard.detail.onCall')}</dd></div>
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
  mightywork: 'bot.wizard.add.mightywork',
};

/** MC-104: the 언제 step: the sentence saying what the bot will do, one card per job (the template's left on, then the added ones),
 *  and the 추가 row (예약, GitHub, then 더 보기 ▾ with 다른 봇, 앱 이벤트, MightyWork 일감; a template job taken away comes back from
 *  here too). */
export function WhenStep({ template, choices: c, bots, onChange: set }: { template: WizardTemplate; choices: WizardChoices; bots: string[]; onChange: (patch: Partial<WizardChoices>) => void }) {
  const { t } = useServices();
  const [more, setMore] = useState(false);
  const sentence = summaryWords(template, c, t);
  const setExtra = (i: number, next: ExtraJob) => set({ extra: c.extra.map((job, k) => (k === i ? next : job)) });
  const setJob = (i: number, on: boolean) => set({ jobs: c.jobs.map((v, k) => (k === i ? on : v)) });
  const away = template.jobs.map((_, i) => i).filter((i) => c.jobs[i] === false);
  const add = (kind: ExtraKind) => set({ extra: [...c.extra, newExtra(kind, c.project)] });
  return (
    <>
      <p className="bot-wizard-sentence" data-testid="bot-wizard-sentence" aria-live="polite">{sentence || t('bot.wizard.noJobs')}</p>
      {template.jobs.map((_, i) => {
        if (c.jobs[i] === false) return null;
        const time = c.times[i] ?? null;
        const rule = c.rules[i] ?? null;
        return (
          <JobCard key={'job-' + i} head={jobWords(template, c, i, t)} testId="bot-wizard-job" onRemove={() => setJob(i, false)}
            prompt={c.prompts[i] ?? ''} main={c.prompt} onPrompt={(value) => set({ prompts: c.prompts.map((p, k) => (k === i ? value : p)) })}
            watch={jobWatch(template, i)} conditions={rule?.conditions ?? null}
            onConditions={(conditions) => set({ rules: c.rules.map((r, k) => (k === i && r ? { ...r, conditions } : r)) })}>
            {/* MC-71: a schedule's time, changed here: the time (or the interval and hours) and the days. */}
            {time && <TimeEditor rule={time} onChange={(next) => set({ times: c.times.map((r, k) => (k === i ? next : r)) })} />}
          </JobCard>
        );
      })}
      {c.extra.map((job, i) => (
        <JobCard key={'extra-' + i} head={extraWords(job, t)} testId="bot-wizard-extra" onRemove={() => set({ extra: c.extra.filter((_, k) => k !== i) })}
          prompt={job.prompt} main={c.prompt} onPrompt={(prompt) => setExtra(i, { ...job, prompt })}
          watch={extraWatch(job)} conditions={job.kind === 'schedule' ? null : job.trigger.conditions}
          onConditions={(conditions) => { if (job.kind !== 'schedule') setExtra(i, { ...job, trigger: { ...job.trigger, conditions } }); }}>
          <ExtraEditor job={job} bots={bots} onChange={(next) => setExtra(i, next)} />
        </JobCard>
      ))}
      <div className="bot-wizard-add" role="group" aria-label={t('bot.wizard.add.title')}>
        <span>{t('bot.wizard.add.title')}</span>
        {extraKinds.map((kind) => (
          <button key={kind} type="button" className="bot-wizard-chip" data-testid={'bot-wizard-add-' + kind} onClick={() => add(kind)}>+ {t(addKeys[kind])}</button>
        ))}
        <button type="button" className="bot-wizard-chip" aria-expanded={more} data-testid="bot-wizard-add-more" onClick={() => setMore(!more)}>
          {t('bot.wizard.add.more')} {more ? '▴' : '▾'}
        </button>
        {more && moreExtraKinds.map((kind) => (
          <button key={kind} type="button" className="bot-wizard-chip" data-testid={'bot-wizard-add-' + kind} onClick={() => add(kind)}>+ {t(addKeys[kind])}</button>
        ))}
        {/* A template job taken away, put back. */}
        {away.map((i) => (
          <button key={'away-' + i} type="button" className="bot-wizard-chip" title={t('bot.wizard.job.restore')} data-testid="bot-wizard-restore" onClick={() => setJob(i, true)}>
            + {jobWords(template, c, i, t)}
          </button>
        ))}
      </div>
    </>
  );
}

/** MC-104: one job's card, the same for a template's and an added one: what it watches and × at the head, then only when it works
 *  (its time, what an added one watches, its conditions as chips), and 고급 folded with its own request and the raw condition rows
 *  (opened at first when it has either, or when the 지침과 요청 step left no request). */
function JobCard({ head, testId, onRemove, prompt, main, onPrompt, watch, conditions, onConditions, children }: {
  head: string; testId: string; onRemove: () => void; prompt: string; main: string; onPrompt: (value: string) => void;
  watch: Watch; conditions: RuleCondition[] | null; onConditions: (conditions: RuleCondition[]) => void; children?: ReactNode;
}) {
  const { t } = useServices();
  const [advanced, setAdvanced] = useState(() => prompt.trim() !== '' || main.trim() === '' || (conditions !== null && conditionChips(watch, conditions).raw.length > 0));
  return (
    <div className="bot-wizard-job-card" data-testid={testId}>
      <div className="bot-wizard-job-head">
        <b>{head}</b>
        <button type="button" className="icon-button" title={t('bot.wizard.job.remove')} aria-label={t('bot.wizard.job.remove')} data-testid="bot-wizard-job-remove" onClick={onRemove}>
          <Icon name="x" size={12} />
        </button>
      </div>
      {children}
      {conditions && <ConditionChips watch={watch} conditions={conditions} onChange={onConditions} />}
      <details className="bot-wizard-advanced" open={advanced} onToggle={(e) => setAdvanced(e.currentTarget.open)}>
        <summary>{t('bot.wizard.advanced')}</summary>
        <div className="bot-wizard-advanced-body">
          <PromptField value={prompt} main={main} onChange={onPrompt} />
          {conditions && <RawConditions watch={watch} conditions={conditions} onChange={onConditions} />}
        </div>
      </details>
    </div>
  );
}

/** MC-104: a trigger's conditions as chips, each a preset of its event type's catalogue with its value edited in place and × to take it
 *  away, then + 조건 offering that type's presets. */
function ConditionChips({ watch, conditions, onChange }: { watch: Watch; conditions: RuleCondition[]; onChange: (conditions: RuleCondition[]) => void }) {
  const { t } = useServices();
  const [adding, setAdding] = useState(false);
  const { chips } = conditionChips(watch, conditions);
  const presets = conditionPresets[watch];
  if (presets.length === 0 && chips.length === 0) return null;
  const setValue = (index: number, value: string) => onChange(conditions.map((c, k) => (k === index ? { ...c, value } : c)));
  return (
    <div className="bot-wizard-conds" role="group" aria-label={t('bot.wizard.cond.title')}>
      {chips.map((chip) => {
        const value = conditions[chip.index]?.value ?? '';
        const label = t(chip.preset.label);
        return (
          <span key={chip.index} className="bot-wizard-cond" data-testid={'bot-wizard-cond-' + chip.preset.id}>
            <span>{label}</span>
            {chip.preset.input === 'stage' ? (
              <select value={value} aria-label={label} onChange={(e) => setValue(chip.index, e.target.value)}>
                {mightyworkStages.map((stage) => <option key={stage} value={stage}>{t(stageKeys[stage])}</option>)}
                {!(mightyworkStages as readonly string[]).includes(value) && <option value={value}>{value}</option>}
              </select>
            ) : (
              <input value={value} aria-label={label} placeholder={chip.preset.hint} size={Math.max(6, value.length + 1)} onChange={(e) => setValue(chip.index, e.target.value)} />
            )}
            <button type="button" className="bot-wizard-cond-x" title={t('automation.form.whereRemove')} aria-label={t('automation.form.whereRemove')}
              onClick={() => onChange(withoutChip(conditions, chip))}><Icon name="x" size={10} /></button>
          </span>
        );
      })}
      {presets.length > 0 && (
        <button type="button" className="bot-wizard-chip" aria-expanded={adding} data-testid="bot-wizard-cond-add" onClick={() => setAdding(!adding)}>{t('bot.wizard.cond.add')}</button>
      )}
      {adding && presets.map((p) => (
        <button key={p.id} type="button" className="bot-wizard-chip" data-testid={'bot-wizard-cond-add-' + p.id}
          onClick={() => { onChange(withPreset(conditions, p)); setAdding(false); }}>+ {t(p.label)}</button>
      ))}
    </div>
  );
}

/** MC-104: the conditions no chip says, as rows under 고급: the field from its event type's list (or typed), the op in words, the
 *  value, 값이 있을 때만 and ×; + 직접 조건 adds one. */
function RawConditions({ watch, conditions, onChange }: { watch: Watch; conditions: RuleCondition[]; onChange: (conditions: RuleCondition[]) => void }) {
  const { t } = useServices();
  const { raw } = conditionChips(watch, conditions);
  const fields = watchFields(watch);
  const other = ':other';
  const setRow = (i: number, patch: Partial<RuleCondition>) => onChange(conditions.map((c, k) => (k === i ? { ...c, ...patch } : c)));
  return (
    <>
      <span className="bot-wizard-label">{t('bot.wizard.cond.raw')}</span>
      {raw.map((i) => {
        const c = conditions[i]!;
        const known = fields.includes(c.field.trim());
        return (
          <div key={i} className="bot-wizard-rule-row" data-testid="bot-wizard-raw-row">
            <select className="dialog-field" value={known ? c.field.trim() : other} aria-label={t('automation.form.whereField')}
              onChange={(e) => setRow(i, { field: e.target.value === other ? '' : e.target.value })}>
              {fields.map((field) => <option key={field} value={field}>{field}</option>)}
              <option value={other}>{t('bot.wizard.cond.fieldOther')}</option>
            </select>
            {!known && <input className="dialog-field mono" value={c.field} aria-label={t('automation.form.whereField')} placeholder="data.env" onChange={(e) => setRow(i, { field: e.target.value })} />}
            <select className="dialog-field" value={c.op} aria-label={t('automation.form.whereOp')} onChange={(e) => setRow(i, { op: e.target.value as RuleOp })}>
              {ruleOps.map((op) => <option key={op} value={op}>{opWords(op, t)}</option>)}
            </select>
            <input className="dialog-field mono" value={c.value} aria-label={t('automation.form.whereValue')} onChange={(e) => setRow(i, { value: e.target.value })} />
            <label className="bot-wizard-rule-check">
              <input type="checkbox" checked={c.ifPresent} onChange={(e) => setRow(i, { ifPresent: e.target.checked })} />
              <span>{t('automation.form.whereIfPresent')}</span>
            </label>
            <button type="button" className="icon-button" title={t('automation.form.whereRemove')} aria-label={t('automation.form.whereRemove')}
              onClick={() => onChange(conditions.filter((_, k) => k !== i))}><Icon name="x" size={12} /></button>
          </div>
        );
      })}
      <div>
        <button type="button" className="bot-wizard-chip" disabled={conditions.length >= 16} data-testid="bot-wizard-raw-add"
          onClick={() => onChange([...conditions, { field: fields[0] ?? '', op: 'eq', value: '', ifPresent: false }])}>{t('bot.wizard.cond.addRaw')}</button>
      </div>
    </>
  );
}

/** MC-79: what an added job watches: its time, its GitHub event and repository, the bot whose request ending (MC-104: or failing)
 *  wakes it, the event, or the MightyWork project. */
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
          <select className="dialog-field" value={job.outcome} aria-label={t('bot.wizard.bot.outcome')} data-testid="bot-wizard-bot-outcome"
            onChange={(e) => onChange({ ...job, outcome: e.target.value === 'failed' ? 'failed' : 'done' })}>
            <option value="done">{t('bot.wizard.bot.done')}</option>
            <option value="failed">{t('bot.wizard.bot.failed')}</option>
          </select>
        </div>
      );
    case 'event':
      return (
        <div className="bot-wizard-time-field">
          <input className="dialog-field" value={job.name} aria-label={t('bot.wizard.event.name')} placeholder={t('bot.wizard.event.hint')}
            onChange={(e) => onChange({ ...job, name: e.target.value })} />
        </div>
      );
    case 'mightywork':
      return (
        <div className="bot-wizard-time-field">
          <span>{t('bot.wizard.project')}</span>
          <input className="dialog-field mono" value={job.project} aria-label={t('bot.wizard.project')} placeholder="MC" data-testid="bot-wizard-extra-project"
            onChange={(e) => onChange({ ...job, project: e.target.value })} />
          <small className="bot-wizard-hint">{t('bot.wizard.project.hint')}</small>
        </div>
      );
  }
}

/** MC-104: the 얼마나 맡길까요 step's part for the triggers: what each does and its policy, flat for one, by its words for several. */
export function TriggerActions({ template, choices, onChange }: { template: WizardTemplate; choices: WizardChoices; onChange: (choices: WizardChoices) => void }) {
  const { t } = useServices();
  const items = triggerRules(choices);
  if (items.length === 0) return null;
  const words = (item: (typeof items)[number]) => ('index' in item ? jobLine(template, choices, item.index, t) : extraLine(choices.extra[item.extra]!, t));
  const action = (item: (typeof items)[number]) => <RuleAction rule={item.rule} onChange={(rule) => onChange(withTriggerRule(choices, item, rule))} />;
  return (
    <>
      <span className="bot-wizard-label">{t('bot.wizard.limits.rules')}</span>
      {items.length === 1 ? <div className="bot-wizard-limit">{action(items[0]!)}</div> : items.map((item) => (
        <section key={'index' in item ? 'job-' + item.index : 'extra-' + item.extra} className="bot-wizard-limit" data-testid="bot-wizard-limit">
          <b className="bot-wizard-limit-head">{words(item)}</b>
          {action(item)}
        </section>
      ))}
    </>
  );
}

/** MC-104 (from MC-99's rule editor): what a trigger does in words (요청 보내기, 개발하기 with PR and merge, 명령 실행하기 with its
 *  command, 이벤트 보내기 with its name), two plain toggles (실행 전에 내 승인 받기, its default said; 같은 일감·PR엔 한 번만), and the
 *  rest of its policy under 고급 (동시 실행, 쉬는 시간, 하루 상한). */
function RuleAction({ rule, onChange }: { rule: WizardRule; onChange: (rule: WizardRule) => void }) {
  const { t } = useServices();
  const state = t(approvesByDefault(rule) ? 'bot.wizard.limits.on' : 'bot.wizard.limits.off');
  const [advanced, setAdvanced] = useState(() => rule.concurrency !== '' || rule.cooldown !== '' || rule.cap !== '');
  return (
    <div className="bot-wizard-action" data-testid="bot-wizard-action">
      <div className="bot-wizard-rule-row">
        <label className="bot-wizard-time-field">
          <span>{t('automation.form.do')}</span>
          <select className="dialog-field" value={rule.kind} aria-label={t('automation.form.do')} data-testid="bot-wizard-rule-do"
            onChange={(e) => onChange({ ...rule, kind: e.target.value as ActionKindName })}>
            {actionKinds.map((kind) => <option key={kind} value={kind}>{t(doKeys[kind])}</option>)}
          </select>
        </label>
        {rule.kind === 'develop' && (
          <>
            <label className="bot-wizard-rule-check"><input type="checkbox" checked={rule.pr} onChange={(e) => onChange({ ...rule, pr: e.target.checked, merge: e.target.checked && rule.merge })} /><span>{t('automation.form.withPr')}</span></label>
            <label className="bot-wizard-rule-check"><input type="checkbox" checked={rule.merge} disabled={!rule.pr} onChange={(e) => onChange({ ...rule, merge: e.target.checked })} /><span>{t('automation.form.withMerge')}</span></label>
          </>
        )}
        {rule.kind === 'run' && (
          <input className="dialog-field mono" value={rule.command} aria-label={t('automation.form.withCommand')} placeholder="/release {{task.version}}"
            onChange={(e) => onChange({ ...rule, command: e.target.value })} />
        )}
        {rule.kind === 'emit' && (
          <input className="dialog-field mono" value={rule.event} aria-label={t('automation.form.withEvent')} placeholder="release.asked"
            onChange={(e) => onChange({ ...rule, event: e.target.value })} />
        )}
      </div>
      <small className="bot-wizard-hint">{t(actionNoteKeys[rule.kind])}</small>
      <div className="bot-wizard-rule-row">
        <label className="bot-wizard-rule-check">
          <input type="checkbox" checked={approves(rule)} data-testid="bot-wizard-rule-approve" onChange={(e) => onChange({ ...rule, approve: e.target.checked ? 'on' : 'off' })} />
          <span>{t('bot.wizard.limits.approve')}</span>
        </label>
        {rule.approve === ''
          ? <small className="bot-wizard-hint">{t('bot.wizard.limits.default', { state })}</small>
          : <button type="button" className="bot-wizard-link" onClick={() => onChange({ ...rule, approve: '' })}>{t('bot.wizard.limits.reset', { state })}</button>}
      </div>
      <label className="bot-wizard-rule-check">
        <input type="checkbox" checked={rule.oncePer} data-testid="bot-wizard-rule-once" onChange={(e) => onChange({ ...rule, oncePer: e.target.checked })} />
        <span>{t('bot.wizard.limits.oncePer')}</span>
      </label>
      <details className="bot-wizard-advanced" open={advanced} onToggle={(e) => setAdvanced(e.currentTarget.open)}>
        <summary>{t('bot.wizard.advanced')}</summary>
        <div className="bot-wizard-rule-row">
          {([['concurrency', 'automation.form.concurrency', '1'], ['cooldown', 'automation.form.cooldown', '30'], ['cap', 'automation.form.policyCap', '5']] as const).map(([key, label, hint]) => (
            <label key={key} className="bot-wizard-rule-number">
              <span>{t(label)}</span>
              <input className="dialog-field" inputMode="numeric" value={rule[key]} placeholder={hint} aria-label={t(label)} onChange={(e) => onChange({ ...rule, [key]: e.target.value })} />
            </label>
          ))}
        </div>
      </details>
    </div>
  );
}

/** MC-79: the request one job sends, the 지침과 요청 step's (`main`, MC-101) unless the person writes another. MC-104: under the
 *  card's 고급, so no fold of its own. */
function PromptField({ value, main, onChange }: { value: string; main: string; onChange: (value: string) => void }) {
  const { t } = useServices();
  const fallback = main.trim();
  return (
    <div className="bot-wizard-prompt">
      <span className="bot-wizard-label">{t('bot.wizard.job.prompt')}</span>
      <textarea className="dialog-field bot-wizard-textarea" value={value} aria-label={t('bot.wizard.job.prompt')}
        placeholder={fallback || t('bot.wizard.job.promptNeeded')} onChange={(e) => onChange(e.target.value)} />
      {fallback !== '' && <small className="bot-wizard-hint">{t('bot.wizard.job.promptHint')}</small>}
    </div>
  );
}

/** MC-101: the 지침과 요청 step: the template's examples as chips (one asks before it replaces words the person wrote), then the
 *  instructions and the request every job sends, both filled in and editable, and the event values the request may use. */
function PromptStep({ template, choices, onChange }: { template: WizardTemplate; choices: WizardChoices; onChange: (patch: Partial<WizardChoices>) => void }) {
  const { t } = useServices();
  const [asking, setAsking] = useState<WizardExample | null>(null);
  const on = exampleOn(template, choices);
  const use = (example: WizardExample) => {
    const next = withExample(choices, example);
    onChange({ instructions: next.instructions, prompt: next.prompt });
    setAsking(null);
  };
  const choose = (example: WizardExample) => (example.id !== on && ownWords(template, choices) ? setAsking(example) : use(example));
  const schedulesOnly = template.jobs.length > 0 && template.jobs.every((job) => job.cron !== null);
  return (
    <>
      {template.examples.length > 0 && (
        <>
          <span className="bot-wizard-label">{t('bot.wizard.examples')}</span>
          <div className="bot-wizard-chips" role="group" aria-label={t('bot.wizard.examples')}>
            {template.examples.map((example) => (
              <button key={example.id} type="button" aria-pressed={example.id === on} className={'bot-wizard-chip' + (example.id === on ? ' on' : '')}
                data-testid={'bot-wizard-example-' + example.id} onClick={() => choose(example)}>
                {example.label}
              </button>
            ))}
          </div>
          {asking && (
            <div className="bot-wizard-ask" role="alertdialog" aria-label={t('bot.wizard.example.replace', { name: asking.label })} data-testid="bot-wizard-example-ask">
              <span>{t('bot.wizard.example.replace', { name: asking.label })}</span>
              <button type="button" className="btn primary" onClick={() => use(asking)}>{t('bot.wizard.example.replaceYes')}</button>
              <button type="button" className="btn" onClick={() => setAsking(null)}>{t('bot.wizard.example.keep')}</button>
            </div>
          )}
        </>
      )}
      <label className="bot-wizard-label" htmlFor="bot-wizard-instructions">{t('bot.wizard.instructions')}</label>
      <textarea id="bot-wizard-instructions" className="dialog-field bot-wizard-textarea" value={choices.instructions} placeholder={t('bot.dialog.instructionsHint')}
        data-testid="bot-wizard-instructions" onChange={(e) => onChange({ instructions: e.target.value })} data-autofocus />
      <small className="bot-wizard-hint">{t('bot.dialog.instructionsHint')}</small>
      <label className="bot-wizard-label" htmlFor="bot-wizard-main-prompt">{t('bot.wizard.prompt')}</label>
      <textarea id="bot-wizard-main-prompt" className="dialog-field bot-wizard-textarea" value={choices.prompt} placeholder={t('bot.wizard.job.promptNeeded')}
        data-testid="bot-wizard-prompt" onChange={(e) => onChange({ prompt: e.target.value })} />
      <small className="bot-wizard-hint">{t(template.jobs.length > 0 ? 'bot.wizard.prompt.hint' : 'bot.wizard.prompt.hintNone')}</small>
      {template.vars.length > 0 ? (
        <small className="bot-wizard-hint bot-wizard-vars" data-testid="bot-wizard-vars">
          {t('bot.wizard.prompt.vars')} {template.vars.map((name) => <code key={name}>{`{{${name}}}`}</code>)}
        </small>
      ) : schedulesOnly && <small className="bot-wizard-hint">{t('bot.wizard.prompt.varsNone')}</small>}
    </>
  );
}
