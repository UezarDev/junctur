/**
 * Who talks when (2g): the edit list's speaker turns, searched by time. Turns are sorted by start
 * and those of different speakers overlap (two people at once, 5b), so "who talks at t" is not
 * one binary search: `reach[i]` is the furthest end among turns 0..=i, which only grows, so the
 * first turn that can still be going at t is found by a binary search on it, and the scan from
 * there stops at the first turn starting after t. A 30 h recording's thousands of turns cost a few
 * steps per position update.
 */
import type { Turns } from "./api";

export class Talk {
  readonly start: Float64Array;
  readonly end: Float64Array;
  readonly speaker: Uint32Array;
  private readonly reach: Float64Array;

  constructor(t: Turns | null) {
    const n = t ? Math.min(t.start.length, t.end.length, t.speaker.length) : 0;
    this.start = Float64Array.from(t?.start.slice(0, n) ?? []);
    this.end = Float64Array.from(t?.end.slice(0, n) ?? []);
    this.speaker = Uint32Array.from(t?.speaker.slice(0, n) ?? []);
    this.reach = new Float64Array(n);
    let r = -Infinity;
    for (let i = 0; i < n; i++) {
      r = Math.max(r, this.end[i]);
      this.reach[i] = r;
    }
  }

  get length(): number {
    return this.start.length;
  }

  /** The first turn that may end after `t`. */
  private first(t: number): number {
    let lo = 0;
    let hi = this.reach.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.reach[mid] <= t) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  /** The speakers talking at source time `t`. */
  at(t: number): Set<number> {
    const who = new Set<number>();
    for (let i = this.first(t); i < this.start.length && this.start[i] <= t; i++) {
      if (this.end[i] > t) who.add(this.speaker[i]);
    }
    return who;
  }

  /** Calls `f` for every turn overlapping `[from, to)`. */
  each(from: number, to: number, f: (start: number, end: number, speaker: number) => void) {
    for (let i = this.first(from); i < this.start.length && this.start[i] < to; i++) {
      if (this.end[i] > from) f(this.start[i], this.end[i], this.speaker[i]);
    }
  }
}

/** Two speaker sets hold the same ids. */
export function same(a: Set<number>, b: Set<number>): boolean {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}
