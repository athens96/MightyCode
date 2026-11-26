// MC-104: the bot wizard's 언제 and 얼마나 맡길까요 steps, drawn: the sentence on top, one card shape for every job with its
// conditions as chips, the raw rows under 고급, and what a trigger does with its two toggles in 얼마나 맡길까요.
import ko from '@locales/ko.json';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { TriggerActions, WhenStep } from './BotWizard';
import botWizardCss from './botWizard.css?raw';
import { choicesFor, newExtra, type HostRule, type WizardChoices, type WizardModel, type WizardTemplate } from './botWizardRules';

const services = { t: translator([ko as Record<string, string>]) } as unknown as AppServices;
const draw = (element: ReturnType<typeof createElement>) => renderToStaticMarkup(createElement(ServicesContext.Provider, { value: services }, element));
const release: HostRule = {
  where: [{ field: 'type', op: 'eq', value: 'mightywork.task.stage_changed' }, { field: 'task.to', op: 'eq', value: 'todo' },
    { field: 'task.title', op: 'matches', value: '^v?\\d+ 릴리스' }, { field: 'task.key', op: 'not_eq', value: 'MC-1' }],
  do: 'run', with: { command: '/release {{task.version}}' }, policy: { approve: true, oncePer: 'subject' },
};
const template: WizardTemplate = {
  id: 'mwRelease', icon: '🚀', category: 'c', label: 'MightyWork 릴리스 봇', what: '', instructions: 'i', needs: 'project', commandHint: '', prompt: 'p',
  jobs: [{ when: '릴리스 일감이 할 일로 가면', cron: null, rule: release, source: { kind: 'mcp', preset: 'mightywork' } }], examples: [], vars: [],
};
const model: WizardModel = { categories: [], templates: [template], workspaces: [], activeWorkspace: null, agents: [], bots: ['두부'], name: '콩이' };

describe('the 언제 step drawn (MC-104)', () => {
  const choices: WizardChoices = { ...choicesFor(model, template), project: 'MC', extra: [newExtra('github')] };

  it('reads the bot in a sentence and draws every job as the same card, conditions as chips, the rest under 고급', () => {
    const html = draw(createElement(WhenStep, { template, choices, bots: model.bots, onChange: () => {} }));
    expect(html).toContain('data-testid="bot-wizard-sentence"');
    expect(html).toContain('MightyWork MC 일감이 바뀌면 (할 일 단계로 바뀔 때, 제목 패턴 ^v?\\d+ 릴리스, task.key 같지 않음 MC-1) → 명령을 실행합니다: /release {{task.version}} (먼저 내 승인을 받음)');
    expect(html).toContain(' · 새 PR이 열리면 → 요청을 보냅니다');
    expect(html.match(/class="bot-wizard-job-card"/g)).toHaveLength(2);
    expect(html.match(/data-testid="bot-wizard-job-remove"/g)).toHaveLength(2);
    expect(html).toContain('data-testid="bot-wizard-cond-stage"');
    expect(html).toContain('data-testid="bot-wizard-cond-title"');
    // The task.key row is not a preset: a raw row under 고급, its op in words; 고급 opens since it has one.
    expect(html.match(/data-testid="bot-wizard-raw-row"/g)).toHaveLength(1);
    expect(html).toContain('<option value="not_eq" selected="">같지 않음</option>');
    expect(html).toMatch(/<details class="bot-wizard-advanced" open="">/);
    // A pull request's card offers its presets; the 추가 row shows 예약 and GitHub, the rest under 더 보기.
    expect(html).toContain('data-testid="bot-wizard-cond-add"');
    expect(html).toContain('data-testid="bot-wizard-add-schedule"');
    expect(html).toContain('data-testid="bot-wizard-add-more"');
    expect(html).not.toContain('data-testid="bot-wizard-add-mightywork"');
    // Nothing of what it does here.
    expect(html).not.toContain('bot-wizard-rule-do');
  });

  it('puts a template job taken away back from the 추가 row', () => {
    const html = draw(createElement(WhenStep, { template, choices: { ...choices, jobs: [false] }, bots: [], onChange: () => {} }));
    expect(html.match(/class="bot-wizard-job-card"/g)).toHaveLength(1);
    expect(html).toContain('data-testid="bot-wizard-restore"');
  });

  it('sets what each trigger does in 얼마나 맡길까요, by its words when there are several, with two plain toggles', () => {
    const html = draw(createElement(TriggerActions, { template, choices, onChange: () => {} }));
    expect(html).toContain('트리거가 할 일');
    expect(html.match(/data-testid="bot-wizard-limit"/g)).toHaveLength(2);
    expect(html).toContain('<option value="run" selected="">명령 실행하기</option>');
    expect(html).toContain('value="/release {{task.version}}"');
    expect(html).toContain('실행 전에 내 승인 받기');
    expect(html).toContain('기본으로 되돌리기 (켜짐)');
    expect(html).toContain('기본: 꺼짐');
    expect(html).toContain('같은 일감·PR엔 한 번만');
    const one = draw(createElement(TriggerActions, { template, choices: { ...choices, extra: [] }, onChange: () => {} }));
    expect(one).not.toContain('data-testid="bot-wizard-limit"');
    expect(one.match(/data-testid="bot-wizard-action"/g)).toHaveLength(1);
  });

  it('keeps every card border uniform, never a colored left edge', () => {
    expect(botWizardCss).not.toMatch(/\bborder-left\b|\bborder-inline-start\b/);
    expect(botWizardCss).not.toMatch(/box-shadow\s*:[^;]*\binset\s+[1-9]/);
  });
});
