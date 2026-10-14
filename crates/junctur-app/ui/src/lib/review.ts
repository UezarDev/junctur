/**
 * Review mode's clips (2o): every stretch of the edit list in order, the kept ranges and the cuts
 * between them (and at either end), as `nudge.ts` `under` finds them one at a time. A module of its
 * own because the list is frozen when review starts and outlives the edits made from it: removing
 * a kept range merges it with the cuts on both sides into one live stretch, so following the live
 * list would make "the next clip" the merged cut just decided. A clip is the proposal's stretch;
 * what it is now is read from the live list for the readout (`now`).
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
 * Moves an edge of clip `i` by `by` frames (2s: review's steppers), with the matching edge of the
 * clip beside it, since the two share it: the new frozen list (the clips moved are new objects,
 * the rest kept) and the frame edit that makes it so in the live list, or null for none. Null
 * altogether when nothing moves: the first clip's start and the last one's end are the file's, and
 * an edge stays one frame inside both clips. The frames passed over take the kind of the clip that
 * grows: what it is now in the live list when it is wholly one (a Removed range grows as a cut),
 * else what review proposed. No edit when those frames already are that kind (both clips were kept,
 * say), so the undo history holds only real changes; the clips still move, which is what plays.
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
