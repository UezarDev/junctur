/**
 * Where an analysis run stands, for the three places that show it (2h): the Properties tab, the
 * chip on the picture and the start window's rows. One place for the step names and the
 * arithmetic, which 2g had as a regex on the pipeline's log line in each of them.
 */
import type * as api from "./api";

/** What each pipeline stage is called while it runs. */
export const STEP: Record<string, string> = {
  tracks: "Reading tracks",
  speech: "Finding speech",
  speakers: "Finding speakers",
  transcribe: "Transcribing",
  edit: "Cutting",
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

/** A run is waiting or running, on either of its lanes (2j). */
export function busy(p: api.Progress | null): boolean {
  return p?.phase === "queued" || p?.phase === "running" || p?.phase === "waiting";
}

/** Where a run is, as a number that only grows through its life: running again after waiting to
 * transcribe is later than waiting (2j), so an older event answered late never moves it back. */
export function rank(p: api.Progress | null): number {
  if (!p) return 0;
  if (p.phase === "running") return p.listed ? 4 : 2;
  return { queued: 1, waiting: 3, done: 5, failed: 5 }[p.phase];
}

/** One line on the run: "Transcribing · 43%", "Finding speech…", "Waiting for another analysis",
 * "Waiting to transcribe"; null when nothing is running. */
export function label(p: api.Progress | null): string | null {
  if (p?.phase === "queued") return "Waiting for another analysis";
  if (p?.phase === "waiting") return "Waiting to transcribe";
  if (p?.phase !== "running") return null;
  const stage = running(p);
  const name = stage ? (STEP[stage] ?? stage) : "Analysing";
  const pct = whole(current(p));
  return pct == null ? `${name}…` : `${name} · ${pct}%`;
}
