/** `h:mm:ss` for a 30 h recording, `m:ss` for short ones; `--:--` when unknown. */
export function clock(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "--:--";
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/**
 * `h:mm:ss:ff` for a frame number the engine counted: non-drop timecode, `ceil(fps)` frames to a
 * second, as Resolve counts it, so it names the same frame as the render and the FCPXML timeline.
 * No rounding happens here. `--:--:--:--` when unknown.
 */
export function timecode(frame: number | null | undefined, fps: { num: number; den: number }): string {
  if (frame == null || !Number.isFinite(frame)) return "--:--:--:--";
  const base = Math.ceil(fps.num / fps.den);
  const ff = frame % base;
  const s = Math.floor(frame / base);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${Math.floor(s / 3600)}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}:${pad(ff)}`;
}

type Rate = { num: number; den: number };

/** Source seconds where `frame` starts. */
export function toSeconds(frame: number, fps: Rate): number {
  return (frame * fps.den) / fps.num;
}

/** Source milliseconds where `frame` starts. */
export function msOf(frame: number, fps: Rate): number {
  return toSeconds(frame, fps) * 1000;
}

/** The frame on screen at `seconds` (`round` for the nearest frame boundary instead). */
export function toFrame(seconds: number, fps: Rate, round: (x: number) => number = Math.floor): number {
  return round((seconds * fps.num) / fps.den + (round === Math.floor ? 1e-6 : 0));
}
