/**
 * Range-level edits from the playhead: the stretch under it (a kept range, or the cut between two)
 * and what moving one of its ends by some frames means as an edit. Moving a boundary turns the
 * frames it passes over into whatever the growing side is, so every nudge is one exact frame edit.
 */
import type { EditState } from "./api";
import { cutStretches, type Stretch } from "./transcript";

export interface Under extends Stretch {
  kept: boolean;
  /** 1-based number of the kept range (kept stretches only), for the readout. */
  index: number;
}

/** The stretch holding source frame `frame`. */
export function under(edit: EditState, frame: number): Under | null {
  const r = edit.ranges;
  for (let i = 0; i < r.length; i += 2) {
    if (frame >= r[i] && frame < r[i + 1]) return { start: r[i], end: r[i + 1], kept: true, index: i / 2 + 1 };
  }
  const c = cutStretches(edit).find((s) => frame >= s.start && frame < s.end);
  return c ? { ...c, kept: false, index: 0 } : null;
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
