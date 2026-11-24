import { useContext, useEffect, useRef, type ReactNode, type RefObject } from 'react';
import { useServices } from '../app/context';
import { PaneShownContext } from '../pane/shown';
import { overlayBoxes } from '../shell/overlay';
import { pageCovers, shows, SlotReporter, slotReport } from './bounds';
import { useHostCapabilities, type SurfaceKind } from './host';

const SAFE_SNAPSHOT_URL = /^data:image\/(?:png|jpeg|webp);base64,[A-Za-z0-9+/=]+$/i;

/** Validates a native slot snapshot data URL before interpolating it into CSS `url(...)`. */
export function safeSnapshotUrl(url: string | undefined): string | null {
  if (!url) return null;
  return SAFE_SNAPSHOT_URL.test(url) ? url : null;
}

/**
 * Reports `element` as a native view's slot (docs/tauri-architecture.md §3): its bounds in device pixels on the next frame after
 * anything that can move or cover it (its size, the window, a scroll, the frame's model, the dialogs, the host's views, a page
 * overlay, the pane being shown or not), hidden when it goes. Nothing happens until the Rust host's `host` push has come: under
 * the C# host this hook does nothing. While the slot is covered, the view's last picture (when the platform gives one) stands in
 * as the slot's background. `onReported` hears, after each report is sent, whether the view now shows (false when the slot goes),
 * so a call that needs the view shown (MC-50's focus) goes to the host after the report that shows it.
 */
export function useNativeSlot(element: RefObject<HTMLElement | null>, paneId: string, surface: SurfaceKind, onReported?: (shows: boolean) => void): void {
  const { bridge, store } = useServices();
  const shown = useContext(PaneShownContext);
  const host = useHostCapabilities();
  const reported = useRef(onReported);
  reported.current = onReported;
  useEffect(() => {
    const target = element.current;
    if (!target || !host) return undefined;
    const picture = (url: string | undefined) => {
      const safe = safeSnapshotUrl(url);
      target.style.backgroundImage = safe ? `url("${safe}")` : '';
      target.style.backgroundSize = safe ? '100% 100%' : '';
    };
    const reporter = new SlotReporter(paneId, surface, (method, args) => bridge.call(method, args), picture);
    let frame = 0;
    const measure = () => {
      frame = 0;
      const box = target.getBoundingClientRect();
      const covered = pageCovers(store.getState(), box, () => overlayBoxes(document));
      const report = slotReport(box, window.devicePixelRatio || 1, shown && target.isConnected, covered);
      reporter.update(report);
      reported.current?.(shows(report));
    };
    const schedule = () => { if (!frame) frame = requestAnimationFrame(measure); };
    const observer = new ResizeObserver(schedule);
    observer.observe(target);
    observer.observe(document.documentElement);
    // Only the frame's model, the dialogs, the host's views and the page's overlays move or cover the slot; a streamed reply does not.
    let seen = store.getState();
    const unsubscribe = store.subscribe(() => {
      const current = store.getState();
      if (current.shell !== seen.shell || current.dialogs !== seen.dialogs || current.natives !== seen.natives || current.chosen !== seen.chosen || current.overlays !== seen.overlays) schedule();
      seen = current;
    });
    window.addEventListener('resize', schedule);
    // Any scrolling container (capture) may carry the slot along.
    window.addEventListener('scroll', schedule, true);
    schedule();
    return () => {
      if (frame) cancelAnimationFrame(frame);
      observer.disconnect();
      unsubscribe();
      window.removeEventListener('resize', schedule);
      window.removeEventListener('scroll', schedule, true);
      reporter.release();
      reported.current?.(false);
    };
  }, [bridge, store, element, paneId, surface, shown, host]);
}

/** An empty slot a native view covers (the Mac's Ghostty terminal); focusing it gives the view the keyboard. */
export function NativeSlot({ paneId, surface, className, label, children, onReported }: { paneId: string; surface: SurfaceKind; className?: string; label?: string; children?: ReactNode; onReported?: (shows: boolean) => void }) {
  const { bridge } = useServices();
  const element = useRef<HTMLDivElement>(null);
  useNativeSlot(element, paneId, surface, onReported);
  return (
    <div ref={element} className={className} data-native-slot={surface} data-testid={'native-slot-' + surface} aria-label={label} tabIndex={-1}
      onFocus={() => void bridge.call('surfaceAction', { paneId, surface, action: 'focus' }).catch(() => {})}>
      {children}
    </div>
  );
}
