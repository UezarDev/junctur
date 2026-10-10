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
  carries speech, read as-is, a Whisper large-v3 transcript of only those regions (Spanish with English;
  word timestamps), and a first automatic cut. Every stage is cached and an interrupted run resumes.
  The result is an edit list: a plain text file of ranges to keep plus a score per 5 s window, meant to be
  read, diffed and hand-edited. `junctur cut` re-cuts with other padding, `junctur status` shows the
  stages, and `junctur export --fcpxml` writes a timeline DaVinci Resolve imports.
- `junctur render` turns the edit list into an MP4 by smart rendering: only the frames between each cut
  and the nearest keyframe are re-encoded (NVENC, matched to the source's profile, level and color tags);
  everything between keyframes is copied untouched. Every audio track is kept, with a short crossfade at
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
  Without it, or when the GPU cannot start, everything runs on CPU. Whisper picks its model from the free
  VRAM: large-v3 (f16) from 4.8 GB free, the 5-bit large-v3 from 2.2 GB, CPU below that, and says so.
  Building whisper.cpp's CUDA kernels: with several CUDA versions installed set `CUDAToolkit_ROOT` to the
  CUDA 13 folder; CMake 4.0 does not know Visual Studio 2026, so set `CMAKE_GENERATOR` to an older
  installed one (e.g. `Visual Studio 16 2019`) if it picks that.

## Build and use

```sh
cargo build --release                  # CPU
cargo build --release --features cuda  # CUDA, falls back to CPU at runtime
junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
                      [--windows <n>] [--window-seconds <s>]
junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
                        [--no-speakers] [--json <out.json>] [--cache-dir <dir>]
junctur analyze <file> [--project <dir>] [--cpu] [--force] [--no-transcript]
                       [--model auto|large-v3|large-v3-q5_0] [cut flags]
junctur status <file> [--project <dir>]
junctur cut <file> [--project <dir>] [cut flags]
junctur export <file> --fcpxml <out.fcpxml> [--project <dir>]
junctur render <file> --out <out.mp4> [--project <dir>] [--full] [--force] [--jobs <n>]
cut flags: [--pad-before <s>] [--pad-after <s>] [--merge-gap <s>] [--crossfade <ms>]
```

A recording's project folder (transcript, speech regions, `edit.junctur`, `jobs.json`) lives in the
cache unless `--project <dir>` puts it somewhere else, for example a git repository.

`render` writes scratch pieces in a `<out>.junctur-parts` folder beside the output and deletes them;
`--full` re-encodes everything, `--force` replaces an existing output (never the source).

`separate` without `--track` picks the track `tracks` called the mix. Model tests download the models
and are ignored by default: `cargo test -- --ignored`.

## Models

Downloaded on first use into `<cache>/models/`, checked against pinned SHA-256 digests, never committed:

| Model | Use | License |
|---|---|---|
| Silero VAD v5 | speech detection | MIT |
| UVR MDX-Net Kim Vocal 2 | voice isolation | no explicit license on the weights |
| 3D-Speaker CAM++ (VoxCeleb) | speaker embeddings | Apache-2.0 |
| Whisper large-v3, ggml f16 or 5-bit (whisper.cpp) | transcription | MIT |

Results are cached in the OS user cache folder under `junctur/`; override it with `--cache-dir` or the
`JUNCTUR_CACHE_DIR` environment variable.

## Roadmap

- Track analysis, voice isolation, speakers (done)
- Engine core: voice activity detection, transcription, edit list, FCPXML export (done)
- Smart rendering: re-encode only around cuts (done)
- Desktop app with preview and transcript editing
- Highlight signals, review mode and an auto mode that learns from your edits

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
