/**
 * The Suggestions lane: Laya's suggested keeps and cuts drawn as blocks on one thin row, and which
 * one is under the pointer. Apart from Lanes.svelte, which only places the row and asks these.
 */
import type { KeepItem } from "./api";
import { clock } from "./time";

/** The item under time `t` (items are in time order and apart), or null. */
export function itemAt(items: KeepItem[], t: number): KeepItem | null {
  let lo = 0;
  let hi = items.length;
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (items[m].end <= t) lo = m + 1;
    else hi = m;
  }
  const i = items[lo];
  return i && i.start <= t ? i : null;
}

/** What hovering one says. */
export function describe(i: KeepItem): string {
  const what = i.keep ? "keep" : "cut";
  const sure = i.uncertain ? ", uncertain" : "";
  const took = i.state === "accepted" ? `Accepted: ${what}. Click to undo.` : `Laya would ${what} this. Click to accept or dismiss.`;
  const part = i.kind === "talk" ? "talk" : "nobody talks";
  return `${took}\n${Math.round(i.p * 100)}% to keep${sure}\n${clock(i.start)} to ${clock(i.end)}, ${part}`;
}

/**
 * Paints `items` in the row from `y`, `h` tall: a suggested keep in --good, a suggested cut in --bad
 * (colours written out: a canvas cannot read a token), fainter when uncertain; an accepted one an
 * outline only, since the cuts across the lanes already show what it did.
 */
export function paint(
  ctx: CanvasRenderingContext2D,
  items: KeepItem[],
  x: (t: number) => number,
  y: number,
  h: number,
  width: number,
  view: { start: number; end: number },
) {
  for (const i of items) {
    if (i.end <= view.start || i.start >= view.end) continue;
    const x0 = Math.max(0, x(i.start));
    const x1 = Math.min(width, x(i.end));
    const w = Math.max(1, x1 - x0 - 1);
    const rgb = i.keep ? "127, 201, 138" : "242, 95, 143";
    if (i.state === "accepted") {
      ctx.strokeStyle = `rgba(${rgb}, 0.7)`;
      ctx.lineWidth = 1;
      ctx.strokeRect(x0 + 1, y + 3.5, Math.max(1, w - 1), h - 7);
      continue;
    }
    ctx.fillStyle = `rgba(${rgb}, ${i.uncertain ? 0.3 : 0.75})`;
    ctx.fillRect(x0 + 0.5, y + 3, w, h - 6);
  }
}
