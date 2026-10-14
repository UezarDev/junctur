/**
 * The transcript laid out for a virtualised list, in two steps so each change redoes only its part.
 * `tokens` (per edit): words marked kept or cut from the edit list's frame ranges, each run of cut
 * words inside one cut stretch folded behind a header, cuts holding no words as thin marks, every
 * token's width measured once. `pack` (per resize or fold toggle): the tokens broken into
 * fixed-height rows, numbers only. The view then renders just the rows on screen. A 30 h transcript
 * is a few hundred thousand words; both steps are one pass (plus a binary search per word).
 *
 * Where the speaker changes (5a) the next row is the speaker's name, a row of its own: rows have
 * one height, so a name row needs no width accounting and never shares a line with words.
 */
import type { EditState, TranscriptView } from "./api";
import { msOf } from "./time";

export interface Words {
  text: string[];
  /** Milliseconds. */
  start: Float64Array;
  end: Float64Array;
  /** Speaker id per word, 0 for none. */
  speaker: Uint32Array;
  /** A second speaker saying the word at the same time (5b), 0 for none. */
  also: Uint32Array;
  /** How sure Whisper was of each word, whole percent; 255 for never flagged (5o). */
  sure: Uint8Array;
}

export function words(v: TranscriptView): Words {
  return {
    text: v.text,
    start: Float64Array.from(v.start),
    end: Float64Array.from(v.end),
    speaker: Uint32Array.from(v.speaker ?? []),
    also: Uint32Array.from(v.also ?? []),
    sure: sureOf(v),
  };
}

/** A word that is never flagged (5o): no confidence to go by, or the user fixed or confirmed it. The
 * core's `NOT_FLAGGED` (src/editor/words.rs), which sends it. */
export const NOT_FLAGGED = 255;

/** The view's confidences, every word unflagged when it has none (an app from before 5o). */
export function sureOf(v: TranscriptView): Uint8Array {
  return v.sure?.length === v.text.length ? Uint8Array.from(v.sure) : new Uint8Array(v.text.length).fill(NOT_FLAGGED);
}

/**
 * The kept words to check (5o), in order: those Whisper was less sure of than `below` percent. Cut
 * words are left out: they are in no caption. One pass over the tokens.
 */
export function flagged(t: Tokens, sure: Uint8Array, below: number): number[] {
  const out: number[] = [];
  if (below <= 0) return out;
  for (const tok of t.list) if (tok.kind === "word" && sure[tok.i] < below) out.push(tok.i);
  return out;
}

/** The first of `list` (ascending) after `from`, or before it with `back`; -1 for none. */
export function stepFrom(list: number[], from: number, back: boolean): number {
  let lo = 0;
  let hi = list.length;
  // `lo` ends at how many entries come before `from` (going back), or at or before it (forward).
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (back ? list[mid] < from : list[mid] <= from) lo = mid + 1;
    else hi = mid;
  }
  if (back) return lo > 0 ? list[lo - 1] : -1;
  return lo < list.length ? list[lo] : -1;
}

/** A stretch of source frames, `[start, end)`. */
export interface Stretch {
  start: number;
  end: number;
}

export type Token =
  | { kind: "word"; i: number }
  /** The header of cut words `first..=last`, all in the cut stretch `frames`. */
  | { kind: "cut"; first: number; last: number; frames: Stretch }
  /** One cut word, shown while its fold (keyed by the fold's first word) is open. */
  | { kind: "cutword"; i: number; fold: number }
  /** A cut with no words in it. */
  | { kind: "gap"; frames: Stretch };

export interface Tokens {
  list: Token[];
  /** Per token: source ms where it starts and ends (for paragraphs), and its width in px. */
  at: Float64Array;
  end: Float64Array;
  width: Float64Array;
}

export interface Row {
  /** Indices into `Tokens.list`. */
  tokens: number[];
  /** Source milliseconds of the paragraph this row starts, if it starts one. */
  para: number | null;
  /** A name row: the speaker (id) whose words follow. Such a row holds no tokens. */
  speaker: number | null;
}

export interface Layout {
  rows: Row[];
  /** The row each word is shown in (a folded word: its fold's row). */
  rowOf: Int32Array;
}

/** A new paragraph after a pause this long. */
const PARAGRAPH_MS = 3000;

