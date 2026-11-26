import { describe, expect, it } from 'vitest';
import {
  choicesFor, createRequest, daysField, exampleOn, extraWords, filterTemplates, ownWords, withExample, independentHere, independentPick, newExtra, parseCron, parseDays, stepProblem, steps, timeWords,
  toCron, validRepo, whenWords, whereProblem, hostRule, ruleOf, ruleProblem, ruleWords, validProject, type HostRule, type WizardModel, type WizardTemplate,
} from './botWizardRules';

const template = (id: string, extra: Partial<WizardTemplate> = {}): WizardTemplate => ({
  id, icon: '🔍', category: 'bot.wizard.category.review', label: id, what: id + ' does', instructions: id + ' rules', needs: 'nothing', commandHint: '', prompt: id + ' prompt', jobs: [], examples: [], vars: [], ...extra,
});
const reviewer = template('prReviewer', { label: 'PR 리뷰어', what: '새 PR을 리뷰', needs: 'repo', jobs: [{ when: '새 PR이 열리면', cron: null }, { when: '리뷰를 요청받으면', cron: null }] });
const site = template('siteWatch', { category: 'bot.wizard.category.watch', label: '사이트 상태 지킴이', needs: 'command', jobs: [{ when: '점검 결과가 바뀌면', cron: null }] });
const briefing = template('briefing', { category: 'bot.wizard.category.other', label: '아침 브리핑', jobs: [{ when: '평일 9:00', cron: '0 9 * * 1-5' }, { when: '30분마다', cron: '*/30 9-19 * * 1-5' }] });
const t = (key: string, subs?: Readonly<Record<string, string | number>>) => key + (subs ? JSON.stringify(subs) : '');
const blank = template('blank', { category: 'bot.wizard.category.other', label: '빈 봇', instructions: '', prompt: '' });
const model: WizardModel = {
  categories: [{ id: 'bot.wizard.category.review', label: '코드 리뷰' }, { id: 'bot.wizard.category.watch', label: '감시' }, { id: 'bot.wizard.category.other', label: '기타' }],
  templates: [reviewer, site, blank],
  workspaces: [{ id: 'w1', name: 'Shop' }, { id: 'w2', name: 'Docs' }],
  activeWorkspace: 'w2',
  agents: [
    { id: 'claude', label: 'Claude', available: false, permissions: [], models: [] },
    { id: 'codex', label: 'Codex', available: true, permissions: [{ value: 'auto', label: 'Auto' }], models: [{ value: 'gpt-5', label: 'GPT-5' }] },
  ],
  bots: ['콩이', '두부'],
  name: '콩이',
};

