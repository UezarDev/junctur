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
  /** The picture as shown (rotation applied); null without video. */
  width: number | null;
  height: number | null;
  /** Width over height: below 1 the editor lays out for portrait (2g). */
  aspect: number | null;
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
  /** Caption settings: the edit list's `captions` line, else the user's defaults (2i). */
  captions: Captions;
  /** The list has its own caption settings; without, it follows the defaults. */
  captionsOwn: boolean;
}

export interface Speaker {
  id: number;
  name: string;
  /** Caption colour, `#RRGGBB`. */
  color: string;
  /** Their audio track (0-based) when speakers were told apart by track. */
  track: number | null;
}

/** Who talks when (source seconds, speaker ids from 1), sorted by start; turns of different
 * speakers overlap. */
export interface Turns {
  start: number[];
  end: number[];
  speaker: number[];
}

/** How far the open recording's waveforms are (src/waves.rs). */
export interface WavesState {
  file: string;
  tracks: number;
  seconds: number;
  duration: number | null;
  done: boolean;
  error: string | null;
}

/** Each track's peak per column (0..255 for -60..0 dBFS), track by track. */
export interface WavesView {
  columns: number;
  tracks: number;
  peaks: Uint8Array;
}

/** A recent recording's length and analysis (src/project.rs). */
export interface RecordingInfo {
  duration: number | null;
  ranges: number | null;
  stagesDone: number;
  stages: number;
}

/** How the caption file is cut (src: junctur-core edit/captions.rs). */
export interface Captions {
  /** Export: a render writes the SRT beside the video. */
  on: boolean;
  /** Show the captions over the picture as it plays (5d). */
  preview: boolean;
  chars: number;
  lines: number;
  /** Most words per speaker in a caption, 0 for no limit, 1 for one word at a time (5i). */
  words: number;
  seconds: number;
  /** Every caption bold / italic in the SRT (5d: Resolve's import applies both). */
  bold: boolean;
  italic: boolean;
  /** Every caption underlined (5f: Uezar's import showed Resolve underlines in the speaker's colour). */
  underline: boolean;
  /** 5g's look, for the Resolve Text+ send and the preview; the SRT ignores it. */
  font: string;
  /** Text height in % of the picture height. */
  size: number;
  /** Picture bottom to caption bottom, % of the picture height. */
  position: number;
  outline: boolean;
  /** 1 to 10. */
  outlineWidth: number;
  outlineColor: string;
  /** Where people talking at once go. */
  layout: CaptionLayout;
  /** A speaker's name: they keep the normal spot when people talk at once; empty for none. */
  protagonist: string;
  /** A Fusion Title's clip name in Resolve's media pool; empty for Junctur's own Text+. */
  template: string;
}

export type CaptionLayout = "stacked" | "side" | "topBottom";

/** A caption as the SRT export cuts it (junctur-core export/srt.rs), output seconds; one part per
 * speaker talking in it, so several only where people talk at once (5e), each placed where the
 * Resolve send puts it (5g, export/layout.rs): the centre as fractions of the picture, x from the
 * left, y from the bottom. */
export interface Cue {
  start: number;
  end: number;
  parts: { speaker: number; lines: string[]; x: number; y: number }[];
}

/** What a send to Resolve did (junctur-core resolve/reply.rs). */
export interface Sent {
  placed: number;
  restyled: number;
  removed: number;
  failed: number;
  firstError: string | null;
  timeline: string;
}

/** A Whisper model the Settings dialog offers (src/settings.rs). */
export interface ModelChoice {
  key: string;
  label: string;
  note: string;
}

/** What the Settings dialog edits (2i), kept in junctur/config.json. */
export interface Settings {
  whisperModel: string;
  /** The caption settings a recording without its own starts from. */
  captions: Captions;
}

export interface SettingsView {
  settings: Settings;
  models: ModelChoice[];
  /** The config file; null with no config folder. */
  path: string | null;
  /** Why the file could not be read (saving replaces it). */
  error: string | null;
}

/** A voice Junctur remembers (5b), used to name speakers in every recording. */
export interface Voice {
  name: string;
  color: string;
  /** Seconds of speech its voiceprint was made from. */
  seconds: number;
}

