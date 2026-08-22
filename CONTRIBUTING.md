# Contributing to AudioDistillery

Thanks for considering a contribution! This project has two non-negotiable
rules and a few conventions that every PR must respect — they are what
keeps the codebase safe to change.

## The two non-negotiable rules

1. **Strict frontend/backend separation** (ARCHITECTURE.md §3). The
   frontend is a display terminal: it never decodes audio, never computes
   peaks, never validates positions, never owns canonical state. It sends
   intentions via Tauri commands and displays what comes back. *Decisive
   test:* the entire frontend could be replaced by a CLI without touching
   one line of backend code.
2. **Non-destructive editing** (ARCHITECTURE.md §3 bis). Source audio
   files are opened read-only, everywhere, always. Exports only ever
   create new files. *Decisive test:*
   `src-tauri/core/tests/integration.rs::full_scenario_is_non_destructive_and_sample_accurate`
   must stay green forever.

If a change breaks either decisive test, the feature is designed wrong —
please rethink it rather than weaken the test.

## Ground rules

- **`src/types/` is generated** by [ts-rs] from the Rust types. Never
  edit those files by hand; regenerate them with
  `cargo test -p still-core`.
- **`src-tauri/COMMANDS.md` is the frontend/backend contract.** Update it
  in the same PR as any command change.
- **Business logic lives in `src-tauri/core/`** (`still-core`, zero Tauri
  dependency). The Tauri layer (`src-tauri/src/`) stays thin.
- **Plugin hosting** follows the rules in ARCHITECTURE.md §5 (main-thread
  lifecycle, subprocess VST3 scanning, container NSViews, deferred
  editor resizes). Each rule was learned from a real crash — don't
  regress them.
- Errors are never silent: surface an actionable English message via
  `StillError`.
- UI language is English (localization comes later). Follow
  `design/DESIGN.md` ("Alambic") for any UI work.

## Building and testing

```sh
npm install
npm run tauri dev        # run the app
cargo test               # backend tests (run from src-tauri/)
npm test                 # frontend tests
npm run build            # type-check + bundle
```

Notes:

- FFmpeg is resolved at runtime (`STILL_FFMPEG` env var, well-known
  paths, PATH); export tests skip gracefully without it. The dev/build
  flow fetches a sidecar binary automatically.
- The AU/VST3 hosting tests exercise real plugins when they are
  installed (they look for iZotope Neutron 5) and **skip cleanly
  otherwise** — silently-skipped hosting coverage on your machine is
  expected, not a bug.
- Debug diagnostics: `STILL_AU_DEBUG=1` and `STILL_VST3_DEBUG=1` print
  plugin lifecycle/RMS diagnostics; `STILL_EXPORT_JOBS=n` overrides
  export parallelism.
- On a fresh clone your editor may flag
  `src-tauri/capabilities/default.json`'s `$schema` as missing — it
  points into `src-tauri/gen/`, which is created by the first build.

## Pull requests

- One focused change per PR, with tests for backend logic.
- Run `cargo test` and `npm run build` before pushing.
- Keep commit messages in English, imperative mood, explaining the *why*.

[ts-rs]: https://github.com/Aleph-Alpha/ts-rs
