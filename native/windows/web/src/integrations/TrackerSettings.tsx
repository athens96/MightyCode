// MC-31: settings for the external issue trackers (Linear, Jira Cloud, GitLab) in the issue picker's settings drawer:
// turn one on, its team or project, its site and email, a token (kept only in the OS keychain), the default filter,
// the write-back switches, and connect / test / disconnect.
import { useEffect, useState } from 'react';
import { useServices } from '../app/context';
import type { Translate } from '../i18n/locale';
import type { WorkspaceIntegrationStatus } from './IssuePickerPanel';

export type ExternalTracker = 'linear' | 'jira' | 'gitlab';

export const EXTERNAL_TRACKERS: readonly ExternalTracker[] = ['linear', 'jira', 'gitlab'];

/** One external tracker's settings as `integrationsStatus` gives them: never a token, only whether one is kept. */
export interface TrackerStatus {
  tracker: ExternalTracker;
  name: string;
  enabled: boolean;
  connected: boolean;
  project: string;
  site: string;
  email: string;
  filterMine: boolean;
  filterOpen: boolean;
  writeStart: boolean;
  writePr: boolean;
  writeMerge: boolean;
  commentPr: boolean;
  defaultFilter?: 'mine' | 'open' | 'all';
  glab?: boolean;
  detectedProject?: string;
  detectedSite?: string;
}

export const TRACKER_NAMES: Record<ExternalTracker, string> = {
  linear: 'Linear',
  jira: 'Jira',
  gitlab: 'GitLab',
};

const PROJECT_LABEL_KEYS: Record<ExternalTracker, string> = {
  linear: 'integrations.tracker.linearTeam',
  jira: 'integrations.tracker.jiraProject',
  gitlab: 'integrations.tracker.gitlabProject',
};

const PROJECT_PLACEHOLDER_KEYS: Record<ExternalTracker, string> = {
  linear: 'integrations.tracker.linearTeamPlaceholder',
  jira: 'integrations.tracker.jiraProjectPlaceholder',
  gitlab: 'integrations.tracker.gitlabProjectPlaceholder',
};

const TOKEN_LABEL_KEYS: Record<ExternalTracker, string> = {
  linear: 'integrations.tracker.linearToken',
  jira: 'integrations.tracker.jiraToken',
  gitlab: 'integrations.tracker.gitlabToken',
};

/** The tracker's status from `integrationsStatus`, or an empty one (off, nothing kept). */
export function trackerStatus(status: WorkspaceIntegrationStatus | null, tracker: ExternalTracker): TrackerStatus {
  const found = status?.trackers?.find((entry) => entry.tracker === tracker);
  return (
    found ?? {
      tracker,
      name: TRACKER_NAMES[tracker],
      enabled: false,
      connected: false,
      project: '',
      site: '',
      email: '',
      filterMine: true,
      filterOpen: true,
      writeStart: false,
      writePr: false,
      writeMerge: false,
      commentPr: true,
    }
  );
}

/** The external trackers turned on for this workspace, in tab order. */
export function enabledTrackers(status: WorkspaceIntegrationStatus | null): ExternalTracker[] {
  return EXTERNAL_TRACKERS.filter((tracker) => trackerStatus(status, tracker).enabled);
}

/** The message for a failed connection test (`AUTH_REQUIRED` reads as the missing or refused token). */
export function testFailureText(error: string | undefined, t: Translate): string {
  const reason = !error || error === 'AUTH_REQUIRED' ? t('integrations.tracker.authRequired') : error;
  return t('integrations.tracker.testFailed', { reason });
}

interface TrackerSettingsProps {
  workspaceId: string | null;
  status: WorkspaceIntegrationStatus | null;
  onStatus: (next: WorkspaceIntegrationStatus) => void;
  onError: (message: string | null) => void;
}

