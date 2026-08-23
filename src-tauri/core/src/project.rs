use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio::AudioInfo;
use crate::error::{Result, StillError};
use crate::metadata::AlbumMeta;
use crate::peaks::PeakPyramid;

pub const PROJECT_VERSION: u32 = 7;
pub const MIN_GAIN_DB: f32 = -60.0;
pub const MAX_GAIN_DB: f32 = 12.0;
/// A track region may never be shorter than this.
pub const MIN_TRACK_MS: u64 = 200;

/// A span of source audio, in samples. Used for region proposals and bulk adds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RegionSpan {
    #[ts(type = "number")]
    pub start: u64,
    #[ts(type = "number")]
    pub end: u64,
}

/// A track = a named region delimited by a start and an end marker.
/// Audio outside every region is ignored at export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    pub id: u32,
    pub start: u64,
    pub end: u64,
    /// None → default title derived from the track number.
    pub title: Option<String>,
    /// Per-track layer gain overrides (dB), keyed by layer id as a string.
    /// A layer absent from the map uses its session-wide gain.
    #[serde(default)]
    pub gain_overrides: HashMap<String, f32>,
    /// Per-track layer mute overrides (true = muted for this track).
    #[serde(default)]
    pub mute_overrides: HashMap<String, bool>,
    /// Per-track layer solo overrides (true = soloed for this track).
    #[serde(default)]
    pub solo_overrides: HashMap<String, bool>,
    /// Insert chain of THIS track: master-bus position, before the global
    /// mastering chain, active only inside the region's span.
    #[serde(default)]
    pub inserts: Vec<MasteringPluginCfg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../../src/types/")]
pub enum ExportFormat {
    Wav,
    Flac,
    Mp3,
    Aac,
}

impl ExportFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Wav => "wav",
            ExportFormat::Flac => "flac",
            ExportFormat::Mp3 => "mp3",
            ExportFormat::Aac => "m4a",
        }
    }
    pub fn is_lossy(&self) -> bool {
        matches!(self, ExportFormat::Mp3 | ExportFormat::Aac)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ExportConfig {
    pub format: ExportFormat,
    /// Bitrate for lossy formats (kbps).
    pub bitrate_kbps: u32,
    /// Bit depth for WAV output (16 or 24).
    pub bit_depth: u8,
    pub dest_dir: String,
    /// File naming template, e.g. `{n} - {title}`.
    pub template: String,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            format: ExportFormat::Flac,
            bitrate_kbps: 320,
            bit_depth: 16,
            dest_dir: String::new(),
            template: "{n} - {title}".to_string(),
        }
    }
}

/// One source file of a layer, optionally pinned at an explicit timeline
/// position (take alignment). `start: None` = right after the previous clip;
/// `Some(pos)` opens a silent gap up to `pos` when needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub path: String,
    #[serde(default)]
    pub start: Option<u64>,
}

impl SourceRef {
    pub fn sequential(path: String) -> Self {
        Self { path, start: None }
    }
}

/// One time-synchronized layer of the session (e.g. one input of a Zoom
/// recorder). All layers start at t = 0; each holds its own source clips
/// (sequential, or pinned for take alignment), a mix gain, mute and solo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: u32,
    pub sources: Vec<SourceRef>,
    pub gain_db: f32,
    pub muted: bool,
    /// Solo: when any layer is soloed, only soloed layers are audible.
    #[serde(default)]
    pub solo: bool,
    /// Collapsed to a thin strip in the "Layers" waveform view.
    #[serde(default)]
    pub collapsed: bool,
    /// Insert chain of THIS layer (pre-fader, always active).
    #[serde(default)]
    pub inserts: Vec<MasteringPluginCfg>,
    /// User-chosen display name; None = fall back to the first source's
    /// file name.
    #[serde(default)]
    pub custom_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    /// Time-synchronized layers; layer 0 is the base timeline.
    pub layers: Vec<Layer>,
    pub regions: Vec<Region>,
    pub snap_to_zero: bool,
    pub export_config: ExportConfig,
    /// Album metadata written to exported files (format-agnostic; lofty
    /// maps it to each container's native tags).
    #[serde(default)]
    pub album_meta: AlbumMeta,
    /// Master-bus mastering chain: ordered AU plugins with their saved
    /// state. Declarative like everything else in the recipe.
    #[serde(default)]
    pub mastering_chain: Vec<MasteringPluginCfg>,
    #[serde(default = "default_next_plugin_id")]
    pub next_plugin_id: u32,
    pub next_region_id: u32,
    pub next_layer_id: u32,
}

impl Project {
    /// One layer per source group; group 0 is the base timeline.
    pub fn new_layers(layer_sources: Vec<Vec<String>>) -> Self {
        let layers: Vec<Layer> = layer_sources
            .into_iter()
            .enumerate()
            .map(|(i, sources)| Layer {
                id: (i + 1) as u32,
                sources: sources.into_iter().map(SourceRef::sequential).collect(),
                gain_db: 0.0,
                muted: false,
                solo: false,
                collapsed: false,
                inserts: Vec::new(),
                custom_name: None,
            })
            .collect();
        let next_layer_id = layers.len() as u32 + 1;
        Self {
            version: PROJECT_VERSION,
            layers,
            regions: Vec::new(),
            snap_to_zero: false,
            export_config: ExportConfig::default(),
            album_meta: AlbumMeta::default(),
            mastering_chain: Vec::new(),
            next_plugin_id: 1,
            next_region_id: 1,
            next_layer_id,
        }
    }

    /// Single-layer convenience (sequential clips only).
    pub fn new(sources: Vec<String>) -> Self {
        Self::new_layers(vec![sources])
    }

    pub fn source_groups(&self) -> Vec<Vec<SourceRef>> {
        self.layers.iter().map(|l| l.sources.clone()).collect()
    }
}

fn default_next_plugin_id() -> u32 {
    1
}

/// One plugin of the mastering chain, as persisted in the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasteringPluginCfg {
    pub id: u32,
    /// AU component id "aufx:xxxx:yyyy".
    pub component: String,
    pub name: String,
    pub bypass: bool,
    /// Base64 binary plist of the plugin state (ClassInfo).
    #[serde(default)]
    pub state_b64: Option<String>,
}

/// Map persisted chain cfgs to their display views.
fn chain_views(chain: &[MasteringPluginCfg]) -> Vec<MasteringPluginView> {
    chain
        .iter()
        .map(|c| MasteringPluginView {
            id: c.id,
            component: c.component.clone(),
            name: c.name.clone(),
            bypass: c.bypass,
            format: crate::plugins::format_of(&c.component),
        })
        .collect()
}

/// A plugin chain's owner: the master bus, one layer (pre-fader, always
/// active) or one track (master-bus position, active inside its span).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[ts(export, export_to = "../../../src/types/")]
pub enum ChainTarget {
    Master,
    Layer { id: u32 },
    Track { id: u32 },
}

/// Display state of one mastering-chain plugin.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct MasteringPluginView {
    pub id: u32,
    pub component: String,
    pub name: String,
    pub bypass: bool,
    pub format: crate::plugins::PluginFormat,
}

