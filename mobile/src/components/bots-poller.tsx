import { useCallback, useEffect, useRef } from 'react';
import { ApiError, describeError, isAbortError } from '@/api/client';
import { useCapabilities } from '@/hooks/use-capabilities';
import { parseBots } from '@/lib/bots';
import { hasCapability } from '@/lib/capabilities';
import { registerBotsRefresh, useBotsStore } from '@/store/bots';
import { useHostClient, useLiveStore } from '@/store/live';

/**
 * MC-107: keeps one host's bots in the bots store — read once, then again whenever the host's `/m1/state` says its bots moved
 * (`botsRevision`, which the pane list's long poll brings), every `everyMs` when asked (a screen the list's poll does not cover), and
 * whenever a screen asks (`refreshBots`). A host that does not list the `bots` capability is marked unsupported, so the screen asks
 * for a desktop update instead of failing. Draws nothing.
 */
export function BotsPoller({ hostId, everyMs }: { hostId: string; everyMs?: number }) {
  const client = useHostClient(hostId);
  const capabilities = useCapabilities(hostId, client);
  const known = useLiveStore((store) => store.capabilities[hostId] !== undefined);
  const revision = useLiveStore((store) => store.states[hostId]?.botsRevision);
  const supported = hasCapability(capabilities, 'bots');
  const apply = useBotsStore((store) => store.apply);
  const mark = useBotsStore((store) => store.mark);
  const inflight = useRef<AbortController | null>(null);
  const revisionRef = useRef(revision);
  revisionRef.current = revision;

  const read = useCallback(() => {
    if (!client || !supported) return;
    inflight.current?.abort();
    const controller = new AbortController();
    inflight.current = controller;
    const asked = revisionRef.current;
    mark(hostId, { loading: true });
    client
      .bots(controller.signal)
      .then((raw) => {
        if (!controller.signal.aborted) apply(hostId, parseBots(raw), asked);
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted || isAbortError(error)) return;
        // A host that answers 404 has no bot routes after all (an older build that listed something else).
        if (error instanceof ApiError && error.status === 404) mark(hostId, { loading: false, unsupported: true, error: undefined });
        else mark(hostId, { loading: false, error: describeError(error) });
      });
  }, [apply, client, hostId, mark, supported]);

  // An older desktop app: no bots to read; the screen says to update it.
  useEffect(() => {
    if (known && !supported) mark(hostId, { loading: false, unsupported: true, error: undefined });
  }, [hostId, known, mark, supported]);

  useEffect(() => {
    read();
  }, [read, revision]);

  useEffect(() => {
    if (!everyMs || !supported) return undefined;
    const timer = setInterval(read, everyMs);
    return () => clearInterval(timer);
  }, [everyMs, read, supported]);

  useEffect(() => registerBotsRefresh(hostId, read), [hostId, read]);
  useEffect(() => () => inflight.current?.abort(), []);

  return null;
}
