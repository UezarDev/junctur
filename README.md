# Junctur

Junctur is a video editing engine for long screen and camera recordings, written in Rust.

## Status

Early. Two pieces work:

- Track analysis: `junctur tracks` reads a recording and reports, per audio track, what it holds
  (speech, silence, duplicates of another track) so later stages can decide what to keep.
- Transcription: `junctur transcribe` finds the speech tracks, cuts them into short speech chunks and
  runs Whisper on each chunk.

## Requirements

- Rust (stable) with Cargo
- `ffmpeg` and `ffprobe` on `PATH`
- For transcription: whisper.cpp's `whisper-cli` on `PATH`, and a ggml Whisper model file

## Build and use

```sh
cargo build --release
junctur tracks <file> [--json <out.json>] [--cache-dir <dir>] [--no-cache] [--windows <n>] [--window-seconds <s>]
```

```sh
junctur transcribe <file> [--model large-v3-turbo|large-v3] [--language <code>] [--json <out.json>] [--cache-dir <dir>] [--no-cache]
```

Results are cached in the OS user cache folder under `junctur/`; override it with `--cache-dir` or the
`JUNCTUR_CACHE_DIR` environment variable.

## Whisper models

| Model | Use |
| --- | --- |
| `large-v3-turbo` (default, alias `turbo`) | About 5x faster than large-v3 with close quality |
| `large-v3` (alias `best`) | Best quality, opt in with `--model large-v3` |

Download the model file (`ggml-large-v3-turbo.bin` or `ggml-large-v3.bin`) from
[whisper.cpp's models](https://huggingface.co/ggerganov/whisper.cpp/tree/main) into the models folder:
`junctur/models` in the OS user data folder, or the folder named by `JUNCTUR_MODELS_DIR`.

To change the default model or the models folder, create `junctur/config.json` in the OS config folder
(`%APPDATA%` on Windows, `~/.config` elsewhere), or point `JUNCTUR_CONFIG` at a file:

```json
{ "whisper_model": "large-v3", "models_dir": "D:/whisper-models" }
```

`--model` on the command line wins over the config file.

## Roadmap

- Track analysis (done)
- Voice activity detection and transcription (done)
- Engine core: edit list, FCPXML export
- Smart rendering: re-encode only around cuts
- Desktop app with preview and transcript editing
- Highlight signals, review mode and an auto mode that learns from your edits

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
