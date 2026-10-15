/**
 * Where everything in the editor goes, from the window, side panel, picture shape and track and
 * speaker counts. Pure, so App.svelte only places what this returns and tells mpv the margins.
 */

import type { Captions } from "./api";
import { LINE } from "./captionStyle";

/** The title bar's height: TitleBar.svelte is drawn this tall and the editor is laid out in
 * the window below it, so the two cannot disagree. */
export const TITLE = 34;
export const BAR = 52;
export const RULER = 22;
export const ROW_MIN = 22;
export const ROW_MAX = 52;
/** Portrait's thin track lanes. */
export const ROW_THIN = 26;
/** The Labels lane: one thin row of the silent parts' labels, when the recording has them. */
export const LABELS_ROW = 18;
/** The Suggestions lane: Laya's suggested keeps and cuts, when it has judged the recording. */
export const KEEP_ROW = 18;
const VIDEO_MIN = 120;
/** What a portrait side area needs to be worth drawing. */
const STRIP = 58;
const AVATARS = 130;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Layout {
  portrait: boolean;
  /** The area mpv draws in (it centres the picture there). */
  video: Rect;
  /** Where the picture itself is, inside `video`. */
  picture: Rect;
  bar: Rect;
  lanes: Rect;
  /** A track lane's height, the Speakers, Labels and Suggestions lanes' (0: none) and the lanes'
   * name column's width. */
  row: number;
  speakersRow: number;
  labelsRow: number;
  keepRow: number;
  head: number;
  /** Portrait's side areas beside the picture: mixer strips, large avatars; null when too narrow. */
  strips: Rect | null;
  avatars: Rect | null;
  /** Shares of the window kept free of video, for mpv. */
  margins: { right: number; bottom: number };
}

export interface Input {
  width: number;
  height: number;
  /** The side panel's width (0 when hidden). */
  panel: number;
  /** The picture's width over height; null when not known yet (taken as 16:9). */
  aspect: number | null;
  tracks: number;
  speakers: number;
  /** The recording has labels for its silent parts (a Labels lane). */
  labels?: boolean;
  /** Laya has judged the recording (a Suggestions lane). */
  keep?: boolean;
}

const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/** The Speakers lane: a row of bars per speaker, at least a header's worth. */
export function speakersHeight(speakers: number): number {
  return speakers > 0 ? clamp(10 * speakers + 14, 36, 70) : 0;
}

export function layout(i: Input): Layout {
  const aspect = i.aspect && i.aspect > 0 ? i.aspect : 16 / 9;
  const portrait = aspect < 1;
  const column = Math.max(0, i.width - i.panel);
  const tracks = Math.max(0, i.tracks);
  const speakersRow = speakersHeight(i.speakers);
  const labelsRow = i.labels ? LABELS_ROW : 0;
  const keepRow = i.keep ? KEEP_ROW : 0;
  const anyLanes = tracks > 0 || speakersRow > 0 || labelsRow > 0 || keepRow > 0;
  const fixed = anyLanes ? RULER + speakersRow + labelsRow + keepRow : 0;
  const avail = Math.max(0, i.height - BAR);
  let videoH: number;
  let row: number;
  if (portrait) {
    row = ROW_THIN;
    videoH = Math.max(VIDEO_MIN, avail - fixed - tracks * row);
  } else {
    const lanesMin = fixed + tracks * ROW_MIN;
    const lanesMax = fixed + tracks * ROW_MAX;
    videoH = Math.min(column / aspect, avail - lanesMin);
    // More room than the lanes can use: the video area takes it.
    if (avail - videoH > lanesMax) videoH = avail - lanesMax;
    videoH = Math.max(VIDEO_MIN, videoH);
    row = tracks > 0 ? clamp((avail - videoH - fixed) / tracks, ROW_MIN, ROW_MAX) : 0;
  }
  videoH = Math.min(videoH, avail);
  const video = { x: 0, y: 0, w: column, h: videoH };
  const pw = Math.min(column, videoH * aspect);
  const ph = pw / aspect;
  const picture = { x: (column - pw) / 2, y: (videoH - ph) / 2, w: pw, h: ph };
  const bar = { x: 0, y: videoH, w: column, h: BAR };
  const lanes = { x: 0, y: videoH + BAR, w: column, h: Math.max(0, i.height - videoH - BAR) };
  let strips: Rect | null = null;
  let avatars: Rect | null = null;
  if (portrait) {
    const side = picture.x;
    if (side >= STRIP * Math.max(1, tracks) * 0.6 + 16 && tracks > 0) {
      strips = { x: 0, y: 0, w: side, h: videoH };
    }
    if (side >= AVATARS && i.speakers > 0) {
      avatars = { x: picture.x + pw, y: 0, w: side, h: videoH };
    }
  }
  return {
    portrait,
    video,
    picture,
    bar,
    lanes,
    row,
    speakersRow,
    labelsRow,
    keepRow,
    head: clamp(column * 0.24, 130, portrait ? 180 : 200),
    strips,
    avatars,
    margins: {
      right: i.width > 0 ? i.panel / i.width : 0,
      bottom: i.height > 0 ? (i.height - videoH) / i.height : 0,
    },
  };
}

