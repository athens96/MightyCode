/// <reference types="vitest/config" />
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import {
  generatePrunedShikiLangsModule,
  generatePrunedThemesModule,
  patchFiletypeSource,
  patchResolveLanguageSource,
} from './src/panes/files/shikiLanguages.ts';

const repo = (path: string) => fileURLToPath(new URL('../../../' + path, import.meta.url));

const VIRTUAL_SHIKI_LANGS = '\0mighty-shiki-langs';
const VIRTUAL_SHIKI_WASM = '\0mighty-shiki-wasm';
const VIRTUAL_SHIKI_ONIGURUMA = '\0mighty-shiki-oniguruma';
const VIRTUAL_PIERRE_THEMES = '\0mighty-pierre-themes';
const VIRTUAL_PIERRE_HIGHLIGHTS = '\0mighty-pierre-highlights';
const VIRTUAL_PIERRE_HIGHLIGHTS_THEMES = '\0mighty-pierre-highlights-themes';

/**
 * Prunes @pierre/diffs's unused Shiki themes (keeping only `pierre-dark` and `pierre-light`),
 * unused highlighter backends (`@pierre/highlights`, `shiki/wasm` / Oniguruma), and niche Shiki
 * grammars outside `BUNDLED_SHIKI_LANGUAGES`, while patching @pierre/diffs to fall back to
 * plaintext (`text`) gracefully for unbundled file types.
 */
function pruneShiki(): Plugin {
  return {
    name: 'mighty-prune-shiki',
    enforce: 'pre',
    resolveId(source) {
      switch (source) {
        case 'shiki/langs': return VIRTUAL_SHIKI_LANGS;
        case 'shiki/wasm': return VIRTUAL_SHIKI_WASM;
        case 'shiki/engine/oniguruma': return VIRTUAL_SHIKI_ONIGURUMA;
        case '@pierre/theming/themes': return VIRTUAL_PIERRE_THEMES;
        case '@pierre/highlights': return VIRTUAL_PIERRE_HIGHLIGHTS;
        case '@pierre/highlights/themes/loader': return VIRTUAL_PIERRE_HIGHLIGHTS_THEMES;
        default: return null;
      }
    },
    load(id) {
      switch (id) {
        case VIRTUAL_SHIKI_LANGS: return generatePrunedShikiLangsModule();
        case VIRTUAL_SHIKI_WASM: return 'export default undefined;';
        case VIRTUAL_SHIKI_ONIGURUMA:
          return 'export function createOnigurumaEngine() { throw new Error("Oniguruma WASM engine is not bundled"); }';
        case VIRTUAL_PIERRE_THEMES: return generatePrunedThemesModule();
        case VIRTUAL_PIERRE_HIGHLIGHTS:
          return 'export class LiveTokenizer {} export function codeToTokens() { throw new Error("Highlights backend is not bundled"); } export function isSupportedLanguage() { return false; }';
        case VIRTUAL_PIERRE_HIGHLIGHTS_THEMES: return 'export const themes = {};';
        default: return null;
      }
    },
    transform(code, id) {
      const normalized = id.replace(/\\/g, '/').split('?')[0] ?? '';
      if (normalized.endsWith('/@pierre/diffs/dist/highlighter/languages/resolveLanguage.js')) {
        const next = patchResolveLanguageSource(code);
        return next === code ? null : { code: next, map: null };
      }
      if (normalized.endsWith('/@pierre/diffs/dist/utils/getFiletypeFromFileName.js')) {
        const next = patchFiletypeSource(code);
        return next === code ? null : { code: next, map: null };
      }
      return null;
    },
  };
}

// The built page loads only its own files; the dev server needs React's inline refresh preamble, so the policy is a build-only tag.
// The Rust + Tauri app's build (`vite build --mode tauri`, native/tauri) also lets the page reach Tauri's IPC, which it fetches
// from http://ipc.localhost on Windows (ipc: elsewhere), and frames HTML previews from the host's `mightyhtml` protocol, which
// serves each folder under a host of its own (http://mightyhtml.h<id>.html.mighty.invalid on Windows, mightyhtml://h<id>… on
// the Mac) where C# maps it as a WebView2 virtual host on https; the C# app's build keeps the policy as it was.
function contentSecurityPolicy(tauri: boolean): Plugin {
  const ipc = tauri ? "; connect-src 'self' ipc: http://ipc.localhost" : '';
  const frames = tauri ? 'http://*.html.mighty.invalid mightyhtml:' : 'https://*.html.mighty.invalid';
  return {
    name: 'mighty-csp',
    apply: 'build',
    transformIndexHtml: () => [{
      tag: 'meta',
      attrs: { 'http-equiv': 'Content-Security-Policy', content: "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-src " + frames + "; object-src 'none'; base-uri 'none'; form-action 'none'" + ipc },
      injectTo: 'head-prepend',
    }],
  };
}

// The repo's locale files and design tokens are the single source: imported where they live, never copied.
// `base: './'` keeps every asset path relative, so the host can serve dist/ from any folder (WebShell.cs). The Tauri build goes
// to dist-tauri/, so it never replaces the dist/ the C# build ships as Assets\Web.
export default defineConfig(({ mode }) => ({
  base: './',
  // One tsconfig for every file, also the phone app's modules imported from mobile/src (whose own extends Expo's).
  tsconfig: './tsconfig.json',
  plugins: [react(), pruneShiki(), contentSecurityPolicy(mode === 'tauri')],
  resolve: {
    alias: {
      '@locales': repo('locales'),
      '@tokens': repo('native/contracts/fixtures'),
      // The shared parity vectors (graph-vectors.json), read by the Mighty diagram's tests.
      '@contracts': repo('native/contracts'),
      // Pure modules of the phone app (mobile/src/lib), which import each other as `@/…`.
      '@mobile': repo('mobile/src'),
      '@/': repo('mobile/src') + '/',
    },
  },
  server: { fs: { allow: [repo('')] } },
  build: { outDir: mode === 'tauri' ? 'dist-tauri' : 'dist', emptyOutDir: true, target: 'es2022', sourcemap: false },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
}));
