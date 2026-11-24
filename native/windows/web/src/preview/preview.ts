// MC-7: the live preview of a worktree's dev server (src-tauri preview/): what the host pushes of a pane's preview (`preview`), the
// servers `previewTargets` finds, and the words of the pane header's menu and the strip over the composer. Pure.
import type { Translate } from '../i18n/locale';

/** A pane's open preview as the host pushed it. */
export interface PreviewInfo {
  url: string;
  title: string;
  browser: string;
  picking: boolean;
  picked: { tag: string; selector: string } | null;
  consoleErrors: number;
  failedRequests: number;
}

/** The host's push (and `previewState`'s answer): the pane's preview (null when none is open) and the last thing said on it. */
export interface PreviewMessage {
  kind: 'preview';
  paneId: string;
  preview: PreviewInfo | null;
  note: string | null;
}

export interface PreviewPane {
  preview: PreviewInfo | null;
  note: string | null;
}

/** A dev server listening in a place of the workspace: the pane its preview belongs to, its worktree's branch (null: the main checkout). */
export interface PreviewServer {
  paneId: string;
  branch: string | null;
  url: string;
  port: number;
  action: string | null;
  open: boolean;
}

/** `previewTargets`' answer. */
export interface PreviewTargets {
  available: boolean;
  reason?: string;
  /** The browser the preview drives; null: none installed (the address opens in the default browser, with no picking). */
  browser?: string | null;
  servers?: PreviewServer[];
  /** Ports a pane's processes listen on outside its ports: listed, never opened. */
  outside?: { paneId: string; branch: string | null; port: number }[];
}

export function applyPreview(message: PreviewMessage): PreviewPane | undefined {
  return message.preview || message.note ? { preview: message.preview ?? null, note: message.note ?? null } : undefined;
}

/** The record without `paneId` when the message clears it, else with it set. */
export function reducePreview(
  record: Readonly<Record<string, PreviewPane>> | undefined,
  message: PreviewMessage,
): Readonly<Record<string, PreviewPane>> | undefined {
  const next = applyPreview(message);
  if (next) return { ...record, [message.paneId]: next };
  if (!record || !(message.paneId in record)) return record;
  const { [message.paneId]: _gone, ...rest } = record;
  return rest;
}

/** One row of the menu: its words, what pressing it opens (null: a line that says something), whether it is a heading. */
export interface PreviewMenuRow {
  label: string;
  open: { paneId: string; url: string; external: boolean } | null;
  disabled?: boolean;
  separator?: boolean;
}

function place(t: Translate, branch: string | null): string {
  return branch ?? t('preview.menu.main');
}

/** The pane header's menu: each server (its place, its run action, its address), 모두 열기 when more than one pane has one, and why nothing is there. */
export function previewMenu(t: Translate, targets: PreviewTargets): PreviewMenuRow[] {
  if (!targets.available) return [{ label: targets.reason ?? t('preview.error.localOnly'), open: null, disabled: true }];
  const external = !targets.browser;
  const servers = targets.servers ?? [];
  const rows: PreviewMenuRow[] = [];
  if (external && servers.length > 0) rows.push({ label: t('preview.menu.external'), open: null, disabled: true });
  for (const server of servers) {
    const address = server.url.replace(/^https?:\/\//, '').replace(/\/$/, '');
    const label = server.action
      ? t('preview.menu.action', { place: place(t, server.branch), action: server.action, address })
      : t('preview.menu.server', { place: place(t, server.branch), address });
    rows.push({ label: (server.open ? '● ' : '') + label, open: { paneId: server.paneId, url: server.url, external } });
  }
  if (servers.length === 0) rows.push({ label: t('preview.menu.none'), open: null, disabled: true });
  const first = firstPerPane(servers);
  if (first.length > 1) {
    rows.push({ label: '', open: null, separator: true });
    rows.push({ label: t('preview.menu.openAll', { count: String(first.length) }), open: null });
  }
  const outside = targets.outside ?? [];
  if (outside.length > 0) {
    rows.push({ label: '', open: null, separator: true });
    for (const port of outside) rows.push({ label: t('preview.menu.outside', { place: place(t, port.branch), port: String(port.port) }), open: null, disabled: true });
  }
  return rows;
}

/** 모두 열기: the first server of each pane (a pane has one preview). */
export function firstPerPane(servers: readonly PreviewServer[]): PreviewServer[] {
  const seen = new Set<string>();
  return servers.filter((server) => (seen.has(server.paneId) ? false : (seen.add(server.paneId), true)));
}

/** The strip's address: host and path, without the scheme. */
export function shortAddress(url: string): string {
  return url.replace(/^https?:\/\//, '');
}
