// Pure model types, validation rules, grouping helpers, and line-diff calculation for Unified Agent Context & MCP Hub (`MC-10`).

export type AgentProviderId = 'claude' | 'codex' | 'gemini';
export type InstructionFileName = 'CLAUDE.md' | 'AGENTS.md' | 'GEMINI.md';
export type AgentContextTab = 'instructions' | 'mcp';
export type McpTransport = 'http' | 'stdio';
export type McpScope = 'user' | 'project';

export interface InstructionFileItem {
  provider: AgentProviderId | string;
  name: InstructionFileName | string;
  exists: boolean;
  text: string;
  byteLen: number;
}

export interface BuiltinMcpServer {
  name: string;
  active: boolean;
  providers: string[];
  toolCount: number;
}

export interface ExternalMcpServer {
  provider: AgentProviderId | string;
  name: string;
  scope: McpScope | string;
  transport: McpTransport | string;
  target: string;
  url?: string;
  command?: string;
  args?: string[];
  bearerTokenEnvVar?: string;
  source: string;
  oauthConnected: boolean;
}

export interface ClaudeHookItem {
  event: string;
  commands: string[];
  scope: 'local' | 'project' | 'user' | string;
  source: string;
}

export interface AgentContextSnapshot {
  workspacePath: string;
  diverged: boolean;
  files: InstructionFileItem[];
  builtinMcpServers: BuiltinMcpServer[];
  externalMcpServers: ExternalMcpServer[];
  claudeHooks: ClaudeHookItem[];
  savedToToolkit?: boolean;
}

export interface GroupedExternalMcpServer {
  key: string;
  name: string;
  scope: McpScope;
  transport: McpTransport;
  target: string;
  url?: string;
  command?: string;
  args: string[];
  bearerTokenEnvVar?: string;
  providers: AgentProviderId[];
  sources: string[];
  oauthConnected: boolean;
}

export interface DiffLine {
  kind: 'same' | 'remove' | 'add';
  leftLine: number | null;
  rightLine: number | null;
  text: string;
}

export interface McpUpsertFormInput {
  name: string;
  transport: McpTransport;
  url: string;
  bearerTokenEnvVar: string;
  command: string;
  argsText: string;
  scope: McpScope;
  providers: readonly AgentProviderId[];
  saveToToolkit: boolean;
}

export const ALL_AGENT_PROVIDERS: readonly AgentProviderId[] = ['claude', 'codex', 'gemini'];

export const PROVIDER_DISPLAY_NAMES: Readonly<Record<AgentProviderId, string>> = {
  claude: 'Claude Code',
  codex: 'Codex',
  gemini: 'Antigravity',
};

export const RESERVED_MCP_NAMES = new Set(['mighty-terminal', 'mighty-delegation', 'mighty-workspace']);

const MCP_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const ENV_VAR_PATTERN = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;

/** Compares trimmed non-empty contents of existing instruction files (matching Swift 0.4 `AgentContext.diverged`). */
export function computeFilesDiverged(files: readonly InstructionFileItem[]): boolean {
  const unique = new Set<string>();
  for (const file of files) {
    if (!file.exists) continue;
    const trimmed = (file.text ?? '').trim();
    if (trimmed.length > 0) {
      unique.add(trimmed);
    }
  }
  return unique.size > 1;
}

