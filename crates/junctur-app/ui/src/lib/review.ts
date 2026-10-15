/**
 * Review mode's clips: every stretch of the edit list in order. Frozen when review starts, since
 * removing a kept range merges it with the cuts beside it and "the next clip" would be the cut just
 * decided; what a clip is now is read from the live list (`now`).
 */
import type { EditState } from "./api";

export interface Clip {
  start: number;
  end: number;
  /** Kept in the list review started from. */
  kept: boolean;
  /** 1-based number of the kept range (kept clips only), as the range toolbar names it. */
  index: number;
}

/** The speeds review offers, also keys 1 to 4. */
export const SPEEDS = [1, 1.25, 1.5, 2];

/** Every stretch of `edit`, in order; none without frames. */
export function clips(edit: EditState): Clip[] {
  const r = edit.ranges;
  const total = edit.totalFrames;
  const out: Clip[] = [];
  let at = 0;
  for (let i = 0; i + 1 < r.length; i += 2) {
    const s = Math.min(r[i], total);
    const e = Math.min(r[i + 1], total);
    if (s > at) out.push({ start: at, end: s, kept: false, index: 0 });
    if (e > s) out.push({ start: s, end: e, kept: true, index: i / 2 + 1 });
    at = Math.max(at, e);
  }
  if (total > at) out.push({ start: at, end: total, kept: false, index: 0 });
  return out;
}

/** The clip holding `frame` (the last one past the end, the first before any). */
export function holding(list: Clip[], frame: number): number {
  let lo = 0;
  let hi = list.length;
  // The first clip ending after `frame`.
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (list[mid].end <= frame) lo = mid + 1;
    else hi = mid;
  }
  return Math.min(lo, Math.max(0, list.length - 1));
}

/** What `c` is in the live list: wholly kept, wholly cut, or partly either. */
export function now(edit: EditState, c: Clip): "kept" | "cut" | "partly" {
  const r = edit.ranges;
  let kept = 0;
  // Kept frames inside the clip; the ranges are sorted, so a binary search would do, but this
  // runs on a click or an edit, not on every position.
  for (let i = 0; i + 1 < r.length; i += 2) {
    if (r[i] >= c.end) break;
    kept += Math.max(0, Math.min(r[i + 1], c.end) - Math.max(r[i], c.start));
  }
  return kept === 0 ? "cut" : kept >= c.end - c.start ? "kept" : "partly";
}

/**
 * Moves an edge of clip `i` by `by` frames with the matching edge of the clip beside it: the new
 * frozen list and the frame edit for the live list, or null when nothing moves. No edit when the
 * frames passed over already are the growing clip's kind, so undo holds only real changes.
 */
export function moveEdge(
  list: Clip[],
  i: number,
  edge: "start" | "end",
  by: number,
  edit: EditState,
): { list: Clip[]; frames: { start: number; end: number; keep: boolean } | null } | null {
  const c = list[i];
  const j = edge === "start" ? i - 1 : i + 1;
  const n = list[j];
  if (!c || !n) return null;
  const from = edge === "start" ? c.start : c.end;
  const lo = (edge === "start" ? n.start : c.start) + 1;
  const hi = (edge === "start" ? c.end : n.end) - 1;
  const to = Math.min(hi, Math.max(lo, from + by));
  if (to === from) return null;
  const grows = (edge === "start" ? to < from : to > from) ? c : n;
  const is = now(edit, grows);
  const keep = is === "partly" ? grows.kept : is === "kept";
  const span = { start: Math.min(from, to), end: Math.max(from, to), kept: false, index: 0 };
  const out = list.slice();
  out[i] = edge === "start" ? { ...c, start: to } : { ...c, end: to };
  out[j] = edge === "start" ? { ...n, end: to } : { ...n, start: to };
  return { list: out, frames: now(edit, span) === (keep ? "kept" : "cut") ? null : { start: span.start, end: span.end, keep } };
}