pub fn db_to_linear(db: f32) -> f32 {
    if db <= MIN_GAIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct TrackInfo {
    /// Stable region id (rename/move/delete key).
    pub id: u32,
    /// 1-based track number, ordered by position in the file.
    pub number: u32,
    pub title: String,
    #[ts(type = "number")]
    pub start_sample: u64,
    #[ts(type = "number")]
    pub end_sample: u64,
    pub duration_seconds: f64,
    /// Per-track layer gain overrides (dB), keyed by layer id (string).
    #[ts(type = "Record<string, number>")]
    pub gain_overrides: HashMap<String, f32>,
    #[ts(type = "Record<string, boolean>")]
    pub mute_overrides: HashMap<String, bool>,
    #[ts(type = "Record<string, boolean>")]
    pub solo_overrides: HashMap<String, bool>,
    /// Resolved linear volume of every layer for THIS track (session gains,
    /// mutes and solos with the track's overrides applied). Index-aligned
    /// with the layer list; this is exactly what export and playback use.
    pub layer_volumes: Vec<f32>,
    /// This track's insert chain (master-bus position, active in its span).
    pub inserts: Vec<MasteringPluginView>,
}

/// Display state of one mix layer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct LayerView {
    pub id: u32,
    /// Display name: the user's custom name, else the first source's file
    /// name.
    pub name: String,
    /// Always the first source's file name (tooltip after a rename).
    pub source_name: String,
    pub channels: u16,
    pub duration_seconds: f64,
    pub gain_db: f32,
    pub muted: bool,
    pub solo: bool,
    pub collapsed: bool,
    /// This layer's insert chain (pre-fader, always active).
    pub inserts: Vec<MasteringPluginView>,
}

/// Display snapshot sent to the frontend after every mutation.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ProjectView {
    pub audio: AudioInfo,
    pub layers: Vec<LayerView>,
    pub tracks: Vec<TrackInfo>,
    pub snap_to_zero: bool,
    pub export_config: ExportConfig,
    pub project_path: Option<String>,
    pub album_meta: AlbumMeta,
    pub mastering_chain: Vec<MasteringPluginView>,
    /// Backend-computed proposal for the next track title (may be empty).
    pub suggested_title: String,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// Which edge of a region a move targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../../src/types/")]
pub enum RegionEdge {
    Start,
    End,
}

type Snapshot = Vec<Region>;

/// Split a title into its base and a trailing `-<n>` index, if any
/// ("Jam-2" → ("Jam", Some(2)), "Jam" → ("Jam", None)).
fn split_indexed(title: &str) -> (&str, Option<u32>) {
    if let Some((base, idx)) = title.rsplit_once('-') {
        if !base.is_empty() {
            if let Ok(n) = idx.parse::<u32>() {
                return (base, Some(n));
            }
        }
    }
    (title, None)
}

