# Tauri command contract — AudioDistillery

Every interaction between the frontend (display terminal) and the backend
(single source of truth) goes through these commands. All errors are returned
as actionable English strings. Mutating commands return a fresh `ProjectView`
snapshot that the frontend must adopt as-is.

Shared types are generated from Rust with `ts-rs` into `src/types/` (run
`cargo test -p still-core` to regenerate — never edit those files by hand).

## Session & project

| Command | Parameters | Returns | Errors |
|---|---|---|---|
| `load_audio` | `paths: string[]` (1..n files, in order) | `ProjectView` | unsupported extension, file not found, decode failure, clip format mismatch |
| `load_multitrack` | `paths: string[]` | `ProjectView` (each file = one synced LAYER starting at t = 0) | same as `load_audio`, sample-rate mismatch |
| `add_clips` | `paths: string[]` | `ProjectView` (appends to the END of the base layer's timeline; regions and undo history preserved) | same as `load_audio`, no audio loaded |
| `add_layers` | `paths: string[]` | `ProjectView` (each file = one new synced layer) | same as `load_audio`, no audio loaded |
| `add_take` | `paths: string[]` (exactly one per layer; sorted by name and matched to the layer order) | `ProjectView` (whole synced take appended: every file starts together right after the current timeline end; shorter layers got a silent gap) | file count ≠ layer count, same as `load_audio` |
| `set_layer_gain` | `id: number`, `gainDb: number` | `ProjectView` (clamped to [-60, +12] dB; -60 = -∞; applies live to playback) | unknown layer |
| `set_layer_muted` | `id: number`, `muted: boolean` | `ProjectView` (applies live to playback) | unknown layer |
| `set_layer_solo` | `id: number`, `solo: boolean` | `ProjectView` (when any layer is soloed, only soloed layers are audible; solo wins over mute) | unknown layer |
| `set_layer_collapsed` | `id: number`, `collapsed: boolean` | `ProjectView` (display preference for the Layers waveform view, persisted in the project) | unknown layer |
| `set_track_layer_gain` | `trackId: number`, `layerId: number`, `gainDb: number \| null` | `ProjectView` (per-track override of one layer's gain; null clears it; undoable) | unknown track/layer |
| `set_track_layer_mute` | `trackId`, `layerId`, `muted: boolean \| null` | `ProjectView` (per-track mute override; null = follow the session) | unknown track/layer |
| `set_track_layer_solo` | `trackId`, `layerId`, `solo: boolean \| null` | `ProjectView` (per-track solo override; null = follow the session) | unknown track/layer |
| `remove_layer` | `id: number` | `ProjectView` | unknown layer, base layer |
| `cancel_load` | — | `void` (the pending scan then fails with "Import cancelled"; any previous session stays untouched) | — |
| `load_project` | `path: string` (.still file) | `ProjectView` | invalid project JSON, newer project version, missing source file |
| `save_project` | `path?: string` (required the first time) | `ProjectView` | no path known, I/O error |
| `get_project_view` | — | `ProjectView` | no audio loaded |

A session's audio is a stack of **layers** (time-synchronized recordings of
the same session, all starting at t = 0 — e.g. a Zoom recorder's stereo mic
plus its other inputs). Each layer is an ordered list of **clips** (source
files) laid back-to-back — or pinned at an explicit timeline position for
TAKE alignment, with silent gaps filling the difference (peaks, playback and
export all honor those gaps, keeping layers sample-aligned). The base layer
(index 0) carries the timeline shown in the UI (`AudioInfo.clips`). Every position in the API is a timeline
sample. Layers must share the session sample rate; channel counts may differ
(mono inputs next to a stereo mic are fine — session channels = max).
Scanning is read-only, computes one multi-resolution peak pyramid PER LAYER
and emits `load:progress` events (`number`, 0..1) over the batch. Regions
beyond the scanned duration are dropped on project load.

## Waveform data

| Command | Parameters | Returns | Errors |
|---|---|---|---|
| `get_peaks` | `startSample: number`, `endSample: number`, `maxBuckets: number` | `PeakSlice` | no audio loaded |
| `get_peaks_split` | same | `PeakSlice[]` (one per layer, same grid, each scaled by that layer's effective gain) | no audio loaded |

The backend picks the resolution level and returns the peaks of the
GAIN-WEIGHTED MIX of all audible layers — mutes, faders AND per-track gain
overrides applied at their exact timeline positions — so the waveform always
shows what will be heard/exported. `get_peaks_split` returns the same window
as one slice per layer (the "Layers" waveform view), each already scaled the
same way. The frontend only draws
the returned buckets. `PeakSlice.channels` holds interleaved `[min, max, …]`
i8 pairs per channel.

## Track regions

A track is a **region**: a start marker + an end marker + a title. Audio
outside every region is ignored at export. Regions never overlap; the backend
trims or rejects conflicting spans and clamps edge moves.

| Command | Parameters | Returns | Errors |
|---|---|---|---|
| `add_region` | `start: number`, `end: number` (samples, any order), `title?: string` | `ProjectView` | span too short (< 200 ms), contains/covered by an existing track |
| `add_regions` | `regions: RegionSpan[]` | `ProjectView` (misfit spans silently skipped; one undo step) | no audio loaded |
| `move_region_edge` | `id: number`, `edge: "start" \| "end"`, `position: number` | `ProjectView` (position clamped to neighbors, bounds and min length) | unknown id |
| `remove_region` | `id: number` | `ProjectView` (the freed audio is then ignored) | unknown id |
| `rename_track` | `id: number`, `title: string` | `ProjectView` (empty title restores the default `Track NN`) | unknown id |
| `set_snap_to_zero` | `enabled: boolean` | `ProjectView` | no audio loaded |
| `undo` / `redo` | — | `ProjectView` | no audio loaded |
| `detect_silences` | `params: SilenceParams` | `RegionSpan[]` (proposed track regions; leading/trailing silence and gaps excluded) | no audio loaded |

When snap-to-zero is enabled, `add_region`/`move_region_edge` adjust the
requested positions to the nearest zero crossing (backend decision; the
returned view holds the final positions).

`ProjectView.suggested_title` is the backend's proposal for the next track
title: the most recently titled track's base name with the next free `-<n>`
index ("Jam" → "Jam-2" → "Jam-3" …; empty when nothing is titled yet). The
frontend prefills the add-track input with it, nothing more.

## Export

| Command | Parameters | Returns | Errors |
|---|---|---|---|
| `export_tracks` | `config: ExportConfig` | `ExportReport` | export already running, empty destination, ffmpeg not found |
| `cancel_export` | — | `void` | — |
| `set_export_config` | `config: ExportConfig` | `ProjectView` | no audio loaded |
| `set_album_meta` | `meta: AlbumMeta` | `ProjectView` (album/artist/date/genre/comment + `disc_breaks`; persisted in the project) | no audio loaded |
| `get_default_export_dir` | — | `string` (`~/Music/AudioDistillery`) | — |

`export_tracks` pauses playback first (listening is pointless during an
export), then emits `export:progress` events (`ExportProgress`). Only the
defined track regions are exported (everything else is ignored); it errors
when no region exists. Cuts are sample-accurate (`atrim` on the decoded
stream). Each exported track is the SUM of the audible layers at their mix
gains — a track's `gain_overrides` (see `set_track_layer_gain`) replace the
session-wide layer gains for that track only (ffmpeg `amix` with
`normalize=0`; mono layers are upmixed to the session layout). Existing files are never overwritten: names are suffixed ` (1)`,
` (2)`, … Per-track failures land in `ExportReport.errors`; the other tracks
still export.

After each successful encode the backend writes the album metadata into the
NEW file through one abstract model (`AlbumMeta`): lofty maps it to the
container's native tags — ID3v2 (MP3), MP4 atoms (M4A/AAC), Vorbis comments
(FLAC), RIFF INFO (WAV; no disc fields there). Track n°/total restart on
each disc; disc n°/total derive from `disc_breaks` (track numbers starting a
new disc). Every text field and the file-naming template accept the macros
`{title} {n} {ntotal} {disc} {dtotal} {album} {artist} {album_artist}
{date} {year} {source}`. A `/` in the naming TEMPLATE creates subfolders
(e.g. `{disc}/{n} - {title}` sorts a multi-disc album into one folder per
disc; the UI switches to that template automatically when disc breaks exist
and the template is still the default) — templates are split before values
are injected, so a slash inside a title can never create a directory.
Sources are never touched — tagging failures keep the audio file and surface
in `ExportReport.errors`.

Tracks are encoded **in parallel**: one ffmpeg process per worker, with
`available cores − 2` workers (never fewer than 1, never more than the track
count) so the machine stays responsive. `STILL_EXPORT_JOBS` overrides the
worker count. Each `ExportProgress` event concerns ONE track
(`track_number`); the frontend keeps a bar per track and reads
`overall_progress` / `completed_tracks` for the global state. Report order is
always track order, regardless of finish order.

## Playback

| Command | Parameters | Returns |
|---|---|---|
| `player_toggle` | — | `PlaybackState` |
| `player_pause` | — | `PlaybackState` |
| `player_seek` | `positionSamples: number` | `PlaybackState` |
| `player_state` | — | `PlaybackState` |

The backend owns the playback clock; the frontend polls `player_state` and
only interpolates between polls for smooth drawing. Playback follows a
volume AUTOMATION derived from the project: session faders/mutes/solos by
default, replaced inside each track region by that track's overrides — what
you hear is always `TrackInfo.layer_volumes`, the same values the export
uses.

## Events

| Event | Payload | Emitted during |
|---|---|---|
| `load:progress` | `number` (0..1) | `load_audio`, `load_project` |
| `export:progress` | `ExportProgress` | `export_tracks` |