/** The two bars over the picture: the range toolbar (ActionBar) and review's. One size each in
 * every state, so no text moves anything; the components draw themselves at exactly these. */
export const TOOLS = { w: 344, h: 84 };
// Review: 156 = 2 x 10 padding, 2 border, rows of 20, 26, 36 and 28, three 8px gaps.
export const REVIEW = { w: 440, h: 156 };
/** The bars' inset from the picture's bottom-right corner, their gap from a caption block, and the
 * least room kept to the video area's edges. */
const INSET = 12;
const CLEAR = 8;
const EDGE = 4;
/** The engine's guesses for a caption block's size (junctur-core `export/layout.rs`): an average
 * glyph's advance and the gap between stacked blocks, in ems; a side-by-side column round a
 * protagonist, in picture widths. */
const GLYPH = 0.5;
const GAP = 0.3;
const BESIDE_COLUMN = 0.3;

/** What places the caption blocks on the picture (the layout settings, and how many speakers could
 * talk at once); the bars are given null with the preview off. */
export type CaptionBand = Pick<Captions, "position" | "size" | "lines" | "chars" | "layout" | "protagonist"> & { speakers: number };

/**
 * How far the bottom caption blocks reach on a picture `w` by `h`, as the engine places them:
 * `right` in picture widths and `top` in picture heights, allowing for two people at once.
 */
export function captionReach(c: CaptionBand, w: number, h: number): { right: number; top: number } {
  const em = c.size / 100;
  const lines = Math.max(1, c.lines);
  const glyph = w > 0 ? (em * h * GLYPH) / w : 0;
  const block = (n: number) => n * em * LINE;
  const n = Math.max(1, c.speakers);
  if (c.layout === "side" && n > 1) {
    const column = c.protagonist ? BESIDE_COLUMN : 1 / Math.min(n, 2);
    const chars = Math.max(6, Math.floor((column * 0.9) / Math.max(glyph, 1e-6)));
    const wrapped = Math.max(lines, Math.ceil((lines * Math.max(1, c.chars)) / chars));
    return { right: 1, top: c.position / 100 + block(wrapped) };
  }
  const stacked = c.layout === "stacked" ? Math.min(n, 2) : n >= 3 ? 2 : 1;
  return {
    right: Math.min(1, 0.5 + (Math.max(1, c.chars) * glyph) / 2),
    top: c.position / 100 + stacked * block(lines) + (stacked - 1) * GAP * em,
  };
}

/**
 * Where a floating bar of `size` goes: its top-left and draw scale. It sits in the picture's
 * bottom-right corner, rising above the bottom caption blocks when the preview is on, and stays
 * inside `bounds`. It depends only on layout and settings, so it never jumps while playing.
 */
export function floatAt(
  picture: Rect,
  bounds: Rect,
  size: { w: number; h: number },
  captions: CaptionBand | null,
): { left: number; top: number; scale: number } {
  const scale = Math.max(0.1, Math.min(1, (bounds.w - 2 * EDGE) / size.w, (bounds.h - 2 * EDGE) / size.h));
  const w = size.w * scale;
  const h = size.h * scale;
  const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
  const left = clamp(picture.x + picture.w - INSET - w, bounds.x + EDGE, bounds.x + bounds.w - EDGE - w);
  const bottom = picture.y + picture.h;
  let top = bottom - INSET - h;
  if (captions) {
    const reach = captionReach(captions, picture.w, picture.h);
    if (left < picture.x + reach.right * picture.w + CLEAR) top = Math.min(top, bottom - reach.top * picture.h - CLEAR - h);
  }
  top = clamp(top, bounds.y + EDGE, bounds.y + bounds.h - EDGE - h);
  return { left: Math.round(left), top: Math.round(top), scale };
}
