// MC-71: the new-bot wizard (B): the template gallery first (categories, search, what it does, when, its instructions), then
// 어디서·누가 → 언제·권한 → 확인; ⚡ 빠르게 만들기 takes the template as it is to the last look. The host gives what it picks from
// (`botWizard`) and makes the bot (`botWizardCreate`, bot_wizard.rs); the bot's pane opens in the 봇 tab once it is made.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { useServices } from '../app/context';
import { DialogLoading, DialogShell } from '../dialogs/DialogShell';
import { Icon } from '../icons/Icon';
import { ProviderMark } from '../conversation/ProviderMark';
import { usePageOverlay } from './overlay';
import {
  choicesFor, createRequest, filterTemplates, independent, independentHere, independentPick, quickTarget, steps, whenWords, whereProblem,
  type Step, type WizardChoices, type WizardModel, type WizardTemplate,
} from './botWizardRules';
import './botWizard.css';

/** The event that opens the wizard (the 봇 tab's 봇 추가, its empty list's button). */
export const openBotWizardEvent = 'mighty:open-bot-wizard';
export function openBotWizard() {
  window.dispatchEvent(new Event(openBotWizardEvent));
}

const stepTitles: Record<Step, string> = {
  template: 'bot.new.template',
  where: 'bot.wizard.step.where',
  when: 'bot.wizard.step.when',
  confirm: 'bot.wizard.step.confirm',
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
  const problem = template && choices && step === 'where' ? whereProblem(template, choices) : null;
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

  const body = !model || !choices || !template ? <DialogLoading rows={5} /> : (
    <div className="bot-wizard" data-testid="bot-wizard">
      <ol className="bot-wizard-steps" aria-label={t('bot.new.title')}>
        {steps.map((s, i) => (
          <li key={s} className={i < index ? 'done' : i === index ? 'current' : ''}>
            <button type="button" disabled={i > index || busy} onClick={() => go(s)}>
              <span className="bot-wizard-step-dot">{i < index ? <Icon name="check" size={10} weight={3} /> : i + 1}</span>
              {t(stepTitles[s])}
            </button>
          </li>
        ))}
      </ol>

      {step === 'template' && (
        <div className="bot-wizard-gallery">
          <nav className="bot-wizard-categories">
            {[{ id: 'all', label: t('bot.wizard.category.all') }, ...model.categories].map((c) => (
              <button key={c.id} type="button" className={category === c.id ? 'on' : ''} onClick={() => setCategory(c.id)}>
                <span>{c.label}</span>
                <small>{c.id === 'all' ? model.templates.length : model.templates.filter((tp) => tp.category === c.id).length}</small>
              </button>
            ))}
          </nav>
          <div className="bot-wizard-gallery-main">
            <input className="dialog-field bot-wizard-search" type="search" value={query} placeholder={t('bot.wizard.search')} aria-label={t('bot.wizard.search')}
              onChange={(e) => setQuery(e.target.value)} data-autofocus />
            {shown.length === 0 ? <p className="bot-wizard-muted">{t('bot.wizard.noResults')}</p> : (
              <div className="bot-wizard-cards" role="listbox" aria-label={t('bot.new.template')}>
                {shown.map((tp) => (
                  <button key={tp.id} type="button" role="option" aria-selected={tp.id === template.id} className={'bot-wizard-card' + (tp.id === template.id ? ' on' : '')}
                    data-testid={'bot-template-' + tp.id} onClick={() => pick(tp)} onDoubleClick={() => { pick(tp); go('where'); }}>
                    <span className="bot-wizard-card-icon" aria-hidden="true">{tp.icon}</span>
                    <b>{tp.label}</b>
                    <span>{tp.what}</span>
                    <em>{tp.jobs[0] ?? t('bot.wizard.detail.onCall')}</em>
                  </button>
                ))}
              </div>
            )}
            <div className="bot-wizard-detail">
              <div><b>{t('bot.wizard.detail.what')}</b>{template.what}</div>
              <div><b>{t('bot.wizard.detail.when')}</b>{template.jobs.length > 0 ? template.jobs.join(' · ') : t('bot.wizard.detail.onCall')}</div>
              {template.instructions && <div className="wide"><b>{t('bot.wizard.detail.instructions')}</b>{template.instructions}</div>}
            </div>
          </div>
        </div>
      )}

      {step === 'where' && (
        <div className="bot-wizard-columns">
          <div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-where">{t('bot.wizard.where')}</label>
            <select id="bot-wizard-where" className="dialog-field" value={choices.where} onChange={(e) => set({ where: e.target.value })}>
              {model.workspaces.map((w) => <option key={w.id} value={w.id}>{whereLabel(w.id)}</option>)}
              <option value={independentHere}>{whereLabel(independentHere)}</option>
              <option value={independentPick}>{whereLabel(independentPick)}</option>
            </select>
            {template.needs === 'repo' && (
              <>
                <label className="bot-wizard-label" htmlFor="bot-wizard-repo">{t('bot.wizard.repo')}</label>
                <input id="bot-wizard-repo" className="dialog-field" value={choices.repo} placeholder="owner/name" onChange={(e) => set({ repo: e.target.value })} />
                <p className="bot-wizard-hint">{t(independent(choices.where) ? 'bot.wizard.repo.needed' : 'bot.wizard.repo.workspaceHint')}</p>
              </>
            )}
            {template.needs === 'command' && (
              <>
                <label className="bot-wizard-label" htmlFor="bot-wizard-command">{t('bot.wizard.command')}</label>
                <input id="bot-wizard-command" className="dialog-field mono" value={choices.command} placeholder={template.commandHint} onChange={(e) => set({ command: e.target.value })} />
              </>
            )}
            <label className="bot-wizard-label" htmlFor="bot-wizard-instructions">{t('bot.wizard.instructions')}</label>
            <textarea id="bot-wizard-instructions" className="dialog-field bot-wizard-textarea" value={choices.instructions} placeholder={t('bot.dialog.instructionsHint')}
              onChange={(e) => set({ instructions: e.target.value })} />
          </div>
          <div>
            <span className="bot-wizard-label">{t('bot.new.agent')}</span>
            <div className="bot-wizard-agents" role="radiogroup" aria-label={t('bot.new.agent')}>
              {model.agents.map((a) => (
                <button key={a.id} type="button" role="radio" aria-checked={a.id === choices.agent} className={'bot-wizard-agent' + (a.id === choices.agent ? ' on' : '')}
                  onClick={() => set({ agent: a.id, permission: '' })}>
                  <ProviderMark provider={a.id} size={16} />
                  <span>{a.label}</span>
                  {a.available === false && <small>{t('bot.wizard.agentMissing')}</small>}
                </button>
              ))}
            </div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-name">{t('bot.wizard.name')}</label>
            <div className="bot-wizard-row">
              <input id="bot-wizard-name" className="dialog-field" value={choices.name} placeholder={t('bot.new.nameHint')} onChange={(e) => set({ name: e.target.value })} />
              <button type="button" className="btn" title={t('bot.wizard.another')} aria-label={t('bot.wizard.another')} onClick={another}>🎲</button>
            </div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-model">{t('bot.wizard.model')}</label>
            <input id="bot-wizard-model" className="dialog-field" value={choices.model} placeholder={t('bot.wizard.modelHint')} onChange={(e) => set({ model: e.target.value })} />
          </div>
        </div>
      )}

      {step === 'when' && (
        <div className="bot-wizard-columns">
          <div>
            <span className="bot-wizard-label">{t('bot.wizard.when')}</span>
            {template.jobs.length === 0 ? <p className="bot-wizard-muted">{t('bot.wizard.noJobs')}</p> : template.jobs.map((words, i) => (
              <label key={i} className="bot-wizard-job">
                <input type="checkbox" checked={choices.jobs[i] !== false} onChange={(e) => set({ jobs: choices.jobs.map((on, k) => (k === i ? e.target.checked : on)) })} />
                <span>{words}</span>
              </label>
            ))}
            {template.jobs.length > 0 && (
              <>
                <label className="bot-wizard-job">
                  <input type="checkbox" checked={choices.awake} onChange={(e) => set({ awake: e.target.checked })} />
                  <span>{t('bot.wizard.awake')}</span>
                </label>
                <p className="bot-wizard-hint">{t('bot.wizard.awakeHint')}</p>
              </>
            )}
          </div>
          <div>
            <span className="bot-wizard-label">{t('bot.wizard.permission')}</span>
            <div className="bot-wizard-chips" role="radiogroup" aria-label={t('bot.wizard.permission')}>
              {[{ value: '', label: t('bot.wizard.permission.default') }, ...(agent?.permissions ?? [])].map((p) => (
                <button key={p.value || 'default'} type="button" role="radio" aria-checked={choices.permission === p.value} className={'bot-wizard-chip' + (choices.permission === p.value ? ' on' : '')}
                  onClick={() => set({ permission: p.value })}>{p.label}</button>
              ))}
            </div>
            <label className="bot-wizard-label" htmlFor="bot-wizard-budget">{t('bot.wizard.budget')}</label>
            <input id="bot-wizard-budget" type="range" min={0} max={20} step={1} value={choices.budget} onChange={(e) => set({ budget: Number(e.target.value) })} className="bot-wizard-range" />
            <p className="bot-wizard-hint">{choices.budget > 0 ? t('bot.wizard.budget.amount', { amount: choices.budget }) : t('bot.wizard.budget.none')}</p>
            <label className="bot-wizard-job">
              <input type="checkbox" checked={choices.greet} onChange={(e) => set({ greet: e.target.checked })} />
              <span>{t('bot.wizard.greet')}</span>
            </label>
          </div>
        </div>
      )}

      {step === 'confirm' && (
        <dl className="bot-wizard-summary" data-testid="bot-wizard-summary">
          <div><dt>{t('bot.wizard.summary.bot')}</dt><dd>@{choices.name.replace(/^@/, '') || model.name} · {agent?.label ?? choices.agent}{choices.model ? ' · ' + choices.model : ''}</dd></div>
          <div><dt>{t('bot.wizard.summary.task')}</dt><dd>{template.icon} {template.label}</dd></div>
          <div><dt>{t('bot.wizard.summary.where')}</dt><dd>{whereLabel(choices.where)}{choices.repo.trim() ? ' · ' + choices.repo.trim() : ''}</dd></div>
          <div><dt>{t('bot.wizard.summary.when')}</dt><dd>{whenWords(template, choices).join(' · ') || t('bot.wizard.detail.onCall')}</dd></div>
          <div><dt>{t('bot.wizard.summary.limits')}</dt><dd>{(agent?.permissions.find((p) => p.value === choices.permission)?.label ?? t('bot.wizard.permission.default'))} · {choices.budget > 0 ? t('bot.wizard.budget.amount', { amount: choices.budget }) : t('bot.wizard.budget.none')}</dd></div>
          {template.jobs.length > 0 && <div><dt>{t('bot.wizard.summary.start')}</dt><dd>{t(choices.awake ? 'bot.wizard.summary.awake' : 'bot.wizard.summary.asleep')}</dd></div>}
        </dl>
      )}
    </div>
  );

  const shownError = error ?? (problem ? t(problem) : null);
  const buttons = (
    <>
      {index > 0 && <button type="button" className="btn" disabled={busy} onClick={() => go(steps[index - 1] ?? 'template')}>{t('bot.wizard.back')}</button>}
      <button type="button" className="btn" disabled={busy} onClick={onClose}>{t('bot.dialog.cancel')}</button>
      {step === 'template' && template && choices && (
        <button type="button" className="btn" title={t('bot.wizard.quickHelp')} data-testid="bot-wizard-quick" onClick={() => go(quickTarget(template, choices))}>⚡ {t('bot.wizard.quick')}</button>
      )}
      {step === 'confirm'
        ? <button type="button" className="btn primary" disabled={busy || !choices} data-testid="bot-wizard-create" onClick={() => void create()}>{busy ? t('bot.wizard.creating') : t('bot.new.create')}</button>
        : <button type="button" className="btn primary" disabled={!choices || problem !== null} data-testid="bot-wizard-next" onClick={() => go(steps[index + 1] ?? 'confirm')}>{t('bot.wizard.next')}</button>}
    </>
  );

  return createPortal(
    <DialogShell
      kind="panel"
      panel="botWizard"
      width={860}
      height={600}
      minWidth={640}
      minHeight={480}
      label={t('bot.new.title')}
      title={t('bot.new.title')}
      subtitle={template ? template.icon + ' ' + template.label : undefined}
      icon={<Icon name="bot" size={18} weight={1.8} />}
      busy={busy}
      onCancel={onClose}
      testId="bot-wizard-panel"
      backdrop={{ 'data-page-overlay': '' }}
      status={shownError ? <span className="tone-text-err" role="alert">{shownError}</span> : undefined}
      buttons={buttons}
    >
      {body}
    </DialogShell>,
    document.body,
  );
}
