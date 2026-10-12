/**
 * What the user has selected to cut or keep: transcript words (the engine snaps their boundaries
 * into the pauses around them) or a stretch of source frames from the timeline (taken exactly).
 * Shared by the transcript, the timeline and the edit bar so all three show and act on one thing.
 */
import * as api from "./api";
import { toSeconds } from "./time";
import type { Words } from "./transcript";

export type Selection =
  | { kind: "words"; first: number; last: number }
  | { kind: "frames"; start: number; end: number };

/** Words in order, whichever end the drag started from. */
export function words(first: number, last: number): Selection {
  return { kind: "words", first: Math.min(first, last), last: Math.max(first, last) };
}

/** The selection in source seconds, for the timeline to draw. */
export function seconds(
  sel: Selection | null,
  w: Words | null,
  fps: api.Fps | null,
): { start: number; end: number } | null {
  if (!sel) return null;
  if (sel.kind === "words") {
    if (!w || sel.last >= w.start.length) return null;
    return { start: w.start[sel.first] / 1000, end: w.end[sel.last] / 1000 };
  }
  if (!fps) return null;
  return { start: toSeconds(sel.start, fps), end: toSeconds(sel.end, fps) };
}

/** Cuts (`keep` false) or keeps the selection. */
export function apply(sel: Selection, keep: boolean): Promise<void> {
  return sel.kind === "words"
    ? api.editWords(sel.first, sel.last, keep)
    : api.editFrames(sel.start, sel.end, keep);
}

/** A short description for the edit bar. */
export function describe(sel: Selection | null, fps: api.Fps | null): string {
  if (!sel) return "Nothing selected";
  if (sel.kind === "words") {
    const n = sel.last - sel.first + 1;
    return n === 1 ? "1 word" : `${n} words`;
  }
  const s = fps ? toSeconds(sel.end - sel.start, fps) : 0;
  return `${s.toFixed(2)} s of timeline`;
}