/// Canonical in-memory state: the single source of truth (ARCHITECTURE.md §3).
pub struct ProjectState {
    pub project: Project,
    pub info: AudioInfo,
    /// One peak pyramid per layer (index-aligned with `project.layers`).
    pub peaks: Vec<PeakPyramid>,
    /// Path of the `.still` file, once saved/loaded.
    pub project_path: Option<PathBuf>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl ProjectState {
    pub fn new(project: Project, info: AudioInfo, peaks: Vec<PeakPyramid>) -> Self {
        Self {
            project,
            info,
            peaks,
            project_path: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    fn min_len(&self) -> u64 {
        (self.info.sample_rate as u64 * MIN_TRACK_MS / 1000).max(1)
    }

    /// Replace the scanned audio (after appending clips or layers) while
    /// keeping the project recipe and the undo/redo history — regions only
    /// ever reference timeline positions, and appending never moves audio.
    pub fn set_audio(&mut self, info: AudioInfo, peaks: Vec<PeakPyramid>) {
        self.info = info;
        self.peaks = peaks;
    }

    /// THE volume resolver: the linear volume of every layer, either for the
    /// session defaults (`region: None`) or inside a given track region
    /// (gain/mute/solo overrides applied). Solo semantics: if any layer is
    /// soloed in the resolved context, only soloed layers are audible (solo
    /// wins over mute). Display, playback and export all use this.
    pub fn effective_volumes(&self, region: Option<&Region>) -> Vec<f32> {
        let layers = &self.project.layers;
        let resolved: Vec<(bool, bool, f32)> = layers
            .iter()
            .map(|l| {
                let key = l.id.to_string();
                let muted = region
                    .and_then(|r| r.mute_overrides.get(&key))
                    .copied()
                    .unwrap_or(l.muted);
                let solo = region
                    .and_then(|r| r.solo_overrides.get(&key))
                    .copied()
                    .unwrap_or(l.solo);
                let gain = region
                    .and_then(|r| r.gain_overrides.get(&key))
                    .copied()
                    .unwrap_or(l.gain_db);
                (muted, solo, gain)
            })
            .collect();
        let any_solo = resolved.iter().any(|(_, s, _)| *s);
        resolved
            .iter()
            .map(|(muted, solo, gain)| {
                let audible = if any_solo { *solo } else { !*muted };
                if audible {
                    db_to_linear(*gain)
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Regions whose overrides change the audible volumes, with the resolved
    /// values — the volume "automation" of the timeline.
    pub fn volume_spans(&self) -> Vec<(u64, u64, Vec<f32>)> {
        self.project
            .regions
            .iter()
            .filter(|r| {
                !r.gain_overrides.is_empty()
                    || !r.mute_overrides.is_empty()
                    || !r.solo_overrides.is_empty()
            })
            .map(|r| (r.start, r.end, self.effective_volumes(Some(r))))
            .collect()
    }

    pub fn set_layer_solo(&mut self, id: u32, solo: bool) -> Result<()> {
        let idx = self.layer_index(id)?;
        self.project.layers[idx].solo = solo;
        Ok(())
    }

    /// Set or clear (None) a per-track mute/solo override for one layer.
    pub fn set_track_layer_flag(
        &mut self,
        track_id: u32,
        layer_id: u32,
        solo_flag: bool,
        value: Option<bool>,
    ) -> Result<()> {
        if !self.project.layers.iter().any(|l| l.id == layer_id) {
            return Err(StillError::InvalidMarker(format!(
                "unknown layer id {layer_id}"
            )));
        }
        let idx = self
            .project
            .regions
            .iter()
            .position(|r| r.id == track_id)
            .ok_or_else(|| StillError::InvalidMarker(format!("unknown track id {track_id}")))?;
        self.push_undo();
        let region = &mut self.project.regions[idx];
        let key = layer_id.to_string();
        let map = if solo_flag {
            &mut region.solo_overrides
        } else {
            &mut region.mute_overrides
        };
        match value {
            Some(v) => {
                map.insert(key, v);
            }
            None => {
                map.remove(&key);
            }
        }
        Ok(())
    }

    /// Position-dependent linear volume per layer: session defaults
    /// everywhere, replaced inside track regions by their overrides — so the
    /// waveform shows exactly what is heard.
    fn gain_resolver(&self) -> impl Fn(u64, usize) -> f32 + '_ {
        let defaults = self.effective_volumes(None);
        let overridden = self.volume_spans();
        move |sample, li| {
            for (s, e, gains) in &overridden {
                if sample >= *s && sample < *e {
                    return gains.get(li).copied().unwrap_or(0.0);
                }
            }
            defaults.get(li).copied().unwrap_or(0.0)
        }
    }

    /// Display peaks of the current mix over a window (mutes, faders and
    /// per-track overrides applied).
    pub fn peaks_slice(
        &self,
        start_sample: u64,
        end_sample: u64,
        max_buckets: u32,
    ) -> crate::peaks::PeakSlice {
        let pyramids: Vec<&PeakPyramid> = self.peaks.iter().collect();
        crate::peaks::merged_query_with(
            &pyramids,
            self.info.channels.max(1) as usize,
            start_sample,
            end_sample,
            max_buckets,
            self.gain_resolver(),
        )
    }

    /// One display slice PER LAYER over a window, each scaled by that layer's
    /// effective gain at every position (session fader, mute, and the track
    /// overrides where they apply) — the "layers" waveform view.
    pub fn layer_slices(
        &self,
        start_sample: u64,
        end_sample: u64,
        max_buckets: u32,
    ) -> Vec<crate::peaks::PeakSlice> {
        let resolver = self.gain_resolver();
        self.peaks
            .iter()
            .enumerate()
            .map(|(li, p)| {
                crate::peaks::scaled_query_with(p, start_sample, end_sample, max_buckets, |s| {
                    resolver(s, li)
                })
            })
            .collect()
    }

    /// Base-resolution pyramid of the current mix (silence detection input).
    pub fn merged_pyramid(&self) -> PeakPyramid {
        let buckets = self
            .info
            .duration_samples
            .div_ceil(crate::peaks::BASE_SAMPLES_PER_BUCKET as u64)
            .max(1);
        let slice = self.peaks_slice(0, self.info.duration_samples, buckets as u32);
        PeakPyramid {
            levels: vec![crate::peaks::PeakLevel {
                samples_per_bucket: slice.samples_per_bucket,
                channels: slice.channels,
            }],
        }
    }

    fn layer_index(&self, id: u32) -> Result<usize> {
        self.project
            .layers
            .iter()
            .position(|l| l.id == id)
            .ok_or_else(|| StillError::InvalidMarker(format!("unknown layer id {id}")))
    }

    /// Set a layer's mix gain; returns the clamped value actually applied.
    pub fn set_layer_gain(&mut self, id: u32, gain_db: f32) -> Result<f32> {
        let idx = self.layer_index(id)?;
        let clamped = gain_db.clamp(MIN_GAIN_DB, MAX_GAIN_DB);
        self.project.layers[idx].gain_db = clamped;
        Ok(clamped)
    }

    pub fn set_layer_muted(&mut self, id: u32, muted: bool) -> Result<()> {
        let idx = self.layer_index(id)?;
        self.project.layers[idx].muted = muted;
        Ok(())
    }

    /// Display preference persisted in the project: collapsed lanes in the
    /// "Layers" waveform view.
    pub fn set_layer_collapsed(&mut self, id: u32, collapsed: bool) -> Result<()> {
        let idx = self.layer_index(id)?;
        self.project.layers[idx].collapsed = collapsed;
        Ok(())
    }

    /// Remove a layer (never the base layer 0, which carries the timeline).
    pub fn remove_layer(&mut self, id: u32) -> Result<()> {
        let idx = self.layer_index(id)?;
        if idx == 0 {
            return Err(StillError::InvalidMarker(
                "the base layer cannot be removed".into(),
            ));
        }
        self.project.layers.remove(idx);
        if idx < self.info.layers.len() {
            self.info.layers.remove(idx);
        }
        if idx < self.peaks.len() {
            self.peaks.remove(idx);
        }
        // The timeline may shrink if the removed layer was the longest.
        let duration = self
            .info
            .layers
            .iter()
            .map(|l| l.duration_samples)
            .max()
            .unwrap_or(0);
        self.info.duration_samples = duration;
        self.info.duration_seconds = duration as f64 / self.info.sample_rate as f64;
        self.info.channels = self.info.layers.iter().map(|l| l.channels).max().unwrap_or(0);
        sanitize_regions(&mut self.project, duration, self.info.sample_rate);
        Ok(())
    }

    fn push_undo(&mut self) {
        self.undo.push(self.project.regions.clone());
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project.regions, s));
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project.regions, s));
            true
        } else {
            false
        }
    }

    /// Sorted view of regions (by start).
    fn sorted(&self) -> Vec<&Region> {
        let mut v: Vec<&Region> = self.project.regions.iter().collect();
        v.sort_by_key(|r| r.start);
        v
    }

    /// Normalize, clamp and trim a candidate span against existing regions.
    /// Fails when an existing region sits inside the span or no room is left.
    fn fit_span(&self, start: u64, end: u64) -> Result<(u64, u64)> {
        let (mut s, mut e) = if start <= end { (start, end) } else { (end, start) };
        let dur = self.info.duration_samples;
        e = e.min(dur);
        for r in &self.project.regions {
            if r.start >= s && r.end <= e {
                return Err(StillError::InvalidMarker(
                    "the selection contains an existing track".into(),
                ));
            }
            if r.start <= s && s < r.end {
                s = r.end;
            }
            if r.start < e && e <= r.end {
                e = r.start;
            }
        }
        if e.saturating_sub(s) < self.min_len() {
            return Err(StillError::InvalidMarker(
                "not enough room here for a track (it would overlap an existing one or be shorter than 200 ms)".into(),
            ));
        }
        Ok((s, e))
    }

    /// Create a track region from a start/end pair (any order), optionally
    /// titled right away. Overlapping edges are trimmed to the free gap.
    /// Returns the new region id.
    pub fn add_region(&mut self, start: u64, end: u64, title: Option<String>) -> Result<u32> {
        let (s, e) = self.fit_span(start, end)?;
        self.push_undo();
        let id = self.project.next_region_id;
        self.project.next_region_id += 1;
        let title = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        self.project.regions.push(Region {
            id,
            start: s,
            end: e,
            title,
            gain_overrides: HashMap::new(),
            mute_overrides: HashMap::new(),
            solo_overrides: HashMap::new(),
            inserts: Vec::new(),
        });
        Ok(id)
    }

    /// Add several regions as one undoable operation (silence detection).
    /// Spans that don't fit are skipped; returns how many were added.
    pub fn add_regions(&mut self, spans: &[RegionSpan]) -> usize {
        let before = self.project.regions.clone();
        let mut added = 0;
        for span in spans {
            if let Ok((s, e)) = self.fit_span(span.start, span.end) {
                let id = self.project.next_region_id;
                self.project.next_region_id += 1;
                self.project.regions.push(Region {
                    id,
                    start: s,
                    end: e,
                    title: None,
                    gain_overrides: HashMap::new(),
                    mute_overrides: HashMap::new(),
                    solo_overrides: HashMap::new(),
                    inserts: Vec::new(),
                });
                added += 1;
            }
        }
        if added > 0 {
            self.undo.push(before);
            self.redo.clear();
        }
        added
    }

    /// Take an undo snapshot explicitly — called once at the START of an
    /// interactive edge drag, so the whole drag undoes in one step while the
    /// individual move previews stay undo-free.
    pub fn begin_edit(&mut self) {
        self.push_undo();
    }

    /// Move one edge WITHOUT touching the undo stack (live drag preview).
    /// Same clamping as `move_edge`.
    pub fn move_edge_preview(&mut self, id: u32, edge: RegionEdge, position: u64) -> Result<u64> {
        self.move_edge_inner(id, edge, position)
    }

    /// Move one edge of a region as a single undoable operation. The position
    /// is clamped to the file bounds, the neighboring regions and the minimum
    /// track length. Returns the final (validated) position.
    pub fn move_edge(&mut self, id: u32, edge: RegionEdge, position: u64) -> Result<u64> {
        self.push_undo();
        match self.move_edge_inner(id, edge, position) {
            Ok(p) => Ok(p),
            Err(e) => {
                // Drop the snapshot taken for a move that never happened.
                self.undo();
                self.redo.clear();
                Err(e)
            }
        }
    }

    fn move_edge_inner(&mut self, id: u32, edge: RegionEdge, position: u64) -> Result<u64> {
        let min_len = self.min_len();
        let dur = self.info.duration_samples;
        let region = self
            .project
            .regions
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| StillError::InvalidMarker(format!("unknown track id {id}")))?;
        let (own_start, own_end) = (region.start, region.end);

        let (lo, hi) = match edge {
            RegionEdge::Start => {
                let prev_end = self
                    .project
                    .regions
                    .iter()
                    .filter(|r| r.id != id && r.end <= own_start)
                    .map(|r| r.end)
                    .max()
                    .unwrap_or(0);
                (prev_end, own_end.saturating_sub(min_len))
            }
            RegionEdge::End => {
                let next_start = self
                    .project
                    .regions
                    .iter()
                    .filter(|r| r.id != id && r.start >= own_end)
                    .map(|r| r.start)
                    .min()
                    .unwrap_or(dur);
                (own_start + min_len, next_start)
            }
        };
        if lo > hi {
            return Err(StillError::InvalidMarker("no room to move this marker".into()));
        }
        let clamped = position.clamp(lo, hi);
        let region = self
            .project
            .regions
            .iter_mut()
            .find(|r| r.id == id)
            .expect("checked above");
        match edge {
            RegionEdge::Start => region.start = clamped,
            RegionEdge::End => region.end = clamped,
        }
        Ok(clamped)
    }