export function TrackerSettings({ workspaceId, status, onStatus, onError }: TrackerSettingsProps) {
  const { bridge, t } = useServices();
  const [tracker, setTracker] = useState<ExternalTracker>('linear');
  const current = trackerStatus(status, tracker);
  const [form, setForm] = useState<TrackerStatus>(current);
  const [token, setToken] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    setForm(trackerStatus(status, tracker));
    setToken('');
  }, [status, tracker]);

  useEffect(() => {
    setResult(null);
  }, [tracker]);

  const set = <K extends keyof TrackerStatus>(key: K, value: TrackerStatus[K]) =>
    setForm((prev) => ({ ...prev, [key]: value }));

  const save = async (): Promise<boolean> => {
    const next = await bridge.call<WorkspaceIntegrationStatus>('integrationsTrackerSave', {
      workspaceId,
      tracker,
      enabled: form.enabled,
      project: form.project.trim(),
      site: form.site.trim(),
      email: form.email.trim(),
      token: token.trim(),
      filterMine: form.filterMine,
      filterOpen: form.filterOpen,
      writeStart: form.writeStart,
      writePr: form.writePr,
      writeMerge: form.writeMerge,
      commentPr: form.commentPr,
    });
    setToken('');
    if (next) onStatus(next);
    return true;
  };

  const run = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    onError(null);
    setResult(null);
    try {
      await work();
    } catch (reason: unknown) {
      onError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const handleSave = () => run(async () => {
    await save();
  });

  const handleTest = () => run(async () => {
    await save();
    const res = await bridge.call<{ ok: boolean; user?: string; error?: string }>('integrationsTrackerTest', {
      workspaceId,
      tracker,
    });
    if (res?.ok) {
      setResult({ ok: true, text: t('integrations.tracker.testOk', { user: res.user || TRACKER_NAMES[tracker] }) });
    } else {
      setResult({ ok: false, text: testFailureText(res?.error, t) });
    }
  });

  const handleDisconnect = () => run(async () => {
    const next = await bridge.call<WorkspaceIntegrationStatus>('integrationsTrackerDisconnect', { workspaceId, tracker });
    if (next) onStatus(next);
  });

  const projectPlaceholder =
    tracker === 'gitlab' && current.detectedProject ? current.detectedProject : t(PROJECT_PLACEHOLDER_KEYS[tracker]);
  const sitePlaceholder =
    tracker === 'gitlab'
      ? current.detectedSite || 'https://gitlab.com'
      : t('integrations.tracker.jiraSitePlaceholder');

  return (
    <section className="issue-picker-trackers" data-testid="issue-picker-trackers">
      <div className="issue-picker-trackers-head">
        <span className="issue-picker-field-label">{t('integrations.tracker.section')}</span>
        <div className="issue-picker-tabs" role="tablist" aria-label={t('integrations.tracker.section')}>
          {EXTERNAL_TRACKERS.map((id) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tracker === id}
              className={'issue-picker-tab' + (tracker === id ? ' active' : '')}
              data-testid={'tracker-settings-tab-' + id}
              onClick={() => setTracker(id)}
            >
              {TRACKER_NAMES[id]}
            </button>
          ))}
        </div>
        <span className={'issue-picker-conn-badge' + (current.connected ? ' connected' : '')}>
          <span className="issue-picker-conn-dot" aria-hidden="true" />
          <span>
            {current.connected
              ? tracker === 'gitlab' && current.glab
                ? t('integrations.tracker.glab')
                : t('integrations.tracker.connected')
              : t('integrations.tracker.notConnected')}
          </span>
        </span>
      </div>

      <div className="issue-picker-settings-grid">
        <span className="issue-picker-field-label">{TRACKER_NAMES[tracker]}</span>
        <div className="issue-picker-field-row">
          <label className="issue-picker-checkbox">
            <input
              type="checkbox"
              data-testid={'tracker-enabled-' + tracker}
              checked={form.enabled}
              onChange={(event) => set('enabled', event.target.checked)}
            />
            <span>{t('integrations.tracker.enabled')}</span>
          </label>
        </div>

        <label className="issue-picker-field-label" htmlFor={'tracker-project-' + tracker}>
          {t(PROJECT_LABEL_KEYS[tracker])}
        </label>
        <div className="issue-picker-field-row">
          <input
            id={'tracker-project-' + tracker}
            type="text"
            className="dialog-field issue-picker-input-wide mono"
            placeholder={projectPlaceholder}
            value={form.project}
            onChange={(event) => set('project', event.target.value)}
          />
        </div>

        {tracker !== 'linear' && (
          <>
            <label className="issue-picker-field-label" htmlFor={'tracker-site-' + tracker}>
              {t(tracker === 'jira' ? 'integrations.tracker.jiraSite' : 'integrations.tracker.gitlabSite')}
            </label>
            <div className="issue-picker-field-row">
              <input
                id={'tracker-site-' + tracker}
                type="url"
                className="dialog-field issue-picker-input-wide mono"
                placeholder={sitePlaceholder}
                value={form.site}
                onChange={(event) => set('site', event.target.value)}
              />
            </div>
          </>
        )}

        {tracker === 'jira' && (
          <>
            <label className="issue-picker-field-label" htmlFor="tracker-email-jira">
              {t('integrations.tracker.email')}
            </label>
            <div className="issue-picker-field-row">
              <input
                id="tracker-email-jira"
                type="email"
                className="dialog-field issue-picker-input-wide"
                value={form.email}
                onChange={(event) => set('email', event.target.value)}
              />
            </div>
          </>
        )}

        <label className="issue-picker-field-label" htmlFor={'tracker-token-' + tracker}>
          {t(TOKEN_LABEL_KEYS[tracker])}
        </label>
        <div className="issue-picker-field-row issue-picker-auth-row">
          <input
            id={'tracker-token-' + tracker}
            type="password"
            autoComplete="off"
            className="dialog-field issue-picker-token-input mono"
            data-testid={'tracker-token-' + tracker}
            placeholder={t('integrations.tracker.tokenPlaceholder')}
            value={token}
            disabled={busy}
            onChange={(event) => setToken(event.target.value)}
          />
          {current.connected && !(tracker === 'gitlab' && current.glab) && (
            <button
              type="button"
              className="btn"
              data-testid={'tracker-disconnect-' + tracker}
              disabled={busy}
              onClick={() => {
                void handleDisconnect();
              }}
            >
              {t('integrations.auth.logout')}
            </button>
          )}
        </div>

        <span className="issue-picker-field-label">{t('integrations.tracker.filter')}</span>
        <div className="issue-picker-field-row">
          <label className="issue-picker-checkbox">
            <input type="checkbox" checked={form.filterMine} onChange={(event) => set('filterMine', event.target.checked)} />
            <span>{t('integrations.tracker.filterMine')}</span>
          </label>
          <label className="issue-picker-checkbox">
            <input type="checkbox" checked={form.filterOpen} onChange={(event) => set('filterOpen', event.target.checked)} />
            <span>{t('integrations.tracker.filterOpen')}</span>
          </label>
        </div>

        <span className="issue-picker-field-label">{t('integrations.tracker.writeBack')}</span>
        <div className="issue-picker-field-row">
          <label className="issue-picker-checkbox">
            <input
              type="checkbox"
              data-testid={'tracker-write-start-' + tracker}
              checked={form.writeStart}
              onChange={(event) => set('writeStart', event.target.checked)}
            />
            <span>{t('integrations.tracker.writeStart')}</span>
          </label>
          <label className="issue-picker-checkbox">
            <input type="checkbox" checked={form.writePr} onChange={(event) => set('writePr', event.target.checked)} />
            <span>{t('integrations.tracker.writePr')}</span>
          </label>
          <label className="issue-picker-checkbox">
            <input type="checkbox" checked={form.writeMerge} onChange={(event) => set('writeMerge', event.target.checked)} />
            <span>{t('integrations.tracker.writeMerge')}</span>
          </label>
          <label className="issue-picker-checkbox">
            <input type="checkbox" checked={form.commentPr} onChange={(event) => set('commentPr', event.target.checked)} />
            <span>{t('integrations.tracker.commentPr')}</span>
          </label>
        </div>
      </div>

      <div className="issue-picker-settings-foot">
        <span className={result ? (result.ok ? 'tone-text-done' : 'tone-text-err') : undefined} data-testid="tracker-test-result">
          {result?.text ?? ''}
        </span>
        <div className="issue-picker-detail-actions">
          <button
            type="button"
            className="btn"
            data-testid={'tracker-test-' + tracker}
            disabled={busy || workspaceId === null}
            onClick={() => {
              void handleTest();
            }}
          >
            {t('integrations.tracker.test')}
          </button>
          <button
            type="button"
            className="btn primary"
            data-testid={'tracker-save-' + tracker}
            disabled={busy || workspaceId === null}
            onClick={() => {
              void handleSave();
            }}
          >
            {t('integrations.settings.save')}
          </button>
        </div>
      </div>
    </section>
  );
}
