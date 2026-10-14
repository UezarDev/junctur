/**
 * The transcript laid out for a virtualised list in two steps, so each change redoes only its part:
 * `tokens` per edit (words kept or cut, cut runs folded, widths measured once) and `pack` per
 * resize or fold toggle (fixed-height rows, a speaker change on a name row of its own).
 */
import type { EditState, TranscriptView } from "./api";
import { msOf } from "./time";

export interface Words {
  text: string[];
  /** Milliseconds, where the edit list's retimed words put them. */
  start: Float64Array;
  end: Float64Array;
  /** Each word's start as heard: what fixes name a word by. `start` itself when no word is
   * retimed. */
  heard: Float64Array;
  /** Word indices by `start` when retimes left `start` out of order, for `wordAt`; null while it is
   * in order. */
  byStart: Int32Array | null;
  /** Speaker id per word, 0 for none. */
  speaker: Uint32Array;
  /** A second speaker saying the word at the same time, 0 for none. */
  also: Uint32Array;
  /** How sure Whisper was of each word, whole percent; 255 for never flagged. */
  sure: Uint8Array;
  /** The audio track each word was heard on. */
  track: Uint32Array;
  /** The span fixed as one text each word is in, 1-based, 0 for none. */
  span: Uint32Array;
  /** Per span (id - 1): its text, that text's words, and its first and last word. */
  spans: string[];
  spanWords: string[][];
  spanFirst: Int32Array;
  spanLast: Int32Array;
}

export function words(v: TranscriptView): Words {
  const spans = v.spans ?? [];
  const span = Uint32Array.from(v.span ?? []);
  const spanFirst = new Int32Array(spans.length).fill(-1);
  const spanLast = new Int32Array(spans.length).fill(-1);
  for (let i = 0; i < span.length; i++) {
    const s = span[i] - 1;
    if (s < 0) continue;
    if (spanFirst[s] < 0) spanFirst[s] = i;
    spanLast[s] = i;
  }
  const start = Float64Array.from(v.start);
  let sorted = true;
  for (let i = 1; i < start.length && sorted; i++) sorted = start[i - 1] <= start[i];
  const byStart = sorted ? null : Int32Array.from(start.keys()).sort((a, b) => start[a] - start[b] || a - b);
  return {
    text: v.text,
    start,
    end: Float64Array.from(v.end),
    heard: v.heard?.length ? Float64Array.from(v.heard) : start,
    byStart,
    speaker: Uint32Array.from(v.speaker ?? []),
    also: Uint32Array.from(v.also ?? []),
    sure: sureOf(v),
    track: Uint32Array.from(v.track ?? []),
    span,
    spans,
    spanWords: spans.map((t) => t.split(/\s+/).filter((x) => x)),
    spanFirst,
    spanLast,
  };
}

/** A word that is never flagged: no confidence to go by, or the user fixed or confirmed it. The
 * core's `NOT_FLAGGED` (src/editor/words.rs), which sends it. */
export const NOT_FLAGGED = 255;

/** The view's confidences, every word unflagged when it has none. */
export function sureOf(v: TranscriptView): Uint8Array {
  return v.sure?.length === v.text.length ? Uint8Array.from(v.sure) : new Uint8Array(v.text.length).fill(NOT_FLAGGED);
}

/**
 * The kept words to check, in order: those Whisper was less sure of than `below` percent. Cut
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

/** `r` reaching over the whole of any span it starts or ends in: a span is selected whole. */
export function widen(w: Words, r: { first: number; last: number }): { first: number; last: number } {
  const a = (w.span[r.first] ?? 0) - 1;
  const b = (w.span[r.last] ?? 0) - 1;
  return {
    first: a >= 0 ? Math.min(r.first, w.spanFirst[a]) : r.first,
    last: b >= 0 ? Math.max(r.last, w.spanLast[b]) : r.last,
  };
}

/** The words on word `i`'s track starting in its millisecond, when there are several; `null` for
 * a word alone. By the start as heard, by which the words are sorted, so they sit together. */
