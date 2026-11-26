// MC-71: the new-bot wizard's rules, apart from its drawing (BotWizard.tsx): what the gallery shows and whether a step may go on. The words come from the host (`botWizard`, bot_wizard.rs) in the app's language.
// MC-99: a trigger job's rule (its conditions, what it does and its policy) as the 언제 step shows and edits it, and a MightyWork
// recipe's project key.
// MC-101: a template's examples (other instructions and request for the same job) and the 지침과 요청 step that edits both; its
// request is every job's unless the 언제 step gives one its own.
// MC-104: the 언제 step made simpler: what each job watches in words, its conditions as plain chips chosen per event type (a
// catalogue below, the rest kept as raw rows), the ops in words, a sentence saying what the bot will do; what a trigger does and
// its policy belong to the 얼마나 맡길까요 step.

export interface WizardTemplate {
  id: string;
  icon: string;
  category: string;
  label: string;
  what: string;
  instructions: string;
  needs: 'nothing' | 'repo' | 'command' | 'project';
  commandHint: string;
  /** MC-79: the request its jobs send (empty for 빈 봇), shown where the person may write another. */
  prompt: string;
  /** Its schedules and triggers: when each runs, in words, and a schedule's cron line (its time the person may change). */
  jobs: WizardJob[];
  /** MC-101: its examples, the template's own first (empty when it has no other). */
  examples: WizardExample[];
  /** MC-101: the event values its request may use (`pr.title`), none for a schedule. */
  vars: string[];
}

/** MC-101: an example: other instructions and another request (empty: the request stays as it is) for the template's jobs. */
export interface WizardExample {
  id: string;
  label: string;
  instructions: string;
  prompt: string;
}

export interface WizardJob {
  when: string;
  cron: string | null;
  /** MC-99: a trigger's rule as the host starts it (null for a schedule). */
  rule?: HostRule | null;
  /** MC-104: what it watches (null for a schedule; absent from an older host, its words then the template's). */
  source?: JobSource | null;
}

/** MC-104: what a template's trigger watches, as the host says (bot_wizard.rs `JobDef::source`). */
export type JobSource =
  | { kind: 'github'; event: GithubEventName }
  | { kind: 'mcp'; preset: string | null }
  | { kind: 'event'; name: string }
  | { kind: 'standing' };

// ── MC-99: a trigger's rule ────────────────────────────────────────────────────────────────────────────────────────────────

export const ruleOps = ['eq', 'in', 'has', 'matches', 'not_eq', 'not_in', 'not_has', 'not_matches'] as const;
export type RuleOp = (typeof ruleOps)[number];
export const actionKinds = ['ask', 'develop', 'run', 'emit'] as const;
export type ActionKindName = (typeof actionKinds)[number];
/** MC-104: each kind as the 얼마나 맡길까요 step offers it, in words (요청 보내기, 개발하기, 명령 실행하기, 이벤트 보내기), and the
 *  line on what it does. */
export const doKeys: Record<ActionKindName, string> = {
  ask: 'bot.wizard.do.ask', develop: 'bot.wizard.do.develop', run: 'bot.wizard.do.run', emit: 'bot.wizard.do.emit',
};
export const actionNoteKeys: Record<ActionKindName, string> = {
  ask: 'automation.form.doNote.ask', develop: 'automation.form.doNote.develop', run: 'automation.form.doNote.run', emit: 'automation.form.doNote.emit',
};

/** A condition as the host writes it (`value` a list for in and not_in). */
export interface HostCondition { field: string; op: RuleOp; value: string | string[]; ifPresent?: boolean }
export interface RuleWith { pr?: boolean; merge?: boolean; command?: string; event?: string; data?: Record<string, string> }
export interface RulePolicy { approve?: boolean; oncePer?: 'subject'; concurrency?: number; cooldownMinutes?: number; dailyCap?: number }
export interface HostRule { where?: HostCondition[]; do?: ActionKindName; with?: RuleWith; policy?: RulePolicy }

/** A condition row as the 언제 step edits it (an in list comma-separated). */
export interface RuleCondition { field: string; op: RuleOp; value: string; ifPresent: boolean }
export interface WizardRule {
  conditions: RuleCondition[];
  kind: ActionKindName;
  pr: boolean;
  merge: boolean;
  command: string;
  event: string;
  /** An emit's data as the host gave it (kept as it is). */
  data: Record<string, string>;
  /** '' its kind's default, 'on' always, 'off' never. */
  approve: '' | 'on' | 'off';
  oncePer: boolean;
  /** As typed ('' unset). */
  concurrency: string;
  cooldown: string;
  cap: string;
}

const listOp = (op: RuleOp) => op === 'in' || op === 'not_in';

export function ruleOf(host: HostRule): WizardRule {
  const p = host.policy ?? {};
  const w = host.with ?? {};
  return {
    conditions: (host.where ?? []).map((c) => ({ field: c.field, op: c.op, value: Array.isArray(c.value) ? c.value.join(', ') : c.value, ifPresent: c.ifPresent === true })),
    kind: host.do ?? 'ask',
    pr: w.pr !== false,
    merge: w.merge === true,
    command: w.command ?? '',
    event: w.event ?? '',
    data: w.data ?? {},
    approve: p.approve === undefined ? '' : p.approve ? 'on' : 'off',
    oncePer: p.oncePer === 'subject',
    concurrency: p.concurrency === undefined ? '' : String(p.concurrency),
    cooldown: p.cooldownMinutes === undefined ? '' : String(p.cooldownMinutes),
    cap: p.dailyCap === undefined ? '' : String(p.dailyCap),
  };
}

const numberOf = (text: string): number | undefined => (text.trim() === '' ? undefined : Number(text.trim()));

