# Contributing

## Code style

- Keep it modular: small, focused modules; no monolithic files.
- Readable code over clever code.
- Short comments that say why, not what. No long comment blocks.
- Only app code in the repo: no plans, notes, videos or test media.

## Before you commit

All four must pass:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
```

By contributing you agree your work is licensed under GPL-3.0-or-later.
