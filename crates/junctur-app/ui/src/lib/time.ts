/** `h:mm:ss` for a 30 h recording, `m:ss` for short ones; `--:--` when unknown. */
export function clock(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "--:--";
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}
