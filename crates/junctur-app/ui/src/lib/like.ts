/**
 * The Like this lane (7b): the matches of the last "find all like this" drawn on one thin row,
 * brighter the closer they sound, the examples outlined and a waiting proposal's spans under them.
 * Apart from Lanes.svelte, which only places the row and asks these, as moments.ts is.
 */
import type { LikeExample, LikeMatch, LikeView } from "./api";
import { clock } from "./time";

const near = (a: number, b: number) => Math.abs(a - b) < 1e-3;

/** A match the user ticked: shown whatever the slider. */
export const confirmed = (v: LikeView, m: LikeMatch) => v.confirmed.some((p) => near(p, m.point));

/** The matches shown: at or above the slider, or ticked (the engine's `Like::shown`). */
export function shown(v: LikeView | null): LikeMatch[] {
  if (!v) return [];
  return v.matches.filter((m) => m.score >= v.threshold || confirmed(v, m));
}

/** The shown match nearest time `t` within `slack` seconds (a match is a thin mark), or null. */
export function matchAt(matches: LikeMatch[], t: number, slack: number): LikeMatch | null {
  let hit: LikeMatch | null = null;
  let best = Infinity;
  for (const m of matches) {
    const d = t < m.start ? m.start - t : t > m.end ? t - m.end : 0;
    if (d <= slack && d < best) {
      best = d;
      hit = m;
    }
  }
  return hit;
}

/** What hovering one says. */
export function describe(m: LikeMatch): string {
  return `Like this: ${Math.round(m.score * 100)}% alike at ${clock(m.point)}\nClick to hear it; tick or reject it in the Like this tab.`;
}

/** An example's colour: green for "like this", red for "not this" (written out: a canvas reads no token). */
const exampleColour = (e: LikeExample) => (e.positive ? "rgba(86, 196, 120, 0.95)" : "rgba(232, 92, 92, 0.95)");

/**
 * Paints the lane in the row from `y`, `h` tall: a proposal's spans as a low band (keeps green,
 * cuts red), each shown match a mark at least 2 px wide, its opacity its score, and the examples
 * outlined.
 */
export function paint(
  ctx: CanvasRenderingContext2D,
  v: LikeView,
  matches: LikeMatch[],
  x: (t: number) => number,
  y: number,
  h: number,
  width: number,
  view: { start: number; end: number },
) {
  const inView = (a: number, b: number) => b > view.start && a < view.end;
  const p = v.proposal;
  if (p) {
    ctx.fillStyle = p.kind === "keepAround" ? "rgba(86, 196, 120, 0.45)" : "rgba(232, 92, 92, 0.45)";
    for (const [a, b] of p.spans) {
      if (!inView(a, b)) continue;
      const x0 = Math.max(0, x(a));
      ctx.fillRect(x0, y + h - 5, Math.max(1, Math.min(width, x(b)) - x0), 3);
    }
  }
  for (const m of matches) {
    if (!inView(m.start, m.end)) continue;
    const x0 = Math.max(0, x(m.start));
    const w = Math.max(2, Math.min(width, x(m.end)) - x0);
    const a = 0.25 + 0.75 * Math.max(0, Math.min(1, m.score));
    ctx.fillStyle = confirmed(v, m) ? `rgba(86, 196, 120, ${a})` : `rgba(120, 170, 255, ${a})`;
    ctx.fillRect(x0, y + 3, w, h - 9);
  }
  ctx.lineWidth = 1;
  for (const e of v.examples) {
    if (!inView(e.start, e.end)) continue;
    ctx.strokeStyle = exampleColour(e);
    const x0 = Math.max(0, x(e.start));
    ctx.strokeRect(x0 + 0.5, y + 1.5, Math.max(2, Math.min(width, x(e.end)) - x0 - 1), h - 3);
  }
}
