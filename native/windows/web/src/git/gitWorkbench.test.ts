import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { emptyState } from '../store/store';
import cssSource from './GitWorkbenchPanel.css?raw';
import panelSource from './GitWorkbenchPanel.tsx?raw';
import modelSource from './gitWorkbenchModel.ts?raw';
import {
  checkBadgeMeta,
  countStagedFiles,
  defaultDiffStagedTab,
  fileMarkTone,
  formatAgentCommitPrompt,
  formatCiFixPrompt,
  formatSecretFindings,
  isValidBranchName,
  prStateLocaleKey,
  reconcileSelectedPaths,
  safeExternalHttpUrl,
  toggleAllSelectablePaths,
  type GitWorkbenchFile,
} from './gitWorkbenchModel';
import { GitWorkbenchPanel } from './GitWorkbenchPanel';

const SAMPLE_FILES: GitWorkbenchFile[] = [
  {
    path: 'src/git/GitWorkbenchPanel.tsx',
    mark: 'A',
    indexStatus: 'A',
    worktreeStatus: ' ',
    staged: true,
    unstaged: false,
    untracked: false,
    conflicted: false,
  },
  {
    path: 'src/shell/Chrome.tsx',
    mark: 'M',
    indexStatus: ' ',
    worktreeStatus: 'M',
    staged: false,
    unstaged: true,
    untracked: false,
    conflicted: false,
  },
  {
    path: 'src/conflict.ts',
    mark: '!',
    indexStatus: 'U',
    worktreeStatus: 'U',
    staged: false,
    unstaged: true,
    untracked: false,
    conflicted: true,
  },
];

const services = {
  bridge: { call: () => Promise.resolve(null) },
  store: {
    getState: () => emptyState,
    subscribe: () => () => {},
    dispatch: () => {},
  },
  t: translator([
    {
      'dialog.close': 'Close',
      'dialog.resize': 'Resize',
      'dialog.loading': 'Loading…',
      'git.workbench.title': 'Commit & PR',
      'git.workbench.refresh': 'Refresh',
      'git.workbench.newBranch': 'New branch',
      'git.workbench.selectAll': 'Select all',
      'git.workbench.diffEmpty': 'Select a changed file to inspect its diff.',
      'git.commit.files': '{count} changed files',
      'git.commit.clean': 'Nothing to commit.',
      'git.commit.message': 'Commit message',
      'git.commit.aiDraft': 'Draft with AI',
      'git.commit.askAgent': 'Ask the agent',
      'git.commit.button': 'Commit {count}',
      'git.push': 'Push',
      'git.pr.noRemote': 'No origin remote, so no pull request to show.',
    },
  ]),
} as unknown as AppServices;

describe('gitWorkbenchModel prompt formatting', () => {
  it('formats CI failure fix prompt with collision-safe fenced log tail', () => {
    const base = 'CI checks failed on this branch (https://github.com/o/r/pull/5).';
    expect(formatCiFixPrompt(base, '')).toBe(base);
    expect(formatCiFixPrompt(base, null)).toBe(base);

    const normal = formatCiFixPrompt(base, 'error[E0308]: mismatched types\n  --> src/main.rs:12:5');
    expect(normal).toBe(
      `${base}\n\n\`\`\`text\nerror[E0308]: mismatched types\n  --> src/main.rs:12:5\n\`\`\``,
    );

    const withBackticks = formatCiFixPrompt(base, 'output had ```embedded``` fence');
    expect(withBackticks).toContain('````text\noutput had ```embedded``` fence\n````');
  });

  it('formats agent commit requests with markdown file list', () => {
    const header = 'Review the changes in these files and commit only them.';
    expect(formatAgentCommitPrompt(header, ['src/a.ts', 'src/b.ts'])).toBe(
      `${header}\n- \`src/a.ts\`\n- \`src/b.ts\``,
    );
    expect(formatAgentCommitPrompt(header, [])).toBe(header);
  });
});