/** The rule as the host reads it (`botWizardCreate`'s `rules`): empty rows left out, with and policy only what is set. */
export function hostRule(rule: WizardRule): HostRule {
  const where = rule.conditions
    .filter((c) => c.field.trim() !== '' || c.value.trim() !== '')
    .map((c) => ({
      field: c.field.trim(),
      op: c.op,
      value: listOp(c.op) ? c.value.split(',').map((v) => v.trim()).filter(Boolean) : c.value.trim(),
      ...(c.ifPresent ? { ifPresent: true } : {}),
    }));
  const w: RuleWith = {};
  if (rule.kind === 'develop') {
    w.pr = rule.pr;
    if (rule.merge) w.merge = true;
  }
  if (rule.kind === 'run') w.command = rule.command.trim();
  if (rule.kind === 'emit') {
    w.event = rule.event.trim();
    if (Object.keys(rule.data).length > 0) w.data = rule.data;
  }
  const policy: RulePolicy = {};
  if (rule.approve !== '') policy.approve = rule.approve === 'on';
  if (rule.oncePer) policy.oncePer = 'subject';
  const [concurrency, cooldown, cap] = [numberOf(rule.concurrency), numberOf(rule.cooldown), numberOf(rule.cap)];
  if (concurrency !== undefined) policy.concurrency = concurrency;
  if (cooldown !== undefined) policy.cooldownMinutes = cooldown;
  if (cap !== undefined) policy.dailyCap = cap;
  return { where, do: rule.kind, with: w, policy };
}

/** Whether a run, or a merging develop, asks the person first unless set otherwise (the host's default). */
export function approvesByDefault(rule: WizardRule): boolean {
  return rule.kind === 'run' || (rule.kind === 'develop' && rule.pr && rule.merge);
}

/** MC-104: why a rule's conditions cannot be made (a locale key), or null: the 언제 step's part. */
export function conditionsProblem(conditions: readonly RuleCondition[]): string | null {
  for (const c of conditions) {
    if ((c.field.trim() === '') !== (c.value.trim() === '')) return 'bot.wizard.rule.incomplete';
    if ((c.op === 'matches' || c.op === 'not_matches') && c.value.trim()) {
      try { new RegExp(c.value.trim()); } catch { return 'bot.wizard.rule.badRegex'; }
    }
  }
  return null;
}

/** MC-104: why what a rule does, or its policy, cannot be made (a locale key), or null: the 얼마나 맡길까요 step's part. */
export function actionProblem(rule: WizardRule): string | null {
  if (rule.kind === 'run' && rule.command.trim() === '') return 'bot.wizard.rule.needsCommand';
  if (rule.kind === 'emit' && !/^[a-z][a-z0-9._-]*$/.test(rule.event.trim())) return 'bot.wizard.extra.needsEvent';
  if (rule.kind === 'develop' && rule.merge && !rule.pr) return 'bot.wizard.rule.mergeNeedsPr';
  const ranges: [string, number, number][] = [[rule.concurrency, 1, 10], [rule.cooldown, 1, 10080], [rule.cap, 1, 100]];
  for (const [text, min, max] of ranges) {
    const n = numberOf(text);
    if (n !== undefined && !(Number.isInteger(n) && n >= min && n <= max)) return 'bot.wizard.rule.badNumber';
  }
  return null;
}

/** Why a rule cannot be made (a locale key), or null. */
export function ruleProblem(rule: WizardRule): string | null {
  return conditionsProblem(rule.conditions) ?? actionProblem(rule);
}

/** A rule in a few words (MC-104: in words, not code): its conditions as the chips say them, what it does and its policy
 *  (`할 일 단계로 바뀔 때 · 명령을 실행합니다: /release … · 먼저 내 승인을 받음, 같은 일감·PR엔 한 번만`). */
export function ruleWords(rule: WizardRule, t: Translate, watch: Watch = 'other'): string {
  const policy = [
    approves(rule) ? t('bot.wizard.say.approve') : '',
    rule.oncePer ? t('bot.wizard.limits.oncePer') : '',
  ].filter(Boolean).join(', ');
  return [...conditionsWords(watch, rule.conditions, t), actionWords(rule, t), policy].filter(Boolean).join(' · ');
}

/** MC-104: whether a rule waits for the person's approval: as set, else its kind's default. */
export function approves(rule: WizardRule): boolean {
  return rule.approve === 'on' || (rule.approve === '' && approvesByDefault(rule));
}

/** MC-104: what a rule does, as a sentence's end (`개발하고 PR을 올립니다`). */
export function actionWords(rule: WizardRule, t: Translate): string {
  switch (rule.kind) {
    case 'ask': return t('bot.wizard.say.ask');
    case 'develop': return t(!rule.pr ? 'bot.wizard.say.develop' : rule.merge ? 'bot.wizard.say.developMerge' : 'bot.wizard.say.developPr');
    case 'run': return t('bot.wizard.say.run', { command: rule.command.trim() });
    case 'emit': return t('bot.wizard.say.emit', { event: rule.event.trim() });
  }
}

/** A MightyWork project's key: letters, digits, - and _. */
export function validProject(key: string): boolean {
  return /^[A-Za-z0-9_-]{1,32}$/.test(key.trim());
}

export interface WizardAgent {
  id: string;
  label: string;
  /** Whether its CLI is installed (null while the app has not read it yet). */
  available: boolean | null;
  permissions: { value: string; label: string }[];
  /** The models its picker lists (empty: the name is typed). */
  models: { value: string; label: string }[];
}

