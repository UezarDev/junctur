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
  /** Changes when the list's fixed words do (5h): the words' text is fetched again. */
  fixesKey: string;
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
  /** 5m: with a template of the user's, keep its own look; Junctur sets only text, timing and colour. */
  keepLook: boolean;
  /** 5n: what a send does with clips Junctur did not place in its captions' way; "ask" unless this
   * recording remembered a choice. Never a default (Settings saves it as "ask"). */
  existingClips: ExistingClips;
  /** 5j's text options: what every caption output shows, never the transcript. */
  case: CaptionCase;
  /** Punctuation kept (true) or removed. */
  punctuation: boolean;
  /** Words or phrases masked, whole words, case aside. */
  censor: string[];
  censorStyle: CensorStyle;
  /** Find and replace, whole words or phrases, case aside; an empty `to` removes them. */
  replace: { from: string; to: string }[];
  /** The language captions are shown in (5k), a code of `translationLanguages`; empty for as spoken. */
  translate: string;
  /** 5l, Resolve and the preview only: the word being said lit up, word by word. */
  highlight: boolean;
  /** `#RRGGBB`; empty for the speaker's own colour (the caption then white). */
  highlightColor: string;
  /** How a caption comes in and goes out. */
  animation: CaptionAnimation;
  /** Seconds, 0.05 to 1. */
  animationLength: number;
}

export type CaptionAnimation = "none" | "fade" | "pop" | "slide";

export type CaptionCase = "asSpoken" | "upper" | "lower" | "title";
export type CensorStyle = "middle" | "whole";

/** 5n: clips Junctur did not place where its captions go in Resolve: ask, delete them, leave
 * those captions out, or put the speaker's new captions on a track beside. */
export type ExistingClips = "ask" | "replace" | "skip" | "newTrack";

export type CaptionLayout = "stacked" | "side" | "topBottom";

/** A caption as the SRT export cuts it (junctur-core export/srt.rs), output seconds; one part per
 * speaker talking in it, so several only where people talk at once (5e), each placed where the
 * Resolve send puts it (5g, export/layout.rs): the centre as fractions of the picture, x from the
 * left, y from the bottom. */
export interface Cue {
  start: number;
  end: number;
  parts: { speaker: number; lines: string[]; x: number; y: number; words: WordSpan[] }[];
}

/** Where a word of a caption sits on its lines and when it is lit (5l, export/layout.rs), only with
 * the highlight on: `from..to` in characters (code points) of line `line`, a word as the animated
 * caption counts them (a phrase made one by find and replace is a word each, sharing its time). */
export interface WordSpan {
  word: number;
  line: number;
  from: number;
  to: number;
  start: number;
  end: number;
}

/** What a send to Resolve did (junctur-core resolve/reply.rs). */
export interface Sent {
  placed: number;
  restyled: number;
  removed: number;
  failed: number;
  firstError: string | null;
  /** Text+ inputs the template did not take (a user's own title may lack some). */
  unstyled: string[];
  /** The timeline the send made (5z). */
  timeline: string;
  /** 5n: clips Junctur did not place where new captions go; when not 0 no caption was placed and
   * the user is asked. `blocked`: the captions they are in the way of. A send's captions go on the
   * timeline it has just made (5z), whose caption tracks are new, so a send does not meet these. */
  inTheWay: number;
  blocked: number;
  /** 5n: what the choice did: clips deleted, captions left out, speakers put on a track beside. */
  replacedClips: number;
  skipped: number;
  newTracks: number;
  /** Whether it replaced one of that name. */
  replaced: boolean;
  /** Whether only the captions were updated on the timeline already there (5ab). */
  updated: boolean;
  /** What went wrong making it without stopping the send (an old timeline not deleted), or on an
   * update a cut that no longer matches the edit's. */
  note: string | null;
}

/** A Whisper model the Settings dialog offers (src/settings.rs). */
export interface ModelChoice {
  /** What the config file holds. */
  key: string;
  /** The readable name ("Fast"). */
  name: string;
  /** The line under it in the picker (2v). */
  summary: string;
  /** Which icon the picker draws. */
  family: "auto" | "turbo" | "large";
  /** The Whisper model it loads ("large-v3-turbo"). */
  model: string;
  note: string;
}

