/**
 * Who talks when (2g): the edit list's speaker turns, searched by time. Turns are sorted by start
 * and those of different speakers overlap (two people at once, 5b), so "who talks at t" is not one
 * binary search on the starts. A running "furthest end so far" would find the first turn that may
 * still be going, but one long turn early on (a speaker who talks over a whole stretch) keeps it
 * from moving and turns every search after it into a scan from there. Instead the turns are
 * bucketed by time: `BUCKET` seconds per bucket, each listing the turns that overlap it. A position
 * looks in its one bucket; a window (the lanes) walks the turns from its start's bucket on. Each
 * turn is listed once per bucket it spans, so a turn of an hour costs 360 entries, not a scan.
 */
const BUCKET = 10;
import type { Turns } from "./api";

export class Talk {
  readonly start: Float64Array;
  readonly end: Float64Array;
  readonly speaker: Uint32Array;
  /** Per bucket, the turns overlapping it (indexes, ascending). */
  private readonly buckets: Uint32Array[];

  constructor(t: Turns | null) {
    const n = t ? Math.min(t.start.length, t.end.length, t.speaker.length) : 0;
    this.start = Float64Array.from(t?.start.slice(0, n) ?? []);
    this.end = Float64Array.from(t?.end.slice(0, n) ?? []);
    this.speaker = Uint32Array.from(t?.speaker.slice(0, n) ?? []);
    const lists: number[][] = [];
    for (let i = 0; i < n; i++) {
      const a = Math.max(0, Math.floor(this.start[i] / BUCKET));
      // A turn ending exactly on a bucket's start does not overlap it; an empty one sits in its own.
      const b = Math.max(a, Math.ceil(this.end[i] / BUCKET) - 1);
      for (let k = a; k <= b; k++) (lists[k] ??= []).push(i);
    }
    this.buckets = Array.from(lists, (l) => Uint32Array.from(l ?? []));
  }

  get length(): number {
    return this.start.length;
  }

  /** The speakers talking at source time `t`. */
  at(t: number): Set<number> {
    const who = new Set<number>();
    if (!(t >= 0)) return who;
    for (const i of this.buckets[Math.floor(t / BUCKET)] ?? []) {
      if (this.start[i] <= t && this.end[i] > t) who.add(this.speaker[i]);
    }
    return who;
  }

  /** Calls `f` once for every turn overlapping `[from, to)`: each bucket the window covers, a turn
   * taken in the first of them it is listed in. */
  each(from: number, to: number, f: (start: number, end: number, speaker: number) => void) {
    if (!(to > from)) return;
    const a = Math.max(0, Math.floor(from / BUCKET));
    const b = Math.min(this.buckets.length - 1, Math.floor(to / BUCKET));
    for (let k = a; k <= b; k++) {
      for (const i of this.buckets[k] ?? []) {
        const home = Math.max(a, Math.floor(this.start[i] / BUCKET));
        if (home !== k) continue;
        if (this.start[i] < to && this.end[i] > from) f(this.start[i], this.end[i], this.speaker[i]);
      }
    }
  }
}

/** Two speaker sets hold the same ids. */
export function same(a: Set<number>, b: Set<number>): boolean {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}
