/**
 * The Ask lane (7c): the ghosts typed requests would make, drawn on one thin row (cuts red, keeps
 * green; the newest request brighter), the selection a request was asked over outlined, and which
 * ghost is under the pointer. Apart from Lanes.svelte, which only places the row and asks these, as
 * like.ts and moments.ts are.
 */
import type { AskGhost, AskStep, AskView } from "./api";
import { clock } from "./time";

/** A waiting ghost and where it is: its step, and its place among that step's ghosts. */
export interface Placed {
  step: AskStep;
  index: number;
  ghost: AskGhost;
}

/** Every waiting ghost, oldest request first. */
export function waiting(v: AskView | null): Placed[] {
  if (!v) return [];
  return v.steps.flatMap((step) => step.ghosts.map((ghost, index) => ({ step, index, ghost })));
}

/** The newest-asked ghost under time `t`, or null. */
export function ghostAt(all: Placed[], t: number): Placed | null {
  let hit: Placed | null = null;
  for (const p of all) if (p.ghost.start <= t && t < p.ghost.end) hit = p;
  return hit;
}

/** What hovering one says. */
export function describe(p: Placed): string {
  const g = p.ghost;
  const what = g.keep ? "Keep" : "Cut";
  return `${what} ${clock(g.start)} to ${clock(g.end)} (${Math.round((g.end - g.start) * 10) / 10} s)\nAsked: ${p.step.request}\n${p.step.described}\nClick to play from just before it; accept or reject it in the Ask tab.`;
}

/**
 * Paints the lane in the row from `y`, `h` tall: each waiting ghost a block (cuts red, keeps
 * green; colours written out, a canvas reads no token), the newest request's brighter, and the
 * stretch each waiting request was asked over a thin line under them.
 */
export function paint(
  ctx: CanvasRenderingContext2D,
  v: AskView,
  x: (t: number) => number,
  y: number,
  h: number,
  width: number,
  view: { start: number; end: number },
) {
  const inView = (a: number, b: number) => b > view.start && a < view.end;
  const newest = v.steps.length ? v.steps[v.steps.length - 1].id : -1;
  for (const s of v.steps) {
    if (!s.ghosts.length) continue;
    const a = s.id === newest ? 0.7 : 0.35;
    for (const g of s.ghosts) {
      if (!inView(g.start, g.end)) continue;
      const x0 = Math.max(0, x(g.start));
      const w = Math.max(1, Math.min(width, x(g.end)) - x0);
      ctx.fillStyle = g.keep ? `rgba(86, 196, 120, ${a})` : `rgba(232, 92, 92, ${a})`;
      ctx.fillRect(x0, y + 3, w, h - 8);
    }
    const [ra, rb] = s.range ?? [0, v.duration];
    if (s.range && inView(ra, rb)) {
      ctx.fillStyle = "rgba(120, 170, 255, 0.8)";
      const x0 = Math.max(0, x(ra));
      ctx.fillRect(x0, y + h - 3, Math.max(1, Math.min(width, x(rb)) - x0), 2);
    }
  }
}