    pub fn remove_region(&mut self, id: u32) -> Result<()> {
        let idx = self
            .project
            .regions
            .iter()
            .position(|r| r.id == id)
            .ok_or_else(|| StillError::InvalidMarker(format!("unknown track id {id}")))?;
        self.push_undo();
        self.project.regions.remove(idx);
        Ok(())
    }

    /// Set (or clear, with an empty string) a layer's display name.
    pub fn rename_layer(&mut self, id: u32, name: &str) -> Result<()> {
        let layer = self
            .project
            .layers
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or_else(|| StillError::InvalidMarker(format!("unknown layer id {id}")))?;
        let name = name.trim();
        layer.custom_name = if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        };
        Ok(())
    }

    pub fn rename_track(&mut self, id: u32, title: &str) -> Result<()> {
        if !self.project.regions.iter().any(|r| r.id == id) {
            return Err(StillError::InvalidMarker(format!("unknown track id {id}")));
        }
        self.push_undo();
        let title = title.trim();
        let region = self
            .project
            .regions
            .iter_mut()
            .find(|r| r.id == id)
            .expect("checked above");
        region.title = if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        };
        Ok(())
    }

    /// The insert chain owned by `target`, or None when the layer/track id
    /// no longer exists.
    pub fn chain(&self, target: ChainTarget) -> Option<&Vec<MasteringPluginCfg>> {
        match target {
            ChainTarget::Master => Some(&self.project.mastering_chain),
            ChainTarget::Layer { id } => self
                .project
                .layers
                .iter()
                .find(|l| l.id == id)
                .map(|l| &l.inserts),
            ChainTarget::Track { id } => self
                .project
                .regions
                .iter()
                .find(|r| r.id == id)
                .map(|r| &r.inserts),
        }
    }

    pub fn chain_mut(&mut self, target: ChainTarget) -> Option<&mut Vec<MasteringPluginCfg>> {
        match target {
            ChainTarget::Master => Some(&mut self.project.mastering_chain),
            ChainTarget::Layer { id } => self
                .project
                .layers
                .iter_mut()
                .find(|l| l.id == id)
                .map(|l| &mut l.inserts),
            ChainTarget::Track { id } => self
                .project
                .regions
                .iter_mut()
                .find(|r| r.id == id)
                .map(|r| &mut r.inserts),
        }
    }

    /// Every chain cfg of the project (master, then layers, then tracks) —
    /// the id universe rebuilds and retains must be computed over.
    pub fn all_chain_cfgs(&self) -> impl Iterator<Item = &MasteringPluginCfg> {
        self.project
            .mastering_chain
            .iter()
            .chain(self.project.layers.iter().flat_map(|l| l.inserts.iter()))
            .chain(self.project.regions.iter().flat_map(|r| r.inserts.iter()))
    }

    /// Mutable variant of [`all_chain_cfgs`] (state snapshots).
    pub fn all_chain_cfgs_mut(&mut self) -> impl Iterator<Item = &mut MasteringPluginCfg> {
        self.project
            .mastering_chain
            .iter_mut()
            .chain(self.project.layers.iter_mut().flat_map(|l| l.inserts.iter_mut()))
            .chain(self.project.regions.iter_mut().flat_map(|r| r.inserts.iter_mut()))
    }

    /// The chain that owns plugin `id` (ids are globally unique across
    /// master, layers and tracks).
    pub fn chain_containing_mut(&mut self, plugin_id: u32) -> Option<&mut Vec<MasteringPluginCfg>> {
        if self.project.mastering_chain.iter().any(|c| c.id == plugin_id) {
            return Some(&mut self.project.mastering_chain);
        }
        if let Some(l) = self
            .project
            .layers
            .iter_mut()
            .find(|l| l.inserts.iter().any(|c| c.id == plugin_id))
        {
            return Some(&mut l.inserts);
        }
        self.project
            .regions
            .iter_mut()
            .find(|r| r.inserts.iter().any(|c| c.id == plugin_id))
            .map(|r| &mut r.inserts)
    }

    /// Set (or clear, with `None`) a per-track gain override for one layer.
    /// Undoable like any region edit; returns the clamped value applied.
    pub fn set_track_layer_gain(
        &mut self,
        track_id: u32,
        layer_id: u32,
        gain_db: Option<f32>,
    ) -> Result<Option<f32>> {
        if !self.project.layers.iter().any(|l| l.id == layer_id) {
            return Err(StillError::InvalidMarker(format!(
                "unknown layer id {layer_id}"
            )));
        }
        let idx = self
            .project
            .regions
            .iter()
            .position(|r| r.id == track_id)
            .ok_or_else(|| StillError::InvalidMarker(format!("unknown track id {track_id}")))?;
        self.push_undo();
        let region = &mut self.project.regions[idx];
        let key = layer_id.to_string();
        match gain_db {
            Some(db) => {
                let clamped = db.clamp(MIN_GAIN_DB, MAX_GAIN_DB);
                region.gain_overrides.insert(key, clamped);
                Ok(Some(clamped))
            }
            None => {
                region.gain_overrides.remove(&key);
                Ok(None)
            }
        }
    }

    /// Suggest a title for the next track: take the most recently titled
    /// region, strip any trailing `-<n>` index, and propose the base with the
    /// next free index. A bare title counts as the first occurrence, so the
    /// sequence is "Jam" → "Jam-2" → "Jam-3" …. Empty when no track has been
    /// titled yet.
    pub fn suggest_title(&self) -> String {
        let Some(last) = self
            .project
            .regions
            .iter()
            .filter(|r| r.title.is_some())
            .max_by_key(|r| r.id)
        else {
            return String::new();
        };
        let (base, _) = split_indexed(last.title.as_deref().unwrap());
        let max_index = self
            .project
            .regions
            .iter()
            .filter_map(|r| r.title.as_deref())
            .filter_map(|t| {
                let (b, i) = split_indexed(t);
                // A bare "Jam" is the first occurrence (index 1), so the
                // first suggestion after it is "Jam-2".
                (b == base).then_some(i.unwrap_or(1))
            })
            .max()
            .unwrap_or(0);
        format!("{base}-{}", max_index + 1)
    }

    /// Tracks ordered by position, with resolved titles.
    pub fn tracks(&self) -> Vec<TrackInfo> {
        let sr = self.info.sample_rate as f64;
        self.sorted()
            .iter()
            .enumerate()
            .map(|(i, r)| TrackInfo {
                id: r.id,
                number: (i + 1) as u32,
                title: r
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Track {:02}", i + 1)),
                start_sample: r.start,
                end_sample: r.end,
                duration_seconds: (r.end.saturating_sub(r.start)) as f64 / sr,
                gain_overrides: r.gain_overrides.clone(),
                mute_overrides: r.mute_overrides.clone(),
                solo_overrides: r.solo_overrides.clone(),
                layer_volumes: self.effective_volumes(Some(r)),
                inserts: chain_views(&r.inserts),
            })
            .collect()
    }

    pub fn view(&self) -> ProjectView {
        let sr = self.info.sample_rate.max(1) as f64;
        let layers = self
            .project
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let scanned = self.info.layers.get(i);
                let source_name = l
                    .sources
                    .first()
                    .and_then(|s| Path::new(&s.path).file_name())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| format!("Layer {}", i + 1));
                LayerView {
                    id: l.id,
                    name: l
                        .custom_name
                        .clone()
                        .unwrap_or_else(|| source_name.clone()),
                    source_name,
                    channels: scanned.map(|s| s.channels).unwrap_or(0),
                    duration_seconds: scanned
                        .map(|s| s.duration_samples as f64 / sr)
                        .unwrap_or(0.0),
                    gain_db: l.gain_db,
                    muted: l.muted,
                    solo: l.solo,
                    collapsed: l.collapsed,
                    inserts: chain_views(&l.inserts),
                }
            })
            .collect();
        ProjectView {
            audio: self.info.clone(),
            layers,
            tracks: self.tracks(),
            snap_to_zero: self.project.snap_to_zero,
            export_config: self.project.export_config.clone(),
            project_path: self
                .project_path
                .as_ref()
                .map(|p| p.display().to_string()),
            album_meta: self.project.album_meta.clone(),
            mastering_chain: chain_views(&self.project.mastering_chain),
            suggested_title: self.suggest_title(),
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
        }
    }
}