/** Formats a byte count into a compact human-readable label (`"0 B"`, `"1.4 KB"`). */
export function formatByteCount(bytes: number): string {
  if (bytes <= 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

/** Splits an argument string into argv tokens while honoring single and double quotes. */
export function parseArgsInput(raw: string): string[] {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return [];
  const args: string[] = [];
  let current = '';
  let quote: '"' | "'" | null = null;
  let tokenStarted = false;

  for (let i = 0; i < trimmed.length; i += 1) {
    const ch = trimmed[i]!;
    if (quote) {
      if (ch === '\\' && quote === '"' && i + 1 < trimmed.length) {
        i += 1;
        current += trimmed[i]!;
      } else if (ch === quote) {
        quote = null;
      } else {
        current += ch;
      }
      tokenStarted = true;
      continue;
    }
    if (ch === '"' || ch === "'") {
      quote = ch;
      tokenStarted = true;
      continue;
    }
    if (/\s/.test(ch)) {
      if (tokenStarted) {
        args.push(current);
        current = '';
        tokenStarted = false;
      }
      continue;
    }
    current += ch;
    tokenStarted = true;
  }
  if (tokenStarted) {
    args.push(current);
  }
  return args;
}

/** Validates whether an HTTP MCP endpoint URL uses `https://` or loopback `http://`. */
export function isValidMcpHttpUrl(raw: string): boolean {
  const trimmed = raw.trim();
  if (trimmed.length === 0 || trimmed.length > 2048 || /\s/.test(trimmed)) return false;
  try {
    const parsed = new URL(trimmed);
    if (!parsed.hostname) return false;
    if (parsed.protocol === 'https:') return true;
    if (parsed.protocol === 'http:') {
      return (
        parsed.hostname === '127.0.0.1' ||
        parsed.hostname === 'localhost' ||
        parsed.hostname === '[::1]' ||
        parsed.hostname === '::1'
      );
    }
    return false;
  } catch {
    return false;
  }
}

/** Validates an MCP server identifier (`^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`, excluding reserved `mighty-*` servers). */
export function isValidExternalMcpName(raw: string): boolean {
  const trimmed = raw.trim();
  if (RESERVED_MCP_NAMES.has(trimmed.toLowerCase())) return false;
  return MCP_NAME_PATTERN.test(trimmed);
}

/** Validates an optional Bearer token environment variable name (`^[A-Za-z_][A-Za-z0-9_]{0,127}$`). */
export function isValidBearerEnvVar(raw: string): boolean {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return true;
  return ENV_VAR_PATTERN.test(trimmed);
}

/** Validates the multi-CLI MCP registration form and returns whether it is ready to submit. */
export function canSubmitMcpUpsert(form: McpUpsertFormInput): boolean {
  if (!isValidExternalMcpName(form.name)) return false;
  if (form.providers.length === 0) return false;
  if (form.transport === 'http') {
    return isValidMcpHttpUrl(form.url) && isValidBearerEnvVar(form.bearerTokenEnvVar);
  }
  const cmd = form.command.trim();
  if (cmd.length === 0 || cmd.length > 4096) return false;
  // A `..` path component is refused (the host refuses it too); a name that merely contains two dots (`tool..v2`) is fine.
  if (/[;|&`\r\n\0]/.test(cmd) || cmd.split(/[\\/]/).includes('..')) return false;
  return parseArgsInput(form.argsText).length <= 64;
}

/** Groups scanned external MCP server entries by `(scope, name)` across Claude, Codex, and Antigravity. */
export function groupExternalMcpServers(servers: readonly ExternalMcpServer[]): GroupedExternalMcpServer[] {
  const map = new Map<string, GroupedExternalMcpServer>();

  for (const item of servers) {
    const normalizedScope: McpScope = item.scope === 'project' ? 'project' : 'user';
    const key = `${normalizedScope}:${item.name}`;
    const provider = (
      item.provider === 'claude' || item.provider === 'codex' || item.provider === 'gemini'
        ? item.provider
        : 'claude'
    ) as AgentProviderId;
    const transport: McpTransport = item.transport === 'http' || Boolean(item.url) ? 'http' : 'stdio';

    const existing = map.get(key);
    if (!existing) {
      map.set(key, {
        key,
        name: item.name,
        scope: normalizedScope,
        transport,
        target: item.target,
        url: item.url,
        command: item.command,
        args: item.args ? [...item.args] : [],
        bearerTokenEnvVar: item.bearerTokenEnvVar,
        providers: [provider],
        sources: [item.source],
        oauthConnected: Boolean(item.oauthConnected),
      });
    } else {
      if (!existing.providers.includes(provider)) {
        existing.providers.push(provider);
      }
      if (!existing.sources.includes(item.source)) {
        existing.sources.push(item.source);
      }
      if (!existing.url && item.url) existing.url = item.url;
      if (!existing.command && item.command) existing.command = item.command;
      if (!existing.bearerTokenEnvVar && item.bearerTokenEnvVar) {
        existing.bearerTokenEnvVar = item.bearerTokenEnvVar;
      }
      if (item.oauthConnected) existing.oauthConnected = true;
    }
  }

  const orderRank = (p: AgentProviderId) => ALL_AGENT_PROVIDERS.indexOf(p);
  const grouped = Array.from(map.values());
  for (const row of grouped) {
    row.providers.sort((a, b) => orderRank(a) - orderRank(b));
  }
  grouped.sort((a, b) => {
    if (a.scope !== b.scope) return a.scope === 'project' ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  return grouped;
}

/** Computes a bounded LCS line diff between two instruction files (`left` vs `right`). */
export function computeLineDiff(leftText: string, rightText: string): DiffLine[] {
  const normLeft = leftText.replace(/\r\n/g, '\n');
  const normRight = rightText.replace(/\r\n/g, '\n');
  const leftLines = normLeft.length === 0 ? [] : normLeft.split('\n');
  const rightLines = normRight.length === 0 ? [] : normRight.split('\n');

  const n = leftLines.length;
  const m = rightLines.length;
  if (n === 0 && m === 0) return [];

  // Fallback to prefix/suffix + replacement if product exceeds matrix budget.
  if (n * m > 160_000) {
    const fallback: DiffLine[] = [];
    for (let i = 0; i < n; i += 1) {
      fallback.push({ kind: 'remove', leftLine: i + 1, rightLine: null, text: leftLines[i]! });
    }
    for (let j = 0; j < m; j += 1) {
      fallback.push({ kind: 'add', leftLine: null, rightLine: j + 1, text: rightLines[j]! });
    }
    return fallback;
  }

  const cols = m + 1;
  const dp = new Int32Array((n + 1) * cols);
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      if (leftLines[i] === rightLines[j]) {
        dp[i * cols + j] = dp[(i + 1) * cols + (j + 1)]! + 1;
      } else {
        const down = dp[(i + 1) * cols + j]!;
        const right = dp[i * cols + (j + 1)]!;
        dp[i * cols + j] = down >= right ? down : right;
      }
    }
  }

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (leftLines[i] === rightLines[j]) {
      out.push({ kind: 'same', leftLine: i + 1, rightLine: j + 1, text: leftLines[i]! });
      i += 1;
      j += 1;
    } else if (dp[(i + 1) * cols + j]! >= dp[i * cols + (j + 1)]!) {
      out.push({ kind: 'remove', leftLine: i + 1, rightLine: null, text: leftLines[i]! });
      i += 1;
    } else {
      out.push({ kind: 'add', leftLine: null, rightLine: j + 1, text: rightLines[j]! });
      j += 1;
    }
  }
  while (i < n) {
    out.push({ kind: 'remove', leftLine: i + 1, rightLine: null, text: leftLines[i]! });
    i += 1;
  }
  while (j < m) {
    out.push({ kind: 'add', leftLine: null, rightLine: j + 1, text: rightLines[j]! });
    j += 1;
  }
  return out;
}

/**
 * What each instruction file held when the panel read it, by name (null: it did not exist): sent with a save or a sync so the host
 * refuses to write over a change made on disk since (an agent appending to CLAUDE.md while the panel is open).
 */
export function expectedTexts(files: readonly InstructionFileItem[]): Record<string, string | null> {
  const expected: Record<string, string | null> = {};
  for (const file of files) expected[file.name] = file.exists ? file.text ?? '' : null;
  return expected;
}

/** The other instruction files a sync of `text` from `source` would overwrite: those that exist and hold something else. */
export function overwrittenBySync(files: readonly InstructionFileItem[], source: string, text: string): string[] {
  return files.filter((file) => file.name !== source && file.exists && (file.text ?? '').trim() !== text.trim()).map((file) => file.name);
}