/** What the Settings dialog edits (2i), kept in junctur/config.json. */
export interface Settings {
  whisperModel: string;
  /** The caption settings a recording without its own starts from. */
  captions: Captions;
  /** The word list Whisper is told (5h), first entries first. */
  words: string[];
  /** The transcript flags words Whisper was less sure of than this many percent (5o); 0 for none. */
  flagBelow: number;
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
  /** How sure Whisper was of each word (5o), whole percent; 255 when it is never flagged (an older
   * transcript, or a word the user fixed or confirmed). */
  sure: number[];
  /** The audio track each word was heard on (5x). */
  track: number[];
  /** The span fixed as one text each word is in (5x), 1-based into `spans`, 0 for none; such a
   * word's `text` is what was heard. */
  span: number[];
  /** Each span's text. */
  spans: string[];
  /** 2w repair: each word's start as heard, in ms, sent only when some word is retimed (else
   * `start` is it): words starting together (5w) are grouped by it, as fixes name them. */
  heard?: number[];
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
  /** A fresh cut was asked for and nothing started: the edit list holds the user's edits, so ask first. */
  handEdited: boolean;
}

export const startupFile = () => invoke<string | null>("startup_file");
export const playerState = () => invoke<PlayerState>("player_state");
export const openFile = (path: string) => invoke<Opened>("open_file", { path });
/** The recording the player has open, as `openFile` gave it, without opening it again: for a page
 * that started over while the app went on (2q: a reload). */
export const currentRecording = () => invoke<Opened | null>("current_recording");
export const recent = () => invoke<Recent[]>("recent");
export const forgetRecent = (path: string) => invoke<void>("forget_recent", { path });
export const analysis = (path: string) => invoke<Progress | null>("analysis", { path });
/**
 * Analyses `path` again (5s): the user's edit list is kept, with the speakers matched again against
 * the remembered voices. With `fresh` the list is re-cut instead; then, without `replace`, a list with
 * the user's edits starts nothing.
 */
export const reanalyse = (path: string, fresh: boolean, replace: boolean) =>
  invoke<Reanalysed>("reanalyse", { path, fresh, replace });
export const transcription = () => invoke<Transcription>("transcription");
/** Pauses or resumes the transcription lane, from any recording (2k). */
export const pauseTranscription = (paused: boolean) =>
  invoke<Transcription>("pause_transcription", { paused });
export const togglePause = () => invoke<void>("toggle_pause");
/** Resolves to the source time the seek lands on (inside a cut while skipping: the next range). */
export const seek = (seconds: number, exact: boolean) =>
  invoke<number>("seek", { seconds, exact });
/** 2o: plays source seconds `start` to `end` on a loop from `start`. The first call puts the player
 * in review (the whole file plays, Skip cuts off and refused) until `endReview`, which also puts
 * the speed to 1x and Skip cuts back; opening or closing a recording ends it too. `pauseReview`
 * stops the loop and pauses, still in review. */
export const playBetween = (start: number, end: number) => invoke<void>("play_between", { start, end });
/** 2s: plays from source second `at`, unpaused (after a cut's edge is dragged, from just before it).
 * Refused in review. */
export const playFrom = (at: number) => invoke<void>("play_from", { at });
export const pauseReview = () => invoke<void>("pause_review");
export const endReview = () => invoke<void>("end_review");
/** Playback speed, 0.25 to 4 (2o). */
export const setSpeed = (speed: number) => invoke<void>("set_speed", { speed });
export const mixer = () => invoke<MixerState>("mixer");
/** Mutes or unmutes track `track` (0-based) in the preview; remembered for the recording. */
export const setMuted = (track: number, muted: boolean) =>
  invoke<void>("set_muted", { track, muted });
export const setSkip = (on: boolean) => invoke<void>("set_skip", { on });
export const editState = () => invoke<EditState>("edit_state");
export const transcript = () => invoke<TranscriptView | null>("transcript");
/** Cuts (`keep` false) or keeps every run of words (`[first, last]` each), snapped into the pauses
 * around them, as one undo step (2p). */
