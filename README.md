# AudioDistillery

[![CI](https://github.com/GarageDeveloper/audio-distillery/actions/workflows/ci.yml/badge.svg)](https://github.com/GarageDeveloper/audio-distillery/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-a85e24.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/GarageDeveloper/audio-distillery?color=e0883a)](https://github.com/GarageDeveloper/audio-distillery/releases)

**Still** — a non-destructive workshop for turning long audio recordings
(concerts, vinyl rips, DJ mixes, multitrack field recordings) into finished,
mastered, tagged albums — exported to WAV / FLAC / MP3 / AAC.

![Main window — splitting a recording with a mastering chain on the master bus](design/screenshots/main-window.png)

<details>
<summary>More screenshots</summary>

![Plugin picker — the same plugin as Audio Unit and VST3](design/screenshots/chains-panel.png)
![Empty state](design/screenshots/empty-state.png)

</details>

## Why this exists

I play in a band, and every rehearsal ends the same way: a long continuous
recording — sometimes a single stereo file, sometimes the separate inputs
of a portable multitrack recorder — that nobody ever splits into songs,
because doing it with the existing tools is tedious. Audacity means manual
labels and destructive edits; WaveLab does everything, but at the price of
a steep learning curve and, in our experience, real tagging bugs (see our
reports on a [malformed `trkn` atom that makes track numbers invisible in
Apple Music](https://forums.steinberg.net/t/bug-report-wavelab-pro-13-writes-a-malformed-trkn-track-number-invisible-in-apple-music/1036154)
and a [lost disc number written as `0/0`](https://forums.steinberg.net/t/bug-report-m4a-disc-number-tpos-lost-wavelab-13-0-30-writes-the-itunes-disk-atom-as-0-0/1038038)).

AudioDistillery is the tool I wanted instead: load the session (multitrack
or not), let it detect what looks like songs, turn them into a track list
in a couple of clicks, name them, rebalance the inputs, optionally tag the
lot ("rehearsal of March 12th, with the band…" — cover art included), and
export to the usual formats with each format's native tags written
**correctly**. Nothing outrageously complicated, nothing destructive, and
the tagging bugs stop at our door: the exported files are checked against
the pickiest consumer (Apple Music) rather than against a spec reading.

To be clear, none of this is meant as condescending criticism: those
applications do a staggering number of very hard things, and I understand
both how difficult they are to maintain and that their workflow simply
isn't the one that fits me. AudioDistillery only tries to nail one narrow
job — and I hope it can help other amateur musicians at the end of a
rehearsal too. :)

- Load WAV, FLAC, MP3 or AIFF (multi-GB files stream — nothing is loaded whole
  into RAM).
- Navigate a fast multi-resolution waveform (wheel = zoom at cursor, minimap).
- Mark each track as a start/end region: press `M` at the start then at the
  end, or drag a selection and turn it into a track. Everything outside the
  regions (applause, gaps, lead-in) is simply ignored at export. Silence
  detection can propose the regions for you.
- Mix time-synchronized **multitrack layers** (e.g. the separate inputs of a
  field recorder): per-layer gain/mute/solo, per-track overrides, chained
  takes.
- **Master in real time with your own plugins** — Audio Units *and* VST3,
  with native plugin editors, at three scopes: per layer, per track (active
  only inside that track), and on the master bus. Save chains as presets;
  exports render through the exact same signal path, latency-compensated.
- Tag the album: metadata, multi-disc numbering, filename macros, cover art.
- Rename tracks inline, then export sample-accurate cuts in parallel with
  per-track progress and a final report.
- Save the whole session as a tiny `.still` project file.
- Pick your look: Alambic (default), Alambic Light, Signal or Atelier.

**Your source files are never touched.** They are opened read-only; exporting
only ever creates new files. Delete the app and the project file and your
sources are byte-for-byte intact (this is enforced by an integration test).

## Requirements

None: a static [FFmpeg](https://ffmpeg.org) is bundled with the app (fetched
at build time by `scripts/fetch-ffmpeg.mjs`). Resolution order at runtime:
the `STILL_FFMPEG` env var if set, then the bundled sidecar, then a
system-installed ffmpeg (Homebrew/PATH) as fallback.

## Development

```sh
npm install
npm run tauri dev          # run the app
npm test                   # frontend unit tests
cd src-tauri && cargo test # backend tests (core logic + integration)
npm run tauri build        # produce installers
```

Architecture, command contract and design system:

- `ARCHITECTURE.md` — the architecture reference (signal flow, the two rules, plugin hosting).
- `CLAUDE.md` — the two non-negotiable rules and their decisive tests.
- `src-tauri/COMMANDS.md` — frontend/backend contract.
- `design/DESIGN.md` — design system ("Alambic" direction).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) — start with the two non-negotiable
rules; they are what makes this codebase safe to change. Notable releases
are listed in [CHANGELOG.md](CHANGELOG.md).

## License

AudioDistillery is released under the [MIT License](LICENSE).

### Third-party notices

- **VST** is a trademark of Steinberg Media Technologies GmbH, registered in
  Europe and other countries. VST 3 hosting is implemented on the
  MIT-licensed [`vst3`](https://crates.io/crates/vst3) Rust bindings
  (coupler-rs); the VST 3 SDK itself is MIT-licensed by Steinberg since
  version 3.8.
- **Audio Units** and macOS are technologies of Apple Inc.
- Windows builds may include **ASIO Interface Technology by Steinberg Media
  Technologies GmbH** (ASIO is a trademark of Steinberg Media Technologies
  GmbH). The ASIO SDK is fetched from Steinberg at build time and is never
  redistributed by this repository.
- Exports are performed by a bundled [FFmpeg](https://ffmpeg.org) binary
  (separate sidecar process), built and distributed by
  [ffmpeg.martin-riedl.de](https://ffmpeg.martin-riedl.de) under the GPL;
  its sources are available from that site. FFmpeg is a trademark of
  Fabrice Bellard.
- Audio decoding uses [Symphonia](https://github.com/pdeljanov/Symphonia)
  (MPL-2.0, file-level copyleft — consumed unmodified as a registry
  dependency). All other Rust and npm dependencies are under permissive
  licenses (MIT / Apache-2.0 / ISC); see each package's metadata.