export interface WizardModel {
  categories: { id: string; label: string }[];
  templates: WizardTemplate[];
  workspaces: { id: string; name: string }[];
  activeWorkspace: string | null;
  agents: WizardAgent[];
  /** MC-79: the bots there are, for a trigger on one's request ending. */
  bots: string[];
  name: string;
}

/** Where the bot works: a workspace's id, or an independent bot in the app's data folder or a folder the person picks. */
export type WizardWhere = string;
export const independentHere = ':independent';
export const independentPick = ':pick';

export interface WizardChoices {
  template: string;
  where: WizardWhere;
  repo: string;
  command: string;
  agent: string;
  model: string;
  name: string;
  instructions: string;
  /** MC-101: the request of the 지침과 요청 step, every job's unless `prompts` gives one its own. */
  prompt: string;
  jobs: boolean[];
  /** Each job's time as the person set it (null for a trigger, or a cron line the wizard cannot show). */
  times: (TimeRule | null)[];
  /** MC-79: the request each template job sends as the person rewrote it ('' for the template's). */
  prompts: string[];
  /** MC-79: the schedules and triggers the person added. */
  extra: ExtraJob[];
  /** MC-99: each template trigger's rule as the person changed it (null for a schedule). */
  rules: (WizardRule | null)[];
  /** MC-99: a MightyWork recipe's project key. */
  project: string;
  permission: string;
  /** Dollars a day; 0 for no limit. */
  budget: number;
  greet: boolean;
}

/** MC-71 (A, the classic wizard): the steps in order, the rail at the left naming them. MC-101: 지침과 요청 right after the gallery. */
export const steps = ['template', 'prompt', 'where', 'who', 'when', 'limits', 'confirm'] as const;
export type Step = (typeof steps)[number];

/** The gallery's templates for a category (`all` for every one) and a search (label, what or category words; MC-101: or an
 *  example's name, so 브리핑 finds 정기 작업). */
export function filterTemplates(model: WizardModel, category: string, query: string): WizardTemplate[] {
  const words = query.trim().toLowerCase();
  const label = (id: string) => model.categories.find((c) => c.id === id)?.label ?? '';
  return model.templates.filter((t) => (category === 'all' || t.category === category)
    && (words.length === 0 || [t.label, t.what, label(t.category), ...t.examples.map((e) => e.label)].some((text) => text.toLowerCase().includes(words))));
}

// ── MC-101: the 지침과 요청 step ───────────────────────────────────────────────────────────────────────────────────────────

/** Whether the instructions and request are an example's (its request left as it is when it has none). */
function isExample(example: WizardExample, choices: WizardChoices): boolean {
  return choices.instructions.trim() === example.instructions.trim() && (example.prompt === '' || choices.prompt.trim() === example.prompt.trim());
}

/** The example the instructions and request now are, if any (the chip shown on). */
export function exampleOn(template: WizardTemplate, choices: WizardChoices): string | null {
  return template.examples.find((e) => isExample(e, choices))?.id ?? null;
}

/** Whether the person wrote their own instructions or request: neither the template's nor an example's. An example then asks
 *  before it replaces them. */
export function ownWords(template: WizardTemplate, choices: WizardChoices): boolean {
  const asTemplate = choices.instructions.trim() === template.instructions.trim() && choices.prompt.trim() === template.prompt.trim();
  return !asTemplate && exampleOn(template, choices) === null;
}

/** The choices with an example's instructions and request (its request left as it is when it has none). Only the words change:
 *  the jobs, their times and rules stay. */
export function withExample(choices: WizardChoices, example: WizardExample): WizardChoices {
  return { ...choices, instructions: example.instructions, prompt: example.prompt === '' ? choices.prompt : example.prompt };
}

export function validRepo(repo: string): boolean {
  return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo.trim());
}

export function independent(where: WizardWhere): boolean {
  return where === independentHere || where === independentPick;
}

/** The choices a template starts with: its instructions, every job on, where the person is working, an installed agent. */
export function choicesFor(model: WizardModel, template: WizardTemplate, earlier?: WizardChoices): WizardChoices {
  const agent = earlier?.agent ?? (model.agents.find((a) => a.id === 'claude' && a.available !== false) ?? model.agents.find((a) => a.available !== false) ?? model.agents[0])?.id ?? 'claude';
  const where = earlier?.where ?? (model.activeWorkspace && model.workspaces.some((w) => w.id === model.activeWorkspace) ? model.activeWorkspace : model.workspaces[0]?.id ?? independentHere);
  return {
    template: template.id,
    where,
    repo: earlier?.repo ?? '',
    command: earlier?.template === template.id ? earlier.command : '',
    agent,
    model: earlier?.model ?? '',
    name: earlier?.name ?? model.name,
    instructions: template.instructions,
    prompt: template.prompt,
    jobs: template.jobs.map(() => true),
    times: template.jobs.map((job) => (job.cron ? parseCron(job.cron) : null)),
    prompts: template.jobs.map(() => ''),
    extra: [],
    rules: template.jobs.map((job) => (job.rule ? ruleOf(job.rule) : null)),
    project: earlier?.project ?? '',
    permission: earlier?.agent === agent ? earlier.permission : '',
    budget: earlier?.budget ?? 3,
    greet: earlier?.greet ?? true,
  };
}

/** Why the 어디서·누가 step cannot go on (a locale key), or null. */
export function whereProblem(template: WizardTemplate, choices: WizardChoices): string | null {
  const repo = choices.repo.trim();
  const jobsOn = choices.jobs.some(Boolean);
  if (template.needs === 'repo' && jobsOn && independent(choices.where) && !validRepo(repo)) return 'bot.wizard.repo.needed';
  if (repo.length > 0 && !validRepo(repo)) return 'bot.new.needsRepo';
  if (template.needs === 'command' && choices.command.trim().length === 0) return 'bot.wizard.needsCommand';
  if (template.needs === 'project' && jobsOn && !validProject(choices.project)) return 'bot.wizard.project.needed';
  return null;
}