/// Write the project recipe to a `.still` file (always a new/overwritten
/// project file — never anything to do with the audio source).
pub fn save_project(project: &Project, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(project)
        .map_err(|e| StillError::InvalidProject(e.to_string()))?;
    std::fs::write(path, json)?;
    Ok(())
}

/// v1 `.still` files used a split-marker model; migrate them to regions.
#[derive(Deserialize)]
struct LegacyProjectV1 {
    source_path: String,
    markers: Vec<LegacyMarker>,
    #[serde(default)]
    track_names: HashMap<String, String>,
    #[serde(default)]
    snap_to_zero: bool,
    #[serde(default)]
    export_config: Option<ExportConfig>,
}

#[derive(Deserialize)]
struct LegacyMarker {
    id: u32,
    position: u64,
}

/// Wrap pre-v4 single-layer data into the layered model.
fn from_single_layer(
    sources: Vec<String>,
    regions: Vec<Region>,
    snap_to_zero: bool,
    export_config: ExportConfig,
    next_region_id: u32,
) -> Project {
    let mut p = Project::new_layers(vec![sources]);
    p.regions = regions;
    p.snap_to_zero = snap_to_zero;
    p.export_config = export_config;
    p.next_region_id = next_region_id;
    p
}

/// v2 `.still` files had a single `source_path`; v3 had `sources`.
#[derive(Deserialize)]
struct LegacyProjectV2 {
    source_path: String,
    regions: Vec<Region>,
    #[serde(default)]
    snap_to_zero: bool,
    #[serde(default)]
    export_config: Option<ExportConfig>,
    next_region_id: u32,
}

fn migrate_v2(legacy: LegacyProjectV2) -> Project {
    from_single_layer(
        vec![legacy.source_path],
        legacy.regions,
        legacy.snap_to_zero,
        legacy.export_config.unwrap_or_default(),
        legacy.next_region_id,
    )
}

/// v3 `.still` files had `sources: Vec<String>`; v4 has layers.
#[derive(Deserialize)]
struct LegacyProjectV3 {
    sources: Vec<String>,
    regions: Vec<Region>,
    #[serde(default)]
    snap_to_zero: bool,
    #[serde(default)]
    export_config: Option<ExportConfig>,
    next_region_id: u32,
}

fn migrate_v3(legacy: LegacyProjectV3) -> Project {
    from_single_layer(
        legacy.sources,
        legacy.regions,
        legacy.snap_to_zero,
        legacy.export_config.unwrap_or_default(),
        legacy.next_region_id,
    )
}

fn migrate_v1(legacy: LegacyProjectV1) -> Project {
    // Split markers divided the whole file into contiguous tracks: recreate
    // them as regions covering [0, m1], [m1, m2], …, [mN, u64::MAX] — the
    // open end is clamped to the real duration on load (load_session).
    let mut markers = legacy.markers;
    markers.sort_by_key(|m| m.position);
    let mut bounds: Vec<(String, u64)> = vec![("0".into(), 0)];
    for m in &markers {
        bounds.push((m.id.to_string(), m.position));
    }
    let mut regions = Vec::new();
    for (i, (key, start)) in bounds.iter().enumerate() {
        let end = bounds.get(i + 1).map(|(_, p)| *p).unwrap_or(u64::MAX);
        regions.push(Region {
            id: (i + 1) as u32,
            start: *start,
            end,
            title: legacy.track_names.get(key).cloned(),
            gain_overrides: HashMap::new(),
            mute_overrides: HashMap::new(),
            solo_overrides: HashMap::new(),
            inserts: Vec::new(),
        });
    }
    let next_region_id = regions.len() as u32 + 1;
    from_single_layer(
        vec![legacy.source_path],
        regions,
        legacy.snap_to_zero,
        legacy.export_config.unwrap_or_default(),
        next_region_id,
    )
}

