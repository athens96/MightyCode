import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ServicesContext, type AppServices } from '../app/context';
import { translator } from '../i18n/locale';
import { emptyState, Store } from '../store/store';
import { AgentContextPanel } from './AgentContextPanel';
import panelCssSource from './AgentContextPanel.css?raw';
import panelTsxSource from './AgentContextPanel.tsx?raw';
import modelTsSource from './agentContextModel.ts?raw';
import {
  canSubmitMcpUpsert,
  computeFilesDiverged,
  computeLineDiff,
  groupExternalMcpServers,
  isValidBearerEnvVar,
  isValidExternalMcpName,
  isValidMcpHttpUrl,
  parseArgsInput,
  type ExternalMcpServer,
  type InstructionFileItem,
} from './agentContextModel';

const testTranslator = translator([
  {
    'dialog.close': '닫기',
    'dialog.resize': '크기 조절',
    'dialog.loading': '불러오는 중…',
    'savedPrompts.save': '저장',
    'agentContext.open': '에이전트 맥락',
    'agentContext.title': '에이전트 맥락',
    'agentContext.missing': '없음',
    'agentContext.diverged': '에이전트마다 지침 파일 내용이 다릅니다.',
    'agentContext.copy.menu': '다른 파일에 복사',
    'agentContext.copy.to': '{target}에 덮어쓰기',
    'agentContext.copy.confirm': '{source}의 내용으로 {target}을 덮어쓸까요?',
    'agentContext.copy.overwrite': '덮어쓰기',
    'agentContext.copy.help': '지금 파일 내용으로 다른 에이전트의 지침 파일을 덮어씁니다.',
    'agentContext.copy.saveFirst': '먼저 저장하세요.',
    'agentContext.revert': '되돌리기',
    'agentContext.servers': 'MCP 서버 {count}개',
    'agentContext.hooks': 'Claude hook {count}개',
    'agentContext.none': '없음',
    'agentContext.scope.project': '이 프로젝트',
    'agentContext.scope.user': '사용자 전체',
    'agentContext.readOnly': '서버와 hook은 각 CLI의 설정 파일에서 읽기만 합니다.',
    'agentContext.tab.instructions': '지침 파일',
    'agentContext.tab.mcp': 'MCP 허브 & Hook',
    'agentContext.syncAll': '모두 동기화',
    'agentContext.synced': '지침 파일을 동기화했습니다.',
    'agentContext.saved': '{name} 파일을 저장했습니다.',
    'agentContext.compareWith': '{name} 기준 비교 보기',
    'agentContext.builtinTitle': '기본 내장 MCP 서버',
    'agentContext.builtinNote': 'Mighty Code가 실행 시 자동으로 연결하는 시스템 서버입니다.',
    'agentContext.builtinActive': '활성',
    'agentContext.builtinInactive': '비활성',
    'agentContext.externalTitle': '외부 MCP 서버',
    'agentContext.addTitle': '멀티 CLI MCP 서버 등록',
    'agentContext.transport.http': 'HTTP',
    'agentContext.transport.stdio': 'stdio',
    'agentContext.field.name': '서버 이름',
    'agentContext.field.url': '서버 URL',
    'agentContext.field.command': '실행 명령',
    'agentContext.field.args': '인자',
    'agentContext.field.bearerEnv': 'Bearer 토큰 환경변수',
    'agentContext.targets': '대상 CLI',
    'agentContext.registerButton': '원클릭 등록',
    'agentContext.oauthLogin': 'OAuth 로그인',
    'agentContext.oauthConnected': '인증됨',
    'agentContext.saveToolkit': '내 작업 도구 모음(toolkit.json)에도 저장',
    'agentContext.savedToolkit': '설정을 등록하고 도구 모음에 저장했습니다.',
    'agentContext.removeServer': '제거',
  },
]);

