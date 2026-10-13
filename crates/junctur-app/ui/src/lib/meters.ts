/**
 * Every track's level, for whatever draws one (2g: the lanes' meters, the portrait strips, the
 * avatars). Levels come about 20 times a second while playing (2e); one listener takes them and
 * writes every registered bar's style straight away, outside Svelte's reactivity, so a level never
 * costs a component update. With no level for a moment (paused, no audio) every bar drops.
 */
import * as api from "./api";

/** The meters' range, in dBFS. */
export const FLOOR = -60;
/** No levels for this long: the bars drop. */
const QUIET_MS = 250;

type Listener = (levels: (number | null)[]) => void;

const listeners = new Set<Listener>();
let levels: (number | null)[] = [];
let started = false;
let last = 0;

/** A level as a share of the meter (0..1). */
export function share(db: number | null | undefined): number {
  return db == null ? 0 : Math.min(1, Math.max(0, (db - FLOOR) / -FLOOR));
}

function send(l: (number | null)[]) {
  levels = l;
  for (const f of listeners) f(l);
}

function start() {
  if (started) return;
  started = true;
  void api.onLevels((l) => {
    last = performance.now();
    send(l);
  });
  window.setInterval(() => {
    if (last && performance.now() - last > QUIET_MS) {
      last = 0;
      send(levels.map(() => null));
    }
  }, 100);
}

/** Calls `f` with every new set of levels (and once now); returns the unsubscribe. */
export function subscribe(f: Listener): () => void {
  start();
  listeners.add(f);
  f(levels);
  return () => listeners.delete(f);
}

/** Svelte action: `el` shows track `track`'s level, growing along `axis`. */
export function meter(el: HTMLElement, p: { track: number; axis: "x" | "y" }) {
  let { track, axis } = p;
  el.style.transformOrigin = axis === "x" ? "left" : "bottom";
  const off = subscribe((l) => {
    const s = share(l[track]);
    el.style.transform = axis === "x" ? `scaleX(${s})` : `scaleY(${s})`;
  });
  return {
    update(n: { track: number; axis: "x" | "y" }) {
      ({ track, axis } = n);
      el.style.transformOrigin = axis === "x" ? "left" : "bottom";
    },
    destroy: off,
  };
}
