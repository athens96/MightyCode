import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import {
  initialPromptDraftText,
  normalizeSavedPromptName,
  SavedPromptsPanel,
} from './SavedPromptsPanel';

const services = {
  bridge: { call: () => Promise.resolve({ prompts: [], offered: [] }) },
  store: { dispatch: () => {} },
  t: translator([
    {
      'dialog.close': 'Close',
      'dialog.resize': 'Resize',
      'savedPrompts.title': 'Saved requests',
      'savedPrompts.subtitle': 'Type /prompt:name in the composer to use one',
      'savedPrompts.new': 'New request',
      'savedPrompts.empty': 'No saved requests yet.',
      'savedPrompts.name': 'Name (e.g. review-pr)',
      'savedPrompts.textPlaceholder': 'Write the reusable request…',
      'savedPrompts.placeholders': 'Placeholders: {branch} · {workspace}',
      'savedPrompts.shareAll': 'Use in every workspace',
      'savedPrompts.use': 'Put in composer',
      'savedPrompts.sendNow': 'Send now',
      'savedPrompts.save': 'Save',
    },
  ]),
} as unknown as AppServices;

describe('normalizeSavedPromptName', () => {
  it('lowercases ASCII letters and trims surrounding whitespace', () => {
    expect(normalizeSavedPromptName('  Review_PR-12  ')).toBe('review_pr-12');
  });

  it('turns internal spaces into hyphens and strips punctuation', () => {
    expect(normalizeSavedPromptName('Fix: flaky tests! (fast)')).toBe('fix-flaky-tests-fast');
  });

  it('preserves Hangul and other Unicode letters alongside digits', () => {
    expect(normalizeSavedPromptName('코드 리뷰 2차')).toBe('코드-리뷰-2차');
  });

  it('truncates to 40 Unicode code points after filtering invalid characters', () => {
    const raw = '!' + '가a'.repeat(25);
    const normalized = normalizeSavedPromptName(raw);
    expect(normalized).not.toBeNull();
    expect(Array.from(normalized!).length).toBe(40);
    expect(normalized).toBe('가a'.repeat(20));
  });

  it('returns null when empty or containing only unsupported punctuation', () => {
    expect(normalizeSavedPromptName('')).toBeNull();
    expect(normalizeSavedPromptName('   ')).toBeNull();
    expect(normalizeSavedPromptName('!@#$%^&*()')).toBeNull();
  });
});

describe('initialPromptDraftText', () => {
  it('prefills non-command composer text and ignores /prompts invocations', () => {
    expect(initialPromptDraftText('  Check {branch} diff  ')).toBe('Check {branch} diff');
    expect(initialPromptDraftText('/prompts')).toBe('');
    expect(initialPromptDraftText('  /prompts  ')).toBe('');
    expect(initialPromptDraftText(undefined)).toBe('');
  });
});

describe('SavedPromptsPanel rendering', () => {
  it('renders nothing when closed and renders panel shell when open', () => {
    const closed = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(SavedPromptsPanel, {
          open: false,
          paneId: 'p1',
          workspaceId: 'w1',
          onClose: () => {},
          onPutInComposer: () => {},
          onChanged: () => {},
        }),
      ),
    );
    expect(closed).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(SavedPromptsPanel, {
          open: true,
          paneId: 'p1',
          workspaceId: null,
          initialDraft: 'Audit module tests',
          onClose: () => {},
          onPutInComposer: () => {},
          onChanged: () => {},
        }),
      ),
    );
    expect(openHtml).toContain('data-testid="saved-prompts-panel"');
    expect(openHtml).toContain('Saved requests');
    expect(openHtml).toContain('Audit module tests');
    expect(openHtml).toContain('data-testid="saved-prompts-send-now"');
  });
});
