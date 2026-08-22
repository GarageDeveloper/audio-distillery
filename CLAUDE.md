# AudioDistillery ("Still")

Desktop app (Tauri 2 + Rust + React/TS) that splits long audio recordings into
tracks via markers on a waveform, then exports them. Full requirements live in
`ARCHITECTURE.md` — read it before large changes. UI language: **English** (l10n later).

## Two non-negotiable rules

### 1. Strict frontend/backend separation (ARCHITECTURE.md §3)
The frontend is a display terminal, nothing more. It never decodes audio,
never computes peaks, never validates marker positions, never owns canonical
state. It sends *intentions* via Tauri commands and displays what comes back
(`ProjectView` snapshots, `PeakSlice` windows, progress events).
**Decisive test:** the entire frontend could be replaced by a CLI without
touching one line of backend code. If a feature breaks this test, it is in the
wrong layer.

### 2. Non-destructive editing (ARCHITECTURE.md §3 bis)
Source audio files are sacred: opened **read-only**, everywhere, always.
Everything (markers, names, config) is a declarative recipe in the `.still`
project file. Exports only ever create **new** files (name collisions get a
suffix, never an overwrite; default destination is never the source folder).
**Decisive test:** after any full scenario (load → mark → export), source
files are byte-for-byte identical — enforced by
`src-tauri/core/tests/integration.rs::full_scenario_is_non_destructive_and_sample_accurate`.
Keep that test passing forever.

## Layout

- `src/` — React frontend (display only). Generated types in `src/types/`
  (from ts-rs — regenerate with `cargo test -p still-core`, never hand-edit).
- `src-tauri/core/` — `still-core` crate: ALL business logic, zero Tauri
  dependency, fully testable in isolation.
- `src-tauri/src/` — thin Tauri layer (`commands.rs`, `state.rs`). No business
  logic here.
- `src-tauri/COMMANDS.md` — the command contract. Update it whenever a
  command changes.
- `design/DESIGN.md` — design system (direction "Alambic"): tokens,
  dimensions, UX flows. Follow it for any UI work.

## Commands

- Backend tests: `cd src-tauri && cargo test` (includes the decisive tests).
- Frontend tests: `npm test` ; type-check/build: `npm run build`.
- Run the app: `npm run tauri dev`.
- FFmpeg is resolved at runtime (`STILL_FFMPEG` env var, well-known paths,
  PATH). Export tests skip gracefully when it's absent.

## Conventions

- Errors: never silent; every backend error surfaces as an actionable English
  message (`StillError`).
- Sample positions are `u64` samples at the source rate (TS: `number`).
- Long operations emit progress events (`load:progress`, `export:progress`).
- Plugin hosting follows the hard-won rules in ARCHITECTURE.md §5 (main-thread
  lifecycle, subprocess VST3 scanning, container views) — never regress them.
