/**
 * The Moments lane (7a): the streamer's marked moments drawn on one thin row, and which one is under
 * the pointer. Apart from Lanes.svelte, which only places the row and asks these, as suggestions.ts is.
 */
import type { Moment } from "./api";
import { clock } from "./time";

/** The moments shown: rejected ones are not drawn. */
export const shown = (all: Moment[]) => all.filter((m) => m.state !== "rejected");

/** The latest-starting moment under time `t` (moments may overlap), or null. */
export function momentAt(moments: Moment[], t: number): Moment | null {
  let hit: Moment | null = null;
  for (const m of moments) if (m.start <= t && t < m.end) hit = m;
  return hit;
}

/** What hovering one says. */
export function describe(m: Moment): string {
  const how = m.source === "voice" ? `Said ${m.label}` : `Hotkey: ${m.label}`;
  const state = m.state === "accepted" ? "Accepted: kept." : "Waiting: accept or reject it in the Moments panel.";
  return `${how} at ${clock(m.at)}\n${state}\n${clock(m.start)} to ${clock(m.end)}. Click to jump to it.`;
}

/**
 * Paints `moments` in the row from `y`, `h` tall: a waiting one in amber, an accepted one an outline
 * (colours written out: a canvas cannot read a token), the marker itself a tick at its end.
 */
export function paint(
  ctx: CanvasRenderingContext2D,
  moments: Moment[],
  x: (t: number) => number,
  y: number,
  h: number,
  width: number,
  view: { start: number; end: number },
) {
  for (const m of moments) {
    if (m.end <= view.start || m.start >= view.end) continue;
    const x0 = Math.max(0, x(m.start));
    const x1 = Math.min(width, x(m.end));
    const w = Math.max(1, x1 - x0 - 1);
    if (m.state === "accepted") {
      ctx.strokeStyle = "rgba(242, 179, 61, 0.8)";
      ctx.lineWidth = 1;
      ctx.strokeRect(x0 + 1, y + 3.5, Math.max(1, w - 1), h - 7);
    } else {
      ctx.fillStyle = "rgba(242, 179, 61, 0.55)";
      ctx.fillRect(x0 + 0.5, y + 3, w, h - 6);
    }
    ctx.fillStyle = "rgba(242, 179, 61, 1)";
    ctx.fillRect(Math.min(width - 2, x(m.end) - 2), y + 1, 2, h - 2);
  }
}