/** Why a step cannot go on (a locale key), or null: 지침과 요청 needs a request when the template has jobs; 어디서 needs its
 *  repository or command; 누가 needs an agent. */
export function stepProblem(step: Step, template: WizardTemplate, choices: WizardChoices): string | null {
  if (step === 'prompt' && template.jobs.length > 0 && choices.prompt.trim() === '') return 'bot.wizard.prompt.needed';
  if (step === 'where') return whereProblem(template, choices);
  if (step === 'who' && choices.agent.trim().length === 0) return 'bot.wizard.needsAgent';
  if (step === 'when' && choices.times.some((rule, index) => rule !== null && choices.jobs[index] !== false && rule.days.length === 0)) return 'bot.wizard.time.noDays';
  if (step === 'when') {
    // MC-99: a rule of a job left on that cannot be made; MC-104: only its conditions here, what it does in 얼마나 맡길까요.
    const rule = choices.rules.find((r, index) => r !== null && choices.jobs[index] !== false && conditionsProblem(r.conditions) !== null);
    if (rule) return conditionsProblem(rule.conditions);
    return extraProblem(choices);
  }
  if (step === 'limits') {
    for (const { rule } of triggerRules(choices)) {
      const problem = actionProblem(rule);
      if (problem) return problem;
    }
  }
  return null;
}

/** A job's when, in words: its time as the person set it; MC-104: else what it watches (the template's words when the host
 *  does not say). */
export function jobWords(template: WizardTemplate, choices: WizardChoices, index: number, t: Translate): string {
  const rule = choices.times[index];
  if (rule) return timeWords(rule, t);
  const job = template.jobs[index];
  return job?.source ? sourceWords(job.source, choices, t) ?? job.when : job?.when ?? '';
}

/** MC-104: what a template trigger watches, in words (null: the template's own words say it best, as for a standing order). */
function sourceWords(source: JobSource, choices: WizardChoices, t: Translate): string | null {
  switch (source.kind) {
    case 'github': return t(githubEventKey(source.event));
    case 'mcp': return source.preset === 'mightywork' ? mightyworkWords(choices.project, t) : null;
    case 'event': return t('bot.wizard.event.when', { name: source.name });
    case 'standing': return null;
  }
}

const mightyworkWords = (project: string, t: Translate) =>
  (validProject(project) ? t('bot.wizard.watch.mightyworkIn', { project: project.trim() }) : t('bot.wizard.watch.mightywork'));
const githubEventKey = (event: GithubEventName) => githubEvents.find((e) => e.value === event)?.key ?? 'bot.wizard.when.prOpened';

/** MC-104: the condition chips' catalogue a template job takes. */
export function jobWatch(template: WizardTemplate, index: number): Watch {
  const source = template.jobs[index]?.source;
  if (!source) return 'other';
  switch (source.kind) {
    case 'github': return githubWatch(source.event);
    case 'mcp': return source.preset === 'mightywork' ? 'mightywork' : 'other';
    case 'event': return 'event';
    case 'standing': return 'standing';
  }
}

/** MC-99: a job's when, and its rule's conditions in words when it has one (MC-104: what it does is the sentence's end). */
export function jobLine(template: WizardTemplate, choices: WizardChoices, index: number, t: Translate): string {
  const rule = choices.rules[index];
  return withConditions(jobWords(template, choices, index, t), rule ? conditionsWords(jobWatch(template, index), rule.conditions, t) : []);
}

const withConditions = (words: string, conditions: string[]) => (conditions.length > 0 ? `${words} (${conditions.join(', ')})` : words);

/** The jobs left on and the ones added, in words (none: only when called). */
export function whenWords(template: WizardTemplate, choices: WizardChoices, t: Translate): string[] {
  return summaryParts(template, choices, t).map((part) => part.when);
}

/** MC-104: each job left on and each added: when it works (with its conditions) and what it then does. */
function summaryParts(template: WizardTemplate, choices: WizardChoices, t: Translate): { when: string; action: string }[] {
  const ask = t('bot.wizard.say.ask');
  const doing = (rule: WizardRule | null) => (rule ? [actionWords(rule, t), approves(rule) ? `(${t('bot.wizard.say.approve')})` : ''].filter(Boolean).join(' ') : ask);
  const own = template.jobs.map((_, index) => index).filter((index) => choices.jobs[index] !== false)
    .map((index) => ({ when: jobLine(template, choices, index, t), action: doing(choices.rules[index] ?? null) }));
  const added = choices.extra.map((job) => ({ when: extraLine(job, t), action: doing(job.kind === 'schedule' ? null : job.trigger) }));
  return [...own, ...added];
}

/** MC-104: an added job's when, with its conditions in words. */
export function extraLine(job: ExtraJob, t: Translate): string {
  return withConditions(extraWords(job, t), job.kind === 'schedule' ? [] : conditionsWords(extraWatch(job), job.trigger.conditions, t));
}

/** MC-104: the 언제 step's sentence, what the bot will do in plain words (`평일 09:00, 그리고 새 PR이 열리면 → 요청을 보냅니다`):
 *  the jobs that do the same joined, each group's when before what it does. Empty when it only works when called. */
export function summaryWords(template: WizardTemplate, choices: WizardChoices, t: Translate): string {
  const groups: { whens: string[]; action: string }[] = [];
  for (const part of summaryParts(template, choices, t)) {
    const group = groups.find((g) => g.action === part.action);
    if (group) group.whens.push(part.when);
    else groups.push({ whens: [part.when], action: part.action });
  }
  return groups.map((g) => `${g.whens.join(t('bot.wizard.say.and'))} → ${g.action}`).join(' · ');
}