describe('agentContextModel', () => {
  it('computes instruction file divergence across CLAUDE.md, AGENTS.md, and GEMINI.md', () => {
    const emptyFiles: InstructionFileItem[] = [
      { provider: 'claude', name: 'CLAUDE.md', exists: false, text: '', byteLen: 0 },
      { provider: 'codex', name: 'AGENTS.md', exists: false, text: '', byteLen: 0 },
      { provider: 'gemini', name: 'GEMINI.md', exists: false, text: '', byteLen: 0 },
    ];
    expect(computeFilesDiverged(emptyFiles)).toBe(false);

    const singleFile: InstructionFileItem[] = [
      { provider: 'claude', name: 'CLAUDE.md', exists: true, text: '# Shared Rules\n', byteLen: 15 },
      { provider: 'codex', name: 'AGENTS.md', exists: false, text: '', byteLen: 0 },
      { provider: 'gemini', name: 'GEMINI.md', exists: false, text: '', byteLen: 0 },
    ];
    expect(computeFilesDiverged(singleFile)).toBe(false);

    const matchingTrimmed: InstructionFileItem[] = [
      { provider: 'claude', name: 'CLAUDE.md', exists: true, text: '# Shared Rules', byteLen: 14 },
      { provider: 'codex', name: 'AGENTS.md', exists: true, text: '# Shared Rules\n\n', byteLen: 16 },
      { provider: 'gemini', name: 'GEMINI.md', exists: false, text: '', byteLen: 0 },
    ];
    expect(computeFilesDiverged(matchingTrimmed)).toBe(false);

    const divergedFiles: InstructionFileItem[] = [
      { provider: 'claude', name: 'CLAUDE.md', exists: true, text: '# Claude Rules', byteLen: 14 },
      { provider: 'codex', name: 'AGENTS.md', exists: true, text: '# Codex Rules', byteLen: 13 },
      { provider: 'gemini', name: 'GEMINI.md', exists: false, text: '', byteLen: 0 },
    ];
    expect(computeFilesDiverged(divergedFiles)).toBe(true);
  });

  it('computes line-by-line LCS diff for side-by-side comparison', () => {
    const diff = computeLineDiff('# Title\n- Rule A\n- Common', '# Title\n- Rule B\n- Common');
    expect(diff.map((d) => `${d.kind}:${d.text}`)).toEqual([
      'same:# Title',
      'remove:- Rule A',
      'add:- Rule B',
      'same:- Common',
    ]);
  });

  it('validates MCP identifiers, reserved mighty-* names, HTTP URLs, Bearer env vars, and quoted args', () => {
    expect(isValidExternalMcpName('mightywork')).toBe(true);
    expect(isValidExternalMcpName('context7_v1.2-beta')).toBe(true);
    expect(isValidExternalMcpName('mighty-terminal')).toBe(false);
    expect(isValidExternalMcpName('mighty-delegation')).toBe(false);
    expect(isValidExternalMcpName('mighty-workspace')).toBe(false);
    expect(isValidExternalMcpName('-bad')).toBe(false);

    expect(isValidMcpHttpUrl('https://mightywork.duckdns.org/mcp')).toBe(true);
    expect(isValidMcpHttpUrl('http://127.0.0.1:8787/mcp')).toBe(true);
    expect(isValidMcpHttpUrl('http://localhost:3000/sse')).toBe(true);
    expect(isValidMcpHttpUrl('http://evil.example.com/mcp')).toBe(false);

    expect(isValidBearerEnvVar('')).toBe(true);
    expect(isValidBearerEnvVar('MIGHTYWORK_TOKEN')).toBe(true);
    expect(isValidBearerEnvVar('sk-live-secret')).toBe(false);

    expect(parseArgsInput('-y "@modelcontextprotocol/server-filesystem" "/My Path/repo"')).toEqual([
      '-y',
      '@modelcontextprotocol/server-filesystem',
      '/My Path/repo',
    ]);

    expect(
      canSubmitMcpUpsert({
        name: 'mightywork',
        transport: 'http',
        url: 'https://mightywork.duckdns.org/mcp',
        bearerTokenEnvVar: 'MW_TOKEN',
        command: '',
        argsText: '',
        scope: 'user',
        providers: ['claude', 'codex', 'gemini'],
        saveToToolkit: true,
      }),
    ).toBe(true);

    expect(
      canSubmitMcpUpsert({
        name: 'mighty-terminal',
        transport: 'http',
        url: 'https://mightywork.duckdns.org/mcp',
        bearerTokenEnvVar: '',
        command: '',
        argsText: '',
        scope: 'user',
        providers: ['claude'],
        saveToToolkit: false,
      }),
    ).toBe(false);
  });

  it('groups multi-CLI MCP entries by scope and server name in canonical provider order', () => {
    const raw: ExternalMcpServer[] = [
      {
        provider: 'gemini',
        name: 'mightywork',
        scope: 'user',
        transport: 'http',
        target: 'https://mightywork.duckdns.org/mcp',
        url: 'https://mightywork.duckdns.org/mcp',
        source: '~/.gemini/config/mcp_config.json',
        oauthConnected: false,
      },
      {
        provider: 'claude',
        name: 'mightywork',
        scope: 'user',
        transport: 'http',
        target: 'https://mightywork.duckdns.org/mcp',
        url: 'https://mightywork.duckdns.org/mcp',
        source: '~/.claude.json',
        oauthConnected: true,
      },
      {
        provider: 'codex',
        name: 'context7',
        scope: 'project',
        transport: 'stdio',
        target: 'npx -y @upstash/context7-mcp',
        command: 'npx',
        args: ['-y', '@upstash/context7-mcp'],
        source: '.codex/config.toml',
        oauthConnected: false,
      },
    ];

    const grouped = groupExternalMcpServers(raw);
    expect(grouped).toHaveLength(2);
    expect(grouped[0]?.name).toBe('context7');
    expect(grouped[0]?.scope).toBe('project');
    expect(grouped[1]?.name).toBe('mightywork');
    expect(grouped[1]?.providers).toEqual(['claude', 'gemini']);
    expect(grouped[1]?.oauthConnected).toBe(true);
  });
});

