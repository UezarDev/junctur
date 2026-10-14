# Contributing

## Code style

- Keep it modular: small, focused modules; no monolithic files. A file past about 600 lines of
  production code wants splitting at its seams.
- Unit tests longer than about 100 lines live in `<module>/tests.rs` (declared `mod tests;`), so the
  production code reads on its own.
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
