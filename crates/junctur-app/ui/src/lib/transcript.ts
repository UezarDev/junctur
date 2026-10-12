/**
 * The transcript laid out for a virtualised list. Pure functions, no DOM: words are marked kept or
 * cut from the edit list's frame ranges, runs of cut words fold into one token, cuts holding no
 * words become thin marks, and the result is broken into fixed-height rows by measured width, so
 * the view only ever renders the rows on screen. A 30 h transcript is a few hundred thousand words;
 * everything here is one pass over them (plus a binary search per word).
 */
import type { EditState, TranscriptView } from "./api";

export interface Words {
  text: string[];
  /** Milliseconds. */
  start: Float64Array;
  end: Float64Array;
}

export function words(v: TranscriptView): Words {
  return {
    text: v.text.length ? v.text.split("\n") : [],
    start: Float64Array.from(v.start),
    end: Float64Array.from(v.end),
  };
}

/** A stretch of source frames, `[start, end)`. */
export interface Stretch {
  start: number;
  end: number;
}

export type Token =
  | { kind: "word"; i: number }
  /** Cut words `first..=last`, folded (or shown, when expanded), restoring `frames` as a whole. */
  | { kind: "cut"; first: number; last: number; frames: Stretch; open: boolean }
  /** One cut word of an expanded run. */
  | { kind: "cutword"; i: number }
  /** A cut with no words in it. */
  | { kind: "gap"; frames: Stretch };

export interface Row {
  tokens: Token[];
  /** Source milliseconds of the paragraph this row starts, if it starts one. */
  para: number | null;
}

export interface Layout {
  rows: Row[];
  /** The row each word is shown in (a folded word: its fold's row). */
  rowOf: Int32Array;
  kept: Uint8Array;
}

/** A new paragraph after a pause this long. */
const PARAGRAPH_MS = 3000;

/** The cut stretches: everything between the kept ranges, within `[0, total)`. */
export function cutStretches(edit: EditState): Stretch[] {
  const out: Stretch[] = [];
  let at = 0;
  for (let i = 0; i < edit.ranges.length; i += 2) {
    if (edit.ranges[i] > at) out.push({ start: at, end: edit.ranges[i] });
    at = edit.ranges[i + 1];
  }
  if (at < edit.totalFrames) out.push({ start: at, end: edit.totalFrames });
  return out;
}

/** The index of the stretch holding `frame`, or -1. */
function find(stretches: Stretch[], frame: number): number {
  let lo = 0;
  let hi = stretches.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (stretches[mid].end <= frame) lo = mid + 1;
    else hi = mid;
  }
  return lo < stretches.length && stretches[lo].start <= frame ? lo : -1;
}

/**
 * Lays out `w` against the edit state. A word is kept when the frame at its middle is. `width`
 * measures a token in px; `max` is the row width; `open` holds the first word of each expanded run.
 */
export function layout(
  w: Words,
  edit: EditState | null,
  open: Set<number>,
  width: (t: Token) => number,
  max: number,
): Layout {
  const n = w.text.length;
  const kept = new Uint8Array(n).fill(1);
  const stretchOf = new Int32Array(n).fill(-1);
  const fps = edit?.fps;
  const stretches = edit && fps ? cutStretches(edit) : [];
  const hasWords = new Uint8Array(stretches.length);
  if (fps && stretches.length) {
    const perMs = fps.num / fps.den / 1000;
    for (let i = 0; i < n; i++) {
      const s = find(stretches, Math.floor(((w.start[i] + w.end[i]) / 2) * perMs));
      if (s >= 0) {
        kept[i] = 0;
        stretchOf[i] = s;
        hasWords[s] = 1;
      }
    }
  }
  const msOf = (frame: number) => (fps ? (frame * fps.den * 1000) / fps.num : 0);

  // Tokens in time order: words and folds, with the wordless cuts merged in by start time.
  const tokens: { t: Token; at: number; end: number }[] = [];
  let g = 0;
  const gapsUntil = (ms: number) => {
    while (g < stretches.length && msOf(stretches[g].start) <= ms) {
      if (!hasWords[g]) {
        const s = stretches[g];
        tokens.push({ t: { kind: "gap", frames: s }, at: msOf(s.start), end: msOf(s.end) });
      }
      g++;
    }
  };
  for (let i = 0; i < n; ) {
    gapsUntil(w.start[i]);
    if (kept[i]) {
      tokens.push({ t: { kind: "word", i }, at: w.start[i], end: w.end[i] });
      i++;
      continue;
    }
    let j = i;
    while (j + 1 < n && !kept[j + 1]) j++;
    const isOpen = open.has(i);
    const frames = { start: stretches[stretchOf[i]].start, end: stretches[stretchOf[j]].end };
    tokens.push({ t: { kind: "cut", first: i, last: j, frames, open: isOpen }, at: w.start[i], end: w.end[j] });
    if (isOpen) {
      for (let k = i; k <= j; k++) tokens.push({ t: { kind: "cutword", i: k }, at: w.start[k], end: w.end[k] });
    }
    i = j + 1;
  }
  gapsUntil(Infinity);

  const rows: Row[] = [];
  const rowOf = new Int32Array(n);
  let row: Row | null = null;
  let used = 0;
  let lastEnd = -Infinity;
  for (const { t, at, end } of tokens) {
    const wide = width(t);
    const para = at - lastEnd >= PARAGRAPH_MS;
    if (!row || para || used + wide > max) {
      row = { tokens: [], para: para ? at : null };
      rows.push(row);
      used = 0;
    }
    row.tokens.push(t);
    used += wide;
    lastEnd = Math.max(lastEnd, end);
    if (t.kind === "word" || t.kind === "cutword") rowOf[t.i] = rows.length - 1;
    else if (t.kind === "cut") rowOf.fill(rows.length - 1, t.first, t.last + 1);
  }
  return { rows, rowOf, kept };
}

/** The last word starting at or before `ms` (-1 before the first). */
export function wordAt(w: Words, ms: number): number {
  let lo = 0;
  let hi = w.start.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (w.start[mid] <= ms) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/** `h:mm:ss` for a paragraph's gutter. */
export function stamp(ms: number): string {
  const s = Math.floor(ms / 1000);
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${Math.floor(s / 3600)}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
}
