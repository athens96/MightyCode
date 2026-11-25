import { describe, expect, it } from 'vitest';
import {
  choicesFor, createRequest, daysField, filterTemplates, independentHere, independentPick, parseCron, parseDays, stepProblem, steps, timeWords, toCron, validRepo, whenWords, whereProblem,
  type WizardModel, type WizardTemplate,
} from './botWizardRules';

const template = (id: string, extra: Partial<WizardTemplate> = {}): WizardTemplate => ({
  id, icon: '🔍', category: 'bot.wizard.category.review', label: id, what: id + ' does', instructions: id + ' rules', needs: 'nothing', commandHint: '', jobs: [], ...extra,
});
const reviewer = template('prReviewer', { label: 'PR 리뷰어', what: '새 PR을 리뷰', needs: 'repo', jobs: [{ when: '새 PR이 열리면', cron: null }, { when: '리뷰를 요청받으면', cron: null }] });
const site = template('siteWatch', { category: 'bot.wizard.category.watch', label: '사이트 상태 지킴이', needs: 'command', jobs: [{ when: '점검 결과가 바뀌면', cron: null }] });
const briefing = template('briefing', { category: 'bot.wizard.category.other', label: '아침 브리핑', jobs: [{ when: '평일 9:00', cron: '0 9 * * 1-5' }, { when: '30분마다', cron: '*/30 9-19 * * 1-5' }] });
const t = (key: string, subs?: Readonly<Record<string, string | number>>) => key + (subs ? JSON.stringify(subs) : '');
const blank = template('blank', { category: 'bot.wizard.category.other', label: '빈 봇', instructions: '' });
const model: WizardModel = {
  categories: [{ id: 'bot.wizard.category.review', label: '코드 리뷰' }, { id: 'bot.wizard.category.watch', label: '감시' }, { id: 'bot.wizard.category.other', label: '기타' }],
  templates: [reviewer, site, blank],
  workspaces: [{ id: 'w1', name: 'Shop' }, { id: 'w2', name: 'Docs' }],
  activeWorkspace: 'w2',
  agents: [
    { id: 'claude', label: 'Claude', available: false, permissions: [], models: [] },
    { id: 'codex', label: 'Codex', available: true, permissions: [{ value: 'auto', label: 'Auto' }], models: [{ value: 'gpt-5', label: 'GPT-5' }] },
  ],
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
    expect(c).toMatchObject({ template: 'prReviewer', where: 'w2', agent: 'codex', name: '콩이', instructions: 'prReviewer rules', jobs: [true, true], budget: 3, awake: true, greet: true });
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

  it('walks six steps and stops a step only for what it needs', () => {
    expect(steps).toEqual(['template', 'where', 'who', 'when', 'limits', 'confirm']);
    const c = choicesFor(model, site);
    expect(stepProblem('where', site, c)).toBe('bot.wizard.needsCommand');
    expect(stepProblem('where', site, { ...c, command: 'curl -s x' })).toBeNull();
    expect(stepProblem('who', site, { ...c, agent: '' })).toBe('bot.wizard.needsAgent');
    expect(['template', 'when', 'limits', 'confirm'].map((s) => stepProblem(s as never, site, c))).toEqual([null, null, null, null]);
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
});