describe('the new-bot wizard (MC-71)', () => {
  it('shows a category and finds by label, what or category', () => {
    expect(filterTemplates(model, 'all', '').map((t) => t.id)).toEqual(['prReviewer', 'siteWatch', 'blank']);
    expect(filterTemplates(model, 'bot.wizard.category.watch', '').map((t) => t.id)).toEqual(['siteWatch']);
    expect(filterTemplates(model, 'all', ' 리뷰 ').map((t) => t.id)).toEqual(['prReviewer']);
    expect(filterTemplates(model, 'all', '감시').map((t) => t.id)).toEqual(['siteWatch']);
    expect(filterTemplates(model, 'all', 'zzz')).toEqual([]);
  });

  it('starts where the person works, with an installed agent, every job on and a friendly name', () => {
    const c = choicesFor(model, reviewer);
    expect(c).toMatchObject({ template: 'prReviewer', where: 'w2', agent: 'codex', name: '콩이', instructions: 'prReviewer rules', jobs: [true, true], budget: 3, greet: true });
    const next = choicesFor(model, site, { ...c, name: '보리', repo: 'a/b', command: 'x' });
    expect(next).toMatchObject({ template: 'siteWatch', name: '보리', repo: 'a/b', command: '', jobs: [true], instructions: 'siteWatch rules' });
    expect(choicesFor({ ...model, workspaces: [], activeWorkspace: null }, blank).where).toBe(independentHere);
  });

  it('asks a repository of an independent GitHub bot, a valid one when given, and a command when the template runs one', () => {
    const c = choicesFor(model, reviewer);
    expect(whereProblem(reviewer, c)).toBeNull();
    expect(whereProblem(reviewer, { ...c, where: independentHere })).toBe('bot.wizard.repo.needed');
    expect(whereProblem(reviewer, { ...c, where: independentPick, repo: 'owner/name' })).toBeNull();
    expect(whereProblem(reviewer, { ...c, where: independentHere, jobs: [false, false] })).toBeNull();
    expect(whereProblem(reviewer, { ...c, repo: 'not a repo' })).toBe('bot.new.needsRepo');
    expect(whereProblem(site, choicesFor(model, site))).toBe('bot.wizard.needsCommand');
    expect(validRepo(' owner/my.repo ')).toBe(true);
    expect(validRepo('owner')).toBe(false);
  });

  it('walks seven steps and stops a step only for what it needs', () => {
    expect(steps).toEqual(['template', 'prompt', 'where', 'who', 'when', 'limits', 'confirm']);
    const c = choicesFor(model, site);
    expect(stepProblem('where', site, c)).toBe('bot.wizard.needsCommand');
    expect(stepProblem('where', site, { ...c, command: 'curl -s x' })).toBeNull();
    expect(stepProblem('who', site, { ...c, agent: '' })).toBe('bot.wizard.needsAgent');
    expect(['template', 'prompt', 'when', 'limits', 'confirm'].map((s) => stepProblem(s as never, site, c))).toEqual([null, null, null, null, null]);
  });

  it('says the jobs left on and asks the host for what was chosen', () => {
    const c = { ...choicesFor(model, reviewer), jobs: [true, false], name: '@두부 ', budget: 0, where: independentPick, repo: ' o/r ' };
    expect(whenWords(reviewer, c, t)).toEqual(['새 PR이 열리면']);
    expect(createRequest(c)).toMatchObject({ template: 'prReviewer', workspace: null, pickFolder: true, repo: 'o/r', name: '두부', budget: null, jobs: [true, false], agent: 'codex' });
    expect(createRequest(choicesFor(model, reviewer))).toMatchObject({ workspace: 'w2', pickFolder: false, budget: 3 });
  });

  it("reads a schedule's cron as a time, an interval and days, and writes it back", () => {
    expect(parseCron('0 9 * * 1-5')).toEqual({ kind: 'at', hour: 9, minute: 0, days: [1, 2, 3, 4, 5] });
    expect(parseCron('*/30 9-19 * * 1-5')).toEqual({ kind: 'every', minutes: 30, from: 9, to: 19, days: [1, 2, 3, 4, 5] });
    expect(parseCron('0 2 * * *')).toEqual({ kind: 'at', hour: 2, minute: 0, days: [0, 1, 2, 3, 4, 5, 6] });
    expect(parseCron('0 9 1 * *')).toBeNull();
    expect(parseCron('0 9-17/2 * * 1')).toBeNull();
    expect(parseDays('7,1')).toEqual([0, 1]);
    expect(daysField([1, 2, 3, 4, 5])).toBe('1-5');
    expect(daysField([1, 3, 5])).toBe('1,3,5');
    expect(daysField([0, 1, 2, 3, 6])).toBe('0-3,6');
    for (const cron of ['0 9 * * 1-5', '30 8 * * 1,3,5', '*/15 0-23 * * *', '0 2 * * *']) expect(toCron(parseCron(cron)!)).toBe(cron);
    expect(timeWords({ kind: 'at', hour: 8, minute: 5, days: [1, 2, 3, 4, 5] }, t)).toBe('bot.wizard.time.at{"days":"bot.wizard.time.weekdays","time":"08:05"}');
    expect(timeWords({ kind: 'every', minutes: 15, from: 9, to: 18, days: [0, 6] }, t)).toBe('bot.wizard.time.every{"days":"bot.wizard.time.weekend","from":9,"to":18,"minutes":15}');
  });

  it("starts with the template's times, sends the ones changed and stops a schedule with no day", () => {
    const c = choicesFor(model, briefing);
    expect(c.times).toEqual([{ kind: 'at', hour: 9, minute: 0, days: [1, 2, 3, 4, 5] }, { kind: 'every', minutes: 30, from: 9, to: 19, days: [1, 2, 3, 4, 5] }]);
    const changed = { ...c, times: [{ kind: 'at' as const, hour: 7, minute: 30, days: [1, 3] }, c.times[1] ?? null] };
    expect(createRequest(changed).crons).toEqual(['30 7 * * 1,3', '*/30 9-19 * * 1-5']);
    expect(whenWords(briefing, changed, t)[0]).toContain('07:30');
    const none = { ...c, times: [{ kind: 'at' as const, hour: 9, minute: 0, days: [] }, c.times[1] ?? null] };
    expect(stepProblem('when', briefing, none)).toBe('bot.wizard.time.noDays');
    expect(stepProblem('when', briefing, { ...none, jobs: [false, true] })).toBeNull();
    expect(createRequest(choicesFor(model, reviewer)).crons).toEqual([null, null]);
  });

  it('adds schedules and triggers beside the template\'s, edits and removes them, and sends each with its request (MC-79)', () => {
    const c = choicesFor(model, briefing);
    expect(c.extra).toEqual([]);
    const schedule = { kind: 'schedule' as const, rule: { kind: 'at' as const, hour: 9, minute: 0, days: [1, 2, 3, 4, 5] }, prompt: '' };
    expect(newExtra('schedule')).toEqual(schedule);
    const github = { kind: 'github' as const, event: 'pr_opened' as const, repo: 'owner/app', label: 'bot:dev', prompt: '이슈 봐 줘' };
    const bot = { kind: 'bot' as const, bot: '@두부', prompt: '' };
    const event = { kind: 'event' as const, name: 'release.done', prompt: '' };
    const added = { ...c, extra: [schedule, github, bot, event], prompts: ['오늘 할 일만', ''] };
    expect(stepProblem('when', briefing, added)).toBeNull();
    expect(whenWords(briefing, added, t)).toHaveLength(6);
    expect(extraWords(schedule, t)).toContain('bot.wizard.time.at');
    expect(extraWords(github, t)).toBe('bot.wizard.when.prOpened · owner/app · bot.wizard.github.labelWhen{"label":"bot:dev"}');
    expect(extraWords({ ...github, repo: '', label: ' ' }, t)).toBe('bot.wizard.when.prOpened');
    expect(newExtra('github')).toEqual({ kind: 'github', event: 'pr_opened', repo: '', label: '', prompt: '' });
    expect(extraWords(newExtra('bot'), t)).toBe('bot.wizard.bot.any');
    expect(extraWords(bot, t)).toBe('bot.wizard.bot.when{"bot":"두부"}');
    const request = createRequest(added);
    expect(request.prompts).toEqual(['오늘 할 일만', null]);
    expect(request.extra).toEqual([
      { kind: 'schedule', cron: '0 9 * * 1-5', prompt: '' },
      { kind: 'github', event: 'pr_opened', repo: 'owner/app', label: 'bot:dev', prompt: '이슈 봐 줘' },
      { kind: 'bot', bot: '@두부', prompt: '' },
      { kind: 'event', name: 'release.done', prompt: '' },
    ]);
    // What each added job needs before the step goes on.
    const problem = (job: ReturnType<typeof newExtra>, where = c.where, tp = briefing) => stepProblem('when', tp, { ...c, where, extra: [job] });
    expect(problem({ kind: 'schedule', rule: { kind: 'at', hour: 9, minute: 0, days: [] }, prompt: '' })).toBe('bot.wizard.time.noDays');
    expect(problem({ ...github, repo: 'nope' })).toBe('bot.new.needsRepo');
    expect(problem({ ...github, repo: '' }, independentHere)).toBe('bot.wizard.repo.needed');
    expect(problem({ ...bot, bot: '@' })).toBe('bot.wizard.extra.needsBot');
    expect(problem({ ...event, name: 'Not An Event' })).toBe('bot.wizard.extra.needsEvent');
    // MC-101: a job with no request of its own sends the 지침과 요청 step's; a blank bot's needs one or the other.
    const blankChoices = { ...choicesFor(model, blank), extra: [schedule] };
    expect(blankChoices.prompt).toBe('');
    expect(stepProblem('when', blank, blankChoices)).toBe('bot.wizard.job.needsPrompt');
    expect(stepProblem('when', blank, { ...blankChoices, extra: [{ ...schedule, prompt: '매일 정리' }] })).toBeNull();
    expect(stepProblem('when', blank, { ...blankChoices, prompt: '매일 정리' })).toBeNull();
  });
});

