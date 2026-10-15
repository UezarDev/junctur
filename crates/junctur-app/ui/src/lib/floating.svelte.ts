// What a popup over the UI needs besides its trigger: one open at a time, a place beside its
// trigger, and a home outside the boxes that scroll. Apart from `InfoTip` so other popups share it.

/** Space between the trigger and the popup, and kept between the popup and the window's edge. */
export const GAP = 8;
export const MARGIN = 8;
/** A list sits closer to its trigger than a tip: it reads as the trigger opening. */
export const LIST_GAP = 4;

/**
 * The one open popup in the window, by id. One slot, not a flag per popup: closes are deferred, so
 * a pointer sweeping down a column of (i)s would leave several open.
 */
export const slot = $state<{ open: string | null }>({ open: null });

/** Gives the slot up only if it is still `id`'s: a late close must not shut the next popup. */
export function release(id: string) {
  if (slot.open === id) slot.open = null;
}

/**
 * Where a popup of `width` x `height` goes for a trigger at `at`, in window coordinates: above by
 * preference, below when there is no room, kept `MARGIN` inside the window. `"below"` is for lists.
 */
export function place(
  at: { top: number; bottom: number; left: number; width: number },
  width: number,
  height: number,
  view: { width: number; height: number },
  prefer: "above" | "below" = "above",
): { top: number; left: number } {
  const gap = prefer === "below" ? LIST_GAP : GAP;
  const above = at.top - height - gap;
  const below = Math.min(at.bottom + gap, view.height - height - MARGIN);
  const fitsBelow = at.bottom + gap + height <= view.height - MARGIN;
  // A popup taller than the room either side starts at the top margin rather than off-screen.
  const top =
    prefer === "below" && (fitsBelow || above < MARGIN)
      ? Math.max(MARGIN, below)
      : above >= MARGIN
        ? above
        : Math.max(MARGIN, below);
  const start = prefer === "below" ? at.left : at.left + at.width / 2 - width / 2;
  const left = Math.max(MARGIN, Math.min(start, view.width - width - MARGIN));
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
