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
  /** When each of its schedules and triggers runs, in words. */
  jobs: string[];
}

export interface WizardAgent {
  id: string;
  label: string;
  /** Whether its CLI is installed (null while the app has not read it yet). */
  available: boolean | null;
  permissions: { value: string; label: string }[];
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
  return null;
}

/** The jobs left on, in words (none: only when called). */
export function whenWords(template: WizardTemplate, choices: WizardChoices): string[] {
  return template.jobs.filter((_, index) => choices.jobs[index] !== false);
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
    permission: choices.permission,
    budget: choices.budget > 0 ? choices.budget : null,
    awake: choices.awake,
    greet: choices.greet,
  };
}
