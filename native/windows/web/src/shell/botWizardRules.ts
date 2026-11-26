// MC-71: the new-bot wizard's rules, apart from its drawing (BotWizard.tsx): what the gallery shows and whether a step may go on. The words come from the host (`botWizard`, bot_wizard.rs) in the app's language.
// MC-99: a trigger job's rule (its conditions, what it does and its policy) as the 언제 step shows and edits it, and a MightyWork
// recipe's project key.
// MC-101: a template's examples (other instructions and request for the same job) and the 지침과 요청 step that edits both; its
// request is every job's unless the 언제 step gives one its own.

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
}

// ── MC-99: a trigger's rule ────────────────────────────────────────────────────────────────────────────────────────────────

export const ruleOps = ['eq', 'in', 'has', 'matches', 'not_eq', 'not_in', 'not_has', 'not_matches'] as const;
export type RuleOp = (typeof ruleOps)[number];
export const actionKinds = ['ask', 'develop', 'run', 'emit'] as const;
export type ActionKindName = (typeof actionKinds)[number];
/** Each kind's word, and the line on what it does. */
export const actionKindKeys: Record<ActionKindName, string> = {
  ask: 'automation.do.ask', develop: 'automation.do.develop', run: 'automation.do.run', emit: 'automation.do.emit',
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

/** Why a rule cannot be made (a locale key), or null. */
export function ruleProblem(rule: WizardRule): string | null {
  for (const c of rule.conditions) {
    if ((c.field.trim() === '') !== (c.value.trim() === '')) return 'bot.wizard.rule.incomplete';
    if ((c.op === 'matches' || c.op === 'not_matches') && c.value.trim()) {
      try { new RegExp(c.value.trim()); } catch { return 'bot.wizard.rule.badRegex'; }
    }
  }
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

/** A rule in a few words: its conditions, what it does and its policy (`task.to=todo · run /release … · approve, once per subject`). */
export function ruleWords(rule: WizardRule, t: Translate): string {
  const conditions = rule.conditions.filter((c) => c.field.trim()).map((c) => {
    const field = c.ifPresent ? `${c.field.trim()}?` : c.field.trim();
    return c.op === 'eq' ? `${field}=${c.value.trim()}` : c.op === 'not_eq' ? `${field}!=${c.value.trim()}` : `${field} ${c.op} ${c.value.trim()}`;
  });
  const kind = t(actionKindKeys[rule.kind]);
  const action = rule.kind === 'run' ? `${kind} ${rule.command.trim()}` : rule.kind === 'emit' ? `${kind} ${rule.event.trim()}`
    : rule.kind === 'develop' ? [kind, rule.pr ? t('automation.do.withPr') : '', rule.merge ? t('automation.do.withMerge') : ''].filter(Boolean).join(' · ') : kind;
  const policy = [
    (rule.approve === 'on' || (rule.approve === '' && approvesByDefault(rule))) ? t('automation.form.approve') : '',
    rule.oncePer ? t('automation.form.oncePer') : '',
  ].filter(Boolean).join(', ');
  return [...conditions, action, policy].filter(Boolean).join(' · ');
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
    // MC-99: a rule of a job left on that cannot be made.
    const rule = choices.rules.find((r, index) => r !== null && choices.jobs[index] !== false && ruleProblem(r) !== null);
    if (rule) return ruleProblem(rule);
    return extraProblem(choices);
  }
  return null;
}

/** A job's when, in words: its time as the person set it, else the template's words. */
export function jobWords(template: WizardTemplate, choices: WizardChoices, index: number, t: Translate): string {
  const rule = choices.times[index];
  return rule ? timeWords(rule, t) : template.jobs[index]?.when ?? '';
}

/** MC-99: a job's when, and its rule's words when it has one (the summary's line). */
export function jobLine(template: WizardTemplate, choices: WizardChoices, index: number, t: Translate): string {
  const rule = choices.rules[index];
  return [jobWords(template, choices, index, t), rule ? ruleWords(rule, t) : ''].filter(Boolean).join(' — ');
}

/** The jobs left on and the ones added, in words (none: only when called). */
export function whenWords(template: WizardTemplate, choices: WizardChoices, t: Translate): string[] {
  const own = template.jobs.map((_, index) => index).filter((index) => choices.jobs[index] !== false).map((index) => jobLine(template, choices, index, t));
  return [...own, ...choices.extra.map((job) => extraWords(job, t))];
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

export type ExtraJob =
  | { kind: 'schedule'; rule: TimeRule; prompt: string }
  /** MC-97: `label`, when given, is the trigger's `where` condition `labels has <label>`. */
  | { kind: 'github'; event: GithubEventName; repo: string; label: string; prompt: string }
  | { kind: 'bot'; bot: string; prompt: string }
  | { kind: 'event'; name: string; prompt: string };
export type ExtraKind = ExtraJob['kind'];
export const extraKinds: readonly ExtraKind[] = ['schedule', 'github', 'bot', 'event'];

/** A new job of a kind, as the 추가 menu adds it: weekdays at 9, a pull request opened, any bot's request done, no event yet. */
export function newExtra(kind: ExtraKind): ExtraJob {
  switch (kind) {
    case 'schedule': return { kind, rule: { kind: 'at', hour: 9, minute: 0, days: [1, 2, 3, 4, 5] }, prompt: '' };
    case 'github': return { kind, event: 'pr_opened', repo: '', label: '', prompt: '' };
    case 'bot': return { kind, bot: '*', prompt: '' };
    case 'event': return { kind, name: '', prompt: '' };
  }
}

/** An added job's when, in words. */
export function extraWords(job: ExtraJob, t: Translate): string {
  switch (job.kind) {
    case 'schedule': return timeWords(job.rule, t);
    case 'github': {
      const words = t(githubEvents.find((e) => e.value === job.event)?.key ?? 'bot.wizard.when.prOpened');
      const label = job.label.trim() ? t('bot.wizard.github.labelWhen', { label: job.label.trim() }) : '';
      return [words, job.repo.trim(), label].filter(Boolean).join(' · ');
    }
    case 'bot': {
      const bot = job.bot.trim().replace(/^@/, '');
      return bot === '*' || bot === '' ? t('bot.wizard.bot.any') : t('bot.wizard.bot.when', { bot });
    }
    case 'event': return t('bot.wizard.event.when', { name: job.name.trim() });
  }
}

/** Why the added jobs cannot be made (a locale key), or null (MC-101: one with no request of its own sends the 지침과 요청 step's). */
export function extraProblem(choices: WizardChoices): string | null {
  for (const job of choices.extra) {
    if (job.kind === 'schedule' && job.rule.days.length === 0) return 'bot.wizard.time.noDays';
    if (job.kind === 'github' && job.repo.trim() && !validRepo(job.repo)) return 'bot.new.needsRepo';
    if (job.kind === 'github' && !job.repo.trim() && independent(choices.where) && !validRepo(choices.repo)) return 'bot.wizard.repo.needed';
    if (job.kind === 'bot' && job.bot.trim().replace(/^@/, '').trim() === '') return 'bot.wizard.extra.needsBot';
    if (job.kind === 'event' && !/^[a-z0-9._-]+\*?$/.test(job.name.trim())) return 'bot.wizard.extra.needsEvent';
    if (choices.prompt.trim() === '' && job.prompt.trim() === '') return 'bot.wizard.job.needsPrompt';
  }
  return null;
}

/** An added job as the host reads it (`botWizardCreate`'s `extra`). */
export function extraRequest(job: ExtraJob): Record<string, string> {
  switch (job.kind) {
    case 'schedule': return { kind: job.kind, cron: toCron(job.rule), prompt: job.prompt.trim() };
    case 'github': return { kind: job.kind, event: job.event, repo: job.repo.trim(), label: job.label.trim(), prompt: job.prompt.trim() };
    case 'bot': return { kind: job.kind, bot: job.bot.trim() || '*', prompt: job.prompt.trim() };
    case 'event': return { kind: job.kind, name: job.name.trim(), prompt: job.prompt.trim() };
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