/** What a caption export wrote: one file since 5e. */
export interface Exported {
  cues: number;
  /** Speaker files beside it from an earlier export (5b wrote one per speaker). */
  stale: string[];
}

/** The transcript's words (src/editor/words.rs), sorted by start; times in ms. */
export interface TranscriptView {
  text: string[];
  start: number[];
  end: number[];
  /** Each word's speaker id, 0 for none. */
  speaker: number[];
  /** Someone else saying the word too (two people at once), 0 for none. */
  also: number[];
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

/** A distinct track's role from the track analysis (junctur-core tracks/mod.rs). */
export type Role = "mix" | "voice" | "game_pc" | "music" | "silent" | "unknown";

/** One audio track in the preview's mix (src/mixer.rs). */
export interface MixTrack {
  /** 0-based; the play bar shows it from 1. */
  index: number;
  muted: boolean;
  /** The user set this mute; the roles' defaults no longer move it. */
  set: boolean;
  role: Role | null;
  /** The track this one is a copy of. */
  copyOf: number | null;
  /** Soloed (2g): while any track is, only the soloed ones are heard. Not remembered. */
  solo: boolean;
}

/** Every track plays at once (2e); where the mutes came from. */
export interface MixerState {
  tracks: MixTrack[];
  /** `provisional`: no roles yet, track 1 only; `roles`: their defaults; `remembered`: the user's. */
  source: "provisional" | "roles" | "remembered";
}

/** `queued`: behind another recording's first steps; `waiting`: its edit list is made (editable) and its
 * transcription waits for another recording's (2j). */
export type Phase = "queued" | "running" | "waiting" | "done" | "failed";

/** How far one track has got through one stage (junctur-core pipeline/steps.rs, 2h): `at` seconds
 * of the track's `of` are behind it. */
export interface Step {
  stage: string;
  /** 0-based audio ordinal. */
  track: number;
  at: number;
  of: number;
}

/** An analysis run by the app (src/analysis.rs). */
export interface Progress {
  file: string;
  phase: Phase;
  /** The pipeline's last log line. */
  line: string | null;
  error: string | null;
  project: ProjectStatus | null;
  /** Per track, the latest stage it reported; only the running stage's are current (lib/steps.ts). */
  steps: Step[];
  /** The run's edit list is made: it can be edited while the rest runs (2j). */
  listed: boolean;
  /** While transcribing: every word before this many seconds is in, on every track (2j). */
  transcribed: number | null;
}

/** The transcription lane, whichever recording is open (src/analysis.rs, 2k). */
export interface Transcription {
  /** Pause was asked for: the transcribing recording stops after its chunk, the rest wait. */
  paused: boolean;
  /** It has stopped (no longer finishing a chunk). */
  stopped: boolean;
  /** The transcribing recording first, then the ones waiting, in the order they will run. */
  jobs: Progress[];
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
export const transcription = () => invoke<Transcription>("transcription");
/** Pauses or resumes the transcription lane, from any recording (2k). */
export const pauseTranscription = (paused: boolean) =>
  invoke<Transcription>("pause_transcription", { paused });
export const togglePause = () => invoke<void>("toggle_pause");
/** Resolves to the source time the seek lands on (inside a cut while skipping: the next range). */
export const seek = (seconds: number, exact: boolean) =>
  invoke<number>("seek", { seconds, exact });
export const mixer = () => invoke<MixerState>("mixer");
/** Mutes or unmutes track `track` (0-based) in the preview; remembered for the recording. */
export const setMuted = (track: number, muted: boolean) =>
  invoke<void>("set_muted", { track, muted });
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
/** Writes the captions as one SRT at `path`, people talking at once stacked in one caption (5e). */
export const exportCaptions = (path: string) => invoke<Exported>("export_captions", { path });
/** The captions as the export cuts them, for the preview over the picture (5d). */
export const captionCues = () => invoke<Cue[]>("caption_cues");
/** Sends the captions into DaVinci Resolve as Text+ clips through the Junctur script there (5g). */
export const sendToResolve = () => invoke<Sent>("send_to_resolve");
/** Installs the Junctur script into Resolve's Scripts folder; resolves to its path. */
export const installResolveScript = () => invoke<string>("install_resolve_script");
export const voices = () => invoke<Voice[]>("voices");
/** Remembers `name`'s voice from words `first..=last` and relabels the speakers; resolves to a line for the user. */
export const enrollVoice = (first: number, last: number, name: string) =>
  invoke<string>("enroll_voice", { first, last, name });
export const forgetVoice = (name: string) => invoke<void>("forget_voice", { name });
/** Hears the open recording's speakers again from the audio, cut at the seconds pinned to people (5c). Slow. */
export const reanalyzeSpeakers = () => invoke<void>("reanalyze_speakers");
export const undo = () => invoke<void>("undo");
export const redo = () => invoke<void>("redo");
/** The shares of the window (0..1) on the right, at the bottom and at the top (the title bar) to
 * keep free of video. */
export const setVideoMargins = (right: number, bottom: number, top: number) =>
  invoke<void>("set_video_margins", { right, bottom, top });
export const setSolo = (track: number, on: boolean) => invoke<void>("set_solo", { track, on });
export const turns = () => invoke<Turns>("turns");
export const wavesState = () => invoke<WavesState | null>("waves_state");
/** Each track's peak in `columns` equal stretches of `[from, to)` seconds; null with no recording
 * open. Sent as raw bytes (src/waves.rs `WavesView::bytes`): two little-endian u32s, the column and
 * track counts, then the peaks track by track. */
export async function waves(from: number, to: number, columns: number): Promise<WavesView | null> {
  const b = await invoke<ArrayBuffer>("waves", { from, to, columns });
  if (b.byteLength < 8) return null;
  const head = new DataView(b, 0, 8);
  return { columns: head.getUint32(0, true), tracks: head.getUint32(4, true), peaks: new Uint8Array(b, 8) };
}
export const recentInfo = (path: string) => invoke<RecordingInfo>("recent_info", { path });
/** The recording's still, JPEG bytes. */
export const thumbnail = (path: string) => invoke<ArrayBuffer>("thumbnail", { path });
/** Opens the open recording's project folder in the file manager. */
export const reveal = () => invoke<void>("reveal");
/** Closes the open recording: back to the start window (2i). */
export const closeFile = () => invoke<void>("close_file");
export const settings = () => invoke<SettingsView>("settings");
/** Writes the settings to junctur/config.json; never analyses anything. */
export const saveSettings = (settings: Settings) => invoke<SettingsView>("save_settings", { settings });
/** The Captions panel's "Save as default". */
export const saveCaptionDefaults = (captions: Captions) =>
  invoke<SettingsView>("save_caption_defaults", { captions });
/** Why `path`'s analysis is out of date with the settings now (2d's notice). */
export const outdated = (path: string) => invoke<string[]>("outdated", { path });
/** Windows' snap layouts for the window (the pointer resting on the title bar's maximise button). */
export const snapLayouts = () => invoke<void>("snap_layouts");
export const uiLog = (message: string) => invoke<void>("ui_log", { message });

export const onPlayerState = (f: (s: PlayerState) => void): Promise<UnlistenFn> =>
  listen<PlayerState>("player-state", (e) => f(e.payload));

export const onEditState = (f: (s: EditState) => void): Promise<UnlistenFn> =>
  listen<EditState>("edit-state", (e) => f(e.payload));

export const onAnalysis = (f: (p: Progress) => void): Promise<UnlistenFn> =>
  listen<Progress>("analysis", (e) => f(e.payload));

export const onTranscription = (f: (t: Transcription) => void): Promise<UnlistenFn> =>
  listen<Transcription>("transcription", (e) => f(e.payload));

export const onMixer = (f: (s: MixerState) => void): Promise<UnlistenFn> =>
  listen<MixerState>("mixer-state", (e) => f(e.payload));

/** Each track's peak in dBFS, about 20 times a second while playing (none while paused). */
export const onLevels = (f: (levels: (number | null)[]) => void): Promise<UnlistenFn> =>
  listen<{ levels: (number | null)[] }>("levels", (e) => f(e.payload.levels));

export const onWaves = (f: (s: WavesState) => void): Promise<UnlistenFn> =>
  listen<WavesState>("waves", (e) => f(e.payload));

/** A menu item chosen by test mode's `menu=<id>` (src/testmode.rs); ids as in menus.ts. */
export const onMenu = (f: (id: string) => void): Promise<UnlistenFn> =>
  listen<string>("menu", (e) => f(e.payload));