describe('a recipe\'s rule in the 언제 step (MC-99)', () => {
  const releaseRule: HostRule = {
    where: [{ field: 'type', op: 'eq', value: 'mightywork.task.stage_changed' }, { field: 'task.to', op: 'eq', value: 'todo' },
      { field: 'task.title', op: 'matches', value: '^v\\d+\\.\\d+\\.\\d+ 릴리스' }],
    do: 'run', with: { command: '/release {{task.version}}' }, policy: { approve: true, oncePer: 'subject' },
  };
  const release = template('mwRelease', { needs: 'project', jobs: [{ when: '릴리스 일감이 할 일로 가면', cron: null, rule: releaseRule }] });

  it('starts each trigger job with its rule and sends it back as the host reads it', () => {
    const c = choicesFor(model, release);
    expect(c.rules).toHaveLength(1);
    const rule = c.rules[0]!;
    expect(rule).toMatchObject({ kind: 'run', command: '/release {{task.version}}', approve: 'on', oncePer: true });
    expect(hostRule(rule)).toEqual(releaseRule);
    expect(createRequest({ ...c, project: ' MC ' })).toMatchObject({ project: 'MC', rules: [releaseRule] });
    expect(choicesFor(model, briefing).rules).toEqual([null, null]);
    expect(ruleWords(rule, t)).toBe('type=mightywork.task.stage_changed · task.to=todo · task.title matches ^v\\d+\\.\\d+\\.\\d+ 릴리스 · automation.do.run /release {{task.version}} · automation.form.approve, automation.form.oncePer');
  });

  it('asks for the project key and refuses a rule that cannot be made', () => {
    const c = choicesFor(model, release);
    expect(whereProblem(release, c)).toBe('bot.wizard.project.needed');
    expect(whereProblem(release, { ...c, project: 'MC' })).toBeNull();
    expect(validProject('MC') && validProject('my_proj-2') && !validProject('a b') && !validProject('')).toBe(true);
    const rule = c.rules[0]!;
    expect(ruleProblem(rule)).toBeNull();
    expect(ruleProblem({ ...rule, command: ' ' })).toBe('bot.wizard.rule.needsCommand');
    expect(ruleProblem({ ...rule, conditions: [{ field: 'a', op: 'eq', value: '', ifPresent: false }] })).toBe('bot.wizard.rule.incomplete');
    expect(ruleProblem({ ...rule, conditions: [{ field: 'a', op: 'matches', value: '(', ifPresent: false }] })).toBe('bot.wizard.rule.badRegex');
    expect(ruleProblem({ ...rule, kind: 'develop', pr: false, merge: true })).toBe('bot.wizard.rule.mergeNeedsPr');
    expect(ruleProblem({ ...rule, concurrency: '0' })).toBe('bot.wizard.rule.badNumber');
    expect(stepProblem('when', release, { ...c, rules: [{ ...rule, command: '' }] })).toBe('bot.wizard.rule.needsCommand');
    expect(stepProblem('when', release, { ...c, jobs: [false], rules: [{ ...rule, command: '' }] })).toBeNull();
  });

  it('writes a develop pr and merge, an in list and ifPresent, and only the policy set', () => {
    const rule = ruleOf({ where: [{ field: 'author', op: 'in', value: ['kim', 'lee'], ifPresent: true }], do: 'develop', with: { pr: true } });
    expect(rule).toMatchObject({ pr: true, merge: false, approve: '', oncePer: false, conditions: [{ value: 'kim, lee', ifPresent: true }] });
    expect(hostRule({ ...rule, merge: true, cooldown: '30' })).toEqual({
      where: [{ field: 'author', op: 'in', value: ['kim', 'lee'], ifPresent: true }], do: 'develop', with: { pr: true, merge: true }, policy: { cooldownMinutes: 30 },
    });
    expect(hostRule({ ...rule, conditions: [{ field: ' ', op: 'eq', value: '', ifPresent: false }] }).where).toEqual([]);
  });
});

