/**
 * Where everything in the editor goes (2g), from the window, the side panel, the picture's shape and
 * how many tracks and speakers there are. Pure, so App.svelte only places what this returns and
 * tells mpv the margins.
 *
 * The left column holds the video area, the play bar under it and the track lanes under that (B).
 * Landscape gives the picture the column's width when it can and the lanes the rest, each track
 * lane between ROW_MIN and ROW_MAX; when the lanes need less than that leaves, the video area takes
 * the extra (the picture is then pillarboxed) instead of the lanes growing. Portrait (the picture
 * taller than wide, P4) keeps the lanes thin and puts the mixer strips and the large avatars in
 * the space mpv leaves beside the picture: mpv centres the picture in what the margins leave, so
 * nothing but the right and bottom margins is ever sent.
 */

import type { Captions } from "./api";
import { LINE } from "./captionStyle";

/** The title bar's height (2i): TitleBar.svelte is drawn this tall and the editor is laid out in
 * the window below it, so the two cannot disagree. */
export const TITLE = 34;
export const BAR = 52;
export const RULER = 22;
export const ROW_MIN = 22;
export const ROW_MAX = 52;
/** Portrait's thin track lanes. */
export const ROW_THIN = 26;
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
  /** A track lane's height, the Speakers lane's (0: none) and the lanes' name column's width. */
  row: number;
  speakersRow: number;
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
  const anyLanes = tracks > 0 || speakersRow > 0;
  const fixed = anyLanes ? RULER + speakersRow : 0;
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
    head: clamp(column * 0.24, 130, portrait ? 180 : 200),
    strips,
    avatars,
    margins: {
      right: i.width > 0 ? i.panel / i.width : 0,
      bottom: i.height > 0 ? (i.height - videoH) / i.height : 0,
    },
  };
}

/** The two bars over the picture (2r): the range toolbar (ActionBar) and review's. One size each in
 * every state, so no label, timecode or button text moves anything (2g to 2o sized them by their
 * contents); the components draw themselves at exactly these, so `floatAt` and the box agree. */
export const TOOLS = { w: 344, h: 84 };
export const REVIEW = { w: 440, h: 128 };
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

/** What places the caption blocks on the picture (5g's settings, and how many speakers could talk at
 * once); the bars are given null with the preview off. */
export type CaptionBand = Pick<Captions, "position" | "size" | "lines" | "chars" | "layout" | "protagonist"> & { speakers: number };

/**
 * How far the bottom caption blocks reach on a picture `w` by `h`, as the engine places them
 * (`export/layout.rs`): `right`, their right-most edge in picture widths, and `top`, the top of
 * what is stacked at the bottom in picture heights. A full line of `chars` characters is the
 * widest a block gets (glyph `GLYPH` ems). Two people talking at once are allowed for: stacked,
 * the second block sits on the first (`GAP` ems apart); top and bottom, the second goes to the top,
 * so only a third stacks at the bottom; side by side, the blocks sit across the picture's width,
 * so they reach its right edge, each in a column its words are wrapped again to (more lines).
 * Three or more stacked at once can still reach above `top`.
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
 * Where a floating bar of `size` goes (2r): its box's top-left in the editor's coordinates and the
 * scale it is drawn at. The box sits in the picture's bottom-right corner (2n had it over the lanes,
 * where it hid the cuts). With the captions preview on, a box that would reach over the bottom
 * caption blocks (`captionReach`: a default 42-character line spans about the middle half of a 16:9
 * picture, to 0.77 of its width, so the corner alone clears it only on a very wide picture) rises to sit just above them. It depends only on the layout and the
 * settings, never on a caption showing, so it never jumps while playing. It always stays inside the
 * video area `bounds` (never over the play bar or past the window's edge): a picture narrower than
 * the box moves it left only as far as needed, and an area smaller than the box draws it smaller.
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
