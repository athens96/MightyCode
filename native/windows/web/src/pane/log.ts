// The pane menu's 실행 기록 복사 (M/SessionPaneView.swift paneMenu `pane.menu.copyLog`): the pane's log as plain text, each entry
// "[role] text", a blank line between them.
import type { LogEntry } from '../store/models';
import type { Translate } from '../i18n/locale';
import { providerLabel } from '../conversation/format';

/** An entry's role as the Mac's copy named it (SessionPaneView.swift `role(_:)`). */
function role(entry: LogEntry, provider: string, t: Translate): string {
  switch (entry.kind) {
    case 'user': return t('pane.log.roleUser');
    case 'assistant': return providerLabel(entry.provider ?? provider);
    case 'output': return t('composer.sessionInfo.output');
    case 'error': return t('session.state.error');
    default: return t('pane.log.roleSystem');
  }
}

export function logText(logs: readonly LogEntry[], provider: string, t: Translate): string {
  return logs.map((entry) => '[' + role(entry, provider, t) + '] ' + entry.text).join('\n\n');
}