/** The cut stretches: everything between the kept ranges, within `[0, total)`. */
function cutStretches(edit: EditState): Stretch[] {
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
 * Tokens for `w` against the edit state. A word is kept when the frame at its middle is.
 * `wordWidth` is per word (measured once per transcript); `markWidth` measures folds and gaps.
 */
export function tokens(
  w: Words,
  wordWidth: Float64Array,
  edit: EditState | null,
  markWidth: (t: Token) => number,
): Tokens {
  const n = w.text.length;
  const kept = new Uint8Array(n).fill(1);
  const stretchOf = new Int32Array(n).fill(-1);
  const fps = edit?.fps ?? null;
  const stretches = edit && fps ? cutStretches(edit) : [];
  const hasWords = new Uint8Array(stretches.length);
  if (fps) {
    // Frames per millisecond of a word's middle, with the same bias the engine's floor uses.
    const perMs = fps.num / fps.den / 2000;
    for (let i = 0; i < n; i++) {
      const s = find(stretches, Math.floor((w.start[i] + w.end[i]) * perMs + 1e-6));
      if (s >= 0) {
        kept[i] = 0;
        stretchOf[i] = s;
        hasWords[s] = 1;
      }
    }
  }
  const ms = (frame: number) => (fps ? msOf(frame, fps) : 0);
  const list: Token[] = [];
  const at: number[] = [];
  const end: number[] = [];
  const width: number[] = [];
  const push = (t: Token, a: number, b: number) => {
    list.push(t);
    at.push(a);
    end.push(b);
    width.push(t.kind === "word" || t.kind === "cutword" ? wordWidth[t.i] : markWidth(t));
  };
  let g = 0;
  const gapsUntil = (t: number) => {
    while (g < stretches.length && ms(stretches[g].start) <= t) {
      const s = stretches[g];
      if (!hasWords[g]) push({ kind: "gap", frames: s }, ms(s.start), ms(s.end));
      g++;
    }
  };
  for (let i = 0; i < n; ) {
    gapsUntil(w.start[i]);
    if (kept[i]) {
      push({ kind: "word", i }, w.start[i], w.end[i]);
      i++;
      continue;
    }
    // One fold per cut stretch: a fold never reaches across a kept range.
    let j = i;
    while (j + 1 < n && !kept[j + 1] && stretchOf[j + 1] === stretchOf[i]) j++;
    push({ kind: "cut", first: i, last: j, frames: stretches[stretchOf[i]] }, w.start[i], w.end[j]);
    for (let k = i; k <= j; k++) push({ kind: "cutword", i: k, fold: i }, w.start[k], w.end[k]);
    i = j + 1;
  }
  gapsUntil(Infinity);
  return {
    list,
    at: Float64Array.from(at),
    end: Float64Array.from(end),
    width: Float64Array.from(width),
  };
}

/** The speaker of a token: a word's own, a fold's first word's; 0 for none or a gap. */
function speakerOf(tok: Token, speaker: Uint32Array): number {
  switch (tok.kind) {
    case "word":
    case "cutword":
      return speaker[tok.i] ?? 0;
    case "cut":
      return speaker[tok.first] ?? 0;
    case "gap":
      return 0;
  }
}

/**
 * Breaks `t` into rows `max` px wide; cut words show only while their fold is in `open`. A name
 * row goes before the first token of each change of speaker (`speaker`: per word, 0 for none,
 * which continues whoever spoke last).
 */
export function pack(
  t: Tokens,
  words: number,
  open: Set<number>,
  max: number,
  speaker: Uint32Array = new Uint32Array(0),
): Layout {
  const rows: Row[] = [];
  const rowOf = new Int32Array(words);
  let row: Row | null = null;
  let used = 0;
  let lastEnd = -Infinity;
  let talking = 0;
  for (let k = 0; k < t.list.length; k++) {
    const tok = t.list[k];
    if (tok.kind === "cutword" && !open.has(tok.fold)) continue;
    const who = speakerOf(tok, speaker);
    const turn = who !== 0 && who !== talking;
    if (turn) {
      talking = who;
      rows.push({ tokens: [], para: null, speaker: who });
    }
    const para = turn || t.at[k] - lastEnd >= PARAGRAPH_MS;
    if (!row || turn || para || used + t.width[k] > max) {
      row = { tokens: [], para: para ? t.at[k] : null, speaker: null };
      rows.push(row);
      used = 0;
    }
    row.tokens.push(k);
    used += t.width[k];
    lastEnd = Math.max(lastEnd, t.end[k]);
    if (tok.kind === "word" || tok.kind === "cutword") rowOf[tok.i] = rows.length - 1;
    else if (tok.kind === "cut") rowOf.fill(rows.length - 1, tok.first, tok.last + 1);
  }
  return { rows, rowOf };
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
