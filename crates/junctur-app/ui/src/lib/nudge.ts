/**
 * Range-level edits from the playhead: the stretch under it and what moving one of its ends means
 * as an edit. The frames a boundary passes over take the growing side's kind.
 */
import type { EditState } from "./api";
import type { Stretch } from "./transcript";

export interface Under extends Stretch {
  kept: boolean;
  /** 1-based number of the kept range (kept stretches only), for the readout. */
  index: number;
}

/** The stretch holding source frame `frame`: a binary search, as this runs on every position. */
export function under(edit: EditState, frame: number): Under | null {
  const r = edit.ranges;
  const n = r.length / 2;
  // The first range ending after `frame`.
  let lo = 0;
  let hi = n;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (r[2 * mid + 1] <= frame) lo = mid + 1;
    else hi = mid;
  }
  if (lo < n && r[2 * lo] <= frame) return { start: r[2 * lo], end: r[2 * lo + 1], kept: true, index: lo + 1 };
  const start = lo > 0 ? r[2 * lo - 1] : 0;
  const end = lo < n ? r[2 * lo] : edit.totalFrames;
  return frame >= start && frame < end ? { start, end, kept: false, index: 0 } : null;
}

/** The frame edit that moves `edge` of `s` by `by` frames (negative: earlier), or null if none. */
export function nudge(
  s: Under,
  edge: "start" | "end",
  by: number,
  total: number,
): { start: number; end: number; keep: boolean } | null {
  const from = edge === "start" ? s.start : s.end;
  // A stretch may shrink to one frame, not vanish (that would be cutting or keeping it whole).
  const lo = edge === "start" ? 0 : s.start + 1;
  const hi = edge === "start" ? s.end - 1 : total;
  const to = Math.min(hi, Math.max(lo, from + by));
  if (to === from) return null;
  const grows = edge === "start" ? to < from : to > from;
  return { start: Math.min(from, to), end: Math.max(from, to), keep: grows === s.kept };
}

/**
 * A cut's edge on the lanes: a boundary of the kept ranges by its place in `edit.ranges` (even: a
 * start, odd: an end) and its frame, with the room it may be dragged in so no stretch vanishes.
 */
export interface Edge {
  at: number;
  frame: number;
  lo: number;
  hi: number;
}

/** The edge nearest source frame `frame`, if one is within `slack` frames: a binary search, as it
 * runs on every pointer move over the lanes. */
export function edgeNear(edit: EditState, frame: number, slack: number): Edge | null {
  const r = edit.ranges;
  const total = edit.totalFrames;
  // The first boundary at or after `frame`; the nearest is it or the one before.
  let lo = 0;
  let hi = r.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (r[mid] < frame) lo = mid + 1;
    else hi = mid;
  }
  let best: number | null = null;
  for (const j of [lo - 1, lo]) {
    if (j < 0 || j >= r.length || r[j] <= 0 || r[j] >= total) continue;
    const d = Math.abs(r[j] - frame);
    if (d <= slack && (best == null || d < Math.abs(r[best] - frame))) best = j;
  }
  if (best == null) return null;
  return {
    at: best,
    frame: r[best],
    lo: (best > 0 ? r[best - 1] : 0) + 1,
    hi: (best + 1 < r.length ? r[best + 1] : total) - 1,
  };
}

/** Where edge `e` lands when dragged to frame `to`: clamped to its room. */
export const clampEdge = (e: Edge, to: number) => Math.min(e.hi, Math.max(e.lo, Math.round(to)));

/** The frame edit that moves edge `e` to frame `to`, or null when it stays (or the list changed
 * under the drag): the same edit the toolbar's steppers make for the range it belongs to. */
export function dragged(edit: EditState, e: Edge, to: number): { start: number; end: number; keep: boolean } | null {
  const r = edit.ranges;
  if (r[e.at] !== e.frame) return null;
  const k = e.at >> 1;
  const range: Under = { start: r[2 * k], end: r[2 * k + 1], kept: true, index: k + 1 };
  return nudge(range, e.at % 2 === 0 ? "start" : "end", clampEdge(e, to) - e.frame, edit.totalFrames);
}