export function together(w: Words, i: number): { first: number; last: number } | null {
  const h = w.heard;
  let first = i;
  let last = i;
  for (let j = i - 1; j >= 0 && h[j] === h[i]; j--) if (w.track[j] === w.track[i]) first = j;
  for (let j = i + 1; j < h.length && h[j] === h[i]; j++) if (w.track[j] === w.track[i]) last = j;
  return last > first ? { first, last } : null;
}

/** What words `first..=last` show now on the track of the first (a fix field's starting text): a
 * span's text once, a fixed word as fixed, the rest as heard. */
export function shownText(w: Words, r: { first: number; last: number }): string {
  const track = w.track[r.first];
  const out: string[] = [];
  let lastSpan = 0;
  for (let i = r.first; i <= r.last && i < w.text.length; i++) {
    if (w.track[i] !== track) continue;
    const s = w.span[i];
    if (s) {
      if (s !== lastSpan) out.push(w.spans[s - 1]);
      lastSpan = s;
    } else {
      out.push(w.text[i]);
      lastSpan = 0;
    }
  }
  return out.filter((t) => t).join(" ");
}

/** A stretch of source frames, `[start, end)`. */
export interface Stretch {
  start: number;
  end: number;
}

export type Token =
  | { kind: "word"; i: number }
  /** Word `k` of span `s`'s text (`s` 1-based), shown at its first kept word `i`; `words` (on
   * the first only) are the span's kept words, which it stands for. */
  | { kind: "spanword"; s: number; k: number; i: number; words: number[] }
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
 * `wordWidth` is per word (measured once per transcript), `spanWidth` per span text's word;
 * `markWidth` measures folds and gaps.
 */
export function tokens(
  w: Words,
  wordWidth: Float64Array,
  edit: EditState | null,
  markWidth: (t: Token) => number,
  spanWidth: number[][] = [],
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
    width.push(
      t.kind === "word" || t.kind === "cutword"
        ? wordWidth[t.i]
        : t.kind === "spanword"
          ? (spanWidth[t.s - 1]?.[t.k] ?? 0)
          : markWidth(t),
    );
  };
  // Spans already shown, by id.
  const shownSpan = new Uint8Array(w.spans.length + 1);
  let g = 0;
  const gapsUntil = (t: number) => {
    while (g < stretches.length && ms(stretches[g].start) <= t) {
      const s = stretches[g];
      if (!hasWords[g]) push({ kind: "gap", frames: s }, ms(s.start), ms(s.end));
      g++;
    }
  };
  // The latest start so far: a retimed word may start before the one before it, and the gaps go in
  // by time, never back.
  let reached = -Infinity;
  for (let i = 0; i < n; ) {
    reached = Math.max(reached, w.start[i]);
    gapsUntil(reached);
    if (kept[i]) {
      const s = w.span[i] ?? 0;
      if (!s) push({ kind: "word", i }, w.start[i], w.end[i]);
      else if (!shownSpan[s]) {
        shownSpan[s] = 1;
        const last = w.spanLast[s - 1];
        const said: number[] = [];
        for (let j = i; j <= last; j++) if (w.span[j] === s && kept[j]) said.push(j);
        const texts = w.spanWords[s - 1] ?? [];
        for (let k = 0; k < texts.length; k++)
          push({ kind: "spanword", s, k, i, words: k === 0 ? said : [] }, w.start[i], w.end[last]);
      }
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
    case "spanword":
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
    else if (tok.kind === "spanword") for (const j of tok.words) rowOf[j] = rows.length - 1;
    else if (tok.kind === "cut") rowOf.fill(rows.length - 1, tok.first, tok.last + 1);
  }
  return { rows, rowOf };
}

/** The last word starting at or before `ms` (-1 before the first); through `byStart` when retimed
 * words left the starts out of order. */
export function wordAt(w: Words, ms: number): number {
  const order = w.byStart;
  const at = (k: number) => w.start[order ? order[k] : k];
  let lo = 0;
  let hi = w.start.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (at(mid) <= ms) lo = mid + 1;
    else hi = mid;
  }
  return lo === 0 ? -1 : order ? order[lo - 1] : lo - 1;
}

/** `h:mm:ss` for a paragraph's gutter. */
export function stamp(ms: number): string {
  const s = Math.floor(ms / 1000);
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${Math.floor(s / 3600)}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
}
