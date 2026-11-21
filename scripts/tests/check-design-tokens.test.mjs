// node --test scripts/tests/check-design-tokens.test.mjs
// Checks the spacing-literal, font-literal and web colour-literal ratchets of check-design-tokens.js against fixture trees.
// It never touches the real sources or budgets.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const CHECKER = new URL('../check-design-tokens.js', import.meta.url).pathname;
const NODE = process.execPath;
const SPACING_BUDGET = 'scripts/spacing-literal-budget.json';
const FONT_BUDGET = 'scripts/font-literal-budget.json';
const WEB_COLOUR_BUDGET = 'scripts/web-colour-budget.json';

function run(dir, ...args) {
  return spawnSync(NODE, [CHECKER, '--root', dir, ...args], { encoding: 'utf8' });
}

/** A fixture root with an empty colour budget, the given spacing, font and optional web-colour budgets and source files. */
function fixture({ areas = { macOS: 0, Windows: 0, phone: 0 }, exemptFiles, fonts = { files: {} }, webColours, files = {} } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'spacing-ck-'));
  const write = (relative, content) => {
    fs.mkdirSync(path.join(dir, path.dirname(relative)), { recursive: true });
    fs.writeFileSync(path.join(dir, relative), content);
  };
  write('scripts/design-token-budget.json', JSON.stringify({ exempt: {}, files: {} }));
  write(SPACING_BUDGET, JSON.stringify({ areas, ...(exemptFiles ? { exemptFiles } : {}) }));
  write(FONT_BUDGET, JSON.stringify(fonts));
  if (webColours !== undefined) write(WEB_COLOUR_BUDGET, JSON.stringify(webColours));
  for (const [relative, content] of Object.entries(files)) write(relative, content);
  return dir;
}

function areas(dir) {
  return JSON.parse(fs.readFileSync(path.join(dir, SPACING_BUDGET), 'utf8')).areas;
}

const MAC = 'native/macos/Sources/MightyCode/View.swift';
const WIN = 'native/windows/MightyCode.WinUI/MainWindow.View.cs';
const PHONE = 'mobile/src/components/view.tsx';
const WEB_CSS = 'native/windows/web/src/shell/shell.css';
const WEB_TSX = 'native/windows/web/src/shell/Chrome.tsx';

