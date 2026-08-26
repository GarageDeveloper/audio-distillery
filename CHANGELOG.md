# Changelog

All notable changes to AudioDistillery are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.5.0] — 2026-08-26

### Added
- **Professional export, tiers 1–2** ([#5]): output sample-rate conversion
  (aresample, large kernel), dither on lossless depth reduction (auto /
  triangular / triangular HP / Shibata), a one-click CD preset
  (WAV · 44.1 kHz · 16-bit · dithered), and a Red Book **CD image + cue
  sheet** export — 588-sample frame-aligned tracks in a single WAV, CD-Text
  and the album's CATALOG (EAN/UPC field in the metadata) in the .cue.
- **Mastering-grade metering** ([#2]): live EBU R128 meter in the Chains
  panel — momentary and short-term bars, integrated LUFS with a
  calibratable target (Spotify/Apple/YouTube/EBU presets), loudness range
  and max-hold true peak, tapped right after the master bus on the render
  thread. Every export now ends with a **loudness report**: LUFS-I and
  true peak per delivered file (measured on the encoded result), album
  outliers flagged, true peaks above −1 dBTP in red, CD images measured
  per cue segment, with a visible "analyzing" phase before the report.
- **Multitrack stems export** ([#7]): one file per layer per track, one
  folder per track (`{n} - {title}/{ln} - {layer}` by default, `{layer}`
  and `{ln}` macros in any template). Stem content is a choice: raw
  sample-exact cuts (DAW round-trip) or layer mix settings (gain,
  mute/solo, layer inserts). A layer with no audio under a track still
  yields a full-length silent stem, keeping the set time-aligned.
- **"Source" output format** ([#7]): a format choice that mirrors each
  original file's container, bit depth and sample rate — per layer in
  stems mode.

### Fixed
- The load overlay's Cancel button appeared exactly under the pointer
  after picking a layout, so a stray double-click aborted the analysis;
  it now arms after a short delay (Esc stays immediate).

[#2]: https://github.com/GarageDeveloper/audio-distillery/issues/2
[#5]: https://github.com/GarageDeveloper/audio-distillery/issues/5
[#7]: https://github.com/GarageDeveloper/audio-distillery/issues/7

## [0.4.0] — 2026-08-25

### Added
- **Renameable layers** ([#3]): double-click a layer's name to rename it
  ("Room mic", "Bass DI"…) — Enter/Esc/Tab semantics like track renaming,
  file name kept as a tooltip, the name flowing to the per-track mix rows
  and the Chains target selector. Empty name restores the file-name
  display.
- **Auto-split, evolved** ([#4]): pick the detection source in multitrack
  sessions (the mix, or any single layer — a between-songs-quiet input
  often beats a bleeding mix); the proposals bar opens even when nothing
  is found so the source can be switched; and proposals are reviewed by
  ear before adding — playhead-driven navigation (‹ › jump between
  proposals, ⇥ auditions an ending), per-proposal keep/exclude with
  excluded spans staying visible in red on the waveform, and "Add N
  tracks" adding exactly what you kept.

[#3]: https://github.com/GarageDeveloper/audio-distillery/issues/3
[#4]: https://github.com/GarageDeveloper/audio-distillery/issues/4

## [0.3.1] — 2026-08-23

### Fixed
- **No playback audio on Windows**: the engine forced the session sample
  rate onto the output device, which WASAPI shared mode rejects (CoreAudio
  had been resampling transparently on macOS, hiding the bug). The stream
  now opens at the device's own rate with a streaming windowed-sinc
  resampler converting session → device on the render thread — identical
  behaviour on every platform. ([#6])
- Output-device failures are no longer silent: the status bar shows an
  actionable "Audio device error" message.

[#6]: https://github.com/GarageDeveloper/audio-distillery/issues/6

## [0.3.0] — 2026-08-22

### Added
- **VST3 hosting** at full parity with Audio Units: subprocess-based
  scanning with a disk cache, live processing, state persistence, native
  IPlugView editor windows, export through the chain (null-tested
  bit-perfect against the AU version of the same plugin).
- **Insert chains at three scopes**: per layer (pre-fader, always
  active), per track (master-bus position, active inside the track's
  span) and the global mastering chain — one unified command family and
  a Chains panel with Master / Layers / Tracks sections, the Tracks
  section following the playhead.
- **Chain presets**: save/load named chains (live plugin states
  included) across any target and any project.
- Chain latency display in the panel footer.
- Custom VST3 scan directories (persisted, managed from the plugin
  picker).
- Plugin picker: single manufacturer list merging both formats, with
  AU/VST3 badges mirrored on the chain slots.
- In-app About dialog (version, MIT license, third-party notices);
  clicking the Still wordmark opens it.
- Clicking a track in the list starts playback when the transport is
  stopped.
- MIT license, README third-party notices, VST trademark attribution.

## [0.2.0] — 2026-08-21 (unreleased milestone)

### Added
- **Real-time mastering chain** on the master bus: Audio Unit hosting
  with native plugin editors, live processing while editing, export
  rendered through the chain with latency compensation.
- Robust AU lifecycle architecture (main-thread lifecycle, engine
  proxies) fixing silent-DSP and deadlock bugs with third-party plugins.
- Album metadata: format-agnostic tags (ID3v2/MP4/Vorbis/RIFF INFO),
  multi-disc numbering, filename macros, cover art.
- Chained multitrack "takes", per-track layer gain/mute/solo overrides.
- Signed & notarized release CI, bundled FFmpeg sidecar, app icon.

## [0.1.0] — 2026-08-20

### Added
- Initial release: load WAV/FLAC/MP3/AIFF recordings (sequential clips
  and synchronized multitrack layers), multi-resolution waveform with
  zoom/minimap, track regions with titles, silence detection, per-layer
  mixing, sample-accurate parallel export (WAV/FLAC/MP3/AAC) with naming
  templates, `.still` project files, undo/redo, four-theme "Alambic"
  design system — all built on the two non-negotiable rules: strict
  frontend/backend separation and 100 % non-destructive editing.