describe('AgentContextPanel rendering and design/locale guardrails', () => {
  it('renders nothing when closed and renders panel shell when open', () => {
    const store = new Store(emptyState);
    const services = {
      bridge: {
        call: <T = unknown>(): Promise<T> =>
          Promise.resolve({
            workspacePath: '/Users/dev/repo',
            diverged: false,
            files: [],
            builtinMcpServers: [],
            externalMcpServers: [],
            claudeHooks: [],
          } as T),
      },
      store,
      t: testTranslator,
    } as unknown as AppServices;

    const closedHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(AgentContextPanel, { open: false, onClose: () => {} }),
      ),
    );
    expect(closedHtml).toBe('');

    const openHtml = renderToStaticMarkup(
      createElement(
        ServicesContext.Provider,
        { value: services },
        createElement(AgentContextPanel, { open: true, workspaceId: 'ws-1', onClose: () => {} }),
      ),
    );
    expect(openHtml).toContain('data-testid="agent-context-panel"');
    expect(openHtml).toContain('에이전트 맥락');
    expect(openHtml).toContain('CLAUDE.md');
    expect(openHtml).toContain('AGENTS.md');
    expect(openHtml).toContain('GEMINI.md');
  });

  it('contains zero raw color literals and zero hardcoded Hangul in production context files', () => {
    const sources: Array<[string, string]> = [
      ['agentContextModel.ts', modelTsSource],
      ['AgentContextPanel.tsx', panelTsxSource],
      ['AgentContextPanel.css', panelCssSource],
    ];
    const rawColorPattern = /#[0-9a-fA-F]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\s*\(/;
    const hangulPattern = /[\uAC00-\uD7A3\u3131-\u318E]/;

    for (const [name, source] of sources) {
      expect(source, `${name} must not contain raw color literals`).not.toMatch(rawColorPattern);
      expect(source, `${name} must not contain hardcoded Hangul`).not.toMatch(hangulPattern);
    }
  });
});