test('a count above the budget fails', () => {
  const dir = fixture({
    areas: { macOS: 1, Windows: 0, phone: 0 },
    files: { [MAC]: 'VStack(spacing: 4) { Text("a").padding(8) }\n' },
  });
  try {
    const r = run(dir);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /macOS: 2 raw spacing literals, budget 1/);
    assert.match(r.stderr, /View\.swift/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('a count below the budget fails without --write-budget, and --write-budget lowers it', () => {
  const dir = fixture({
    areas: { macOS: 0, Windows: 5, phone: 3 },
    files: {
      [WIN]: 'var a = new Thickness(8, 0, 4, 0); var b = new StackPanel { Spacing = 6 };\n',
      [PHONE]: 'const s = { paddingLeft: 12 };\n',
    },
  });
  try {
    const stale = run(dir);
    assert.notEqual(stale.status, 0);
    assert.match(stale.stderr, /Windows: 3 raw spacing literals, budget 5\. Lower the budget/);
    assert.match(stale.stderr, /phone: 1 raw spacing literals, budget 3/);
    assert.deepEqual(areas(dir), { macOS: 0, Windows: 5, phone: 3 }, 'a plain check never writes');

    const write = run(dir, '--write-budget');
    assert.equal(write.status, 0, write.stderr);
    assert.deepEqual(areas(dir), { macOS: 0, Windows: 3, phone: 1 });
    assert.equal(run(dir).status, 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('--write-budget never raises a budget', () => {
  const dir = fixture({ files: { [PHONE]: 'const s = { gap: 8, marginTop: -4 };\n' } });
  try {
    const r = run(dir, '--write-budget');
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /phone: 2 raw spacing literals, budget 0/);
    assert.deepEqual(areas(dir), { macOS: 0, Windows: 0, phone: 0 });
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('exempt regions are not counted', () => {
  const source = [
    'HStack(spacing: 6) {',
    '    // spacing-exempt-begin: halo centring',
    '    Circle().padding(.horizontal, -5)',
    '    Circle().padding(12)',
    '    // spacing-exempt-end',
    '}',
    '',
  ].join('\n');
  const dir = fixture({ areas: { macOS: 1, Windows: 0, phone: 0 }, files: { [MAC]: source } });
  try {
    const r = run(dir);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /macOS +1 \/ budget 1 in 1 files \(exempt: 2 literals, 1 regions\)/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('an exempt region needs a reason and an end', () => {
  const dir = fixture({
    files: {
      [MAC]: '// spacing-exempt-begin\nText("a").padding(4)\n// spacing-exempt-end\n',
      [PHONE]: '// spacing-exempt-begin: drawn shape\nconst s = { padding: 4 };\n',
    },
  });
  try {
    const r = run(dir);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /View\.swift: \/\/ spacing-exempt-begin needs ": <reason>"/);
    assert.match(r.stderr, /view\.tsx: \/\/ spacing-exempt-begin is never closed/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('0, 1, tokens, comments, tests and exempt files are not counted', () => {
  const dir = fixture({
    exemptFiles: { 'native/macos/Sources/MightyCode/*Diagnostics*.swift': 'smoke fixtures' },
    files: {
      [MAC]: [
        'VStack(spacing: 0) { Text("a").padding(.top, 1).padding(DesignMetrics.Spacing.md) }',
        '// .padding(12) in a comment',
        'HStack(spacing: DesignMetrics.Spacing.sm) {}',
        '',
      ].join('\n'),
      'native/macos/Sources/MightyCode/LayoutDiagnostics.swift': 'Text("a").padding(20)\n',
      'native/macos/Tests/MightyCoreTests/ViewTests.swift': 'Text("a").padding(20)\n',
      [WIN]: 'var a = new Thickness(0, 1, DesignMetrics.Spacing.Md, 0); const double Spacing = 18;\n',
      'mobile/src/__tests__/view.test.tsx': 'const s = { padding: 20 };\n',
    },
  });
  try {
    const r = run(dir);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /macOS +0 \/ budget 0 in 0 files \(exempt: 1 literals, 0 regions\)/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('an exempt-file glob that matches nothing fails', () => {
  const dir = fixture({ exemptFiles: { 'mobile/src/lib/gone.ts': 'removed long ago' } });
  try {
    const r = run(dir);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /exemptFiles mobile\/src\/lib\/gone\.ts matches no file/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('a WinUI font size, line height or family set directly fails; TextSize, TextLineHeight and TextFont do not', () => {
  const dir = fixture({
    files: {
      [WIN]: [
        'var a = new TextBlock { TextSize = 12, TextLineHeight = 15, TextFont = TextArea.Code };',
        'var b = new TextBlock { FontSize = 12, LineHeight = 15, FontFamily = new FontFamily("Consolas") };',
        'b.FontSize += 2; b.SetValue(TextBlock.LineHeightProperty, 20);',
        '// FontSize = 13 in a comment is not counted, and neither is a comparison:',
        'if (b.FontSize == 12) { }',
      ].join('\n') + '\n',
    },
  });
  try {
    const r = run(dir);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /MainWindow\.View\.cs: 6 font literals, budget 0\. Give the size at scale 1 as TextSize/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('a font budget only goes down, and its exempt globs must match', () => {
  const dir = fixture({
    fonts: { exemptFiles: { 'native/windows/MightyCode.WinUI/*Smoke*.cs': 'fixtures' }, files: { [WIN]: 3 } },
    files: {
      [WIN]: 'var b = new TextBlock { FontSize = 12 };\n',
      'native/windows/MightyCode.WinUI/MainWindow.ViewSmoke.cs': 'var c = new TextBlock { FontSize = 9 };\n',
    },
  });
  try {
    const stale = run(dir);
    assert.notEqual(stale.status, 0);
    assert.match(stale.stderr, /MainWindow\.View\.cs: 1 font literals, budget 3\. Lower the budget/);
    const write = run(dir, '--write');
    assert.equal(write.status, 0, write.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(dir, FONT_BUDGET), 'utf8')).files, { [WIN]: 1 });
    assert.equal(run(dir).status, 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  const empty = fixture({ fonts: { exemptFiles: { 'native/windows/MightyCode.WinUI/Gone.cs': 'removed' }, files: {} } });
  try {
    const r = run(empty);
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /exemptFiles native\/windows\/MightyCode\.WinUI\/Gone\.cs matches no file/);
  } finally {
    fs.rmSync(empty, { recursive: true, force: true });
  }
});

test('a hardcoded #ff0000 in a web .css file fails --check', () => {
  const dir = fixture({
    webColours: { files: {} },
    files: {
      [WEB_CSS]: '.banner { color: #ff0000; background: var(--card); }\n',
    },
  });
  try {
    const r = run(dir, '--check');
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /shell\.css: 1 web colour literals, budget 0\. Use a design-token CSS variable/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('a hardcoded #ff0000, rgb() or hsl() in a web .tsx file fails --check', () => {
  const dir = fixture({
    webColours: { files: {} },
    files: {
      [WEB_TSX]: [
        'export function Chrome() {',
        "  return <div style={{ color: '#ff0000', borderColor: 'rgb(10, 20, 30)', background: `hsl(0, 100%, 50%)` }} />;",
        '}',
        '',
      ].join('\n'),
    },
  });
  try {
    const r = run(dir, '--check');
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /Chrome\.tsx: 3 web colour literals, budget 0\. Use a design-token CSS variable/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('web colour ratchet exempts neutral box-shadow black alpha, comments, ID selectors, tests and exempt globs, but catches coloured shadows and non-shadow rgba', () => {
  const clean = fixture({
    webColours: {
      exemptFiles: {
        'native/windows/web/src/theme/**': 'token source',
        'native/windows/web/src/icons/**': 'icons',
      },
      files: {},
    },
    files: {
      [WEB_CSS]: [
        '/* #ff0000 rgba(255, 0, 0, .5) in a comment */',
        '#root, #add-btn { background: var(--page); color: var(--ink); }',
        '.card { box-shadow: 0 1px 2px rgba(0, 0, 0, .12), inset 0 0 0 1px rgb(0 0 0 / 8%); }',
        '.mix { border-color: color-mix(in srgb, var(--accent) 80%, transparent); }',
        '',
      ].join('\n'),
      [WEB_TSX]: [
        '// #ff0000 in a line comment',
        '/* rgba(255, 0, 0, 0.5) in a block comment */',
        "export const tone = 'var(--accent)';",
        '',
      ].join('\n'),
      'native/windows/web/src/theme/tokens.ts': "export const backdrop = 'rgba(0,0,0,0.4)';\n",
      'native/windows/web/src/icons/Icon.tsx': "export const stroke = '#000';\n",
      'native/windows/web/src/shell/shell.test.ts': "export const sample = '#ff0000';\n",
    },
  });
  try {
    const r = run(clean, '--check');
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /colour literals \(web\): 0 in 0 files \(budget 0, exempt 2 globs\)/);
  } finally {
    fs.rmSync(clean, { recursive: true, force: true });
  }

  const badShadowAndBackdrop = fixture({
    webColours: { files: {} },
    files: {
      [WEB_CSS]: [
        '.glow { box-shadow: 0 2px 8px rgba(255, 0, 0, .2); }',
        '.scrim { background: rgba(0, 0, 0, .5); }',
        '',
      ].join('\n'),
    },
  });
  try {
    const r = run(badShadowAndBackdrop, '--check');
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /shell\.css: 2 web colour literals, budget 0/);
  } finally {
    fs.rmSync(badShadowAndBackdrop, { recursive: true, force: true });
  }
});

test('web colour budget only goes down, --write never raises it, and exempt globs must match', () => {
  const dir = fixture({
    webColours: {
      exemptFiles: { 'native/windows/web/src/theme/**': 'token source' },
      files: { [WEB_CSS]: 3 },
    },
    files: {
      [WEB_CSS]: '.a { color: #3fb950; }\n',
      'native/windows/web/src/theme/tokens.ts': "export const x = '#ffffff';\n",
    },
  });
  try {
    const stale = run(dir, '--check');
    assert.notEqual(stale.status, 0);
    assert.match(stale.stderr, /shell\.css: 1 web colour literals, budget 3\. Lower the budget/);

    const write = run(dir, '--write');
    assert.equal(write.status, 0, write.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(dir, WEB_COLOUR_BUDGET), 'utf8')).files, { [WEB_CSS]: 1 });
    assert.equal(run(dir, '--check').status, 0);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }

  const over = fixture({
    webColours: { files: { [WEB_CSS]: 1 } },
    files: { [WEB_CSS]: '.a { color: #3fb950; background: #ff0000; }\n' },
  });
  try {
    const r = run(over, '--write');
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /shell\.css: 2 web colour literals, budget 1/);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(over, WEB_COLOUR_BUDGET), 'utf8')).files, { [WEB_CSS]: 1 });
  } finally {
    fs.rmSync(over, { recursive: true, force: true });
  }

  const unmatched = fixture({
    webColours: { exemptFiles: { 'native/windows/web/src/gone/**': 'removed' }, files: {} },
  });
  try {
    const r = run(unmatched, '--check');
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /exemptFiles native\/windows\/web\/src\/gone\/\*\* matches no file/);
  } finally {
    fs.rmSync(unmatched, { recursive: true, force: true });
  }
});
