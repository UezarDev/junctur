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
  /** There is a list the editor can change (src/editor). */
  editable: boolean;
  canUndo: boolean;
  canRedo: boolean;
  /** What the last edit or outside change did that the user should know. */
  notice: string | null;
  /** The speakers the edit list names (ids from 1). */
  speakers: Speaker[];
  /** Changes when the words' speaker labels would (turns changed on disk), not on a rename. */
  speakersKey: string;
  /** Caption settings (the edit list's `captions` line). */
  captions: Captions;
}

export interface Speaker {
  id: number;
  name: string;
  /** Caption colour, `#RRGGBB`. */
  color: string;
}

/** How the caption file is cut (src: junctur-core edit/captions.rs). */
export interface Captions {
  on: boolean;
  chars: number;
  lines: number;
  seconds: number;
}

/** A voice Junctur remembers (5b), used to name speakers in every recording. */
export interface Voice {
  name: string;
  color: string;
  /** Seconds of speech its voiceprint was made from. */
  seconds: number;
}

/** What a caption export wrote: the combined file and one per speaker (5b). */
export interface Exported {
  cues: number;
  speakers: { path: string; name: string; cues: number }[];
}

/** The transcript's words (src/editor/words.rs), sorted by start; times in ms. */
export interface TranscriptView {
  text: string[];
  start: number[];
  end: number[];
  /** Each word's speaker id, 0 for none. */
  speaker: number[];
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
  /** The recording, absolute: what its analysis progress is keyed by. */
  file: string;
  project: ProjectStatus | null;
  projectError: string | null;
  /** Its analysis, when one is waiting, running or ran in this session (2d). */
  analysis: Progress | null;
  /** Why its analysis is out of date with the settings now; empty when it is not. */
  outdated: string[];
}

/** A recording opened before (src/recent.rs). */
export interface Recent {
  path: string;
  name: string;
  folder: string;
  /** Unix seconds. */
  opened: number;
  missing: boolean;
}

export type Phase = "queued" | "running" | "done" | "failed";

/** An analysis run by the app (src/analysis.rs). */
export interface Progress {
  file: string;
  phase: Phase;
  /** The pipeline's last log line. */
  line: string | null;
  error: string | null;
  project: ProjectStatus | null;
}

export interface Reanalysed {
  progress: Progress | null;
  /** Nothing started: the edit list holds the user's edits, so ask first. */
  handEdited: boolean;
}

export const startupFile = () => invoke<string | null>("startup_file");
export const playerState = () => invoke<PlayerState>("player_state");
export const openFile = (path: string) => invoke<Opened>("open_file", { path });
export const recent = () => invoke<Recent[]>("recent");
export const forgetRecent = (path: string) => invoke<void>("forget_recent", { path });
export const analysis = (path: string) => invoke<Progress | null>("analysis", { path });
/** Analyses `path` again; without `replace`, an edit list with the user's edits starts nothing. */
export const reanalyse = (path: string, replace: boolean) =>
  invoke<Reanalysed>("reanalyse", { path, replace });
export const togglePause = () => invoke<void>("toggle_pause");
/** Resolves to the source time the seek lands on (inside a cut while skipping: the next range). */
export const seek = (seconds: number, exact: boolean) =>
  invoke<number>("seek", { seconds, exact });
export const setAudioTrack = (id: number) => invoke<void>("set_audio_track", { id });
export const setSkip = (on: boolean) => invoke<void>("set_skip", { on });
export const editState = () => invoke<EditState>("edit_state");
export const transcript = () => invoke<TranscriptView | null>("transcript");
/** Cuts (`keep` false) or keeps words `first..=last`, snapped into the pauses around them. */
export const editWords = (first: number, last: number, keep: boolean) =>
  invoke<void>("edit_words", { first, last, keep });
/** Cuts or keeps source frames `[start, end)` exactly. */
export const editFrames = (start: number, end: number, keep: boolean) =>
  invoke<void>("edit_frames", { start, end, keep });
/** Renames speaker `id` in the edit list (saved at once, not an undo step). */
export const renameSpeaker = (id: number, name: string) =>
  invoke<void>("rename_speaker", { id, name });
/** Sets speaker `id`'s caption colour (`#RRGGBB`). */
export const recolorSpeaker = (id: number, color: string) =>
  invoke<void>("recolor_speaker", { id, color });
export const setCaptions = (captions: Captions) => invoke<void>("set_captions", { captions });
/** Writes the captions as an SRT at `path`, and one per speaker beside it (`<name> (<speaker>).srt`). */
export const exportCaptions = (path: string) => invoke<Exported>("export_captions", { path });
export const voices = () => invoke<Voice[]>("voices");
/** Remembers `name`'s voice from words `first..=last` and relabels the speakers; resolves to a line for the user. */
export const enrollVoice = (first: number, last: number, name: string) =>
  invoke<string>("enroll_voice", { first, last, name });
export const forgetVoice = (name: string) => invoke<void>("forget_voice", { name });
export const undo = () => invoke<void>("undo");
export const redo = () => invoke<void>("redo");
/** The share of the window's width (0..1) on the right to keep free of video. */
export const setVideoMargin = (right: number) => invoke<void>("set_video_margin", { right });
export const uiLog = (message: string) => invoke<void>("ui_log", { message });

export const onPlayerState = (f: (s: PlayerState) => void): Promise<UnlistenFn> =>
  listen<PlayerState>("player-state", (e) => f(e.payload));

export const onEditState = (f: (s: EditState) => void): Promise<UnlistenFn> =>
  listen<EditState>("edit-state", (e) => f(e.payload));

export const onAnalysis = (f: (p: Progress) => void): Promise<UnlistenFn> =>
  listen<Progress>("analysis", (e) => f(e.payload));
