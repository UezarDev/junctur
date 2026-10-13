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