export const editWords = (runs: [number, number][], keep: boolean) =>
  invoke<void>("edit_words", { runs, keep });
/** Cuts or keeps source frames `[start, end)` exactly. */
export const editFrames = (start: number, end: number, keep: boolean) =>
  invoke<void>("edit_frames", { start, end, keep });
/** 2w: moves a cut's edge, at frame `from` (one end of `[start, end)`), to the other end by that
 * frame edit, and the words beside the edge with it, as one undo step. An edit that moves no edge
 * of the list (review's clips are not always its ranges) is the frame edit alone. */
export const moveEdge = (step: { start: number; end: number; keep: boolean }, from: number) =>
  invoke<void>("move_edge", { start: step.start, end: step.end, keep: step.keep, from });
/** Renames speaker `id` in the edit list (saved at once, not an undo step). */
export const renameSpeaker = (id: number, name: string) =>
  invoke<void>("rename_speaker", { id, name });
/** A word fix as the edit list holds it (5h, 5w, 5x): handed back to `keepHeard` as it came. */
export interface Fix {
  track: number;
  start: number;
  nth: number;
  through?: { start: number; nth: number };
  text: string;
}
/** What a fix did besides itself (5aa): the entries the word list gained, and the fixes also made on
 * the words heard like it with what each was heard as ("Also fixed N other places", whose Undo is
 * `keepHeard(others, heard)`). */
export interface Fixed {
  learned: string[];
  others: Fix[];
  heard: string[];
}
/** Fixes the spelling of word `index` (5h; empty or the heard text again removes the fix), and every
 * other word heard like it (5aa). Saved at once, not an undo step. `at` is the word's heard start
 * (`words.heard`) when its field opened: refused when that index names another word now (5ae). */
export const fixWord = (index: number, at: number, text: string) =>
  invoke<Fixed>("fix_word", { index, at: Math.round(at), text });
/** Marks word `index` as checked (5o): it keeps what Whisper heard and is no longer flagged. Saved at
 * once, not an undo step. */
export const confirmWord = (index: number) => invoke<void>("confirm_word", { index });
/** Fixes words `first..=last` as one text (5x; empty or the heard words again clears their fixes),
 * and the same words heard elsewhere (5aa). Refused, saying why, across two voice tracks or two
 * people. Saved at once, not an undo step. */
export const fixWords = (first: number, last: number, at: [number, number], text: string) =>
  invoke<Fixed>("fix_words", { first, last, at: [Math.round(at[0]), Math.round(at[1])], text });
/** A fix of the recording's list where the transcript has it (5ae: the list of fixes). */
export interface ListedFix {
  fix: Fix;
  /** Its first and last words' indices; null when it is on no word now. */
  first: number | null;
  last: number | null;
  /** Its words as heard. */
  heard: string;
  /** A fix of the heard text (Undo those, a removal, Looks right): the words read as heard. */
  asHeard: boolean;
}
/** Every fix of the open recording's list with its words (5ae). */
export const fixesList = () => invoke<ListedFix[]>("fixes_list");
/** Takes `fixes` (as `fixesList` gave them) back to what their words were heard as, those still as
 * listed; how many (5ae). Saved at once, not an undo step. */
export const removeFixes = (fixes: Fix[]) => invoke<number>("remove_fixes", { fixes });
/** Takes back fixes a fix made on other words (5aa), those still as made: the words keep what they
 * were heard as, and no later spread or Correct gives them the fix again. */
