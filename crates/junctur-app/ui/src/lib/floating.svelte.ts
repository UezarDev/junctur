// What a popup over the UI needs besides its trigger (2t): one open at a time, a place beside its
// trigger, and a home outside the boxes that scroll. Apart from `InfoTip` so a second kind of popup
// shares the same slot and rules rather than growing its own.

/** Space between the trigger and the popup, and kept between the popup and the window's edge. */
export const GAP = 8;
export const MARGIN = 8;

/**
 * The one open popup in the window, by id. A single slot rather than a flag per popup: closes are
 * deferred so the pointer can cross into a popup, and with a flag each a pointer sweeping down a
 * column of (i)s left several up at once. Opening one overwrites the slot, so the other is gone.
 */
export const slot = $state<{ open: string | null }>({ open: null });

/** Gives the slot up only if it is still `id`'s: a late close must not shut the next popup. */
export function release(id: string) {
  if (slot.open === id) slot.open = null;
}

/**
 * Where a popup of `width` x `height` goes for a trigger at `at`, in window coordinates: above by
 * preference (the trigger is what is being looked at), below when there is no room above, centred
 * on the trigger and kept `MARGIN` inside the window.
 */
export function place(
  at: { top: number; bottom: number; left: number; width: number },
  width: number,
  height: number,
  view: { width: number; height: number },
): { top: number; left: number } {
  const above = at.top - height - GAP;
  const below = Math.min(at.bottom + GAP, view.height - height - MARGIN);
  // A popup taller than the room either side starts at the top margin rather than off-screen.
  const top = above >= MARGIN ? above : Math.max(MARGIN, below);
  const centred = at.left + at.width / 2 - width / 2;
  const left = Math.max(MARGIN, Math.min(centred, view.width - width - MARGIN));
  return { top, left };
}

/**
 * Moves the element to `<body>`. The Settings dialog and the panels scroll (`overflow-y: auto`), which
 * clips anything inside them however it is positioned; a fixed box in `<body>` is clipped by nothing.
 */
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
