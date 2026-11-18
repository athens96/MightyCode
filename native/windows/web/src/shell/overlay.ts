// The page's menus, popovers and in-page dialogs over a native view: the Mac's Ghostty terminal is a native view laid over the
// page (src/slot), so anything the page draws where it stands would be hidden under it. Each overlay counts itself open in the
// store (`overlays`) and marks its element `data-page-overlay`; a slot one of them overlaps reports itself covered
// (slot/bounds.ts pageCovers), and uncovered again once none does.
import { useEffect } from 'react';
import { useServices } from '../app/context';

/** Counts an overlay open in the store while `open` holds. */
export function usePageOverlay(open: boolean): void {
  const { store } = useServices();
  useEffect(() => {
    if (!open) return undefined;
    store.dispatch({ kind: 'overlay', open: true });
    return () => store.dispatch({ kind: 'overlay', open: false });
  }, [open, store]);
}

export interface Box { left: number; top: number; right: number; bottom: number }

/** Whether any of `overlays` covers part of `slot` (an empty box covers nothing). */
export function overlapsAny(slot: Box, overlays: readonly Box[]): boolean {
  if (slot.right <= slot.left || slot.bottom <= slot.top) return false;
  return overlays.some((box) => box.right > box.left && box.bottom > box.top && box.left < slot.right && box.right > slot.left && box.top < slot.bottom && box.bottom > slot.top);
}

/** The boxes of the overlays open in the page now. */
export function overlayBoxes(document: Document): Box[] {
  return [...document.querySelectorAll('[data-page-overlay]')].map((element) => element.getBoundingClientRect());
}
