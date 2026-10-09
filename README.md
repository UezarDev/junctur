# Junctur

Junctur is a video editing engine for long screen and camera recordings, written in Rust.

## Status

Early. Two pieces work:

- `junctur tracks` reads a recording and reports, per audio track, what it holds (mix, voice, game/PC,
  silent, duplicates of another track), using speech detection (Silero VAD) to decide.
- `junctur separate` splits a mix track into a vocal stem and the rest (MDX-Net), then finds speech
  regions and speakers (CAM++ embeddings) on the vocal stem. Stems go to the cache, never beside the input.

## Requirements

- Rust (stable) with Cargo
- `ffmpeg` and `ffprobe` on `PATH`
- `curl` on `PATH` (shipped with Windows 10+), to fetch the models on first use
- Optional, for GPU: an NVIDIA GPU with CUDA 13 and cuDNN 9 installed, and a build with `--features cuda`.
  Without it, or when the GPU cannot start, everything runs on CPU.

## Build and use

```sh
cargo build --release                  # CPU
cargo build --release --features cuda  # CUDA, falls back to CPU at runtime
junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--no-speech]
                      [--windows <n>] [--window-seconds <s>]
junctur separate <file> [--track <n>] [--start <s>] [--duration <s>] [--cpu] [--force]
                        [--no-speakers] [--json <out.json>] [--cache-dir <dir>]
```

`separate` without `--track` picks the track `tracks` called the mix. Model tests download the models
and are ignored by default: `cargo test -- --ignored`.

## Models

Downloaded on first use into `<cache>/models/`, checked against pinned SHA-256 digests, never committed:

| Model | Use | License |
|---|---|---|
| Silero VAD v5 | speech detection | MIT |
| UVR MDX-Net Kim Vocal 2 | voice isolation | no explicit license on the weights |
| 3D-Speaker CAM++ (VoxCeleb) | speaker embeddings | Apache-2.0 |

Results are cached in the OS user cache folder under `junctur/`; override it with `--cache-dir` or the
`JUNCTUR_CACHE_DIR` environment variable.

## Roadmap

- Track analysis, voice isolation, speakers (done)
- Engine core: voice activity detection, transcription, edit list, FCPXML export
- Smart rendering: re-encode only around cuts
- Desktop app with preview and transcript editing
- Highlight signals, review mode and an auto mode that learns from your edits

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