/** MC-104: every trigger's rule left on, the template's (`index`) and the added ones (`extra`), with its when in words: what the
 *  얼마나 맡길까요 step sets (what it does and its policy). */
export function triggerRules(choices: WizardChoices): ({ rule: WizardRule } & ({ index: number } | { extra: number }))[] {
  const own = choices.rules.flatMap((rule, index) => (rule && choices.jobs[index] !== false ? [{ rule, index }] : []));
  const added = choices.extra.flatMap((job, extra) => (job.kind === 'schedule' ? [] : [{ rule: job.trigger, extra }]));
  return [...own, ...added];
}

/** MC-104: the choices with one trigger's rule changed (a template job's by `index`, an added one's by `extra`). */
export function withTriggerRule(choices: WizardChoices, at: { index: number } | { extra: number }, rule: WizardRule): WizardChoices {
  if ('index' in at) return { ...choices, rules: choices.rules.map((r, k) => (k === at.index ? rule : r)) };
  return { ...choices, extra: choices.extra.map((job, k) => (k === at.extra && job.kind !== 'schedule' ? { ...job, trigger: rule } : job)) };
}

// ── MC-104: a trigger's conditions as chips ────────────────────────────────────────────────────────────────────────────────

/** The kinds of event a trigger may watch, each with its own condition presets and known fields. */
export type Watch = 'pr' | 'ci' | 'issue' | 'release' | 'mightywork' | 'bot' | 'event' | 'standing' | 'other';

export function githubWatch(event: GithubEventName): Watch {
  switch (event) {
    case 'pr_opened': case 'review_requested': case 'pr_merged': return 'pr';
    case 'ci_failed': return 'ci';
    case 'issue_opened': return 'issue';
    case 'release_published': return 'release';
  }
}

export function extraWatch(job: ExtraJob): Watch {
  switch (job.kind) {
    case 'schedule': return 'other';
    case 'github': return githubWatch(job.event);
    case 'bot': return 'bot';
    case 'event': return 'event';
    case 'mightywork': return 'mightywork';
  }
}

/** A condition in plain words: what the chip says (`label`, its value beside it), the words with the value for the sentence
 *  (`words`), the condition it writes (`field`, `op`, `ifPresent`, `start` its first value), the ones that come and go with it
 *  (`with`: the MightyWork stage's event type) and how its value is picked (`stage`: a MightyWork stage; else typed). */
export interface ConditionPreset {
  id: 'base' | 'label' | 'notAuthor' | 'stage' | 'tag' | 'title';
  label: string;
  words: string;
  field: string;
  op: RuleOp;
  ifPresent: boolean;
  start: string;
  with: readonly RuleCondition[];
  input: 'text' | 'stage';
  hint: string;
}

const preset = (p: Omit<ConditionPreset, 'with' | 'input' | 'hint' | 'start' | 'ifPresent'> & Partial<ConditionPreset>): ConditionPreset =>
  ({ with: [], input: 'text', hint: '', start: '', ifPresent: false, ...p });
const basePreset = preset({ id: 'base', label: 'bot.wizard.cond.base', words: 'bot.wizard.cond.baseWords', field: 'base', op: 'eq', ifPresent: true, start: 'main', hint: 'main' });
const labelPreset = preset({ id: 'label', label: 'bot.wizard.cond.label', words: 'bot.wizard.cond.labelWords', field: 'labels', op: 'has', hint: 'bug' });
const notAuthorPreset = preset({ id: 'notAuthor', label: 'bot.wizard.cond.notAuthor', words: 'bot.wizard.cond.notAuthorWords', field: 'author', op: 'not_eq', hint: 'dependabot[bot]' });
/** The MightyWork stage move: `task.to eq`, with the event type it needs. */
export const stageChanged: RuleCondition = { field: 'type', op: 'eq', value: 'mightywork.task.stage_changed', ifPresent: false };
const stagePreset = preset({ id: 'stage', label: 'bot.wizard.cond.stage', words: 'bot.wizard.cond.stageWords', field: 'task.to', op: 'eq', start: 'todo', with: [stageChanged], input: 'stage' });
const tagPreset = preset({ id: 'tag', label: 'bot.wizard.cond.tag', words: 'bot.wizard.cond.tagWords', field: 'task.tags', op: 'has', hint: 'bot:dev' });
const titlePreset = preset({ id: 'title', label: 'bot.wizard.cond.title', words: 'bot.wizard.cond.titleWords', field: 'task.title', op: 'matches', hint: '^v\\d+' });

/** Each kind's presets, what the + 조건 menu offers: a pull request's, a CI run's or a merge's target branch, label and author
 *  left out; an issue's label; a MightyWork task's stage, tag and title. */
export const conditionPresets: Record<Watch, readonly ConditionPreset[]> = {
  pr: [basePreset, labelPreset, notAuthorPreset],
  ci: [basePreset, labelPreset, notAuthorPreset],
  issue: [labelPreset],
  release: [],
  mightywork: [stagePreset, tagPreset, titlePreset],
  bot: [],
  event: [],
  standing: [],
  other: [],
};