export const keepHeard = (others: Fix[], heard: string[]) => invoke<number>("keep_heard", { others, heard });
/** A fix a re-transcription could not put on any word (5aa): Whisper hears something else there now. */
export interface SetAside {
  track: number;
  /** Source seconds. */
  at: number;
  heard: string;
  text: string;
  /** A word time set by moving a cut's edge (5ad), not a fix: the start and end it was given. */
  retimed?: [number, number];
  /** Its track has no words in the new transcript at all: not heard, rather than heard otherwise. */
  silent?: boolean;
  /** Not a fix: what Correct could not do, in a sentence; the other fields are empty. */
  note?: string;
}
/** The open recording's fixes set aside by a re-transcription, until `forgetFixesSetAside`. */
export const fixesSetAside = () => invoke<SetAside[]>("fixes_set_aside");
/** Forgets `seen`, as `fixesSetAside` gave them: any set aside since stay listed (5ad). */
export const forgetFixesSetAside = (seen: SetAside[]) => invoke<void>("forget_fixes_set_aside", { seen });
/** What the Correct button started (5aa). */
export interface Corrected {
  progress: Progress;
  /** The fixes made on words heard like a fixed one, and what each was heard as. */
  others: Fix[];
  heard: string[];
  /** Heard texts fixed two ways, left as heard. */
  unclear: string[];
}
/** The Correct button (5aa): every fix given to the words heard like it, then the open recording's
 * speakers heard again and its words transcribed again with the word list, the user's edits kept. */
export const correct = () => invoke<Corrected>("correct");
/** Sets speaker `id`'s caption colour (`#RRGGBB`). */
export const recolorSpeaker = (id: number, color: string) =>
  invoke<void>("recolor_speaker", { id, color });
export const setCaptions = (captions: Captions) => invoke<void>("set_captions", { captions });
/** Writes the captions as one SRT at `path`, people talking at once stacked in one caption (5e). */
export const exportCaptions = (path: string) => invoke<Exported>("export_captions", { path });
/** The captions as the export cuts them, for the preview over the picture (5d), placed on a
 * picture `aspect` (height over width) in shape (5g). */
export const captionCues = (aspect: number) => invoke<Cue[]>("caption_cues", { aspect });
/** What to do when Resolve already has the timeline a send makes (5z): make another beside it,
 * update only the captions on it (5ab), or replace it. */
export type TimelineAnswer = "new" | "update" | "replace";
/** What a send came to (junctur-core resolve/mod.rs): sent, or held back because Resolve already
 * has a timeline called `name` (nothing changed; send again with an answer). */
export type SendOutcome = { kind: "exists"; name: string } | ({ kind: "sent" } & Sent);
/** Sends the edit into DaVinci Resolve as a timeline of its own, captions on it as Text+ clips,
 * through the Junctur script there (5z, 5g). `existing` (5n) answers, for this send, a send that
 * found clips Junctur did not place in the captions' way. */
export const sendToResolve = (answer: TimelineAnswer | null = null, existing?: ExistingClips) =>
  invoke<SendOutcome>("send_to_resolve", { answer, existing: existing ?? null });
/** A caption translation run in the background (5k, src/translation.rs). */
export interface TranslationRun {
  /** The recording, as opened. */
  file: string;
  to: string;
  done: number;
  total: number;
  running: boolean;
  cancelled: boolean;
  /** Sentences the run that ended left untranslated: no transcript language, or no usable translation. */
  unknown: number;
  failed: number;
  error: string | null;
}
export interface TranslationStatus {
  run: TranslationRun;
  /** The open recording's caption sentences with no translation yet, of `sentences`. */
  missing: number;
  sentences: number;
}
export interface Language {
  code: string;
  name: string;
}
/** Translates the open recording's captions in the background, into their `translate` language. */
export const translateCaptions = () => invoke<TranslationRun>("translate_captions");
export const cancelTranslation = () => invoke<void>("cancel_translation");
export const translationStatus = () => invoke<TranslationStatus>("translation_status");
let languages: Promise<Language[]> | null = null;
/** The languages captions can be translated into, by name; asked once. */
export const translationLanguages = () => (languages ??= invoke<Language[]>("translation_languages"));
/** The Fusion Titles in the open Resolve project's media pool by clip name (5m), Junctur's own left out. */
export const resolveTemplates = () => invoke<string[]>("resolve_templates");
export const voices = () => invoke<Voice[]>("voices");
/** Remembers `name`'s voice from every run of words (`[first, last]` each, 2p: one enrolment) and
 * relabels the speakers; resolves to a line for the user. */
export const enrollVoice = (runs: [number, number][], name: string) =>
  invoke<string>("enroll_voice", { runs, name });
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

export const onTranslation = (f: (r: TranslationRun) => void): Promise<UnlistenFn> =>
  listen<TranslationRun>("translation", (e) => f(e.payload));

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
