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
 * `h:mm:ss:ff`: the frame on screen at `seconds`, counted the way Resolve's non-drop timecode
 * counts it (whole frames, `ceil(fps)` to a second), so it names the same frame as the render and
 * the FCPXML timeline. `--:--:--:--` when unknown.
 */
export function timecode(seconds: number | null | undefined, fps: { num: number; den: number }): string {
  if (seconds == null || !Number.isFinite(seconds)) return "--:--:--:--";
  // Same small bias as the engine's frame rounding, for float error at frame starts.
  const frame = Math.max(0, Math.floor((seconds * fps.num) / fps.den + 1e-6));
  const base = Math.ceil(fps.num / fps.den);
  const ff = frame % base;
  const s = Math.floor(frame / base);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${Math.floor(s / 3600)}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}:${pad(ff)}`;
}
