# AudioDistillery ("Still") — Architecture reference

> Product name: **AudioDistillery**. Official short name: **Still** (the
> copper still — used as the binary name and technical identifier). Project
> file extension: `.still`.
>
> This document is the architectural reference the source code points to
> (comments cite it as "ARCHITECTURE.md §3" / "§3 bis"). It describes the
> application as shipped, not a roadmap.

---

## 1. What the product does

A desktop application (macOS today; the core is platform-agnostic) that
turns long continuous recordings — concerts, rehearsals, vinyl sides,
multitrack field recordings — into finished, tagged, mastered albums:

1. **Load** one or more audio files (WAV, FLAC, MP3, AIFF): sequential
   clips on a timeline, and/or time-synchronized **layers** (e.g. the
   separate inputs of a field recorder), including chained multitrack
   "takes".
2. **Visualize** the mix as a multi-resolution waveform with fluid zoom,
   a minimap, and per-layer views.
3. **Mark** track regions (start/end pairs with titles) directly on the
   waveform — by hand, or from backend silence detection. Audio outside
   every region is ignored.
4. **Mix** the layers: per-layer gain/mute/solo, plus per-track overrides.
5. **Master** in real time through plugin chains — Audio Units and VST3 —
   with native plugin editors, at three scopes: per layer (pre-fader), per
   track (master bus, active inside the track's span) and on the master
   bus. Chains are saved as named presets.
6. **Tag** the output: album metadata, multi-disc numbering, filename
   macros, cover art (format-agnostic via lofty).
7. **Export** each track as a new file (WAV/FLAC/MP3/AAC), rendered
   through the plugin chains sample-accurately, in parallel.

Philosophy: do what a full mastering suite does for this exact use case,
ten times simpler. Every screen must be understandable without a manual.

## 2. Stack

| Layer | Technology | Role |
|---|---|---|
| App shell | **Tauri 2** | Windowing, packaging, front/back bridge |
| Backend | **Rust** | 100 % of the business logic |
| Decoding | **Symphonia** | All audio formats, peak extraction |
| Realtime audio | **cpal** + own render thread | Playback engine, plugin processing |
| Plugin hosting | AudioToolbox (AU) + **vst3** crate (VST3) | Mastering chains, native editors |
| Encoding | **FFmpeg** (bundled sidecar, subprocess) | Export encoding only |
| Frontend | **TypeScript + React** | Display only |
| Waveform UI | Custom canvas fed by backend peak data | Drawing, nothing else |

Shared types are generated from Rust with `ts-rs` into `src/types/`
(regenerate with `cargo test -p still-core`; never hand-edit).

---

## 3. NON-NEGOTIABLE ARCHITECTURAL RULE: strict frontend/backend separation

**The frontend is a display terminal. Nothing more.**

The frontend may: display backend-provided data (peaks, tracks, layers,
chains, progress), handle purely visual logic (zoom, scroll, hover,
in-progress drags, themes, animations), convert screen ↔ time coordinates
for display only, and send **intentions** through Tauri commands.

The frontend may never: decode or touch an audio file, compute peaks,
validate or correct positions (snapping, bounds, overlaps — the backend
decides and returns the final value), own canonical state, or reach the
filesystem outside the native dialogs (whose resulting paths go straight
to the backend).

The backend owns: all project state (single source of truth, typed and
serializable), decoding and multi-resolution peaks, region/marker logic,
the realtime engine and plugin hosting, export, and `.still` persistence.

Interface contract: every Tauri command is documented in
`src-tauri/COMMANDS.md` (name, parameters, return, errors); shared types
are ts-rs-generated, never duplicated; long operations emit progress
events the frontend merely displays.

**Decisive test:** the entire frontend could be replaced by a CLI without
touching one line of backend code. If a feature breaks this test, it is
in the wrong layer.

---

## 3 bis. NON-NEGOTIABLE PRODUCT RULE: non-destructive editing

**Source files are sacred. They are never opened for writing.**

- Every source file is opened **read-only**, everywhere, always. No code
  in any layer may hold a write handle on a source file.
- Regions, titles, layer mixes, plugin chains and their states, metadata:
  everything is a **declarative recipe** in the `.still` project file. The
  project is a recipe, not a result.
- Export only ever creates **new** files. Name collisions get a suffix —
  never a silent overwrite — and the default destination is never the
  source folder.
- Tags are written to exported files only, never to a source, even when
  the source is a taggable format.
- Plugin processing happens on the fly (playback) or per export render;
  no intermediate render ever replaces anything.
- Undo/redo operates on project state and is trivial because everything
  is declarative.

**Decisive test:** at any moment, deleting the app and the project file
must leave the source files byte-for-byte identical to their initial
state. This is enforced by
`src-tauri/core/tests/integration.rs::full_scenario_is_non_destructive_and_sample_accurate`
(checksums before/after a full load → mark → export scenario) — and by
every chain/export integration test since. Keep them green forever.

---

## 4. The engine

One render thread (`still-engine`), fed by per-layer decoders, produces
interleaved f32 blocks (512 frames) into a ring buffer (~90 ms) that a
minimal cpal callback drains. Plugins never run in the device callback.

Signal flow, identical in playback and export:

```
per layer:  decode → layer insert chain (pre-fader) → gain automation ─┐
                                                                  sum ─┤
master bus: active track's insert chain (gated to its region span,     │
            reset on entry) → global mastering chain → output ◄────────┘
```

- **Volume automation**: session gains/mutes/solos plus per-track
  override spans, resolved by the core (`VolumeAutomation`), consulted per
  block, ramped per block against zipper noise.
- **Insert sections**: the master bus holds ordered `(span, chain)`
  sections — `None` span = the always-on mastering chain, `Some` = a
  track's chain, gated per block and reset when (re)entering its span.
- **Latency**: master-bus chains (track + mastering) are compensated at
  export by pre-roll; layer-chain latency is deliberately uncompensated
  (it would skew that layer against the others — typical layer inserts
  are zero-latency). Live latency is displayed in the Chains panel.

## 5. Plugin hosting (the hard-won rules)

`BlockProcessor` is the only currency above the plugin objects: the
engine, the chain host, and export all deal in `Box<dyn BlockProcessor>`.
`plugins::create_plugin` dispatches on the component id prefix
(`"aufx:…"` = Audio Unit, `"vst3:<32 hex>"` = VST3).

Rules that were each learned from a real crash or deadlock:

1. **Plugin lifecycle runs on the main thread** (instantiate, state
   get/set, dispose). `ChainHost` (Tauri layer) owns every live instance
   in `Arc<Mutex<Box<dyn BlockProcessor>>>` and hops to the main thread
   for lifecycle; the engine only receives `SharedInsert` proxies that
   `try_lock` and pass the dry signal on contention. Disposal happens on
   the caller's thread after the engine acknowledges releasing its
   proxies. (Export workers are the one sanctioned exception: they
   instantiate, render and dispose on a single worker thread, serialized
   by a global lifecycle mutex — concurrent instantiation from one module
   crashes real plugins.)
2. **VST3 scanning never happens in the app process.** Loading dozens of
   plugin dylibs poisons the process (duplicate ObjC classes; static
   destructors crash at exit). A throwaway `--vst3-scan` subprocess
   writes a JSON cache and `_exit(0)`s; the app reads the cache and only
   loads the bundles a chain actually uses.
3. **Host-side COM objects must be real.** `IHostApplication::createInstance`
   must actually produce `IMessage`/`IAttributeList` objects — some
   plugins dereference the result without a null check.
4. **Editors**: plugin views are mounted inside a container `NSView`,
   never as the window's `contentView`; VST3 resize requests are deferred
   through an `IPlugFrame` mailbox drained by a main-thread pump
   (synchronous resize from the plugin's own callback deadlocks).
5. **State**: an opaque per-plugin blob (`state_b64`) — AU: `ClassInfo`
   binary plist; VST3: a container packing the component + controller
   chunks, restored with the full three-step protocol.

Debugging aids: `STILL_AU_DEBUG=1` and `STILL_VST3_DEBUG=1` print
lifecycle, property and RMS diagnostics from the hosts.

## 6. Persistence

The `.still` file is versioned JSON (currently v7) with explicit
migrations for every older version (`project.rs::read_project`). New
fields are `#[serde(default)]` whenever possible so older files parse as
current. Chain presets live outside projects, one JSON per preset in the
app data directory. The VST3 scan cache and extra scan paths live in the
app config directory.

## 7. Layout

- `src/` — React frontend (display only). `src/types/` is generated.
- `src-tauri/core/` — `still-core`: ALL business logic, zero Tauri
  dependency, fully testable in isolation.
- `src-tauri/src/` — thin Tauri layer (commands, chain host, editors).
  No business logic.
- `src-tauri/native/` — ObjC shim for plugin editor windows.
- `src-tauri/COMMANDS.md` — the command contract (keep in sync).
- `design/DESIGN.md` — the design system ("Alambic"), normative for UI.

## 8. Quality bar

- Errors are never silent; every backend error surfaces as an actionable
  English message (`StillError`).
- Sample positions are `u64` samples at the source rate (`number` in TS).
- Long operations emit progress events (`load:progress`,
  `export:progress`).
- Business logic is unit-tested in `still-core`; integration tests cover
  the decisive rules and the export paths, generating their own audio and
  skipping gracefully when FFmpeg or specific plugins are absent.
- CI builds and tests on every push; tagged releases are signed and
  notarized.
