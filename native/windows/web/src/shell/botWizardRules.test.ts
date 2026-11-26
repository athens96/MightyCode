import { describe, expect, it } from 'vitest';
import {
  actionProblem, conditionChips, conditionPresets, conditionsProblem, conditionsWords, extraLine, jobWatch, jobWords, opKeys, opWords, ruleOps, stageWords,
  summaryWords, triggerRules, watchFields, withoutChip, withPreset, withTriggerRule, type RuleCondition,
  choicesFor, createRequest, daysField, exampleOn, extraWords, filterTemplates, ownWords, withExample, independentHere, independentIn, independentPick, needsOwnRepo, whereChoices, folderWorkspace, newExtra, parseCron, parseDays, stepProblem, steps, timeWords,
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

  it('starts as an independent bot in the folder of the workspace the person works in (MC-108), with an installed agent, every job on and a friendly name', () => {
    const c = choicesFor(model, reviewer);
    expect(c).toMatchObject({ template: 'prReviewer', where: independentIn('w2'), agent: 'codex', name: '콩이', instructions: 'prReviewer rules', jobs: [true, true], budget: 3, greet: true });
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
    expect(createRequest(choicesFor(model, reviewer))).toMatchObject({ workspace: null, folderOf: 'w2', pickFolder: false, budget: 3 });
    expect(createRequest({ ...c, where: 'w1' })).toMatchObject({ workspace: 'w1', folderOf: null, pickFolder: false });
  });

  it('offers the independent places first and a workspace bot last; only one outside every workspace must name its repository (MC-108)', () => {
    expect(whereChoices(model)).toEqual([independentHere, independentIn('w1'), independentIn('w2'), independentPick, 'w1', 'w2']);
    expect(folderWorkspace(independentIn('w2'))).toBe('w2');
    expect(folderWorkspace('w2')).toBeNull();
    expect([independentHere, independentPick, independentIn('w1'), 'w1'].map(needsOwnRepo)).toEqual([true, true, false, false]);
    const c = { ...choicesFor(model, reviewer), repo: '' };
    expect(whereProblem(reviewer, c)).toBeNull();
    expect(whereProblem(reviewer, { ...c, where: independentHere })).toBe('bot.wizard.repo.needed');
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
    const labelled = { ...ruleOf({ do: 'ask' }), conditions: [{ field: 'labels', op: 'has' as const, value: 'bot:dev', ifPresent: false }] };
    const github = { kind: 'github' as const, event: 'pr_opened' as const, repo: 'owner/app', prompt: '이슈 봐 줘', trigger: labelled };
    const bot = { kind: 'bot' as const, bot: '@두부', outcome: 'done' as const, prompt: '', trigger: ruleOf({ do: 'ask' }) };
    const event = { kind: 'event' as const, name: 'release.done', prompt: '', trigger: ruleOf({ do: 'ask' }) };
    const added = { ...c, extra: [schedule, github, bot, event], prompts: ['오늘 할 일만', ''] };
    expect(stepProblem('when', briefing, added)).toBeNull();
    expect(whenWords(briefing, added, t)).toHaveLength(6);
    expect(extraWords(schedule, t)).toContain('bot.wizard.time.at');
    // MC-104: what it watches, its label a chip said in words beside it.
    expect(extraWords(github, t)).toBe('bot.wizard.when.prOpened · owner/app');
    expect(whenWords(briefing, added, t)[3]).toBe('bot.wizard.when.prOpened · owner/app (bot.wizard.cond.labelWords{"value":"bot:dev"})');
    expect(extraWords({ ...github, repo: '' }, t)).toBe('bot.wizard.when.prOpened');
    expect(newExtra('github')).toEqual({ kind: 'github', event: 'pr_opened', repo: '', prompt: '', trigger: ruleOf({ do: 'ask' }) });
    expect(extraWords(newExtra('bot'), t)).toBe('bot.wizard.bot.any');
    expect(extraWords(bot, t)).toBe('bot.wizard.bot.when{"bot":"두부"}');
    expect(extraWords({ ...bot, outcome: 'failed' }, t)).toBe('bot.wizard.bot.whenFailed{"bot":"두부"}');
    const request = createRequest(added);
    expect(request.prompts).toEqual(['오늘 할 일만', null]);
    const plain = { where: [], do: 'ask', with: {}, policy: {} };
    expect(request.extra).toEqual([
      { kind: 'schedule', cron: '0 9 * * 1-5', prompt: '' },
      { kind: 'github', event: 'pr_opened', repo: 'owner/app', prompt: '이슈 봐 줘', rule: { ...plain, where: [{ field: 'labels', op: 'has', value: 'bot:dev' }] } },
      { kind: 'bot', bot: '@두부', on: 'done', prompt: '', rule: plain },
      { kind: 'event', name: 'release.done', prompt: '', rule: plain },
    ]);
    // What each added job needs before the step goes on.
    const problem = (job: ReturnType<typeof newExtra>, where = c.where, tp = briefing) => stepProblem('when', tp, { ...c, where, extra: [job] });
    expect(problem({ kind: 'schedule', rule: { kind: 'at', hour: 9, minute: 0, days: [] }, prompt: '' })).toBe('bot.wizard.time.noDays');
    expect(problem({ ...github, trigger: { ...labelled, conditions: [{ field: 'labels', op: 'has', value: ' ', ifPresent: false }] } })).toBe('bot.wizard.rule.incomplete');
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
    // MC-104: in words: the MightyWork stage chip (its event type with it) and the title chip, what it does, its policy.
    expect(ruleWords(rule, t, 'mightywork')).toBe('bot.wizard.cond.stageWords{"value":"bot.wizard.stage.todo"} · bot.wizard.cond.titleWords' + JSON.stringify({ value: '^v\\d+\\.\\d+\\.\\d+ 릴리스' }) + ' · bot.wizard.say.run{"command":"/release {{task.version}}"} · bot.wizard.say.approve, bot.wizard.limits.oncePer');
    expect(ruleWords(rule, t)).toBe('type bot.wizard.op.eq mightywork.task.stage_changed · task.to bot.wizard.op.eq todo · task.title bot.wizard.op.matches ^v\\d+\\.\\d+\\.\\d+ 릴리스 · bot.wizard.say.run{"command":"/release {{task.version}}"} · bot.wizard.say.approve, bot.wizard.limits.oncePer');
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
    // MC-104: what it does is the 얼마나 맡길까요 step's; its conditions the 언제 step's.
    expect(stepProblem('limits', release, { ...c, rules: [{ ...rule, command: '' }] })).toBe('bot.wizard.rule.needsCommand');
    expect(stepProblem('when', release, { ...c, rules: [{ ...rule, command: '' }] })).toBeNull();
    expect(stepProblem('limits', release, { ...c, jobs: [false], rules: [{ ...rule, command: '' }] })).toBeNull();
    expect(stepProblem('when', release, { ...c, rules: [{ ...rule, conditions: [{ field: 'a', op: 'eq', value: '', ifPresent: false }] }] })).toBe('bot.wizard.rule.incomplete');
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

// ── MC-104: the 언제 step made simpler ─────────────────────────────────────────────────────────────────────────────────────────

/** The ten templates' jobs as the host gives them (bot_wizard.rs `model`): each job's words, cron and rule. */
const ask: HostRule = { where: [], do: 'ask', with: {}, policy: {} };
const stageTodo = [{ field: 'type', op: 'eq' as const, value: 'mightywork.task.stage_changed' }, { field: 'task.to', op: 'eq' as const, value: 'todo' }];
const hostTemplates: WizardTemplate[] = [
  template('prReviewer', { needs: 'repo', jobs: [{ when: '새 PR이 열리면', cron: null, rule: ask, source: { kind: 'github', event: 'pr_opened' } },
    { when: '리뷰를 요청받으면', cron: null, rule: ask, source: { kind: 'github', event: 'review_requested' } }] }),
  template('afterMerge', { needs: 'repo', jobs: [{ when: 'PR이 병합되면', cron: null, rule: ask, source: { kind: 'github', event: 'pr_merged' } }] }),
  template('issueTriage', { needs: 'repo', jobs: [{ when: '새 이슈가 열리면', cron: null, rule: ask, source: { kind: 'github', event: 'issue_opened' } }] }),
  template('ciDevelop', { needs: 'repo', jobs: [{ when: 'main의 CI가 실패하면', cron: null, source: { kind: 'github', event: 'ci_failed' }, rule: {
    where: [{ field: 'base', op: 'eq', value: 'main', ifPresent: true }], do: 'develop', with: { pr: true }, policy: { oncePer: 'subject' } } }] }),
  template('afterRelease', { jobs: [{ when: 'release.done 이벤트가 오면', cron: null, rule: ask, source: { kind: 'event', name: 'release.done' } }] }),
  template('mwRelease', { needs: 'project', jobs: [{ when: '릴리스 일감이 할 일로 가면', cron: null, source: { kind: 'mcp', preset: 'mightywork' }, rule: {
    where: [...stageTodo, { field: 'task.title', op: 'matches', value: '^v?\\d+\\.\\d+\\.\\d+ 릴리[스즈]' }], do: 'run',
    with: { command: '/release {{task.version}}' }, policy: { approve: true, oncePer: 'subject' } } }] }),
  template('mwDevelop', { needs: 'project', jobs: [{ when: 'bot:dev 일감이 할 일로 가면', cron: null, source: { kind: 'mcp', preset: 'mightywork' }, rule: {
    where: [...stageTodo, { field: 'task.tags', op: 'has', value: 'bot:dev' }], do: 'develop', with: { pr: true }, policy: { oncePer: 'subject' } } }] }),
  template('routine', { jobs: [{ when: '평일 9:00', cron: '0 9 * * 1-5', rule: null, source: null }] }),
  template('siteWatch', { needs: 'command', jobs: [{ when: '점검 결과가 바뀌면', cron: null, rule: ask, source: { kind: 'standing' } }] }),
  template('blank', { instructions: '', prompt: '', jobs: [] }),
];

describe('the 언제 step made simpler (MC-104)', () => {
  it('asks the host for exactly what it asked before when the person changes nothing, for every template', () => {
    const hostModel: WizardModel = { ...model, templates: hostTemplates };
    for (const tp of hostTemplates) {
      expect(createRequest(choicesFor(hostModel, tp)), tp.id).toEqual({
        template: tp.id, workspace: null, pickFolder: false, folderOf: 'w2', repo: '', command: '', agent: 'codex', model: '', name: '콩이',
        instructions: tp.instructions, prompt: tp.prompt,
        jobs: tp.jobs.map(() => true),
        crons: tp.jobs.map((job) => job.cron),
        prompts: tp.jobs.map(() => null),
        extra: [],
        rules: tp.jobs.map((job) => job.rule ?? null),
        project: '', permission: '', budget: 3, greet: true,
      });
    }
  });

  const cond = (field: string, op: RuleCondition['op'], value: string, ifPresent = false): RuleCondition => ({ field, op, value, ifPresent });
  const hostModel: WizardModel = { ...model, templates: hostTemplates };
  const byId = (id: string) => hostTemplates.find((tp) => tp.id === id)!;

  it('says each op in words, its 아님 form too, one word each', () => {
    expect(ruleOps.map((op) => opWords(op, t))).toEqual(['bot.wizard.op.eq', 'bot.wizard.op.in', 'bot.wizard.op.has', 'bot.wizard.op.matches',
      'bot.wizard.op.not_eq', 'bot.wizard.op.not_in', 'bot.wizard.op.not_has', 'bot.wizard.op.not_matches']);
    expect(new Set(Object.values(opKeys)).size).toBe(ruleOps.length);
  });

  it('keeps a small catalogue of conditions per event type, and the fields each carries', () => {
    const ids = (watch: keyof typeof conditionPresets) => conditionPresets[watch].map((p) => p.id);
    expect(ids('pr')).toEqual(['base', 'label', 'notAuthor']);
    expect(ids('ci')).toEqual(['base', 'label', 'notAuthor']);
    expect(ids('issue')).toEqual(['label']);
    expect(ids('mightywork')).toEqual(['stage', 'tag', 'title']);
    expect([ids('bot'), ids('event'), ids('standing'), ids('release'), ids('other')]).toEqual([[], [], [], [], []]);
    expect(conditionPresets.pr.map((p) => [p.field, p.op, p.ifPresent])).toEqual([['base', 'eq', true], ['labels', 'has', false], ['author', 'not_eq', false]]);
    expect(conditionPresets.mightywork.map((p) => [p.field, p.op])).toEqual([['task.to', 'eq'], ['task.tags', 'has'], ['task.title', 'matches']]);
    expect(watchFields('mightywork').slice(0, 2)).toEqual(['task.to', 'task.from']);
    expect(watchFields('pr')).toContain('subject');
    expect(watchFields('event')).toEqual(['type', 'subject', 'source', 'actor']);
    expect(stageWords('in_progress', t)).toBe('bot.wizard.stage.in_progress');
    expect(stageWords('later', t)).toBe('later');
  });

  it('reads conditions as chips, the rest as raw rows, and never reorders them', () => {
    const release = choicesFor(hostModel, byId('mwRelease')).rules[0]!.conditions;
    const { chips, raw } = conditionChips('mightywork', release);
    // The stage move takes its event type with it; the title is a chip.
    expect(chips.map((c) => [c.preset.id, c.index, c.companions])).toEqual([['stage', 1, [0]], ['title', 2, []]]);
    expect(raw).toEqual([]);
    const ci = choicesFor(hostModel, byId('ciDevelop')).rules[0]!.conditions;
    expect(conditionChips('ci', ci).chips.map((c) => c.preset.id)).toEqual(['base']);
    // Not a preset: another op, ifPresent different, or a field the catalogue does not know; and an event type alone.
    const mixed = [cond('labels', 'not_has', 'wip'), cond('base', 'eq', 'main'), cond('pr.title', 'matches', 'fix'), cond('labels', 'has', 'bug')];
    expect(conditionChips('pr', mixed)).toMatchObject({ chips: [{ preset: { id: 'label' }, index: 3 }], raw: [0, 1, 2] });
    expect(conditionChips('mightywork', [cond('type', 'eq', 'mightywork.task.stage_changed')]).raw).toEqual([0]);
    expect(conditionChips('event', [cond('data.env', 'eq', 'prod')]).raw).toEqual([0]);
  });

  it('adds a preset with what comes with it and takes a chip away with it', () => {
    const stage = conditionPresets.mightywork[0]!;
    const added = withPreset([], stage);
    expect(added).toEqual([cond('type', 'eq', 'mightywork.task.stage_changed'), cond('task.to', 'eq', 'todo')]);
    // Its event type is not added twice.
    expect(withPreset(added, stage)).toEqual([...added, cond('task.to', 'eq', 'todo')]);
    const base = withPreset([cond('labels', 'has', 'bug')], conditionPresets.pr[0]!);
    expect(base).toEqual([cond('labels', 'has', 'bug'), cond('base', 'eq', 'main', true)]);
    const tagged = withPreset(added, conditionPresets.mightywork[1]!);
    const chips = conditionChips('mightywork', tagged).chips;
    expect(withoutChip(tagged, chips[0]!)).toEqual([cond('task.tags', 'has', '')]);
    expect(withoutChip(tagged, chips[1]!)).toEqual(added);
    // A preset's empty value stops the step until it is written.
    expect(conditionsProblem(withPreset([], conditionPresets.pr[1]!))).toBe('bot.wizard.rule.incomplete');
  });

  it('says conditions in words, a chip as its preset, a raw row with its op in words', () => {
    const words = conditionsWords('pr', [cond('base', 'eq', 'main', true), cond('pr.title', 'not_matches', 'wip', true), cond('author', 'not_eq', 'bot')], t);
    expect(words).toEqual(['bot.wizard.cond.baseWords{"value":"main"}', 'pr.title? bot.wizard.op.not_matches wip', 'bot.wizard.cond.notAuthorWords{"value":"bot"}']);
    expect(conditionsWords('mightywork', withPreset([], conditionPresets.mightywork[0]!), t)).toEqual(['bot.wizard.cond.stageWords{"value":"bot.wizard.stage.todo"}']);
  });

  it('says what each card watches: a time, a GitHub event, a MightyWork project, an event, else the template\'s words', () => {
    const c = choicesFor(hostModel, byId('mwRelease'));
    expect(jobWords(byId('mwRelease'), c, 0, t)).toBe('bot.wizard.watch.mightywork');
    expect(jobWords(byId('mwRelease'), { ...c, project: ' MC ' }, 0, t)).toBe('bot.wizard.watch.mightyworkIn{"project":"MC"}');
    expect(jobWords(byId('ciDevelop'), choicesFor(hostModel, byId('ciDevelop')), 0, t)).toBe('bot.wizard.when.ciFailed');
    expect(jobWords(byId('siteWatch'), choicesFor(hostModel, byId('siteWatch')), 0, t)).toBe('점검 결과가 바뀌면');
    expect(jobWords(byId('afterRelease'), choicesFor(hostModel, byId('afterRelease')), 0, t)).toBe('bot.wizard.event.when{"name":"release.done"}');
    expect(jobWords(byId('routine'), choicesFor(hostModel, byId('routine')), 0, t)).toContain('bot.wizard.time.at');
    expect(['prReviewer', 'ciDevelop', 'issueTriage', 'mwDevelop', 'afterRelease', 'siteWatch'].map((id) => jobWatch(byId(id), 0))).toEqual(['pr', 'ci', 'issue', 'mightywork', 'event', 'standing']);
    expect(jobWatch(reviewer, 0)).toBe('other');
  });

  it('reads the whole bot in one sentence that follows each change', () => {
    const tp = byId('prReviewer');
    const c = { ...choicesFor(hostModel, tp), extra: [newExtra('schedule')] };
    const and = 'bot.wizard.say.and';
    expect(summaryWords(tp, c, t)).toBe(`bot.wizard.when.prOpened${and}bot.wizard.when.reviewRequested${and}bot.wizard.time.at{"days":"bot.wizard.time.weekdays","time":"09:00"} → bot.wizard.say.ask`);
    // A job taken away leaves it; a condition joins its when; another action is a group of its own.
    const changed = { ...c, jobs: [true, false], rules: [{ ...c.rules[0]!, kind: 'develop' as const, conditions: [cond('labels', 'has', 'bug')] }, c.rules[1]!] };
    expect(summaryWords(tp, changed, t)).toBe('bot.wizard.when.prOpened (bot.wizard.cond.labelWords{"value":"bug"}) → bot.wizard.say.developPr · bot.wizard.time.at{"days":"bot.wizard.time.weekdays","time":"09:00"} → bot.wizard.say.ask');
    // A run says its command and that it waits for the person.
    const release = byId('mwRelease');
    expect(summaryWords(release, { ...choicesFor(hostModel, release), project: 'MC' }, t)).toBe(
      'bot.wizard.watch.mightyworkIn{"project":"MC"} (bot.wizard.cond.stageWords{"value":"bot.wizard.stage.todo"}, bot.wizard.cond.titleWords'
      + JSON.stringify({ value: '^v?\\d+\\.\\d+\\.\\d+ 릴리[스즈]' }) + ') → bot.wizard.say.run{"command":"/release {{task.version}}"} (bot.wizard.say.approve)');
    expect(summaryWords(byId('blank'), choicesFor(hostModel, byId('blank')), t)).toBe('');
  });

  it('gives an added trigger a rule like a template job\'s, set in 얼마나 맡길까요 and sent to the host', () => {
    const tp = byId('routine');
    const c = { ...choicesFor(hostModel, tp), project: 'MC' };
    const mw = newExtra('mightywork', c.project);
    if (mw.kind !== 'mightywork') throw new Error('a MightyWork job');
    expect(mw).toMatchObject({ kind: 'mightywork', project: 'MC', trigger: { kind: 'ask', conditions: withPreset([], conditionPresets.mightywork[0]!) } });
    expect(newExtra('mightywork')).toMatchObject({ project: '' });
    const withJobs = { ...c, extra: [newExtra('schedule'), newExtra('github'), mw] };
    // The template's schedule has no rule; the added schedule neither; the two added triggers do.
    expect(triggerRules(withJobs).map((r) => ('extra' in r ? r.extra : -1))).toEqual([1, 2]);
    const developed = withTriggerRule(withJobs, { extra: 1 }, { ...ruleOf({ do: 'develop', with: { pr: true } }), conditions: [cond('base', 'eq', 'main', true)], oncePer: true });
    const ran = withTriggerRule(developed, { extra: 2 }, { ...mw.trigger, kind: 'run', command: '' });
    expect(stepProblem('limits', tp, ran)).toBe('bot.wizard.rule.needsCommand');
    expect(stepProblem('when', tp, ran)).toBeNull();
    const ready = withTriggerRule(ran, { extra: 2 }, { ...mw.trigger, kind: 'run', command: '/check {{task.key}}', approve: 'off' });
    expect(stepProblem('limits', tp, ready)).toBeNull();
    expect(extraLine(ready.extra[1]!, t)).toBe('bot.wizard.when.prOpened (bot.wizard.cond.baseWords{"value":"main"})');
    expect(createRequest(ready).extra).toEqual([
      { kind: 'schedule', cron: '0 9 * * 1-5', prompt: '' },
      { kind: 'github', event: 'pr_opened', repo: '', prompt: '', rule: { where: [{ field: 'base', op: 'eq', value: 'main', ifPresent: true }], do: 'develop', with: { pr: true }, policy: { oncePer: 'subject' } } },
      { kind: 'mightywork', project: 'MC', prompt: '', rule: {
        where: [{ field: 'type', op: 'eq', value: 'mightywork.task.stage_changed' }, { field: 'task.to', op: 'eq', value: 'todo' }],
        do: 'run', with: { command: '/check {{task.key}}' }, policy: { approve: false } } },
    ]);
    // A MightyWork job needs its project; a template job's rule changes by its place.
    expect(stepProblem('when', tp, { ...c, extra: [newExtra('mightywork')] })).toBe('bot.wizard.project.needed');
    const reviewerChoices = choicesFor(hostModel, byId('prReviewer'));
    const once = withTriggerRule(reviewerChoices, { index: 1 }, { ...reviewerChoices.rules[1]!, oncePer: true });
    expect(createRequest(once).rules).toEqual([ask, { ...ask, policy: { oncePer: 'subject' } }]);
    expect(actionProblem({ ...ruleOf({ do: 'emit' }), event: 'Bad Name' })).toBe('bot.wizard.extra.needsEvent');
  });
});
