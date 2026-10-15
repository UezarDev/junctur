/**
 * Where an analysis run stands, for the Properties tab, the chip on the picture and the start
 * window's rows: one place for the step names and the arithmetic.
 */
import type * as api from "./api";

/** What each pipeline stage is called while it runs. */
export const STEP: Record<string, string> = {
  tracks: "Reading tracks",
  speech: "Finding speech",
  speakers: "Finding speakers",
  transcribe: "Transcribing",
  edit: "Cutting",
  labels: "Labelling the silent parts",
};

/** The stage running now, while the run is. */
export function running(p: api.Progress | null): string | null {
  if (p?.phase !== "running") return null;
  return p.project?.stages.find((s) => s.state === "running")?.name ?? null;
}

/** The running stage's tracks, in track order; empty for a stage with no per-track measure. */
export function current(p: api.Progress | null): api.Step[] {
  const stage = running(p);
  return stage ? (p?.steps ?? []).filter((s) => s.stage === stage) : [];
}

/** A step's share done, 0-100, whole: never 100 before it is. */
export function percent(s: api.Step): number {
  if (s.of <= 0) return 100;
  return Math.floor(Math.min(1, Math.max(0, s.at / s.of)) * 100);
}

/** The stage's whole: every track's seconds behind it over their total length. */
export function whole(steps: api.Step[]): number | null {
  const of = steps.reduce((n, s) => n + Math.max(0, s.of), 0);
  if (!steps.length || of <= 0) return null;
  return percent({ stage: "", track: 0, at: steps.reduce((n, s) => n + Math.min(s.of, s.at), 0), of });
}

/** A run is waiting or running, on either of its lanes. */
export function busy(p: api.Progress | null): boolean {
  return p?.phase === "queued" || p?.phase === "running" || p?.phase === "waiting";
}

/** Where a run is, as a number that only grows through its life: running again after waiting to
 * transcribe is later than waiting, so an older event answered late never moves it back. */
export function rank(p: api.Progress | null): number {
  if (!p) return 0;
  if (p.phase === "running") return p.listed ? 4 : 2;
  return { queued: 1, waiting: 3, done: 5, failed: 5 }[p.phase];
}

/** Whether the transcription lane holds `file` paused: `pausing` while it finishes its chunk,
 * `paused` once stopped; null when it is not the one transcribing or the lane is not paused. */
export function paused(lane: api.Transcription | null, file: string | null | undefined): "pausing" | "paused" | null {
  if (!lane?.paused || !file || lane.jobs[0]?.file !== file) return null;
  return lane.stopped ? "paused" : "pausing";
}

/** One line on the run: "Transcribing · 43%", "Finding speech…", "Waiting for another analysis",
 * "Waiting to transcribe", "Transcription paused · 43%"; null when nothing is running. */
export function label(p: api.Progress | null, pause: "pausing" | "paused" | null = null): string | null {
  if (p?.phase === "queued") return "Waiting for another analysis";
  if (p?.phase === "waiting") return "Waiting to transcribe";
  if (p?.phase !== "running") return null;
  const stage = running(p);
  let name = stage ? (STEP[stage] ?? stage) : "Analysing";
  if (pause) name = pause === "paused" ? "Transcription paused" : "Pausing transcription";
  const pct = whole(current(p));
  return pct == null ? `${name}…` : `${name} · ${pct}%`;
}
