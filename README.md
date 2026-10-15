# Junctur

Junctur is a video editing engine for long screen and camera recordings, written in Rust.

## Status

Early. These pieces work:

- `junctur tracks` reads a recording and reports, per audio track, what it holds (mix, voice, game/PC,
  silent, duplicates of another track), using speech detection (Silero VAD) to decide.
- `junctur separate` splits a mix track into a vocal stem and the rest (MDX-Net), then finds speech
  regions and speakers (CAM++ embeddings) on the vocal stem. Stems go to the cache, never beside the input.
  It is opt-in: nothing else depends on it.
- `junctur analyze` runs the pipeline on a recording: track roles, speech regions (VAD) on every track that
  carries speech, read as-is, a Whisper transcript of only those regions (large-v3-turbo by default) (Spanish with English,
  the language detected once per ~30 s of speech; word timestamps), and a first automatic cut. Every stage is cached and an interrupted run resumes.
  The result is an edit list: a plain text file of ranges to keep plus a score per 5 s window, meant to be
  read, diffed and hand-edited. `junctur cut` re-cuts with other padding, `junctur status` shows the
  stages, and `junctur export --fcpxml` writes a timeline DaVinci Resolve imports.
- `junctur render` turns the edit list into an MP4 by smart rendering: only the frames between each cut
  and the nearest keyframe are re-encoded (NVENC, matched to the source's profile, level and color tags);
  everything between keyframes is copied untouched (a range with at most one GOP to copy is re-encoded
  whole, and nearby re-encodes share one ffmpeg). Every audio track is kept, with a short crossfade at
  each cut. A source that cannot be matched (not 8-bit 4:2:0 H.264, variable frame rate) is re-encoded
  in full, and the command says why. The source is only read; the output goes where `--out` says.

## Requirements

- Rust (stable) with Cargo
- `ffmpeg` and `ffprobe` on `PATH`
- `curl` on `PATH` (shipped with Windows 10+), to fetch the models on first use
- CMake and a C++ compiler (MSVC on Windows), and libclang for bindgen (`LIBCLANG_PATH`; LLVM, or
  `pip install libclang`): whisper.cpp is built from source by `whisper-rs`.
- For `render`: an NVIDIA GPU and an ffmpeg build with NVENC/NVDEC (`h264_nvenc`, `-hwaccel cuda`).
- Optional, for GPU: an NVIDIA GPU with CUDA 13 and cuDNN 9 installed, and a build with `--features cuda`.
  Without it, or when the GPU cannot start, everything runs on CPU. Whisper runs large-v3-turbo on the
  GPU when 2.1 GB of VRAM is free, else on CPU, and says so (also when the build has no GPU support, or
  the CUDA 13 runtime's `cublas64_13.dll` is not installed). A CUDA build starts on a PC without CUDA:
  it finds the CUDA DLLs through `CUDA_PATH` (or `JUNCTUR_CUDA_DIR`, `JUNCTUR_CUDNN_DIR`) when needed.
  Building whisper.cpp's CUDA kernels: with several CUDA versions installed set `CUDAToolkit_ROOT` to the
  CUDA 13 folder; CMake 4.0 does not know Visual Studio 2026, so set `CMAKE_GENERATOR` to an older
  installed one (e.g. `Visual Studio 16 2019`) if it picks that. The app's `npm run tauri` sets both
  when they are unset.

## Build and use

```sh
cargo build --release                  # CPU
cargo build --release --features cuda  # CUDA, falls back to CPU at runtime
junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
                      [--windows <n>] [--window-seconds <s>]
junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
                        [--no-speakers] [--json <out.json>] [--cache-dir <dir>]
junctur analyze <file> [--project <dir>] [--cpu] [--force] [--no-transcript] [--no-speakers]
                       [--labels] [--keep <prompt>] [--model auto|large-v3-turbo|large-v3|large-v3-q5_0]
                       [cut flags]
junctur judge <file> [--prompt <prompt>] [--project <dir>]
junctur status <file> [--project <dir>]
junctur cut <file> [--project <dir>] [cut flags]
junctur export <file> [--fcpxml <out.fcpxml>] [--srt <out.srt>] [--project <dir>]
junctur render <file> --out <out.mp4> [--project <dir>] [--full] [--force] [--jobs <n>]
junctur voices [<file> --add <name> --at <start>-<end>[,...] | <file> --rehear] [--forget <name>]
cut flags: [--pad-before <s>] [--pad-after <s>] [--merge-gap <s>] [--crossfade <ms>]
```

A recording's project folder (transcript, speech regions, speakers, `edit.junctur`, `jobs.json`) lives
in the cache unless `--project <dir>` puts it somewhere else, for example a git repository.

`analyze` also tells speakers apart (`--no-speakers` skips it): one speaker per speech track when OBS
recorded the voices on their own tracks, otherwise by voice on the mix (CAM++ embeddings of 1.5 s
windows, and pyannote's segmentation model for where two people talk at once). Both always run on the
CPU, as tracks and speech do, also in a CUDA build, where only Whisper and the labels' picture model
run on the GPU (`junctur separate` still separates on the GPU): on ONNX Runtime's CUDA provider CAM++'s
embeddings were wrong, and every sentence became its own speaker. They are written into
`edit.junctur` as `speaker <id> <name>` lines and `turn <start> <end> <id>` lines; turns of two people
overlap where both talk. Rename or recolour a speaker by editing its line (or in the app). A re-cut keeps
the names.

Labels for the parts where nobody talks are off by default (`"labels": {"on": true}` in the config file,
Settings in the app, or `analyze --labels` for one run). Each silent stretch of 2 s or more (cut into
pieces of at most 30 s) is looked at every 2 s, at the recording's keyframes, by SigLIP 2 base (zero-shot
against a list of labels: `"kind": "generic"` or `"league"`, plus your own in `"added"`), and scored for how
busy it sounds: the share of the recording's seconds quieter on the game/PC track, else the mix. They
land in `labels.json` in the project folder and on the app's Labels lane, between the edit list and
Whisper. On the 1h45 sample they take about 75 s on an RTX 3060 (about 45 s per hour of recording), plus a
1.5 GB download the first time; a changed list is scored again from the kept frames in seconds. Base reads
menus, the client and the desktop well, and calls most of a League game "teamfight": the busy score is
what tells a real fight from walking.

Smart keep is off by default: give a recording a prompt saying what kind of video you are making (the
app's Properties, `analyze --keep "<prompt>"`, or `judge --prompt`), or turn it on (`"keep": {"on": true}`,
Settings) to run it on every analysed recording with the last prompt used. Talk stays as the automatic cut
leaves it. The silent stretches are looked at in windows of at most 5 s, each ranked within this recording
by how busy its game audio is (the game/PC track, else the mix) and by its share of the "fighting" and
"teamfight" labels when there are labels; a window busier than usual on either counts. How many of them are
kept is read from the prompt's words, in English or Spanish: "podcast", "only the talk" or "solo la charla"
keep no silence; "highlights", "only the fights", "mejores momentos" the busiest 10%; "dead air", "whole
stream", "tiempo muerto" the busiest 50%; anything else 25% (a negation such as "not only the fights" is
read as its phrase). The kept windows, padded by 2 s and merged, are suggestions (the app's Suggestions lane,
Accept all, accept, dismiss or undo one, Review the close calls), and nothing is applied until you accept it;
a part you edited yourself is never suggested. No model and no download: about 6 s for the 1h45 sample, most
of it reading the game track when the app has not drawn its waveforms. On that sample the League preset
(read as strict) suggests 67 spans (378 s) of its 56 minutes of silence, Full VOD minus dead air (loose) 260
(1853 s), Podcast-style talk none. Phase 3b had Laya, a decision model, judge every part instead; it judged
no better than chance, so smart keep no longer calls it (its code stays in the repo).

Voices can be remembered once and recognised in every recording: in the app, select words only one
person says and choose "Remember voice…", or `junctur voices <file> --add <name> --at 61-75,300-312`
(source seconds). A person's speaker then gets their name and colour in every recording once it is
labelled again: the open recording at once, any other at its next `analyze` or `cut` (`export` writes the
edit list as it is). Voices nobody remembered are grouped automatically as "Speaker N". Remembering the same name again adds to the voice;
half a minute from several places works best. The voices live in `junctur/voices.json` beside the config
file (or the file `JUNCTUR_VOICES` names); `junctur voices` lists them, `--forget <name>` drops one. A
voice remembered before that fix (or with another model) is no longer read, and leaves the file at its next
save: remember it again.
Enrolling relabels from what `analyze` kept (`windows.f32` in the project), without reading the
recording again; `analyze` and `cut` relabel with the current voices. A rename of a speaker nobody
enrolled follows that speaker through a relabel (matched by when they talk, not by number).

Any length works, even one word: a short pick makes a rough voice (Junctur says so), but the seconds
picked are pinned to that person in that recording, kept in its `speakers.json`, and every later labelling
gives those seconds to them and nobody else. `junctur voices <file> --rehear` (the app's "Analyse speakers
again") reads the recording's speakers again from the audio with the voice windows cut where the pins
begin and end, reusing every other stage; it takes about as long as the speakers stage did.

`export --srt <out.srt>` writes the captions: the transcript's kept words on the edited timeline (the
same frames as the render and the FCPXML), each cue as `<font color>` in its speaker's colour, cut by the
`captions` line (`chars=` per line, `lines=` per speaker in a caption, `seconds=` longest, and
`words=` per speaker in a caption, left out for no limit, `words=1` for one word at a time; set in the
app's Captions panel or by hand). It is one file for one Resolve subtitle track, which shows one caption
at a time: when people talk at once (on separate voice tracks, or where `analyze` heard two voices on
the mix), one caption holds them all, each speaker on their own line(s) in their colour, lasting until
the last of them stops. Each word is shown once, under its one speaker. Per-speaker files that earlier
versions wrote beside it (`<out> (<name>).srt`) are not deleted; the export lists them. Bold, italic
and underline (`bold=on`, `italic=on`, `underline=on` on the `captions` line) wrap every line outside
its colour, `<b><i><u><font color>`; nothing else an SRT could say reaches Resolve. The text options
change what every caption output shows (the SRT, Send to Resolve, the preview), never the transcript:
`replace="gonna>going to|um>"` finds whole words or phrases, case aside, and shows the text after `>`
(nothing removes them), then `case=upper|lower|title`, `punctuation=off`, and `censor="word|..."`
masks whole words or phrases from the user's own list, `f**k`, or `****` with `censor_style=whole`.
`translate=fr` (any of the 100 languages of the local translation model, by code) shows the captions
in another language, translated on this machine a sentence at a time and spread over the time it was
said, so timing, speakers and cuts carry over; the text options then apply to the translated words. The
model (MIT, about 630 MB) is downloaded on first use, translations are kept in the project folder
(`translations.json`), and `export --srt` and `render` translate what is missing first (about a
quarter of a second a sentence on the CPU); the app translates in the background, and its export and
"Send to Resolve" wait until every sentence is done. Captions are
optional: with `captions on`, `render` also writes the file beside the video.
Checked in DaVinci Resolve (File > Import > Subtitle keeps the colours); Premiere Pro is unverified and
likely shows its default caption style. SRT carries no size or position.

The app's Captions panel can also send the whole edit straight into DaVinci Resolve ("Send to
Resolve"): a new timeline in the open project, named `<recording> (Junctur)` and made from the same
FCPXML as `junctur export --fcpxml` (the recording, every audio track, cut as the edit list says), with
the captions on it as Fusion Text+ clips. If the project already has that timeline, Junctur asks whether
to create a new one beside it (`<recording> (Junctur) 2`), update only the captions on it (its cuts and
anything changed in Resolve left alone; the result says when its cut no longer matches the edit's) or
replace it; it never replaces one without asking. Text+ reaches what an SRT cannot: font, size, height, outline, and where people
talking at once go (stacked, side by side, or one at the top; a protagonist keeps the normal spot).
Each speaker gets their own video track in their colour.
Junctur installs a script into Resolve's Scripts folder; start it once per Resolve session with
Workspace > Scripts > Junctur and leave it running (it works on the free Resolve). The bundled Text+
template is auto-subs' `caption-bin.drb` (MIT, see `crates/junctur-core/src/resolve/LICENSE-auto-subs`);
a Text+ title of your own can be picked instead from a list of the Fusion Titles in the open project's
media pool, and with "Keep the template's look" Junctur sets only each caption's text, timing and
speaker colour on it. When clips Junctur did not place sit where new captions go on its tracks, a send
changes nothing and asks: put that speaker's new captions on a track beside (`… (2)`), replace only the
clips a caption overlaps, or skip those captions; the answer can be remembered for the recording. These
settings are on the `captions`
line too (`font="…"`, `size=`, `position=`, `outline=`, `outline_width=`, `outline_color=`, `layout=`,
`protagonist="…"`, `template="…"`, `keep_look=on`, `existing_clips=`, each written only when not the
default) and drive the preview; the SRT ignores them.

`render` writes scratch pieces in a `<out>.junctur-parts` folder beside the output and deletes them;
`--full` re-encodes everything, `--force` replaces an existing output (never the source). The source's
keyframe index is kept in the cache, so rendering the same recording again skips reading it.

`separate` without `--track` picks the track `tracks` called the mix. Model tests download the models
and are ignored by default: `cargo test -- --ignored`.

## Desktop app (early)

`crates/junctur-app` is a Tauri 2 window over the same engine. It opens on a start window: drop a
recording, choose one, or pick one of the recent ones (`junctur/recent.json` beside the config file).
`junctur-app <file>` opens a file directly. A recording never analysed opens at once and is analysed in
the background with `junctur analyze`'s defaults (model from the config file, remembered voices); the
project panel shows each stage as it runs, and the transcript and edit list fill in when it is done. An
analysed recording opens straight away; when the config file now asks for another Whisper model than the
transcript was made with, the panel says so and points to Correct (below; never on its own). `npm run tauri dev` / `build` build the app with `--features cuda` when
the CUDA 13 toolkit is installed (`CUDA_PATH_V13_0`), so it analyses on the GPU; `JUNCTUR_CPU=1` builds
a CPU one. Plain `cargo build` is always CPU-only.

Play/pause, seek, pick the audio track, and see the recording's project state (stages, transcript, edit
list), read from the same folder `junctur status` uses. Video is played by
embedded mpv (libmpv's render API over OpenGL, NVDEC on), drawn under a transparent WebView so the UI
sits on top of it. Windows only for now.

With an edit list, the preview plays only the kept ranges, and the transcript panel edits by text: select
words and press Delete to cut them (K keeps), restore a folded cut, or nudge a range's ends from the bar
above the timeline (shift-drag on the timeline selects a stretch). Boundaries snap into the pauses the way
`junctur cut` places them, every edit is saved to `edit.junctur` at once, Ctrl+Z / Ctrl+Y undo and redo,
and a list the app did not write itself is first kept as `edit.junctur.bak` (then `.bak2`, ...).
A word Whisper timed badly can be put right on the track lanes: select it (or a run of words) and drag
the blue span's start or end to where the word really starts or ends (Alt grabs the word's edge over a
cut's edge beside it); the word's neighbour on its track is trimmed if run into, no cut moves, and the
new time is a `time` line, one undo step, used by the transcript, the captions and the SRT.
Double-click a misheard word, or point at it and press F2, to fix its spelling (F2 with no word under
the pointer opens Fix text for the words selected, when they are in view; a field left as it opened
saves nothing when you click away): the fix is a `fix` line in `edit.junctur`, shown in the
transcript, the caption preview and the SRT, and the spelling joins the word list (below). A fix
changes only the words you fixed: the same misheard word elsewhere stays as heard until you fix it
there too. (An earlier version also gave a fix to every place heard the same way; those copies are
ordinary fixes now, and Fixes is how to take the wrong ones away.) **Fixes**, in the panel's header,
lists every fix of the recording grouped by its text (how many places, what they were heard as), each
place with its time to go there; tick places or whole groups and Remove takes them out, so those words
read as heard.

**Correct…**, the big button at the right of the word bar (there with nothing selected too, and in an
empty transcript), analyses the recording again, part by part. Its pop-up has two things to redo,
"Transcribe the words again" (Whisper with the word list as it is now) and "Hear the speakers again"
(against the remembered voices, your pinned seconds still deciding theirs), and three of yours to keep:
**Cuts** (unticked, a fresh cut from the analysis; when the list holds your edits it asks first and keeps
them as `edit.junctur.bak*`, as `junctur analyze` does), **Names and voices** (unticked, the speakers are
heard again and named afresh, without your names or pins; the list and `speakers.json` are backed up, the
remembered voices file is never touched; if they cannot be heard, for instance offline before their model
is downloaded, the run stops before the list and the note under the transcript says why) and **Corrected
words** (your fixes and the word times set by moving a cut's edge or a word's own edge; unticked, they
are dropped, the list backed up, so "transcribe again ignoring my fixes" is one untick). It opens on
everything redone and everything kept, and Re-analyse whole timeline does what is ticked: with nothing
ticked to redo it reruns only what the settings changed, as after a model change. Re-analyse selection
works on the stretch selected on the timeline, or the words selected in the transcript (several stretches
count as the span holding them): the words are transcribed again on the Whisper chunks it touches (whole
chunks, so no word is cut in half), the cut made again only inside it, the fixes dropped only inside it,
the rest of the transcript and the cut as they were. The speakers are heard over the whole recording
only, so with either speakers box asking for them the selection button is greyed and says why; a
selection also needs a transcript made with the settings' model and languages. A run is an analysis like
any other (on the transcription lane, pausable, the words coming back five minutes at a time). Kept fixes
and word times move onto the new words, and nothing is fixed beyond what you fixed yourself; one whose
word Whisper now spells your way is no longer needed, and one whose word is heard otherwise now is set
aside and listed above the transcript until you dismiss it. An edit list that no longer reads is left as
it is, and the analysis says so.
Words Whisper was unsure of (the lowest probability of their letters' tokens under 25 %, `flag_below`
below) are underlined; the panel's header steps through them, and "Looks right" keeps one as heard and
stops flagging it (a `fix` line with the heard text). Transcripts made before this have no flags.

Setup, once (Node.js 20+ for the frontend):

```sh
pwsh scripts/fetch-libmpv.ps1        # pinned libmpv build into vendor/mpv/ (git-ignored)
cd crates/junctur-app && npm install
npm run tauri dev                    # dev server + app; `npm run build` builds the frontend only
```

The app looks for `libmpv-2.dll` in `JUNCTUR_LIBMPV`, beside the executable, then in `vendor/mpv/`.
Without it the window still opens and says what is missing. Plain `cargo build`/`cargo test` do not need
the DLL or the built frontend (`dist/`); only a release build (`npm run tauri build`) embeds the frontend.
`JUNCTUR_TEST_OFFSCREEN=<seconds>` runs the app off-screen and silent, prints the player state as one
JSON line and exits, for scripted launch checks; it is in dev builds, and in a release build only with
`--features test-hooks`. The libmpv test is ignored by default:
`cargo test -p junctur-app -- --ignored`.

## Models

Downloaded on first use into `<cache>/models/`, checked against pinned SHA-256 digests, never committed:

| Model | Use | License |
|---|---|---|
| Silero VAD v5 | speech detection | MIT |
| UVR MDX-Net Kim Vocal 2 | voice isolation | no explicit license on the weights |
| 3D-Speaker CAM++ (VoxCeleb) | speaker embeddings | Apache-2.0 |
| pyannote segmentation-3.0 (k2-fsa's ONNX export) | overlapping speech | MIT |
| SigLIP 2 base 224 px (onnx-community's export) | labels for the silent parts | Apache-2.0 |
| Laya multilingual (Convai Innovations; our fp16 ONNX export, tokenizer from Convai) | phase 3b's smart keep, no longer called | Apache-2.0 |
| Whisper large-v3-turbo, large-v3 f16 or 5-bit large-v3, ggml (whisper.cpp) | transcription | MIT |

## Whisper models

| `--model` | Use |
|---|---|
| `auto` (default) | large-v3-turbo, on the GPU when 2.1 GB of VRAM is free, else on CPU |
| `large-v3-turbo` (alias `turbo`) | the fastest; close to large-v3, weaker at telling Spanish from English on short lines |
| `large-v3` (alias `best`) | best quality, opt in; f16, wants about 4.8 GB of free VRAM |
| `large-v3-q5_0` | large-v3 quantised to 5 bits: large-v3's decoder in about 2.2 GB of VRAM, a little more than turbo |

To change the default, create `junctur/config.json` in the OS config folder (`%APPDATA%` on Windows,
`~/.config` elsewhere), or point `JUNCTUR_CONFIG` at a file; the desktop app's Settings (the gear in its
title bar) writes the same file:

```json
{ "whisper_model": "large-v3", "words": ["Uezar", "Tsunheto"], "captions": { "on": true, "chars": 32 } }
```

`words` is the word list: names and terms (speaker names, recurring jargon) Whisper is told before every
stretch it hears, so they come out spelled that way. It applies to the next transcription; a finished one
is not redone or called out of date. A list too long for Whisper's prompt (about 220 tokens) is cut from
the end, and the log says what was left out. The app edits it in Settings and adds a word you fix at
the top of the list, so a newly fixed word is always told to Whisper: only words the recording never
heard anywhere (or a name whose case you fixed), so a retyped sentence's common words do not push the
names out.

`flag_below` (default 25, 0 for none) is the percentage under which the app's transcript flags a word to
check; only the app reads it.

`captions` holds the caption defaults: what a recording whose edit list has no `captions` line of its own
cuts its captions with (`on`, `chars` per line, `lines` per speaker, `words` per speaker with 0 for no
limit, `seconds` at most, and the text options: `case`, `punctuation`, `censor` as a list of
words, `censorStyle`, `replace` as a list of `{"from": .., "to": ..}`, and `translate`, a language code
or empty), in the app and in
`render` and `export --srt`. A key left out keeps its built-in value.

`--model` on the command line wins over the config file. A config file that does not parse, names an
unknown model or key, or is named by `JUNCTUR_CONFIG` but missing, is an error rather than a silent
fallback.

A finished transcript is reused when you rerun `analyze`, whichever model made it, unless `--model` names a
different one (then it is redone with that model) or `--force` is given. An interrupted run resumes with the
model it started with; asking for another model starts over.

Results are cached in the OS user cache folder under `junctur/`; override it with `--cache-dir` or the
`JUNCTUR_CACHE_DIR` environment variable.

## Roadmap

- Track analysis, voice isolation, speakers (done)
- Engine core: voice activity detection, transcription, edit list, FCPXML export (done)
- Smart rendering: re-encode only around cuts (done)
- Desktop app: embedded mpv playback, a preview that skips cuts, transcript editing (started)
- Highlight signals, review mode and an auto mode that learns from your edits

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