/// v4 `.still` files stored layer sources as plain strings (always
/// sequential); v5 stores SourceRef with optional take-alignment offsets.
#[derive(Deserialize)]
struct LegacyLayerV4 {
    id: u32,
    sources: Vec<String>,
    gain_db: f32,
    muted: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    collapsed: bool,
}

#[derive(Deserialize)]
struct LegacyProjectV4 {
    layers: Vec<LegacyLayerV4>,
    regions: Vec<Region>,
    #[serde(default)]
    snap_to_zero: bool,
    #[serde(default)]
    export_config: Option<ExportConfig>,
    next_region_id: u32,
    next_layer_id: u32,
}

fn migrate_v4(legacy: LegacyProjectV4) -> Project {
    Project {
        version: PROJECT_VERSION,
        layers: legacy
            .layers
            .into_iter()
            .map(|l| Layer {
                id: l.id,
                sources: l.sources.into_iter().map(SourceRef::sequential).collect(),
                gain_db: l.gain_db,
                muted: l.muted,
                solo: l.solo,
                collapsed: l.collapsed,
                inserts: Vec::new(),
                custom_name: None,
            })
            .collect(),
        regions: legacy.regions,
        snap_to_zero: legacy.snap_to_zero,
        export_config: legacy.export_config.unwrap_or_default(),
        album_meta: AlbumMeta::default(),
        mastering_chain: Vec::new(),
        next_plugin_id: 1,
        next_region_id: legacy.next_region_id,
        next_layer_id: legacy.next_layer_id,
    }
}

pub fn read_project(path: &Path) -> Result<Project> {
    let data = std::fs::read_to_string(path)
        .map_err(|_| StillError::FileNotFound(path.display().to_string()))?;
    let version = serde_json::from_str::<serde_json::Value>(&data)
        .ok()
        .and_then(|v| v.get("version").and_then(|x| x.as_u64()))
        .unwrap_or(0) as u32;
    if version > PROJECT_VERSION {
        return Err(StillError::InvalidProject(format!(
            "this project was created by a newer version of AudioDistillery (v{version})"
        )));
    }
    if version <= 1 {
        let legacy: LegacyProjectV1 = serde_json::from_str(&data)
            .map_err(|e| StillError::InvalidProject(e.to_string()))?;
        return Ok(migrate_v1(legacy));
    }
    if version == 2 {
        let legacy: LegacyProjectV2 = serde_json::from_str(&data)
            .map_err(|e| StillError::InvalidProject(e.to_string()))?;
        return Ok(migrate_v2(legacy));
    }
    if version == 3 {
        let legacy: LegacyProjectV3 = serde_json::from_str(&data)
            .map_err(|e| StillError::InvalidProject(e.to_string()))?;
        return Ok(migrate_v3(legacy));
    }
    if version == 4 {
        let legacy: LegacyProjectV4 = serde_json::from_str(&data)
            .map_err(|e| StillError::InvalidProject(e.to_string()))?;
        return Ok(migrate_v4(legacy));
    }
    // v5 → v6 added `album_meta`; v6 → v7 added the per-layer and
    // per-track `inserts` — all #[serde(default)]: parse as current and
    // bump the version.
    if version == 5 || version == 6 {
        let mut p: Project = serde_json::from_str(&data)
            .map_err(|e| StillError::InvalidProject(e.to_string()))?;
        p.version = PROJECT_VERSION;
        return Ok(p);
    }
    serde_json::from_str(&data).map_err(|e| StillError::InvalidProject(e.to_string()))
}

