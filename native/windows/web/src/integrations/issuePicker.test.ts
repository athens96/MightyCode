import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import {
  defaultFilterForTab,
  filterIntegrationItems,
  formatChecklistProgress,
  IssuePickerPanel,
  normalizeStageKey,
  safeExternalUrl,
  shouldAutoExpandSettings,
  type IntegrationItem,
  type WorkspaceIntegrationStatus,
} from './IssuePickerPanel';

const services = {
  bridge: { call: () => Promise.resolve(null) },
  store: { dispatch: () => {} },
  t: translator([
    {
      'dialog.close': 'Close',
      'dialog.resize': 'Resize',
      'integrations.title': 'Issues & Wiki',
      'integrations.subtitle': 'MightyWork and GitHub context',
      'integrations.tab.tasks': 'Tasks',
      'integrations.tab.wiki': 'Wiki',
      'integrations.tab.github': 'GitHub Issues',
      'integrations.auth.disconnected': 'Disconnected',
      'integrations.settings.toggle': 'Settings',
      'integrations.search.placeholder': 'Search tasks, wiki, issues…',
      'integrations.filter.all': 'All',
      'integrations.filter.todo': 'To Do',
      'integrations.filter.inProgress': 'In Progress',
      'integrations.filter.backlog': 'Backlog',
      'integrations.refresh': 'Refresh',
    },
  ]),
} as unknown as AppServices;

const SAMPLE_ITEMS: IntegrationItem[] = [
  {
    source: 'mightywork_task',
    key: 'MC-11',
    title: 'Desktop Web UI panels for MightyWork',
    stage: 'in_progress',
    tags: ['v0.5.9', 'desktop'],
  },
  {
    source: 'mightywork_task',
    key: 'MC-12',
    title: 'Backlog cleanup and docs',
    stage: 'backlog',
    tags: ['docs'],
  },
  {
    source: 'mightywork_wiki',
    key: 'wiki-arch',
    title: 'Bridge Architecture Guide',
  },
  {
    source: 'github_issue',
    key: '#42',
    title: 'Windows dark theme contrast',
    stage: 'open',
    tags: ['ui'],
  },
  {
    source: 'github_issue',
    key: '#39',
    title: 'Old closed issue',
    stage: 'closed',
  },
];

describe('IssuePickerPanel helpers', () => {
  it('chooses expected default filter per tab', () => {
    expect(defaultFilterForTab('tasks')).toBe('all');
    expect(defaultFilterForTab('wiki')).toBe('all');
    expect(defaultFilterForTab('github')).toBe('open');
  });

  it('normalizes stage labels consistently', () => {
    expect(normalizeStageKey('In Progress')).toBe('in_progress');
    expect(normalizeStageKey('in-progress')).toBe('in_progress');
    expect(normalizeStageKey(undefined)).toBe('');
  });

  it('filters integration items by tab, stage/state filter, and multi-term query', () => {
    expect(filterIntegrationItems(SAMPLE_ITEMS, { tab: 'tasks', filter: 'in_progress' })).toEqual([
      SAMPLE_ITEMS[0],
    ]);
    expect(filterIntegrationItems(SAMPLE_ITEMS, { tab: 'github', filter: 'open' })).toEqual([
      SAMPLE_ITEMS[3],
    ]);
    expect(
      filterIntegrationItems(SAMPLE_ITEMS, { tab: 'tasks', filter: 'all', query: 'mc-11 desktop' }),
    ).toEqual([SAMPLE_ITEMS[0]]);
    expect(
      filterIntegrationItems(SAMPLE_ITEMS, { tab: 'wiki', query: 'architecture' }),
    ).toEqual([SAMPLE_ITEMS[2]]);
  });

  it('formats checklist progress and validates external URLs', () => {
    expect(formatChecklistProgress([])).toBeNull();
    expect(
      formatChecklistProgress([
        { text: 'Write component', done: true },
        { text: 'Write unit tests', done: true },
        { text: 'Wire into shell', done: false },
      ]),
    ).toBe('2/3');

    expect(safeExternalUrl('https://mightywork.duckdns.org/tasks/MC-11')).toBe(
      'https://mightywork.duckdns.org/tasks/MC-11',
    );
    expect(safeExternalUrl('javascript:alert(1)')).toBeNull();
    expect(safeExternalUrl('')).toBeNull();
  });

  it('auto-expands settings when requested or when workspace has no configured integration', () => {
    const baseStatus: WorkspaceIntegrationStatus = {
      workspaceId: 'w1',
      workspaceName: 'MightyCode',
      isGitRepo: true,
      mightyworkProject: '',
      detectedMightyworkProject: '',
      mightyworkUrl: 'https://mightywork.duckdns.org/mcp',
      mightyworkConnected: false,
      mightyworkProjects: [],
      githubRepo: '',
      detectedGithubRepo: '',
      mightyworkAutoSync: false,
    };
    expect(shouldAutoExpandSettings(baseStatus, false)).toBe(true);
    expect(
      shouldAutoExpandSettings(
        { ...baseStatus, mightyworkConnected: true, mightyworkProject: 'MC' },
        false,
      ),
    ).toBe(false);
    expect(
      shouldAutoExpandSettings(
        { ...baseStatus, mightyworkConnected: true, mightyworkProject: 'MC' },
        true,
      ),
    ).toBe(true);
  });
});

describe('IssuePickerPanel rendering', () => {
  it('renders nothing when closed and renders tabs, filters, and settings drawer when open', () => {
    const closed = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(IssuePickerPanel, {
          open: false,
          workspaceId: 'w1',
          paneId: 'p1',
          onClose: () => {},
          onAttachToComposer: () => {},
        }),
      ),
    );
    expect(closed).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(IssuePickerPanel, {
          open: true,
          workspaceId: 'w1',
          paneId: 'p1',
          initialSettingsOpen: true,
          onClose: () => {},
          onAttachToComposer: () => {},
        }),
      ),
    );
    expect(openHtml).toContain('data-testid="issue-picker-panel"');
    expect(openHtml).toContain('Tasks');
    expect(openHtml).toContain('Wiki');
    expect(openHtml).toContain('GitHub Issues');
    expect(openHtml).toContain('data-testid="issue-picker-settings"');
  });
});
