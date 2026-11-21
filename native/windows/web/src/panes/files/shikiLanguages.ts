// Curated Shiki languages and Pierre themes bundled with @pierre/diffs.
// Every canonical ID matches a module under `@shikijs/langs/<id>`, and aliases match
// what `@pierre/diffs`'s `EXTENSION_TO_FILE_FORMAT` and Shiki grammar metadata resolve.

export interface ShikiLanguageEntry {
  readonly id: string;
  readonly name: string;
  readonly aliases?: readonly string[];
}

/** Themes actually used by FilesView, DiffView and ResultRollUp (`DEFAULT_THEMES`). */
export const BUNDLED_PIERRE_THEMES = ['pierre-light', 'pierre-dark'] as const;

/**
 * Common software engineering grammars kept in the desktop bundle.
 * Languages outside this list fall back cleanly to plaintext (`text`).
 */
export const BUNDLED_SHIKI_LANGUAGES: readonly ShikiLanguageEntry[] = [
  { id: 'javascript', name: 'JavaScript', aliases: ['js', 'cjs', 'mjs'] },
  { id: 'jsx', name: 'JSX' },
  { id: 'typescript', name: 'TypeScript', aliases: ['ts', 'cts', 'mts'] },
  { id: 'tsx', name: 'TSX' },
  { id: 'json', name: 'JSON' },
  { id: 'jsonc', name: 'JSON with Comments' },
  { id: 'json5', name: 'JSON5' },
  { id: 'jsonl', name: 'JSON Lines' },
  { id: 'html', name: 'HTML' },
  { id: 'css', name: 'CSS' },
  { id: 'scss', name: 'SCSS' },
  { id: 'xml', name: 'XML' },
  { id: 'markdown', name: 'Markdown', aliases: ['md'] },
  { id: 'mdx', name: 'MDX' },
  { id: 'yaml', name: 'YAML', aliases: ['yml'] },
  { id: 'toml', name: 'TOML' },
  { id: 'ini', name: 'INI', aliases: ['properties'] },
  { id: 'dotenv', name: 'dotenv' },
  { id: 'shellscript', name: 'Shell', aliases: ['bash', 'sh', 'shell', 'zsh'] },
  { id: 'powershell', name: 'PowerShell', aliases: ['ps', 'ps1', 'pwsh'] },
  { id: 'bat', name: 'Batch File', aliases: ['batch', 'cmd'] },
  { id: 'rust', name: 'Rust', aliases: ['rs'] },
  { id: 'swift', name: 'Swift' },
  { id: 'c', name: 'C' },
  { id: 'cpp', name: 'C++', aliases: ['c++'] },
  { id: 'csharp', name: 'C#', aliases: ['c#', 'cs'] },
  { id: 'java', name: 'Java' },
  { id: 'kotlin', name: 'Kotlin', aliases: ['kt', 'kts'] },
  { id: 'go', name: 'Go' },
  { id: 'python', name: 'Python', aliases: ['py'] },
  { id: 'ruby', name: 'Ruby', aliases: ['rb'] },
  { id: 'php', name: 'PHP' },
  { id: 'sql', name: 'SQL' },
  { id: 'graphql', name: 'GraphQL', aliases: ['gql'] },
  { id: 'docker', name: 'Dockerfile', aliases: ['dockerfile'] },
  { id: 'make', name: 'Makefile', aliases: ['makefile'] },
  { id: 'cmake', name: 'CMake' },
  { id: 'diff', name: 'Diff' },
  { id: 'hcl', name: 'HCL' },
  { id: 'terraform', name: 'Terraform', aliases: ['tf', 'tfvars'] },
  { id: 'proto', name: 'Protocol Buffer 3', aliases: ['protobuf'] },
  { id: 'lua', name: 'Lua' },
  { id: 'dart', name: 'Dart' },
  { id: 'zig', name: 'Zig' },
  { id: 'nix', name: 'Nix' },
  { id: 'prisma', name: 'Prisma' },
  { id: 'vue', name: 'Vue' },
  { id: 'svelte', name: 'Svelte' },
  { id: 'astro', name: 'Astro' },
  { id: 'regexp', name: 'RegExp', aliases: ['regex'] },
  { id: 'git-commit', name: 'Git Commit Message' },
  { id: 'git-rebase', name: 'Git Rebase Message' },
  { id: 'codeowners', name: 'CODEOWNERS' },
];

/** Extension overrides so common developer files use bundled grammars cleanly. */
export const EXTENSION_FORMAT_OVERRIDES: Readonly<Record<string, string>> = {
  // `.h` defaults to `objective-cpp` in @pierre/diffs; map to bundled `cpp` when objective-cpp is pruned.
  h: 'cpp',
  svg: 'xml',
};

/** Every bundled language ID and alias (plus built-in `text` and `ansi`). */
export const BUNDLED_SHIKI_LANGUAGE_NAMES: ReadonlySet<string> = new Set([
  'text',
  'ansi',
  ...BUNDLED_SHIKI_LANGUAGES.flatMap((entry) => [entry.id, ...(entry.aliases ?? [])]),
]);

export function isBundledShikiLanguage(lang: string): boolean {
  return BUNDLED_SHIKI_LANGUAGE_NAMES.has(lang);
}

