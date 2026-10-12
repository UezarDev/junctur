/**
 * Range-level edits from the playhead: the stretch under it (a kept range, or the cut between two)
 * and what moving one of its ends by some frames means as an edit. Moving a boundary turns the
 * frames it passes over into whatever the growing side is, so every nudge is one exact frame edit.
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
