import { describe, expect, it } from 'vitest';
import {
  choicesFor, createRequest, filterTemplates, independentHere, independentPick, quickTarget, validRepo, whenWords, whereProblem,
  type WizardModel, type WizardTemplate,
} from './botWizardRules';

const template = (id: string, extra: Partial<WizardTemplate> = {}): WizardTemplate => ({
  id, icon: '🔍', category: 'bot.wizard.category.review', label: id, what: id + ' does', instructions: id + ' rules', needs: 'nothing', commandHint: '', jobs: [], ...extra,
});
const reviewer = template('prReviewer', { label: 'PR 리뷰어', what: '새 PR을 리뷰', needs: 'repo', jobs: ['새 PR이 열리면', '리뷰를 요청받으면'] });
const site = template('siteWatch', { category: 'bot.wizard.category.watch', label: '사이트 상태 지킴이', needs: 'command', jobs: ['점검 결과가 바뀌면'] });
const blank = template('blank', { category: 'bot.wizard.category.other', label: '빈 봇', instructions: '' });
const model: WizardModel = {
  categories: [{ id: 'bot.wizard.category.review', label: '코드 리뷰' }, { id: 'bot.wizard.category.watch', label: '감시' }, { id: 'bot.wizard.category.other', label: '기타' }],
  templates: [reviewer, site, blank],
  workspaces: [{ id: 'w1', name: 'Shop' }, { id: 'w2', name: 'Docs' }],
  activeWorkspace: 'w2',
  agents: [
    { id: 'claude', label: 'Claude', available: false, permissions: [] },
    { id: 'codex', label: 'Codex', available: true, permissions: [{ value: 'auto', label: 'Auto' }] },
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

  it('⚡ goes to the last look unless something is missing', () => {
    expect(quickTarget(reviewer, choicesFor(model, reviewer))).toBe('confirm');
    expect(quickTarget(site, choicesFor(model, site))).toBe('where');
    expect(quickTarget(blank, choicesFor(model, blank))).toBe('confirm');
  });

  it('says the jobs left on and asks the host for what was chosen', () => {
    const c = { ...choicesFor(model, reviewer), jobs: [true, false], name: '@두부 ', budget: 0, where: independentPick, repo: ' o/r ' };
    expect(whenWords(reviewer, c)).toEqual(['새 PR이 열리면']);
    expect(createRequest(c)).toMatchObject({ template: 'prReviewer', workspace: null, pickFolder: true, repo: 'o/r', name: '두부', budget: null, jobs: [true, false], agent: 'codex' });
    expect(createRequest(choicesFor(model, reviewer))).toMatchObject({ workspace: 'w2', pickFolder: false, budget: 3 });
  });
});