/**
 * Generates the pruned `shiki/langs` virtual module exporting `bundledLanguages`,
 * `bundledLanguagesBase`, `bundledLanguagesAlias`, and `bundledLanguagesInfo`.
 */
export function generatePrunedShikiLangsModule(): string {
  const baseEntries: string[] = [];
  const aliasEntries: string[] = [];
  const infoEntries: string[] = [];

  for (const entry of BUNDLED_SHIKI_LANGUAGES) {
    const idLit = JSON.stringify(entry.id);
    const loader = `() => import(${JSON.stringify('@shikijs/langs/' + entry.id)})`;
    baseEntries.push(`  ${idLit}: ${loader}`);
    const aliasesProp = entry.aliases?.length ? `, aliases: ${JSON.stringify(entry.aliases)}` : '';
    infoEntries.push(`  { id: ${idLit}, name: ${JSON.stringify(entry.name)}${aliasesProp}, import: ${loader} }`);
    for (const alias of entry.aliases ?? []) {
      aliasEntries.push(`  ${JSON.stringify(alias)}: bundledLanguagesBase[${idLit}]`);
    }
  }

  return [
    'const bundledLanguagesBase = {',
    baseEntries.join(',\n'),
    '};',
    'const bundledLanguagesAlias = {',
    aliasEntries.join(',\n'),
    '};',
    'const bundledLanguages = { ...bundledLanguagesBase, ...bundledLanguagesAlias };',
    'const bundledLanguagesInfo = [',
    infoEntries.join(',\n'),
    '];',
    'export { bundledLanguages, bundledLanguagesAlias, bundledLanguagesBase, bundledLanguagesInfo };',
  ].join('\n');
}

/**
 * Generates the pruned `@pierre/theming/themes` virtual module keeping only
 * `pierre-light` and `pierre-dark` so the 60+ unused Shiki/Pierre themes are omitted.
 */
export function generatePrunedThemesModule(): string {
  return `
import { createThemeCollection } from "@pierre/theming";
import { normalizeTheme } from "shiki/core";

function createTheme({ name, load, colorScheme, collection, displayName }) {
  return {
    name,
    colorScheme,
    collection,
    displayName,
    load: async () => {
      const raw = await load();
      return normalizeTheme(raw && typeof raw === "object" && "default" in raw ? raw.default : raw);
    },
  };
}

const pierreThemes = createThemeCollection({
  themes: [
    createTheme({
      name: "pierre-light",
      collection: "pierre",
      colorScheme: "light",
      displayName: "Pierre Light",
      load: () => import("@pierre/theme/pierre-light"),
    }),
    createTheme({
      name: "pierre-dark",
      collection: "pierre",
      colorScheme: "dark",
      displayName: "Pierre Dark",
      load: () => import("@pierre/theme/pierre-dark"),
    }),
  ],
});
const shikiThemes = createThemeCollection({ themes: [] });
const themes = createThemeCollection({ themes: [pierreThemes] });
export { createTheme, pierreThemes, shikiThemes, themes };
`;
}

/**
 * Patches `@pierre/diffs/dist/highlighter/languages/resolveLanguage.js` so any unbundled
 * language requested directly falls back to a plaintext TextMate grammar instead of throwing.
 */
export function patchResolveLanguageSource(code: string): string {
  const target = 'if (loader == null) throw new Error(`resolveLanguage: "${lang}" not found in bundled or custom languages`);';
  if (!code.includes(target)) return code;
  const fallback =
    'if (loader == null) { const resolvedLang = { name: lang, data: [{ name: lang, scopeName: "text.plain." + lang, patterns: [], repository: {} }] }; if (!ResolvedLanguages.has(lang)) ResolvedLanguages.set(lang, resolvedLang); return resolvedLang; }';
  return code.replace(target, fallback);
}

/**
 * Patches `@pierre/diffs/dist/utils/getFiletypeFromFileName.js` so extensions mapped to pruned
 * grammars resolve directly to `"text"` (plaintext) and `.h`/`.svg` map cleanly to bundled grammars.
 */
export function patchFiletypeSource(code: string): string {
  const anchor = 'function getFiletypeFromFileName(fileName) {';
  if (!code.includes(anchor)) return code;
  const keptList = JSON.stringify([...BUNDLED_SHIKI_LANGUAGE_NAMES]);
  const overrides = JSON.stringify(EXTENSION_FORMAT_OVERRIDES);
  const patch = `
const __BUNDLED_LANGS__ = new Set(${keptList});
const __FORMAT_OVERRIDES__ = ${overrides};
for (const __ext__ of Object.keys(EXTENSION_TO_FILE_FORMAT)) {
  if (Object.hasOwn(__FORMAT_OVERRIDES__, __ext__)) EXTENSION_TO_FILE_FORMAT[__ext__] = __FORMAT_OVERRIDES__[__ext__];
  else if (!__BUNDLED_LANGS__.has(EXTENSION_TO_FILE_FORMAT[__ext__])) EXTENSION_TO_FILE_FORMAT[__ext__] = "text";
}
for (const [__ext__, __lang__] of Object.entries(__FORMAT_OVERRIDES__)) {
  if (!Object.hasOwn(EXTENSION_TO_FILE_FORMAT, __ext__)) EXTENSION_TO_FILE_FORMAT[__ext__] = __lang__;
}
`;
  return code.replace(anchor, patch + anchor);
}
