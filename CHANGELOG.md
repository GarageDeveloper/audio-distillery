# Changelog

All notable changes to AudioDistillery are documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