describe('examples and the 지침과 요청 step (MC-101)', () => {
  const example = (id: string, prompt = id + ' asks') => ({ id, label: id + ' label', instructions: id + ' rules', prompt });
  const routine = template('routine', {
    label: '정기 작업', instructions: 'briefing rules', prompt: 'briefing asks', jobs: [{ when: '평일 9:00', cron: '0 9 * * 1-5' }],
    examples: [example('briefing'), example('weekly'), example('guide', '')],
  });
  const routineModel: WizardModel = { ...model, templates: [reviewer, routine, blank] };

  it("finds a template by an example's name", () => {
    expect(filterTemplates(routineModel, 'all', 'weekly LABEL').map((t) => t.id)).toEqual(['routine']);
  });

  it("starts with the template's instructions and request, and sends the request every job takes", () => {
    const c = choicesFor(routineModel, routine);
    expect(c).toMatchObject({ instructions: 'briefing rules', prompt: 'briefing asks' });
    expect(exampleOn(routine, c)).toBe('briefing');
    expect(ownWords(routine, c)).toBe(false);
    expect(createRequest({ ...c, prompt: ' 이번 주 회고 ' })).toMatchObject({ prompt: '이번 주 회고', prompts: [null] });
    expect(choicesFor(routineModel, blank)).toMatchObject({ instructions: '', prompt: '' });
  });

  it('fills both from an example and changes nothing else; one without a request leaves the request', () => {
    const c = { ...choicesFor(routineModel, routine), times: [{ kind: 'at' as const, hour: 7, minute: 0, days: [1] }] };
    const weekly = withExample(c, routine.examples[1]!);
    expect(weekly).toEqual({ ...c, instructions: 'weekly rules', prompt: 'weekly asks' });
    expect(exampleOn(routine, weekly)).toBe('weekly');
    const guide = withExample(weekly, routine.examples[2]!);
    expect(guide).toMatchObject({ instructions: 'guide rules', prompt: 'weekly asks' });
    expect(exampleOn(routine, guide)).toBe('guide');
  });

  it('knows when the person wrote their own words, so an example asks first', () => {
    const c = choicesFor(routineModel, routine);
    expect(ownWords(routine, { ...c, prompt: 'briefing asks, only PRs' })).toBe(true);
    expect(ownWords(routine, { ...c, instructions: 'mine' })).toBe(true);
    expect(exampleOn(routine, { ...c, instructions: 'mine' })).toBeNull();
    expect(ownWords(routine, withExample(c, routine.examples[1]!))).toBe(false);
    // A template without examples: its own words are not the person's.
    expect(ownWords(reviewer, choicesFor(model, reviewer))).toBe(false);
  });

  it('asks for a request when the template has jobs, not for a blank bot', () => {
    const c = choicesFor(routineModel, routine);
    expect(stepProblem('prompt', routine, { ...c, prompt: '  ' })).toBe('bot.wizard.prompt.needed');
    expect(stepProblem('prompt', blank, choicesFor(routineModel, blank))).toBeNull();
  });
});