/** The MightyWork stages, in the pipeline's order, with their names. */
export const mightyworkStages = ['backlog', 'todo', 'in_progress', 'review', 'qa', 'done'] as const;
export const stageKeys: Record<(typeof mightyworkStages)[number], string> = {
  backlog: 'bot.wizard.stage.backlog', todo: 'bot.wizard.stage.todo', in_progress: 'bot.wizard.stage.in_progress',
  review: 'bot.wizard.stage.review', qa: 'bot.wizard.stage.qa', done: 'bot.wizard.stage.done',
};
export function stageWords(stage: string, t: Translate): string {
  const key = stageKeys[stage.trim() as keyof typeof stageKeys];
  return key ? t(key) : stage.trim();
}

/** The fields each kind's events carry (the raw rows' dropdown), then what every event has. */
const envelopeFields = ['type', 'subject', 'source', 'actor'];
const kindFields: Record<Watch, readonly string[]> = {
  pr: ['base', 'labels', 'author', 'repo', 'pr.title', 'pr.branch', 'pr.number'],
  ci: ['base', 'author', 'repo', 'ci.name', 'ci.branch', 'ci.title'],
  issue: ['labels', 'author', 'repo', 'issue.title', 'issue.number'],
  release: ['author', 'repo', 'release.tag'],
  mightywork: ['task.to', 'task.from', 'task.stage', 'task.tags', 'task.title', 'task.key', 'task.version', 'label'],
  bot: [],
  event: [],
  standing: ['check.exit', 'check.output', 'check.command'],
  other: [],
};
export function watchFields(watch: Watch): string[] {
  return [...new Set([...kindFields[watch], ...envelopeFields])];
}

/** Each op in words, and its 아님 form. */
export const opKeys: Record<RuleOp, string> = {
  eq: 'bot.wizard.op.eq', in: 'bot.wizard.op.in', has: 'bot.wizard.op.has', matches: 'bot.wizard.op.matches',
  not_eq: 'bot.wizard.op.not_eq', not_in: 'bot.wizard.op.not_in', not_has: 'bot.wizard.op.not_has', not_matches: 'bot.wizard.op.not_matches',
};
export function opWords(op: RuleOp, t: Translate): string {
  return t(opKeys[op]);
}

const samePlace = (c: RuleCondition, p: ConditionPreset) => c.field.trim() === p.field && c.op === p.op && c.ifPresent === p.ifPresent;
const sameCondition = (a: RuleCondition, b: RuleCondition) => a.field.trim() === b.field && a.op === b.op && a.value.trim() === b.value && a.ifPresent === b.ifPresent;

/** A chip: the preset a condition (`index`) is, and the conditions that came with it (`companions`). */
export interface ConditionChip { preset: ConditionPreset; index: number; companions: number[] }

/** The conditions as chips, where one is a preset of the kind's catalogue, and the rest (`raw`, shown as rows under 고급), each by
 *  its place in the list; the list itself is never reordered, so what the host gets stays as it was. */
export function conditionChips(watch: Watch, conditions: readonly RuleCondition[]): { chips: ConditionChip[]; raw: number[] } {
  const presets = conditionPresets[watch];
  const used = new Set<number>();
  const chips: ConditionChip[] = [];
  conditions.forEach((c, index) => {
    const found = presets.find((p) => samePlace(c, p));
    if (!found) return;
    used.add(index);
    chips.push({ preset: found, index, companions: [] });
  });
  for (const chip of chips) {
    for (const companion of chip.preset.with) {
      const at = conditions.findIndex((c, k) => !used.has(k) && sameCondition(c, companion));
      if (at >= 0) { used.add(at); chip.companions.push(at); }
    }
  }
  return { chips, raw: conditions.map((_, k) => k).filter((k) => !used.has(k)) };
}

/** The conditions with a preset added at the end: the ones that come with it first (unless there), then its own with its first value. */
export function withPreset(conditions: readonly RuleCondition[], p: ConditionPreset): RuleCondition[] {
  const companions = p.with.filter((w) => !conditions.some((c) => sameCondition(c, w))).map((w) => ({ ...w }));
  return [...conditions, ...companions, { field: p.field, op: p.op, value: p.start, ifPresent: p.ifPresent }];
}

/** The conditions without a chip: its own and the ones that came with it. */
export function withoutChip(conditions: readonly RuleCondition[], chip: ConditionChip): RuleCondition[] {
  const gone = new Set([chip.index, ...chip.companions]);
  return conditions.filter((_, k) => !gone.has(k));
}

/** The conditions in words: a chip as its preset says it (`대상 브랜치 main`), a raw row as field, op in words and value. */
export function conditionsWords(watch: Watch, conditions: readonly RuleCondition[], t: Translate): string[] {
  const { chips, raw } = conditionChips(watch, conditions);
  const words = new Map<number, string>();
  for (const chip of chips) {
    const value = conditions[chip.index]?.value.trim() ?? '';
    words.set(chip.index, t(chip.preset.words, { value: chip.preset.input === 'stage' ? stageWords(value, t) : value }));
  }
  for (const k of raw) {
    const c = conditions[k];
    if (c && c.field.trim()) words.set(k, `${c.field.trim()}${c.ifPresent ? '?' : ''} ${opWords(c.op, t)} ${c.value.trim()}`);
  }
  return [...words.entries()].sort(([a], [b]) => a - b).map(([, w]) => w);
}

// ── MC-79: the schedules and triggers the person adds in the 언제 step ─────────────────────────────────────────────────────────

/** The GitHub events a trigger may watch, with the words saying when (keys shared with the templates' jobs). */
export const githubEvents = [
  { value: 'pr_opened', key: 'bot.wizard.when.prOpened' },
  { value: 'review_requested', key: 'bot.wizard.when.reviewRequested' },
  { value: 'pr_merged', key: 'bot.wizard.when.prMerged' },
  { value: 'ci_failed', key: 'bot.wizard.when.ciFailed' },
  { value: 'release_published', key: 'bot.wizard.when.releasePublished' },
  { value: 'issue_opened', key: 'bot.wizard.when.issueOpened' },
] as const;
export type GithubEventName = (typeof githubEvents)[number]['value'];

