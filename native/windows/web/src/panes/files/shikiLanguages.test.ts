import { describe, expect, it } from 'vitest';
import { createHighlighterCore, type ThemeRegistrationResolved } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import { EXTENSION_TO_FILE_FORMAT } from '@pierre/diffs';
import {
  BUNDLED_PIERRE_THEMES,
  BUNDLED_SHIKI_LANGUAGES,
  BUNDLED_SHIKI_LANGUAGE_NAMES,
  generatePrunedShikiLangsModule,
  generatePrunedThemesModule,
  isBundledShikiLanguage,
  patchFiletypeSource,
  patchResolveLanguageSource,
} from './shikiLanguages';

describe('shiki language and theme pruning', () => {
  it('keeps pierre-dark and pierre-light and omits unused Shiki/Pierre themes', () => {
    expect(BUNDLED_PIERRE_THEMES).toEqual(['pierre-light', 'pierre-dark']);
    const moduleCode = generatePrunedThemesModule();
    expect(moduleCode).toContain('@pierre/theme/pierre-light');
    expect(moduleCode).toContain('@pierre/theme/pierre-dark');
    expect(moduleCode).not.toContain('@shikijs/themes');
    expect(moduleCode).not.toContain('pierre-dark-vibrant');
    expect(moduleCode).not.toContain('dracula');
  });

  it('includes all common software engineering languages and omits niche grammars', () => {
    const required = [
      'js', 'javascript', 'jsx', 'ts', 'typescript', 'tsx', 'json', 'jsonc',
      'html', 'css', 'scss', 'xml', 'md', 'markdown', 'mdx', 'yaml', 'yml',
      'toml', 'ini', 'sh', 'bash', 'zsh', 'shellscript', 'powershell', 'bat', 'cmd',
      'rust', 'swift', 'c', 'cpp', 'csharp', 'java', 'kotlin', 'kts', 'go',
      'python', 'ruby', 'php', 'sql', 'graphql', 'docker', 'dockerfile',
      'make', 'makefile', 'diff', 'hcl', 'lua', 'dart', 'zig', 'nix',
      'prisma', 'vue', 'svelte', 'astro', 'regexp', 'text',
    ];
    for (const lang of required) {
      expect(isBundledShikiLanguage(lang), `expected ${lang} to be bundled`).toBe(true);
    }

    const pruned = ['emacs-lisp', 'wolfram', 'fortran-free-form', 'wikitext', 'vyper', 'stata', 'ballerina'];
    const langsCode = generatePrunedShikiLangsModule();
    for (const lang of pruned) {
      expect(isBundledShikiLanguage(lang), `expected ${lang} to be pruned`).toBe(false);
      expect(langsCode).not.toContain(`@shikijs/langs/${lang}`);
    }
  });

  it('patches getFiletypeFromFileName so pruned extensions fall back to text and .h/.svg highlight cleanly', () => {
    const sample = `
const EXTENSION_TO_FILE_FORMAT = ${JSON.stringify({
  ts: EXTENSION_TO_FILE_FORMAT.ts,
  rs: EXTENSION_TO_FILE_FORMAT.rs,
  sh: EXTENSION_TO_FILE_FORMAT.sh,
  bat: EXTENSION_TO_FILE_FORMAT.bat,
  Dockerfile: EXTENSION_TO_FILE_FORMAT.Dockerfile,
  Makefile: EXTENSION_TO_FILE_FORMAT.Makefile,
  h: EXTENSION_TO_FILE_FORMAT.h,
  el: EXTENSION_TO_FILE_FORMAT.el,
  wl: EXTENSION_TO_FILE_FORMAT.wl,
  f90: EXTENSION_TO_FILE_FORMAT.f90,
})};
function getFiletypeFromFileName(fileName) {
  const ext = fileName.includes('.') ? fileName.split('.').pop() : fileName;
  return Object.hasOwn(EXTENSION_TO_FILE_FORMAT, ext) ? EXTENSION_TO_FILE_FORMAT[ext] : 'text';
}
return getFiletypeFromFileName;
`;
    const patched = patchFiletypeSource(sample);
    const getFiletype = new Function(patched)() as (name: string) => string;

    expect(getFiletype('app.ts')).toBe('typescript');
    expect(getFiletype('main.rs')).toBe('rust');
    expect(getFiletype('run.sh')).toBe('zsh');
    expect(getFiletype('build.bat')).toBe('cmd');
    expect(getFiletype('Dockerfile')).toBe('dockerfile');
    expect(getFiletype('Makefile')).toBe('makefile');
    expect(getFiletype('header.h')).toBe('cpp');
    expect(getFiletype('icon.svg')).toBe('xml');
    // Pruned grammars resolve directly to plaintext.
    expect(getFiletype('init.el')).toBe('text');
    expect(getFiletype('model.wl')).toBe('text');
    expect(getFiletype('solver.f90')).toBe('text');
    expect(getFiletype('README.unknownext')).toBe('text');
  });

  it('patches resolveLanguage to return a valid plaintext fallback grammar and tokenizes cleanly with Shiki', async () => {
    const snippet = 'if (loader == null) throw new Error(`resolveLanguage: "${lang}" not found in bundled or custom languages`);';
    const patched = patchResolveLanguageSource(snippet);
    expect(patched).not.toContain('throw new Error');
    expect(patched).toContain('text.plain.');

    // Verify the pruned virtual themes module, pruned virtual shiki/langs module,
    // a bundled grammar, and the plaintext fallback grammar all work end-to-end with Shiki.
    const { themes } = await import('@pierre/theming/themes');
    expect(themes.getThemeNames().slice().sort()).toEqual(['pierre-dark', 'pierre-light']);
    expect(themes.getTheme('dracula')).toBeUndefined();
    const darkTheme = await themes.getTheme('pierre-dark')!.load() as ThemeRegistrationResolved;
    const lightTheme = await themes.getTheme('pierre-light')!.load() as ThemeRegistrationResolved;

    const { bundledLanguages } = await import('shiki/langs');
    expect(Object.keys(bundledLanguages).sort()).toEqual(
      [...BUNDLED_SHIKI_LANGUAGE_NAMES].filter((n) => n !== 'text' && n !== 'ansi').sort(),
    );
    expect(BUNDLED_SHIKI_LANGUAGES.length).toBeGreaterThan(45);

    const tsGrammar = (await bundledLanguages.typescript()).default;
    const fallbackGrammar = { name: 'emacs-lisp', scopeName: 'text.plain.emacs-lisp', patterns: [], repository: {} };
    const highlighter = await createHighlighterCore({
      themes: [darkTheme, lightTheme],
      langs: [tsGrammar, fallbackGrammar],
      engine: createJavaScriptRegexEngine(),
    });
    try {
      const tsResult = highlighter.codeToTokens('const x: number = 1;', { lang: 'typescript', theme: 'pierre-dark' });
      expect(tsResult.tokens[0]!.length).toBeGreaterThan(1);

      const fallbackResult = highlighter.codeToTokens('(defun hi () 42)', { lang: 'emacs-lisp', theme: 'pierre-light' });
      expect(fallbackResult.tokens[0]!.map((token) => token.content).join('')).toBe('(defun hi () 42)');
    } finally {
      highlighter.dispose();
    }
  });
});
