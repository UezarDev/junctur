/**
 * Who talks when: the edit list's speaker turns, bucketed by time (`BUCKET` seconds each), since
 * overlapping turns make "who talks at t" more than one binary search, and one long turn would turn
 * a "furthest end so far" search into a scan.
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