/// Clamp region bounds against the real (scanned) source duration and drop
/// degenerate regions — used right after loading a project.
pub fn sanitize_regions(project: &mut Project, duration_samples: u64, sample_rate: u32) {
    let min_len = (sample_rate as u64 * MIN_TRACK_MS / 1000).max(1);
    for r in &mut project.regions {
        r.end = r.end.min(duration_samples);
        r.start = r.start.min(duration_samples);
    }
    project.regions.retain(|r| r.end.saturating_sub(r.start) >= min_len);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;
    const SEC: u64 = SR as u64;

    fn state(duration_secs: u64) -> ProjectState {
        let clips = vec![crate::audio::ClipInfo {
            path: "/tmp/test.wav".into(),
            name: "test.wav".into(),
            start_sample: 0,
            duration_samples: duration_secs * SEC,
        }];
        let info = AudioInfo {
            path: "/tmp/test.wav".into(),
            clips: clips.clone(),
            layers: vec![crate::audio::ScannedLayer {
                clips,
                channels: 2,
                duration_samples: duration_secs * SEC,
            }],
            duration_samples: duration_secs * SEC,
            sample_rate: SR,
            channels: 2,
            format: "WAV".into(),
            duration_seconds: duration_secs as f64,
        };
        ProjectState::new(
            Project::new(vec![info.path.clone()]),
            info,
            vec![PeakPyramid::default()],
        )
    }

    #[test]
    fn add_region_orders_and_numbers_tracks() {
        let mut s = state(120);
        s.add_region(60 * SEC, 90 * SEC, None).unwrap();
        s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        let tracks = s.tracks();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].start_sample, 10 * SEC);
        assert_eq!(tracks[0].number, 1);
        assert_eq!(tracks[0].title, "Track 01");
        assert_eq!(tracks[1].start_sample, 60 * SEC);
        assert_eq!(tracks[1].title, "Track 02");
    }

    #[test]
    fn add_region_accepts_reversed_bounds() {
        let mut s = state(120);
        s.add_region(40 * SEC, 10 * SEC, None).unwrap();
        let t = &s.tracks()[0];
        assert_eq!(t.start_sample, 10 * SEC);
        assert_eq!(t.end_sample, 40 * SEC);
    }

    #[test]
    fn add_region_trims_overlapping_edges() {
        let mut s = state(120);
        s.add_region(30 * SEC, 60 * SEC, None).unwrap();
        // Overlaps the start of the existing region: trimmed to end at 30 s.
        s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        let tracks = s.tracks();
        assert_eq!(tracks[0].end_sample, 30 * SEC);
        // Fully containing an existing region: rejected.
        assert!(s.add_region(5 * SEC, 100 * SEC, None).is_err());
        // Fully inside an existing region: rejected (no room).
        assert!(s.add_region(35 * SEC, 55 * SEC, None).is_err());
    }

    #[test]
    fn add_region_rejects_too_short() {
        let mut s = state(120);
        assert!(s.add_region(SEC, SEC + 100, None).is_err());
    }

    #[test]
    fn move_edges_clamp_to_neighbors_and_length() {
        let mut s = state(120);
        let a = s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        let b = s.add_region(60 * SEC, 90 * SEC, None).unwrap();
        // b.start can't cross a.end.
        let p = s.move_edge(b, RegionEdge::Start, 20 * SEC).unwrap();
        assert_eq!(p, 40 * SEC);
        // a.end can't cross b.start (now 40 s).
        let q = s.move_edge(a, RegionEdge::End, 80 * SEC).unwrap();
        assert_eq!(q, 40 * SEC);
        // a.start can't make the region shorter than min length.
        let r = s.move_edge(a, RegionEdge::Start, 119 * SEC).unwrap();
        assert!(r < 40 * SEC);
        // end can't exceed the file duration.
        let e = s.move_edge(b, RegionEdge::End, 500 * SEC).unwrap();
        assert_eq!(e, 120 * SEC);
    }

    #[test]
    fn rename_and_default_titles() {
        let mut s = state(120);
        let a = s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        s.rename_track(a, "Opening").unwrap();
        assert_eq!(s.tracks()[0].title, "Opening");
        // A region added before it renumbers, but the custom title sticks.
        s.add_region(SEC, 5 * SEC, None).unwrap();
        let tracks = s.tracks();
        assert_eq!(tracks[0].title, "Track 01");
        assert_eq!(tracks[1].title, "Opening");
        // Empty title restores the default.
        s.rename_track(a, "  ").unwrap();
        assert_eq!(s.tracks()[1].title, "Track 02");
    }

    #[test]
    fn remove_region_frees_the_span() {
        let mut s = state(120);
        let a = s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        s.remove_region(a).unwrap();
        assert!(s.tracks().is_empty());
        // The span is free again.
        s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        assert!(s.remove_region(999).is_err());
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut s = state(120);
        s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        assert_eq!(s.tracks().len(), 1);
        assert!(s.undo());
        assert!(s.tracks().is_empty());
        assert!(s.redo());
        assert_eq!(s.tracks().len(), 1);
        assert!(!s.redo());
    }

    #[test]
    fn bulk_add_skips_misfits_single_undo() {
        let mut s = state(120);
        s.add_region(30 * SEC, 60 * SEC, None).unwrap();
        let added = s.add_regions(&[
            RegionSpan { start: 5 * SEC, end: 20 * SEC },
            RegionSpan { start: 35 * SEC, end: 50 * SEC }, // inside existing
            RegionSpan { start: 70 * SEC, end: 90 * SEC },
        ]);
        assert_eq!(added, 2);
        assert_eq!(s.tracks().len(), 3);
        assert!(s.undo());
        assert_eq!(s.tracks().len(), 1);
    }

    #[test]
    fn solo_and_overrides_resolve_volumes() {
        let mut s = state(600);
        s.project = Project::new_layers(vec![
            vec!["/tmp/a.wav".into()],
            vec!["/tmp/b.wav".into()],
            vec!["/tmp/c.wav".into()],
        ]);
        let ids: Vec<u32> = s.project.layers.iter().map(|l| l.id).collect();

        // No solo, no mute: everyone at unity.
        assert_eq!(s.effective_volumes(None), vec![1.0, 1.0, 1.0]);
        // Session solo on layer 2: only layer 2 audible.
        s.set_layer_solo(ids[1], true).unwrap();
        assert_eq!(s.effective_volumes(None), vec![0.0, 1.0, 0.0]);
        // Solo wins over mute on the soloed layer itself.
        s.set_layer_muted(ids[1], true).unwrap();
        assert_eq!(s.effective_volumes(None), vec![0.0, 1.0, 0.0]);
        s.set_layer_muted(ids[1], false).unwrap();

        // A track that overrides the solo away and mutes layer 3 instead.
        let track = s.add_region(10 * SEC, 20 * SEC, None).unwrap();
        s.set_track_layer_flag(track, ids[1], true, Some(false)).unwrap();
        s.set_track_layer_flag(track, ids[2], false, Some(true)).unwrap();
        let region = s.project.regions.iter().find(|r| r.id == track).cloned().unwrap();
        assert_eq!(s.effective_volumes(Some(&region)), vec![1.0, 1.0, 0.0]);
        // The session mix is untouched outside the track.
        assert_eq!(s.effective_volumes(None), vec![0.0, 1.0, 0.0]);
        // volume_spans exposes the override region for playback automation.
        let spans = s.volume_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, 10 * SEC);
        assert_eq!(spans[0].2, vec![1.0, 1.0, 0.0]);
        // Clearing both flags empties the span list.
        s.set_track_layer_flag(track, ids[1], true, None).unwrap();
        s.set_track_layer_flag(track, ids[2], false, None).unwrap();
        assert!(s.volume_spans().is_empty());
    }

    #[test]
    fn peaks_reflect_track_overrides_in_place() {
        use crate::peaks::PeakBuilder;
        // Two mono layers of constant 0.5 amplitude over 60 s.
        let n = 60 * SEC as usize;
        let make_pyr = || {
            let mut b = PeakBuilder::new(1);
            b.push_interleaved(&vec![0.5f32; n]);
            b.finish().0
        };
        let mut s = state(60);
        s.project = Project::new_layers(vec![
            vec!["/tmp/a.wav".into()],
            vec!["/tmp/b.wav".into()],
        ]);
        s.peaks = vec![make_pyr(), make_pyr()];
        // A track covering [10 s, 20 s) that silences layer 2.
        let track = s.add_region(10 * SEC, 20 * SEC, None).unwrap();
        let layer2 = s.project.layers[1].id;
        s.set_track_layer_gain(track, layer2, Some(-60.0)).unwrap();

        let slice = s.peaks_slice(0, 60 * SEC, 600);
        let spb = slice.samples_per_bucket as u64;
        let value_at = |sec: u64| slice.channels[0][((sec * SEC / spb) as usize) * 2 + 1];
        // Outside the track: 0.5 + 0.5 = 1.0 → 127. Inside: 0.5 alone → ~63.
        assert!(value_at(5) > 120, "outside = {}", value_at(5));
        assert!(
            (55..=72).contains(&value_at(15)),
            "inside override = {}",
            value_at(15)
        );
        assert!(value_at(30) > 120, "after = {}", value_at(30));

        // Per-layer view: layer 2's lane is silent inside the track only.
        let lanes = s.layer_slices(0, 60 * SEC, 600);
        let lane_at = |li: usize, sec: u64| {
            lanes[li].channels[0][((sec * SEC / spb) as usize) * 2 + 1]
        };
        assert!((55..=72).contains(&lane_at(1, 5)), "{}", lane_at(1, 5));
        assert_eq!(lane_at(1, 15), 0);
        assert!((55..=72).contains(&lane_at(0, 15)), "{}", lane_at(0, 15));
    }

    #[test]
    fn suggests_indexed_titles() {
        let mut s = state(600);
        // Nothing titled yet → no suggestion.
        assert_eq!(s.suggest_title(), "");
        s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        assert_eq!(s.suggest_title(), "");
        // First custom title "Jam" counts as the first occurrence → propose
        // "Jam-2", then "Jam-3", …
        s.add_region(60 * SEC, 90 * SEC, Some("Jam".into())).unwrap();
        assert_eq!(s.suggest_title(), "Jam-2");
        s.add_region(100 * SEC, 130 * SEC, Some(s.suggest_title())).unwrap();
        assert_eq!(s.suggest_title(), "Jam-3");
        // A new base resets the sequence.
        s.add_region(150 * SEC, 180 * SEC, Some("Ballad".into())).unwrap();
        assert_eq!(s.suggest_title(), "Ballad-2");
        // Existing higher index wins: "Ballad-7" → propose "Ballad-8".
        let id = s.add_region(200 * SEC, 230 * SEC, Some("Ballad-7".into())).unwrap();
        assert_eq!(s.suggest_title(), "Ballad-8");
        // Renaming does not change which region was titled most recently,
        // but the title used for the base comes from that region's current title.
        s.rename_track(id, "Encore").unwrap();
        assert_eq!(s.suggest_title(), "Encore-2");
    }

    #[test]
    fn add_region_with_blank_title_uses_default() {
        let mut s = state(120);
        s.add_region(10 * SEC, 40 * SEC, Some("   ".into())).unwrap();
        assert_eq!(s.tracks()[0].title, "Track 01");
        assert_eq!(s.suggest_title(), "");
    }

    #[test]
    fn project_save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.still");
        let mut s = state(120);
        let a = s.add_region(10 * SEC, 40 * SEC, None).unwrap();
        s.rename_track(a, "Intro").unwrap();
        save_project(&s.project, &path).unwrap();
        let loaded = read_project(&path).unwrap();
        assert_eq!(loaded.version, PROJECT_VERSION);
        assert_eq!(loaded.regions.len(), 1);
        assert_eq!(loaded.regions[0].title.as_deref(), Some("Intro"));
    }

    #[test]
    fn migrates_v1_split_projects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.still");
        let v1 = serde_json::json!({
            "version": 1,
            "source_path": "/tmp/test.wav",
            "markers": [
                { "id": 2, "position": 60 * SEC },
                { "id": 1, "position": 30 * SEC }
            ],
            "track_names": { "0": "Opening", "2": "Finale" },
            "snap_to_zero": true,
            "export_config": ExportConfig::default(),
            "next_marker_id": 3
        });
        std::fs::write(&path, v1.to_string()).unwrap();
        let mut p = read_project(&path).unwrap();
        assert_eq!(p.version, PROJECT_VERSION);
        assert!(p.snap_to_zero);
        assert_eq!(p.regions.len(), 3);
        sanitize_regions(&mut p, 120 * SEC, SR);
        assert_eq!(p.regions.len(), 3);
        assert_eq!(p.regions[0].title.as_deref(), Some("Opening"));
        assert_eq!(p.regions[0].start, 0);
        assert_eq!(p.regions[0].end, 30 * SEC);
        assert_eq!(p.regions[2].title.as_deref(), Some("Finale"));
        assert_eq!(p.regions[2].end, 120 * SEC);
    }

    #[test]
    fn migrates_v2_single_source_projects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old2.still");
        let v2 = serde_json::json!({
            "version": 2,
            "source_path": "/tmp/test.wav",
            "regions": [
                { "id": 1, "start": 10 * SEC, "end": 40 * SEC, "title": "Intro" }
            ],
            "snap_to_zero": false,
            "export_config": ExportConfig::default(),
            "next_region_id": 2
        });
        std::fs::write(&path, v2.to_string()).unwrap();
        let p = read_project(&path).unwrap();
        assert_eq!(p.version, PROJECT_VERSION);
        assert_eq!(p.layers.len(), 1);
        assert_eq!(p.layers[0].sources.len(), 1);
        assert_eq!(p.layers[0].sources[0].path, "/tmp/test.wav");
        assert_eq!(p.layers[0].sources[0].start, None);
        assert_eq!(p.regions.len(), 1);
        assert_eq!(p.regions[0].title.as_deref(), Some("Intro"));
        assert_eq!(p.next_region_id, 2);
    }

    #[test]
    fn sanitize_drops_degenerate_regions() {
        let mut p = Project::new(vec!["/tmp/x.wav".into()]);
        p.regions.push(Region { id: 1, start: 0, end: 10 * SEC, title: None, gain_overrides: HashMap::new(), mute_overrides: HashMap::new(), solo_overrides: HashMap::new(), inserts: Vec::new() });
        p.regions.push(Region { id: 2, start: 200 * SEC, end: 300 * SEC, title: None, gain_overrides: HashMap::new(), mute_overrides: HashMap::new(), solo_overrides: HashMap::new(), inserts: Vec::new() });
        sanitize_regions(&mut p, 120 * SEC, SR);
        assert_eq!(p.regions.len(), 1);
    }

    /// A v6 project (no per-layer/per-track inserts) opens as v7 with empty
    /// chains everywhere, keeping its master chain intact.
    #[test]
    fn migrates_v6_to_v7() {
        let mut p = Project::new(vec!["/tmp/x.wav".into()]);
        p.regions.push(Region { id: 1, start: 0, end: 10 * SEC, title: None, gain_overrides: HashMap::new(), mute_overrides: HashMap::new(), solo_overrides: HashMap::new(), inserts: Vec::new() });
        p.mastering_chain.push(MasteringPluginCfg {
            id: 1,
            component: "aufx:lpas:appl".into(),
            name: "Lowpass".into(),
            bypass: false,
            state_b64: None,
        });
        p.version = 6;
        // Serialize then strip the fields v6 didn't have.
        let mut v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        for l in v["layers"].as_array_mut().unwrap() {
            l.as_object_mut().unwrap().remove("inserts");
        }
        for r in v["regions"].as_array_mut().unwrap() {
            r.as_object_mut().unwrap().remove("inserts");
        }
        let dir = std::env::temp_dir().join(format!("still-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v6.still");
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        let loaded = read_project(&path).unwrap();
        assert_eq!(loaded.version, PROJECT_VERSION);
        assert_eq!(loaded.mastering_chain.len(), 1);
        assert!(loaded.layers.iter().all(|l| l.inserts.is_empty()));
        assert!(loaded.regions.iter().all(|r| r.inserts.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Renaming a layer sets/clears the custom name; the view resolves it
    /// with the file-name fallback.
    #[test]
    fn renames_layers_with_fallback() {
        let mut state = state(60);
        let id = state.project.layers[0].id;
        assert!(state.rename_layer(id, "  Room mic  ").is_ok());
        assert_eq!(state.project.layers[0].custom_name.as_deref(), Some("Room mic"));
        let v = state.view();
        assert_eq!(v.layers[0].name, "Room mic");
        assert!(v.layers[0].source_name.ends_with(".wav"));
        // Empty clears back to the fallback.
        state.rename_layer(id, "   ").unwrap();
        assert!(state.project.layers[0].custom_name.is_none());
        assert_eq!(state.view().layers[0].name, state.view().layers[0].source_name);
        // Unknown id errors.
        assert!(state.rename_layer(9999, "x").is_err());
    }
}
