// MC-71: the new-bot wizard's rules, apart from its drawing (BotWizard.tsx): what the gallery shows and whether a step may go on. The words come from the host (`botWizard`, bot_wizard.rs) in the app's language.

export interface WizardTemplate {
  id: string;
  icon: string;
  category: string;
  label: string;
  what: string;
  instructions: string;
  needs: 'nothing' | 'repo' | 'command';
  commandHint: string;
  /** Its schedules and triggers: when each runs, in words, and a schedule's cron line (its time the person may change). */
  jobs: WizardJob[];
}

export interface WizardJob {
  when: string;
  cron: string | null;
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
  jobs: boolean[];
  /** Each job's time as the person set it (null for a trigger, or a cron line the wizard cannot show). */
  times: (TimeRule | null)[];
  permission: string;
  /** Dollars a day; 0 for no limit. */
  budget: number;
  awake: boolean;
  greet: boolean;
}

/** MC-71 (A, the classic wizard): six steps in order, the rail at the left naming them. */
export const steps = ['template', 'where', 'who', 'when', 'limits', 'confirm'] as const;
export type Step = (typeof steps)[number];

/** The gallery's templates for a category (`all` for every one) and a search (label, what or category words). */
export function filterTemplates(model: WizardModel, category: string, query: string): WizardTemplate[] {
  const words = query.trim().toLowerCase();
  const label = (id: string) => model.categories.find((c) => c.id === id)?.label ?? '';
  return model.templates.filter((t) => (category === 'all' || t.category === category)
    && (words.length === 0 || [t.label, t.what, label(t.category)].some((text) => text.toLowerCase().includes(words))));
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
    jobs: template.jobs.map(() => true),
    times: template.jobs.map((job) => (job.cron ? parseCron(job.cron) : null)),
    permission: earlier?.agent === agent ? earlier.permission : '',
    budget: earlier?.budget ?? 3,
    awake: earlier?.awake ?? true,
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
  return null;
}

/** Why a step cannot go on (a locale key), or null: 어디서 needs its repository or command; 누가 needs an agent. */
export function stepProblem(step: Step, template: WizardTemplate, choices: WizardChoices): string | null {
  if (step === 'where') return whereProblem(template, choices);
  if (step === 'who' && choices.agent.trim().length === 0) return 'bot.wizard.needsAgent';
  if (step === 'when' && choices.times.some((rule, index) => rule !== null && choices.jobs[index] !== false && rule.days.length === 0)) return 'bot.wizard.time.noDays';
  return null;
}

/** A job's when, in words: its time as the person set it, else the template's words. */
export function jobWords(template: WizardTemplate, choices: WizardChoices, index: number, t: Translate): string {
  const rule = choices.times[index];
  return rule ? timeWords(rule, t) : template.jobs[index]?.when ?? '';
}

/** The jobs left on, in words (none: only when called). */
export function whenWords(template: WizardTemplate, choices: WizardChoices, t: Translate): string[] {
  return template.jobs.map((_, index) => index).filter((index) => choices.jobs[index] !== false).map((index) => jobWords(template, choices, index, t));
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
    jobs: choices.jobs,
    crons: choices.times.map((rule) => (rule ? toCron(rule) : null)),
    permission: choices.permission,
    budget: choices.budget > 0 ? choices.budget : null,
    awake: choices.awake,
    greet: choices.greet,
  };
}
