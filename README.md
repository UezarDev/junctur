# Junctur

Junctur is a video editing engine for long screen and camera recordings, written in Rust.

## Status

Early. The only working piece is track analysis: `junctur tracks` reads a recording and reports, per
audio and video track, what it holds (speech, silence, duplicates of another track) so later stages can
decide what to keep.

## Requirements

- Rust (stable) with Cargo
- `ffmpeg` and `ffprobe` on `PATH`

## Build and use

```sh
cargo build --release
junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--windows <n>] [--window-seconds <s>]
```

Results are cached in the OS user cache folder under `junctur/`; override it with `--cache-dir` or the
`JUNCTUR_CACHE_DIR` environment variable.

## Roadmap

- Track analysis (done)
- Engine core: voice activity detection, transcription, edit list, FCPXML export
- Smart rendering: re-encode only around cuts
- Desktop app with preview and transcript editing
- Highlight signals, review mode and an auto mode that learns from your edits

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