/** MC-104: a trigger the person added carries a rule like a template job's (`trigger`: its conditions, what it does, its policy). */
export type ExtraJob =
  | { kind: 'schedule'; rule: TimeRule; prompt: string }
  | { kind: 'github'; event: GithubEventName; repo: string; prompt: string; trigger: WizardRule }
  /** MC-104: `outcome` its request done (the default) or failed. */
  | { kind: 'bot'; bot: string; outcome: 'done' | 'failed'; prompt: string; trigger: WizardRule }
  | { kind: 'event'; name: string; prompt: string; trigger: WizardRule }
  /** MC-104: a MightyWork project's tasks (`project`, its key), the preset's MCP source. */
  | { kind: 'mightywork'; project: string; prompt: string; trigger: WizardRule };
export type ExtraKind = ExtraJob['kind'];
/** The 추가 row: 예약 and GitHub first, the rest under 더 보기. */
export const extraKinds: readonly ExtraKind[] = ['schedule', 'github'];
export const moreExtraKinds: readonly ExtraKind[] = ['bot', 'event', 'mightywork'];

const plainRule = (where: RuleCondition[] = []): WizardRule => ({ ...ruleOf({ do: 'ask' }), conditions: where });

/** A new job of a kind, as the 추가 menu adds it: weekdays at 9, a pull request opened, any bot's request done, no event yet;
 *  MC-104: a MightyWork task moved to 할 일 in the recipe's project (`project`, when one was given). */
export function newExtra(kind: ExtraKind, project = ''): ExtraJob {
  switch (kind) {
    case 'schedule': return { kind, rule: { kind: 'at', hour: 9, minute: 0, days: [1, 2, 3, 4, 5] }, prompt: '' };
    case 'github': return { kind, event: 'pr_opened', repo: '', prompt: '', trigger: plainRule() };
    case 'bot': return { kind, bot: '*', outcome: 'done', prompt: '', trigger: plainRule() };
    case 'event': return { kind, name: '', prompt: '', trigger: plainRule() };
    case 'mightywork': return { kind, project: project.trim(), prompt: '', trigger: plainRule(withPreset([], stagePreset)) };
  }
}

/** An added job's when, in words (MC-104: what it watches; its conditions are its chips). */
export function extraWords(job: ExtraJob, t: Translate): string {
  switch (job.kind) {
    case 'schedule': return timeWords(job.rule, t);
    case 'github': return [t(githubEventKey(job.event)), job.repo.trim()].filter(Boolean).join(' · ');
    case 'bot': {
      const bot = job.bot.trim().replace(/^@/, '');
      const failed = job.outcome === 'failed';
      return bot === '*' || bot === '' ? t(failed ? 'bot.wizard.bot.anyFailed' : 'bot.wizard.bot.any') : t(failed ? 'bot.wizard.bot.whenFailed' : 'bot.wizard.bot.when', { bot });
    }
    case 'event': return t('bot.wizard.event.when', { name: job.name.trim() });
    case 'mightywork': return mightyworkWords(job.project, t);
  }
}

/** Why the added jobs cannot be made (a locale key), or null (MC-101: one with no request of its own sends the 지침과 요청 step's;
 *  MC-104: a trigger's conditions checked here, what it does in 얼마나 맡길까요). */
export function extraProblem(choices: WizardChoices): string | null {
  for (const job of choices.extra) {
    if (job.kind === 'schedule' && job.rule.days.length === 0) return 'bot.wizard.time.noDays';
    if (job.kind === 'github' && job.repo.trim() && !validRepo(job.repo)) return 'bot.new.needsRepo';
    if (job.kind === 'github' && !job.repo.trim() && independent(choices.where) && !validRepo(choices.repo)) return 'bot.wizard.repo.needed';
    if (job.kind === 'bot' && job.bot.trim().replace(/^@/, '').trim() === '') return 'bot.wizard.extra.needsBot';
    if (job.kind === 'event' && !/^[a-z0-9._-]+\*?$/.test(job.name.trim())) return 'bot.wizard.extra.needsEvent';
    if (job.kind === 'mightywork' && !validProject(job.project)) return 'bot.wizard.project.needed';
    if (job.kind !== 'schedule') {
      const problem = conditionsProblem(job.trigger.conditions);
      if (problem) return problem;
    }
    if (choices.prompt.trim() === '' && job.prompt.trim() === '') return 'bot.wizard.job.needsPrompt';
  }
  return null;
}

/** An added job as the host reads it (`botWizardCreate`'s `extra`; MC-104: a trigger with its `rule`). */
export function extraRequest(job: ExtraJob): Record<string, unknown> {
  switch (job.kind) {
    case 'schedule': return { kind: job.kind, cron: toCron(job.rule), prompt: job.prompt.trim() };
    case 'github': return { kind: job.kind, event: job.event, repo: job.repo.trim(), prompt: job.prompt.trim(), rule: hostRule(job.trigger) };
    case 'bot': return { kind: job.kind, bot: job.bot.trim() || '*', on: job.outcome, prompt: job.prompt.trim(), rule: hostRule(job.trigger) };
    case 'event': return { kind: job.kind, name: job.name.trim(), prompt: job.prompt.trim(), rule: hostRule(job.trigger) };
    case 'mightywork': return { kind: job.kind, project: job.project.trim(), prompt: job.prompt.trim(), rule: hostRule(job.trigger) };
  }
}

// ── MC-71: a schedule's time, as the wizard shows it ─────────────────────────────────────────────────────────────────────────

