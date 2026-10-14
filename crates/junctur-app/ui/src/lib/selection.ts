/**
 * What is selected to cut or keep: transcript words or source frames, shared by the transcript,
 * timeline and edit bars. Word runs keep creation order (Shift+click extends the latest) and may
 * overlap, so readers go through `runs`, which sorts and joins them.
 */
import * as api from "./api";
import { toSeconds } from "./time";
import type { Words } from "./transcript";

/** Words `first..=last`, in order. */
export interface Run {
  first: number;
  last: number;
}

export type Selection =
  | { kind: "words"; runs: Run[] }
  | { kind: "frames"; start: number; end: number };

/** Words in order, whichever end the drag started from. */
export function run(a: number, b: number): Run {
  return { first: Math.min(a, b), last: Math.max(a, b) };
}

/** A selection of one run of words. */
export function words(first: number, last: number): Selection {
  return { kind: "words", runs: [run(first, last)] };
}

/** The word runs of a selection, sorted and joined where they overlap or touch (none for frames). */
export function runs(sel: Selection | null): Run[] {
  if (sel?.kind !== "words") return [];
  const sorted = [...sel.runs].sort((a, b) => a.first - b.first);
  const out: Run[] = [];
  for (const r of sorted) {
    const prev = out[out.length - 1];
    if (prev && r.first <= prev.last + 1) prev.last = Math.max(prev.last, r.last);
    else out.push({ ...r });
  }
  return out;
}

/** Whether word `i` is in `joined` (from `runs`). */
export function has(joined: Run[], i: number): boolean {
  let lo = 0;
  let hi = joined.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (i < joined[mid].first) hi = mid - 1;
    else if (i > joined[mid].last) lo = mid + 1;
    else return true;
  }
  return false;
}

/** The run made last (the one Shift+click extends). */
export function latest(sel: Selection | null): Run | null {
  return sel?.kind === "words" ? (sel.runs[sel.runs.length - 1] ?? null) : null;
}

/** The word runs of `sel` with `r` added as the latest (frames are dropped: words replace them). */
export function adding(sel: Selection | null, r: Run): Selection {
  return { kind: "words", runs: [...(sel?.kind === "words" ? sel.runs : []), r] };
}

/** Word `i` taken out of every run it is in (Ctrl+click on a selected word); `null` when nothing is left. */
export function without(sel: Selection | null, i: number): Selection | null {
  if (sel?.kind !== "words") return sel;
  const out: Run[] = [];
  for (const r of sel.runs) {
    if (i < r.first || i > r.last) out.push(r);
    else {
      if (r.first < i) out.push({ first: r.first, last: i - 1 });
      if (i < r.last) out.push({ first: i + 1, last: r.last });
    }
  }
  return out.length ? { kind: "words", runs: out } : null;
}

/** The selection in source seconds, a span per stretch, for the lanes and timeline to draw. */
export function seconds(
  sel: Selection | null,
  w: Words | null,
  fps: api.Fps | null,
): { start: number; end: number }[] {
  if (!sel) return [];
  if (sel.kind === "words") {
    if (!w) return [];
    return runs(sel)
      .filter((r) => r.last < w.start.length)
      .map((r) => ({ start: w.start[r.first] / 1000, end: w.end[r.last] / 1000 }));
  }
  if (!fps) return [];
  return [{ start: toSeconds(sel.start, fps), end: toSeconds(sel.end, fps) }];
}

/**
 * The edges of a selection of one stretch of words that can be dragged on the lanes: the
 * start of its first word and the end of its last, in seconds, each with the frames it may be
 * dragged between as the engine clamps it (`Words::retimed`): a frame inside the word's own other
 * end, and a frame inside the far end of its neighbour on its track (the word before it for a
 * start, after it for an end), whose near end the edge trims when it runs into it; the file's ends.
 * An edge with no room is left out. Null for nothing, frames, or several stretches (which word an
 * edge would move is not plain between them).
 */
export interface WordEdge {
  word: number;
  side: "start" | "end";
  /** Where it is now, in source seconds. */
  at: number;
  lo: number;
  hi: number;
}

export function wordEdges(
  sel: Selection | null,
  w: Words | null,
  fps: api.Fps | null,
  duration: number,
): WordEdge[] {
  const joined = runs(sel);
  if (!w || !fps || joined.length !== 1) return [];
  const { first, last } = joined[0];
  if (last >= w.start.length) return [];
  const per = fps.num / fps.den;
  const frame = 1 / per;
  const s = (i: number) => w.start[i] / 1000;
  const e = (i: number) => w.end[i] / 1000;
  // Whole frames inside a span of seconds, a hair allowed for the milliseconds.
  const up = (t: number) => Math.ceil(t * per - 1e-6);
  const down = (t: number) => Math.floor(t * per + 1e-6);
  // The nearest word on word `i`'s track before it (`step` -1) or after it (1), or -1.
  const near = (i: number, step: number) => {
    for (let j = i + step; j >= 0 && j < w.start.length; j += step) if (w.track[j] === w.track[i]) return j;
    return -1;
  };
  const prev = near(first, -1);
  const next = near(last, 1);
  const out: WordEdge[] = [];
  const start = { word: first, side: "start" as const, at: s(first), lo: up(prev >= 0 ? s(prev) + frame : 0), hi: down(e(first) - frame) };
  const end = { word: last, side: "end" as const, at: e(last), lo: up(s(last) + frame), hi: down(next >= 0 ? e(next) - frame : duration) };
  for (const g of [start, end]) if (g.lo <= g.hi) out.push(g);
  return out;
}

/** As the engine takes word runs: `[first, last]` pairs, sorted and joined. */
export function pairs(sel: Selection | null): [number, number][] {
  return runs(sel).map((r) => [r.first, r.last]);
}

/** Cuts (`keep` false) or keeps the selection: every stretch of it in one undoable edit. */
export function apply(sel: Selection, keep: boolean): Promise<void> {
  return sel.kind === "words"
    ? api.editWords(pairs(sel), keep)
    : api.editFrames(sel.start, sel.end, keep);
}

/** A short description for the edit bars: "1 word", "12 words", "12 words in 3 places". */
export function describe(sel: Selection | null, fps: api.Fps | null): string {
  if (!sel) return "Nothing selected";
  if (sel.kind === "words") {
    const joined = runs(sel);
    const n = joined.reduce((sum, r) => sum + r.last - r.first + 1, 0);
    const count = n === 1 ? "1 word" : `${n} words`;
    return joined.length > 1 ? `${count} in ${joined.length} places` : count;
  }
  const s = fps ? toSeconds(sel.end - sel.start, fps) : 0;
  return `${s.toFixed(2)} s of timeline`;
}
