// The Rust side's commands and events (src/commands.rs, src/player.rs), typed.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface AudioTrack {
  id: number;
  title: string | null;
  lang: string | null;
  codec: string | null;
  channels: number | null;
  selected: boolean;
}

export interface PlayerState {
  path: string | null;
  position: number | null;
  duration: number | null;
  paused: boolean;
  audioTracks: AudioTrack[];
  hwdec: string | null;
  error: string | null;
  /** Only the kept ranges are playing (the edit list's EDL). */
  cuts: boolean;
  /** Output (edited) time and length, when there is an edit list. `position` is source time. */
  output: number | null;
  outputDuration: number | null;
  /** Source and output frame on screen (with an edit list), rounded by the engine. */
  frame: number | null;
  outputFrame: number | null;
  /** The source position is inside a cut (only while the whole file plays). */
  inCut: boolean;
}

export interface Fps {
  num: number;
  den: number;
}

export interface EditSummary {
  ranges: number;
  kept: number;
  duration: number;
}

/** The edit list as the timeline draws it (src/cuts/mod.rs `EditState`). */
export interface EditState {
  skip: boolean;
  fps: Fps | null;
  /** Kept source frames, flattened: [start0, end0, start1, end1, ...]. */
  ranges: number[];
  totalFrames: number;
  outputFrames: number;
  summary: EditSummary | null;
  error: string | null;
}

export type StageState = "pending" | "running" | "done" | "failed";

export interface ProjectStatus {
  dir: string;
  stages: { name: string; state: StageState }[];
  transcript: boolean;
  edit: EditSummary | null;
  editError: string | null;
}

export interface Opened {
  project: ProjectStatus | null;
  projectError: string | null;
}

export const startupFile = () => invoke<string | null>("startup_file");
export const playerState = () => invoke<PlayerState>("player_state");
export const openFile = (path: string) => invoke<Opened>("open_file", { path });
export const togglePause = () => invoke<void>("toggle_pause");
/** Resolves to the source time the seek lands on (inside a cut while skipping: the next range). */
export const seek = (seconds: number, exact: boolean) =>
  invoke<number>("seek", { seconds, exact });
export const setAudioTrack = (id: number) => invoke<void>("set_audio_track", { id });
export const setSkip = (on: boolean) => invoke<void>("set_skip", { on });
export const editState = () => invoke<EditState>("edit_state");

export const onPlayerState = (f: (s: PlayerState) => void): Promise<UnlistenFn> =>
  listen<PlayerState>("player-state", (e) => f(e.payload));

export const onEditState = (f: (s: EditState) => void): Promise<UnlistenFn> =>
  listen<EditState>("edit-state", (e) => f(e.payload));