type Translate = (key: string, subs?: Readonly<Record<string, string | number>>) => string;

/** At a time on some days (`M H * * D`), or every N minutes between two hours on some days (`*\/N H1-H2 * * D`); days 0 (Sunday)–6. */
export type TimeRule =
  | { kind: 'at'; hour: number; minute: number; days: number[] }
  | { kind: 'every'; minutes: number; from: number; to: number; days: number[] };

/** The week as the wizard lists it, Monday first. */
export const weekOrder = [1, 2, 3, 4, 5, 6, 0] as const;
export const dayKeys = ['bot.wizard.day.0', 'bot.wizard.day.1', 'bot.wizard.day.2', 'bot.wizard.day.3', 'bot.wizard.day.4', 'bot.wizard.day.5', 'bot.wizard.day.6'] as const;
export const intervalChoices = [5, 10, 15, 20, 30, 60] as const;

function number(text: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= min && value <= max ? value : null;
}

/** The days of a cron day-of-week field (`*`, `1-5`, `1,3,5`, 7 as Sunday), sorted; null when it is something else. */
export function parseDays(field: string): number[] | null {
  if (field === '*') return [0, 1, 2, 3, 4, 5, 6];
  const days = new Set<number>();
  for (const part of field.split(',')) {
    const [a, b] = part.split('-');
    const from = number(a ?? '', 0, 7);
    const to = b === undefined ? from : number(b, 0, 7);
    if (from === null || to === null || to < from) return null;
    for (let d = from; d <= to; d += 1) days.add(d % 7);
  }
  return [...days].sort((x, y) => x - y);
}

/** A cron day-of-week field for days: `*` for all, runs of three or more as ranges. */
export function daysField(days: readonly number[]): string {
  const sorted = [...new Set(days)].sort((x, y) => x - y);
  if (sorted.length === 7) return '*';
  const parts: string[] = [];
  for (let i = 0; i < sorted.length;) {
    let j = i;
    while (j + 1 < sorted.length && sorted[j + 1] === (sorted[j] ?? 0) + 1) j += 1;
    const [a, b] = [sorted[i], sorted[j]];
    parts.push(j - i >= 2 ? `${a}-${b}` : sorted.slice(i, j + 1).join(','));
    i = j + 1;
  }
  return parts.join(',');
}

/** The rule a cron line is, when the wizard can show it; null otherwise (kept as it is). */
export function parseCron(cron: string): TimeRule | null {
  const fields = cron.trim().split(/\s+/);
  if (fields.length !== 5 || fields[2] !== '*' || fields[3] !== '*') return null;
  const [m = '', h = '', , , d = ''] = fields;
  const days = parseDays(d);
  if (!days) return null;
  const minute = number(m, 0, 59);
  const hour = number(h, 0, 23);
  if (minute !== null && hour !== null) return { kind: 'at', hour, minute, days };
  const step = /^\*\/(\d+)$/.exec(m);
  const range = h === '*' ? ['0', '23'] : /^(\d+)-(\d+)$/.exec(h)?.slice(1);
  if (step && range) {
    const minutes = Number(step[1]);
    const [from, to] = [number(range[0] ?? '', 0, 23), number(range[1] ?? '', 0, 23)];
    if (minutes >= 1 && minutes <= 60 && from !== null && to !== null && from <= to) return { kind: 'every', minutes, from, to, days };
  }
  return null;
}

export function toCron(rule: TimeRule): string {
  const days = daysField(rule.days);
  return rule.kind === 'at' ? `${rule.minute} ${rule.hour} * * ${days}` : `*/${rule.minutes} ${rule.from}-${rule.to} * * ${days}`;
}

const pad = (n: number) => String(n).padStart(2, '0');
export const clock = (hour: number, minute: number) => `${pad(hour)}:${pad(minute)}`;

/** The days in words: every day, weekdays, the weekend, else the days' short names (Monday first). */
export function daysWords(days: readonly number[], t: Translate): string {
  const key = daysField(days);
  if (key === '*') return t('bot.wizard.time.everyday');
  if (key === '1-5') return t('bot.wizard.time.weekdays');
  if (key === '0,6') return t('bot.wizard.time.weekend');
  return weekOrder.filter((d) => days.includes(d)).map((d) => t(dayKeys[d])).join(', ');
}

export function timeWords(rule: TimeRule, t: Translate): string {
  const days = daysWords(rule.days, t);
  return rule.kind === 'at'
    ? t('bot.wizard.time.at', { days, time: clock(rule.hour, rule.minute) })
    : t('bot.wizard.time.every', { days, from: rule.from, to: rule.to, minutes: rule.minutes });
}

/** The wizard's request to the host (`botWizardCreate`). */
export function createRequest(choices: WizardChoices): Record<string, unknown> {
  const workspace = independent(choices.where) ? null : choices.where;
  return {
    template: choices.template,
    workspace,
    pickFolder: choices.where === independentPick,
    repo: choices.repo.trim(),
    command: choices.command.trim(),
    agent: choices.agent,
    model: choices.model.trim(),
    name: choices.name.trim().replace(/^@/, ''),
    instructions: choices.instructions.trim(),
    prompt: choices.prompt.trim(),
    jobs: choices.jobs,
    crons: choices.times.map((rule) => (rule ? toCron(rule) : null)),
    prompts: choices.prompts.map((p) => (p.trim() ? p.trim() : null)),
    extra: choices.extra.map(extraRequest),
    rules: choices.rules.map((rule) => (rule ? hostRule(rule) : null)),
    project: choices.project.trim(),
    permission: choices.permission,
    budget: choices.budget > 0 ? choices.budget : null,
    greet: choices.greet,
  };
}