describe('gitWorkbenchModel selection and staged state logic', () => {
  it('reconciles selected paths across reloads while excluding conflicted files', () => {
    const initial = reconcileSelectedPaths(new Set(), new Set(), SAMPLE_FILES);
    expect(Array.from(initial)).toEqual([
      'src/git/GitWorkbenchPanel.tsx',
      'src/shell/Chrome.tsx',
    ]);

    // User unchecks Chrome.tsx, and a new untracked file arrives on reload:
    const known = new Set(SAMPLE_FILES.map((f) => f.path));
    const userPicked = new Set(['src/git/GitWorkbenchPanel.tsx']);
    const nextFiles: GitWorkbenchFile[] = [
      ...SAMPLE_FILES,
      {
        path: 'src/git/newHelper.ts',
        mark: '?',
        indexStatus: '?',
        worktreeStatus: '?',
        staged: false,
        unstaged: true,
        untracked: true,
        conflicted: false,
      },
    ];
    const afterReload = reconcileSelectedPaths(userPicked, known, nextFiles);
    expect(Array.from(afterReload)).toEqual([
      'src/git/GitWorkbenchPanel.tsx',
      'src/git/newHelper.ts',
    ]);
  });

  it('toggles select-all across non-conflicted files and computes staged state', () => {
    const all = toggleAllSelectablePaths(SAMPLE_FILES, new Set(['src/git/GitWorkbenchPanel.tsx']));
    expect(Array.from(all)).toEqual(['src/git/GitWorkbenchPanel.tsx', 'src/shell/Chrome.tsx']);

    const cleared = toggleAllSelectablePaths(SAMPLE_FILES, all);
    expect(cleared.size).toBe(0);

    expect(countStagedFiles(SAMPLE_FILES)).toBe(1);
    expect(defaultDiffStagedTab(SAMPLE_FILES[0], false)).toBe(true);
    expect(defaultDiffStagedTab(SAMPLE_FILES[1], true)).toBe(false);
    expect(
      defaultDiffStagedTab(
        { ...SAMPLE_FILES[0]!, staged: true, unstaged: true },
        true,
      ),
    ).toBe(true);
  });

  it('maps check rollup badges, PR states, file marks, branches, and secret summaries', () => {
    expect(checkBadgeMeta('passed')).toEqual({
      icon: 'circle-check',
      tone: 'done',
      labelKey: 'git.pr.checks.passed',
    });
    expect(checkBadgeMeta('failed')).toEqual({
      icon: 'circle-x',
      tone: 'err',
      labelKey: 'git.pr.checks.failed',
    });
    expect(checkBadgeMeta('pending')).toEqual({
      icon: 'clock',
      tone: 'wait',
      labelKey: 'git.pr.checks.pending',
    });
    expect(checkBadgeMeta('none')).toBeNull();

    expect(prStateLocaleKey('OPENED')).toBe('git.pr.state.open');
    expect(prStateLocaleKey('merged')).toBe('git.pr.state.merged');
    expect(prStateLocaleKey('closed')).toBe('git.pr.state.closed');

    expect(fileMarkTone('A')).toBe('done');
    expect(fileMarkTone('?')).toBe('done');
    expect(fileMarkTone('D')).toBe('err');
    expect(fileMarkTone('!')).toBe('err');
    expect(fileMarkTone('M')).toBe('wait');

    expect(isValidBranchName('feat/mc-5')).toBe(true);
    expect(isValidBranchName('-bad')).toBe(false);
    expect(isValidBranchName('bad..name')).toBe(false);
    expect(isValidBranchName('bad space')).toBe(false);

    expect(safeExternalHttpUrl('https://github.com/org/repo/pull/1')).toBe(
      'https://github.com/org/repo/pull/1',
    );
    expect(safeExternalHttpUrl('javascript:alert(1)')).toBeNull();

    const formattedSecrets = formatSecretFindings(
      [
        { path: '.env', line: 0, kind: 'secret file' },
        { path: 'src/cfg.ts', line: 14, kind: 'GitHub token' },
      ],
      8,
    );
    expect(formattedSecrets).toEqual(['.env · secret file', 'src/cfg.ts:14 · GitHub token']);
  });
});

describe('GitWorkbenchPanel rendering and hygiene', () => {
  it('renders nothing when closed and renders workbench chrome when open', () => {
    const closedHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(GitWorkbenchPanel, { open: false, workspaceId: 'ws1', onClose: () => {} }),
      ),
    );
    expect(closedHtml).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(GitWorkbenchPanel, { open: true, workspaceId: 'ws1', onClose: () => {} }),
      ),
    );
    expect(openHtml).toContain('data-testid="git-workbench-panel"');
    expect(openHtml).toContain('Commit &amp; PR');
    expect(openHtml).toContain('data-testid="git-ai-draft-btn"');
    expect(openHtml).toContain('data-testid="git-commit-btn"');
    expect(openHtml).toContain('data-testid="git-push-btn"');
    expect(openHtml).toContain('data-testid="git-ask-agent-btn"');
  });

  it('enforces zero raw color literals and zero hardcoded Hangul in implementation files', () => {
    const files: Array<[string, string]> = [
      ['gitWorkbenchModel.ts', modelSource],
      ['GitWorkbenchPanel.tsx', panelSource],
      ['GitWorkbenchPanel.css', cssSource],
    ];
    for (const [fileName, source] of files) {
      expect(source, `${fileName} must not contain raw hex colors`).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
      expect(source, `${fileName} must not contain rgb/hsl literals`).not.toMatch(/\b(?:rgb|rgba|hsl|hsla)\s*\(/);
      expect(source, `${fileName} must not contain hardcoded Hangul`).not.toMatch(/[\uAC00-\uD7A3]/);
    }
  });
});
