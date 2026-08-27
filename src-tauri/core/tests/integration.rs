//! Integration tests over the full pipeline with a programmatically generated
//! WAV file, including the decisive non-destructive test from ARCHITECTURE.md §3 bis:
//! after a full load → mark → export scenario, the source file must be
//! byte-for-byte identical (checksum) to its initial state.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use still_core::project::{ExportConfig, ExportFormat, Project};
use still_core::{
    plan_export, resolve_ffmpeg, run_export, scan_file, LayerMix, ProjectState, SilenceParams,
};

/// Unity-gain mix of the session's layers, as the export path expects.
fn mix_of(state: &ProjectState) -> Vec<LayerMix> {
    state
        .info
        .layers
        .iter()
        .map(|l| LayerMix {
            clips: l.clips.clone(),
        })
        .collect()
}

const SR: u32 = 44_100;

/// Generate a stereo WAV: `segments` of (seconds, amplitude) sine bursts.
fn write_wav(path: &Path, segments: &[(f32, f32)]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &(secs, amp) in segments {
        let n = (secs * SR as f32) as usize;
        for i in 0..n {
            let v = (amp * (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / SR as f32).sin()
                * i16::MAX as f32) as i16;
            w.write_sample(v).unwrap();
            w.write_sample(v).unwrap();
        }
    }
    w.finalize().unwrap();
}

fn checksum(path: &Path) -> u64 {
    // Simple FNV-1a over the whole file; enough to detect any byte change.
    let data = std::fs::read(path).unwrap();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn scan_reports_exact_duration_and_peaks() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("src.wav");
    write_wav(&wav, &[(2.0, 0.8)]);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    assert_eq!(info.sample_rate, SR);
    assert_eq!(info.channels, 2);
    assert_eq!(info.duration_samples, (2.0 * SR as f32) as u64);
    assert_eq!(peaks.channel_count(), 2);
    let slice = peaks.query(0, info.duration_samples, 500);
    assert!(!slice.channels[0].is_empty());
    // The signal peaks near 0.8.
    let max = slice.channels[0].iter().copied().max().unwrap();
    assert!(max > 90, "expected loud signal, got {max}");
}

#[test]
fn silence_detection_on_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("src.wav");
    // 20 s music, 2 s silence, 20 s music.
    write_wav(&wav, &[(20.0, 0.7), (2.0, 0.0), (20.0, 0.7)]);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let regions = still_core::detect_track_regions(
        &peaks,
        info.sample_rate,
        info.duration_samples,
        &SilenceParams::default(),
    );
    assert_eq!(regions.len(), 2);
    let gap_start = regions[0].end as f64 / SR as f64;
    let gap_end = regions[1].start as f64 / SR as f64;
    assert!(gap_start > 19.5 && gap_start < 20.5, "gap starts at {gap_start}s");
    assert!(gap_end > 21.5 && gap_end < 22.5, "gap ends at {gap_end}s");
}

#[test]
fn full_scenario_is_non_destructive_and_sample_accurate() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("source.wav");
    write_wav(&wav, &[(3.0, 0.6)]);
    let checksum_before = checksum(&wav);

    // Load.
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(
        Project::new(vec![wav.display().to_string()]),
        info.clone(),
        vec![peaks],
    );

    // Mark three regions of exactly 1 s each; the middle second [1s, 2s) is
    // NOT covered by any region and must be ignored by the export.
    let a = state.add_region(0, SR as u64, None).unwrap();
    state.add_region(2 * SR as u64, 3 * SR as u64, None).unwrap();
    state.rename_track(a, "One").unwrap();
    let tracks = state.tracks();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].title, "One");

    // Save the project file (a new file, not the source).
    let project_path = dir.path().join("session.still");
    still_core::save_project(&state.project, &project_path).unwrap();

    // Export (skipped gracefully when ffmpeg isn't installed).
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — export part of the test skipped");
        assert_eq!(checksum(&wav), checksum_before);
        return;
    };
    let out_dir = dir.path().join("out");
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: out_dir.display().to_string(),
        ..Default::default()
    };
    let jobs = plan_export(&tracks, &cfg, &wav).unwrap();
    assert_eq!(jobs.len(), 2);
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(report.errors.is_empty(), "export errors: {:?}", report.errors);
    assert_eq!(report.files.len(), 2);
    assert!(!report.cancelled);

    // Sample accuracy: each exported WAV has exactly the expected frame count
    // (1 s each — the uncovered middle second was ignored).
    for (job, expected) in jobs.iter().zip([SR as u64, SR as u64]) {
        let reader = hound::WavReader::open(&job.out_path).unwrap();
        assert_eq!(
            reader.duration() as u64,
            expected,
            "wrong length for {:?}",
            job.out_path
        );
    }

    // Existing files are never overwritten: exporting again suffixes names.
    let jobs2 = plan_export(&tracks, &cfg, &wav).unwrap();
    assert!(jobs2[0]
        .out_path
        .display()
        .to_string()
        .contains("(1)"));

    // THE decisive test: the source file is byte-for-byte untouched.
    assert_eq!(checksum(&wav), checksum_before);
}

/// Compressed formats (FLAC, MP3) must scan too — generated with ffmpeg when
/// available, skipped otherwise.
#[test]
fn scans_compressed_formats() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — compressed-format scan test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("src.wav");
    write_wav(&wav, &[(5.0, 0.6)]);
    for ext in ["flac", "mp3"] {
        let out = dir.path().join(format!("src.{ext}"));
        let status = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-v", "error", "-i"])
            .arg(&wav)
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg failed to produce {ext}");
        let (info, peaks) = scan_file(&out, |_| {}).unwrap();
        assert_eq!(info.sample_rate, SR, "{ext}");
        assert_eq!(info.channels, 2, "{ext}");
        // MP3 adds encoder padding; duration must still be within ~100 ms.
        let secs = info.duration_seconds;
        assert!((secs - 5.0).abs() < 0.1, "{ext}: duration {secs}");
        assert_eq!(peaks.channel_count(), 2, "{ext}");
    }
}

/// Regression test: a track whose region starts late in the file must NOT
/// carry the source timestamps into the output container (that showed up as
/// leading "silence" up to the start marker in players). The exported file
/// must start at t≈0.
#[test]
fn exported_tracks_start_at_time_zero() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — start-time regression test skipped");
        return;
    };
    // ffprobe lives next to ffmpeg (or on PATH when ffmpeg was PATH-resolved).
    let ffprobe = ffmpeg.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("source.wav");
    write_wav(&wav, &[(4.0, 0.6)]);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, vec![peaks]);
    // Region starting at 2 s — far from the file start.
    state.add_region(2 * SR as u64, 3 * SR as u64, None).unwrap();
    let cfg = ExportConfig {
        format: ExportFormat::Aac,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let jobs = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(report.errors.is_empty(), "export errors: {:?}", report.errors);

    let out = std::process::Command::new(&ffprobe)
        .args(["-v", "error", "-show_entries", "format=start_time,duration", "-of", "csv=p=0"])
        .arg(&jobs[0].out_path)
        .output();
    let Ok(out) = out else {
        eprintln!("ffprobe not found — start-time assertion skipped");
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().split(',');
    let start: f64 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
    let duration: f64 = parts.next().unwrap_or("0").parse().unwrap_or(0.0);
    assert!(
        start.abs() < 0.1,
        "exported track starts at {start}s instead of ~0 (leading-silence bug)"
    );
    // AAC adds a little encoder padding; the duration must stay ~1 s.
    assert!((duration - 1.0).abs() < 0.2, "unexpected duration {duration}s");
}

/// Multi-clip session: two WAVs laid back-to-back, one track inside clip 1,
/// one track CROSSING the clip boundary. Both must export sample-accurately
/// and both sources must stay byte-for-byte untouched.
#[test]
fn multi_clip_timeline_and_cross_boundary_export() {
    let dir = tempfile::tempdir().unwrap();
    let wav_a = dir.path().join("side-a.wav");
    let wav_b = dir.path().join("side-b.wav");
    write_wav(&wav_a, &[(2.0, 0.6)]);
    write_wav(&wav_b, &[(3.0, 0.4)]);
    let (sum_a, sum_b) = (checksum(&wav_a), checksum(&wav_b));

    let (info, peaks) =
        still_core::scan_files(&[wav_a.clone(), wav_b.clone()], &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(info.clips.len(), 2);
    assert_eq!(info.clips[0].duration_samples, 2 * SR as u64);
    assert_eq!(info.clips[1].start_sample, 2 * SR as u64);
    assert_eq!(info.duration_samples, 5 * SR as u64);

    let mut state = ProjectState::new(
        Project::new(vec![
            wav_a.display().to_string(),
            wav_b.display().to_string(),
        ]),
        info,
        vec![peaks],
    );
    // Track 1 inside clip A; track 2 crosses the A→B boundary (1.5s → 3.5s).
    state.add_region(0, SR as u64, None).unwrap();
    state
        .add_region(3 * SR as u64 / 2, 7 * SR as u64 / 2, None)
        .unwrap();

    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — multi-clip export test skipped");
        return;
    };
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let jobs = plan_export(&state.tracks(), &cfg, &wav_a).unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(report.errors.is_empty(), "export errors: {:?}", report.errors);
    assert_eq!(report.files.len(), 2);

    // Track 1: exactly 1 s. Track 2 (cross-boundary): exactly 2 s.
    for (job, expected) in jobs.iter().zip([SR as u64, 2 * SR as u64]) {
        let reader = hound::WavReader::open(&job.out_path).unwrap();
        assert_eq!(reader.duration() as u64, expected, "for {:?}", job.out_path);
    }

    // Both sources untouched.
    assert_eq!(checksum(&wav_a), sum_a);
    assert_eq!(checksum(&wav_b), sum_b);
}

/// Multitrack session: a stereo layer + a synced mono layer, mixed at export.
/// The exported track must be the SUM of the layers (so louder than either
/// alone), muting a layer must remove its contribution, durations stay
/// sample-accurate and every source stays byte-for-byte untouched.
#[test]
fn multitrack_layers_mix_at_export() {
    let dir = tempfile::tempdir().unwrap();
    let stereo = dir.path().join("mic-stereo.wav");
    let mono = dir.path().join("input-3.wav");
    // Same frequency and phase → amplitudes add up predictably.
    write_wav(&stereo, &[(2.0, 0.4)]);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&mono, spec).unwrap();
    for i in 0..(2 * SR as usize) {
        let v = (0.3 * (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / SR as f32).sin()
            * i16::MAX as f32) as i16;
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    let (sum_a, sum_b) = (checksum(&stereo), checksum(&mono));

    let (info, pyramids) = still_core::scan_layers(
        &[vec![(stereo.clone(), None)], vec![(mono.clone(), None)]],
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    assert_eq!(info.layers.len(), 2);
    assert_eq!(info.channels, 2, "session is stereo (max over layers)");
    assert_eq!(info.layers[1].channels, 1);

    let mut state = ProjectState::new(
        Project::new_layers(vec![
            vec![stereo.display().to_string()],
            vec![mono.display().to_string()],
        ]),
        info,
        pyramids,
    );
    state.add_region(0, SR as u64, None).unwrap();

    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — multitrack export test skipped");
        return;
    };
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let peak_of = |path: &Path| -> f32 {
        let mut r = hound::WavReader::open(path).unwrap();
        r.samples::<i16>()
            .map(|s| (s.unwrap() as f32 / i16::MAX as f32).abs())
            .fold(0.0, f32::max)
    };
    let mix = |state: &ProjectState| -> Vec<LayerMix> {
        state
            .project
            .layers
            .iter()
            .zip(state.info.layers.iter())
            .map(|(_, scanned)| LayerMix {
                clips: scanned.clips.clone(),
            })
            .collect()
    };

    // Unity mix: 0.4 (stereo mic) + 0.3 (mono input) ≈ 0.6–0.7 depending on
    // the mono upmix gain — well above either layer alone.
    let jobs = plan_export(&state.tracks(), &cfg, &stereo).unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &mix(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(report.errors.is_empty(), "export errors: {:?}", report.errors);
    let reader = hound::WavReader::open(&jobs[0].out_path).unwrap();
    assert_eq!(reader.duration() as u64, SR as u64);
    let full_peak = peak_of(&jobs[0].out_path);
    assert!(full_peak > 0.55, "expected summed mix, peak = {full_peak}");

    // Mute the mono layer: only the stereo mic remains (≈ 0.4).
    let mono_id = state.project.layers[1].id;
    state.set_layer_muted(mono_id, true).unwrap();
    let jobs2 = plan_export(&state.tracks(), &cfg, &stereo).unwrap();
    let report2 = run_export(&ffmpeg, &mix(&state), 2, SR, &jobs2, &cfg, &cancel, |_| {});
    assert!(report2.errors.is_empty(), "{:?}", report2.errors);
    let muted_peak = peak_of(&jobs2[0].out_path);
    assert!(
        (muted_peak - 0.4).abs() < 0.05,
        "expected the stereo layer alone, peak = {muted_peak}"
    );

    // Per-track override: unmute the mono layer globally but override it to
    // -60 dB (-∞) FOR THIS TRACK only → same audible result as muting it.
    state.set_layer_muted(mono_id, false).unwrap();
    let track_id = state.tracks()[0].id;
    state
        .set_track_layer_gain(track_id, mono_id, Some(-60.0))
        .unwrap();
    let jobs3 = plan_export(&state.tracks(), &cfg, &stereo).unwrap();
    let report3 = run_export(&ffmpeg, &mix(&state), 2, SR, &jobs3, &cfg, &cancel, |_| {});
    assert!(report3.errors.is_empty(), "{:?}", report3.errors);
    let override_peak = peak_of(&jobs3[0].out_path);
    assert!(
        (override_peak - 0.4).abs() < 0.05,
        "override should silence the mono layer for this track, peak = {override_peak}"
    );
    // Clearing the override restores the summed mix.
    state.set_track_layer_gain(track_id, mono_id, None).unwrap();
    assert!(state.tracks()[0].gain_overrides.is_empty());

    // Sources untouched, whatever the mixing.
    assert_eq!(checksum(&stereo), sum_a);
    assert_eq!(checksum(&mono), sum_b);
}

/// Take alignment: a second multitrack take is pinned at the end of the
/// longest layer; shorter layers get a silent gap so both takes stay in
/// sync. Peaks must show real silence in the gap, and an export crossing the
/// take boundary must keep every layer sample-aligned (silence inserted).
#[test]
fn takes_align_with_silent_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let a1 = dir.path().join("take1-mic.wav");
    let b1 = dir.path().join("take1-input.wav");
    let a2 = dir.path().join("take2-mic.wav");
    let b2 = dir.path().join("take2-input.wav");
    write_wav(&a1, &[(2.0, 0.5)]); // layer 1 take 1: 2.0 s
    write_wav(&b1, &[(1.5, 0.5)]); // layer 2 take 1: 1.5 s (shorter)
    write_wav(&a2, &[(1.0, 0.5)]);
    write_wav(&b2, &[(1.0, 0.5)]);

    let take2_start = 2 * SR as u64; // end of the longest layer
    let (info, pyramids) = still_core::scan_layers(
        &[
            vec![(a1.clone(), None), (a2.clone(), Some(take2_start))],
            vec![(b1.clone(), None), (b2.clone(), Some(take2_start))],
        ],
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();

    // Timeline: 3 s total; layer 2's second clip starts at 2 s after a
    // 0.5 s silent gap.
    assert_eq!(info.duration_samples, 3 * SR as u64);
    assert_eq!(info.layers[0].clips[1].start_sample, take2_start);
    assert_eq!(info.layers[1].clips[1].start_sample, take2_start);
    assert_eq!(info.layers[1].duration_samples, 3 * SR as u64);

    // The gap really is silence in layer 2's peaks (~1.6 s → 1.9 s).
    let gap = pyramids[1].query(
        (1.6 * SR as f64) as u64,
        (1.9 * SR as f64) as u64,
        64,
    );
    assert!(
        gap.channels.iter().all(|ch| ch.iter().all(|&v| v == 0)),
        "expected silence in the take gap"
    );

    // A track crossing the gap and the take boundary: every layer must
    // contribute exactly the same number of samples (silence included).
    let mut state = ProjectState::new(
        Project::new_layers(vec![
            vec![a1.display().to_string(), a2.display().to_string()],
            vec![b1.display().to_string(), b2.display().to_string()],
        ]),
        info,
        pyramids,
    );
    let region_start = (1.2 * SR as f64) as u64;
    let region_end = (2.6 * SR as f64) as u64;
    state.add_region(region_start, region_end, None).unwrap();

    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — take export test skipped");
        return;
    };
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let jobs = plan_export(&state.tracks(), &cfg, &a1).unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(report.errors.is_empty(), "export errors: {:?}", report.errors);
    let reader = hound::WavReader::open(&jobs[0].out_path).unwrap();
    assert_eq!(reader.duration() as u64, region_end - region_start);
}

/// Album metadata written through the abstract model must come back intact
/// from each container's NATIVE tag format (Vorbis for FLAC, ID3v2 for MP3,
/// MP4 atoms for M4A), with macro expansion and multi-disc numbering.
#[test]
fn exported_files_carry_native_tags() {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::*;

    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — tagging test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("source.wav");
    write_wav(&wav, &[(4.0, 0.5)]);
    // Cover art: a small PNG generated by ffmpeg.
    let cover = dir.path().join("cover.png");
    let status = std::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-v", "error", "-f", "lavfi", "-i",
               "color=c=orange:s=64x64", "-frames:v", "1"])
        .arg(&cover)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg failed to produce the cover");
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(
        Project::new(vec![wav.display().to_string()]),
        info,
        vec![peaks],
    );
    // 4 tracks of 1 s; disc break at track 3 → discs of 2 tracks each.
    for i in 0..4u64 {
        state
            .add_region(i * SR as u64, (i + 1) * SR as u64, Some(format!("Song {}", i + 1)))
            .unwrap();
    }
    state.project.album_meta = still_core::AlbumMeta {
        album: "Barn Sessions (Disc {disc})".into(),
        album_artist: "The Copper Stills".into(),
        artist: "".into(),
        date: "2026-08-01".into(),
        genre: "Rock".into(),
        comment: "".into(),
        disc_breaks: vec![3],
        catalog_ean: String::new(),
        artwork_path: cover.display().to_string(),
    };

    for format in [ExportFormat::Flac, ExportFormat::Mp3, ExportFormat::Aac] {
        let cfg = ExportConfig {
            format,
            dest_dir: dir.path().join(format!("out-{}", format.extension())).display().to_string(),
            ..Default::default()
        };
        let jobs = still_core::export::plan_export_with_meta(
            &state.tracks(),
            &cfg,
            &wav,
            &state.project.album_meta,
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let report = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
        assert!(report.errors.is_empty(), "{format:?}: {:?}", report.errors);

        // Track 3 = first track of disc 2.
        let tagged = lofty::probe::Probe::open(&jobs[2].out_path)
            .unwrap()
            .read()
            .unwrap();
        let tag = tagged.primary_tag().expect("tag written");
        let get = |k: ItemKey| tag.get_string(k).unwrap_or("").to_string();
        assert_eq!(get(ItemKey::TrackTitle), "Song 3", "{format:?}");
        assert_eq!(get(ItemKey::AlbumTitle), "Barn Sessions (Disc 2)", "{format:?}");
        assert_eq!(get(ItemKey::AlbumArtist), "The Copper Stills", "{format:?}");
        assert_eq!(get(ItemKey::TrackArtist), "The Copper Stills", "{format:?}");
        assert_eq!(get(ItemKey::Genre), "Rock", "{format:?}");
        assert_eq!(tag.track(), Some(1), "{format:?}");
        assert_eq!(tag.track_total(), Some(2), "{format:?}");
        assert_eq!(tag.disk(), Some(2), "{format:?}");
        assert_eq!(tag.disk_total(), Some(2), "{format:?}");
        // Front cover embedded natively (MP4 covr / ID3 APIC / FLAC picture).
        assert_eq!(tag.picture_count(), 1, "{format:?}");
        let pic = &tag.pictures()[0];
        assert!(
            pic.data().starts_with(&[0x89, b'P', b'N', b'G']),
            "{format:?}: cover is not the PNG we embedded"
        );
    }

    // The source is still byte-for-byte untouched by all the tagging.
    let sum = checksum(&wav);
    let (info2, _) = scan_file(&wav, |_| {}).unwrap();
    assert_eq!(info2.duration_samples, 4 * SR as u64);
    assert_eq!(sum, checksum(&wav));
}

/// Mastering chain at export (macOS): a track rendered through a hosted
/// AULowpass must differ audibly from the dry export, keep the exact sample
/// length, and leave the sources untouched. An empty chain stays on the
/// pure-ffmpeg path (covered by every other export test).
#[cfg(target_os = "macos")]
#[test]
fn export_renders_through_mastering_chain() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — chain export test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("source.wav");
    // 440 Hz content: a lowpass with a low cutoff will tame it measurably.
    write_wav(&wav, &[(2.0, 0.6)]);
    let before = checksum(&wav);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(
        Project::new(vec![wav.display().to_string()]),
        info,
        vec![peaks],
    );
    state.add_region(0, SR as u64, None).unwrap();
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);

    // Dry reference.
    let jobs_dry = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    let rep = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs_dry, &cfg, &cancel, |_| {});
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);

    // Through the chain (fresh instance per worker, like the app).
    let chain = vec![still_core::MasterPluginSpec {
        id: 1,
        component: "aufx:lpas:appl".into(),
        bypass: false,
        state: None,
    }];
    let jobs_wet = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    let rep2 = still_core::export::run_export_with_chain(
        &ffmpeg,
        &mix_of(&state),
        2,
        SR,
        &jobs_wet,
        &cfg,
        &chain,
        &[],
        &cancel,
        |_| {},
    );
    assert!(rep2.errors.is_empty(), "{:?}", rep2.errors);

    let read = |p: &Path| -> Vec<i16> {
        hound::WavReader::open(p)
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap())
            .collect()
    };
    let dry = read(&jobs_dry[0].out_path);
    let wet = read(&jobs_wet[0].out_path);
    // Exact same length (latency compensated), audibly different content.
    assert_eq!(dry.len(), wet.len());
    assert_eq!(wet.len() as u64, SR as u64 * 2);
    let diff = dry
        .iter()
        .zip(&wet)
        .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 64)
        .count();
    assert!(
        diff > wet.len() / 10,
        "chain output too close to dry ({diff} differing samples)"
    );
    // RMS should DROP through a lowpass on a 440 Hz tone with default
    // cutoff? Not necessarily — assert difference only (above).

    assert_eq!(checksum(&wav), before);
}

/// Clips with mismatched formats are refused with an actionable error.
#[test]
fn mismatched_clip_formats_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let wav_a = dir.path().join("a.wav");
    let wav_mono = dir.path().join("mono.wav");
    write_wav(&wav_a, &[(1.0, 0.5)]);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav_mono, spec).unwrap();
    for i in 0..SR {
        w.write_sample(((i as f32 * 0.05).sin() * 10_000.0) as i16).unwrap();
    }
    w.finalize().unwrap();

    let err = still_core::scan_files(&[wav_a, wav_mono], &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(err.to_string().contains("channel"), "unexpected error: {err}");
}

#[test]
fn export_report_carries_errors_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("source.wav");
    write_wav(&wav, &[(1.0, 0.5)]);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, vec![peaks]);
    state.add_region(0, SR as u64, None).unwrap();
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        return;
    };
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let jobs = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    // Point ffmpeg at a nonexistent source clip: errors must be reported.
    let bad_layers = vec![LayerMix {
        clips: vec![still_core::ClipInfo {
            path: "/nonexistent/audio.wav".into(),
            name: "audio.wav".into(),
            start_sample: 0,
            duration_samples: SR as u64,
        }],
    }];
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &bad_layers, 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert_eq!(report.files.len(), 0);
    assert!(!report.errors.is_empty());
}

/// Stage E sign-off for VST3: export TWO tracks through a real VST3 plugin
/// (Neutron 5 Equalizer — graceful skip when absent). Parallel workers each
/// instantiate their own instance (serialized by the lifecycle lock), the
/// output keeps the exact sample length, and the sources stay untouched.
#[cfg(target_os = "macos")]
#[test]
fn export_renders_through_vst3_chain() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — VST3 chain export test skipped");
        return;
    };
    // Register ONLY the Neutron bundle via a symlink dir.
    let neutron = match std::fs::read_dir("/Library/Audio/Plug-Ins/VST3")
        .ok()
        .and_then(|d| {
            d.flatten().map(|e| e.path()).find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().contains("Neutron 5 Equalizer"))
                    .unwrap_or(false)
            })
        }) {
        Some(p) => p,
        None => {
            eprintln!("Neutron 5 Equalizer VST3 not installed — test skipped");
            return;
        }
    };
    let plugdir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&neutron, plugdir.path().join(neutron.file_name().unwrap()))
        .unwrap();
    still_core::vst3::scan_dirs(&[plugdir.path().to_path_buf()]);
    let Some(vst3) = still_core::vst3::list_effects()
        .into_iter()
        .find(|p| p.name.contains("Equalizer") || p.name.contains("EQ"))
    else {
        eprintln!("Neutron VST3 class not found after scan — test skipped");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("source.wav");
    write_wav(&wav, &[(2.0, 0.6)]);
    let before = checksum(&wav);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(
        Project::new(vec![wav.display().to_string()]),
        info,
        vec![peaks],
    );
    // Two tracks so at least two workers instantiate concurrently.
    state.add_region(0, SR as u64 - 1, None).unwrap();
    state.add_region(SR as u64, 2 * SR as u64, None).unwrap();
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);

    let chain = vec![still_core::MasterPluginSpec {
        id: 1,
        component: vst3.id.clone(),
        bypass: false,
        state: None,
    }];
    let jobs = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    assert_eq!(jobs.len(), 2);
    let rep = still_core::export::run_export_with_chain(
        &ffmpeg,
        &mix_of(&state),
        2,
        SR,
        &jobs,
        &cfg,
        &chain,
        &[],
        &cancel,
        |_| {},
    );
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);

    for job in &jobs {
        let samples: Vec<i16> = hound::WavReader::open(&job.out_path)
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap())
            .collect();
        let expect = (job.end_sample - job.start_sample) * 2;
        assert_eq!(samples.len() as u64, expect, "{:?}", job.out_path);
        assert!(samples.iter().any(|&s| s != 0), "silent output");
    }
    assert_eq!(checksum(&wav), before);
}

/// Per-layer and per-track chains at export: a lowpass on ONE layer of a
/// two-layer mix changes the output; a track-scoped chain applies only to
/// its own job; sources stay byte-identical. Also exercises the muted-layer
/// compaction (lane chains must follow the ORIGINAL layer indices).
#[cfg(target_os = "macos")]
#[test]
fn export_renders_layer_and_track_chains() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — per-target chain export test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav_a = dir.path().join("a.wav");
    write_wav(&wav_a, &[(3.0, 0.4)]);
    let before = checksum(&wav_a);
    let (info, peaks) = scan_file(&wav_a, |_| {}).unwrap();
    let mut state = ProjectState::new(
        Project::new(vec![wav_a.display().to_string()]),
        info,
        vec![peaks],
    );
    state.add_region(0, SR as u64, None).unwrap();
    state.add_region(2 * SR as u64, 3 * SR as u64, None).unwrap();
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);

    let lowpass = |id: u32| still_core::MasterPluginSpec {
        id,
        component: "aufx:lpas:appl".into(),
        bypass: false,
        state: None,
    };

    // Reference: no chains at all.
    let jobs_dry = plan_export(&state.tracks(), &cfg, &wav_a).unwrap();
    let rep = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs_dry, &cfg, &cancel, |_| {});
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);

    // Layer chain on layer 0 + track chain on job 0 only.
    let mut jobs_wet = plan_export(&state.tracks(), &cfg, &wav_a).unwrap();
    jobs_wet[0].track_chain = vec![lowpass(20)];
    let lane_chains = vec![vec![lowpass(10)]];
    let rep2 = still_core::export::run_export_with_chain(
        &ffmpeg,
        &mix_of(&state),
        2,
        SR,
        &jobs_wet,
        &cfg,
        &[],
        &lane_chains,
        &cancel,
        |_| {},
    );
    assert!(rep2.errors.is_empty(), "{:?}", rep2.errors);

    let read = |p: &Path| -> Vec<i16> {
        hound::WavReader::open(p)
            .unwrap()
            .samples::<i16>()
            .map(|s| s.unwrap())
            .collect()
    };
    let differs = |a: &[i16], b: &[i16]| {
        a.iter()
            .zip(b)
            .filter(|(x, y)| (**x as i32 - **y as i32).abs() > 64)
            .count()
            > a.len() / 10
    };
    for (dry_job, wet_job) in jobs_dry.iter().zip(&jobs_wet) {
        let dry = read(&dry_job.out_path);
        let wet = read(&wet_job.out_path);
        assert_eq!(dry.len(), wet.len());
        // Both tracks go through the LAYER chain → both differ from dry.
        assert!(differs(&dry, &wet), "layer chain had no effect");
    }
    // The TRACK chain applies to job 0 only: render job 1 again with the
    // same lane chain but no track chain — must match the wet job 1 output,
    // and job 0 re-rendered without its track chain must differ from wet 0.
    let mut jobs_check = plan_export(&state.tracks(), &cfg, &wav_a).unwrap();
    let rep3 = still_core::export::run_export_with_chain(
        &ffmpeg,
        &mix_of(&state),
        2,
        SR,
        &jobs_check,
        &cfg,
        &[],
        &lane_chains,
        &cancel,
        |_| {},
    );
    assert!(rep3.errors.is_empty(), "{:?}", rep3.errors);
    let wet0 = read(&jobs_wet[0].out_path);
    let check0 = read(&jobs_check[0].out_path);
    assert!(differs(&wet0, &check0), "track chain had no effect on its job");
    let wet1 = read(&jobs_wet[1].out_path);
    let check1 = read(&jobs_check[1].out_path);
    assert_eq!(wet1, check1, "track chain leaked into another job");
    let _ = &mut jobs_check;

    assert_eq!(checksum(&wav_a), before);
}

/// Tier-1 pro export (#5): dithering a depth reduction and resampling.
/// A −100 dBFS tone is below half an LSB at 16-bit: truncation (dither
/// off) yields digital black, dither keeps energy. And a 48 kHz target
/// resamples the file while preserving its duration.
#[test]
fn export_dithers_and_resamples() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — dither/SRC test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("quiet.wav");
    // −100 dBFS ≈ 0.33 LSB at 16-bit.
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    let amp = 10f32.powf(-100.0 / 20.0);
    for i in 0..SR * 2 {
        let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR as f32).sin() * amp;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, vec![peaks]);
    state.add_region(0, SR as u64, None).unwrap();
    let cancel = AtomicBool::new(false);

    let read16 = |p: &Path| -> Vec<i16> {
        hound::WavReader::open(p).unwrap().samples::<i16>().map(|s| s.unwrap()).collect()
    };
    let run_cfg = |cfg: &ExportConfig| {
        let jobs = plan_export(&state.tracks(), cfg, &wav).unwrap();
        let rep = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, cfg, &cancel, |_| {});
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        jobs
    };

    // Dither OFF: pure truncation → digital black.
    let cfg_off = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 16,
        dither: still_core::project::DitherMode::Off,
        dest_dir: dir.path().join("off").display().to_string(),
        ..Default::default()
    };
    let jobs = run_cfg(&cfg_off);
    let s_off = read16(&jobs[0].out_path);
    assert!(s_off.iter().all(|&s| s == 0), "expected silence after truncation");

    // Dither AUTO: the tone survives as dithered LSB activity.
    let cfg_auto = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 16,
        dest_dir: dir.path().join("auto").display().to_string(),
        ..Default::default()
    };
    let jobs = run_cfg(&cfg_auto);
    let s_auto = read16(&jobs[0].out_path);
    let nonzero = s_auto.iter().filter(|&&s| s != 0).count();
    assert!(
        nonzero > s_auto.len() / 20,
        "dither should keep LSB activity ({nonzero} nonzero)"
    );

    // SRC to 48 kHz: header rate and duration follow.
    let cfg_srvarious = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 24,
        target_sample_rate: Some(48_000),
        dest_dir: dir.path().join("srav").display().to_string(),
        ..Default::default()
    };
    let jobs = run_cfg(&cfg_srvarious);
    let r = hound::WavReader::open(&jobs[0].out_path).unwrap();
    assert_eq!(r.spec().sample_rate, 48_000);
    let dur = r.duration() as f64 / 48_000.0;
    assert!((dur - 1.0).abs() < 0.01, "duration {dur}");
}

/// Tier-2 pro export (#5): the Red Book image + cue sheet. Track INDEX
/// offsets are frame-aligned (588 samples), the image is 44.1 kHz / 16-bit
/// stereo, the total length equals the sum of the frame-padded tracks, and
/// the cue carries CD-Text + CATALOG.
#[test]
fn export_cd_image_and_cue() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — CD image test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("src.wav");
    write_wav(&wav, &[(3.0, 0.4)]);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, vec![peaks]);
    // Two tracks with deliberately NON-frame-aligned lengths.
    state.add_region(0, SR as u64 + 123, None).unwrap();
    state.add_region(SR as u64 + 200, 2 * SR as u64 + 777, None).unwrap();
    let meta = still_core::AlbumMeta {
        album: "Barn Sessions".into(),
        album_artist: "The Copper Stills".into(),
        catalog_ean: "1234567890128".into(),
        ..Default::default()
    };
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 16,
        cd_image: true,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let jobs = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    let rep = still_core::export::run_export_cd_image(
        &ffmpeg,
        &mix_of(&state),
        2,
        SR,
        &jobs,
        &cfg,
        &[],
        &[],
        &meta,
        &cancel,
        |_| {},
    );
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.files.len(), 2);

    let image = &rep.files[0].path;
    let r = hound::WavReader::open(image).unwrap();
    assert_eq!(r.spec().sample_rate, 44_100);
    assert_eq!(r.spec().channels, 2);
    assert_eq!(r.spec().bits_per_sample, 16);
    let total = r.duration() as u64;
    assert_eq!(total % 588, 0, "image not frame-aligned: {total}");
    // Each track padded up to a frame boundary.
    let t1 = (SR as u64 + 123).div_ceil(588) * 588;
    let t2 = (SR as u64 + 577).div_ceil(588) * 588;
    assert_eq!(total, t1 + t2, "unexpected image length");

    let cue = std::fs::read_to_string(&rep.files[1].path).unwrap();
    assert!(cue.contains("CATALOG 1234567890128"), "{cue}");
    assert!(cue.contains("TITLE \"Barn Sessions\""));
    assert!(cue.contains("PERFORMER \"The Copper Stills\""));
    assert!(cue.contains("TRACK 01 AUDIO"));
    assert!(cue.contains("INDEX 01 00:00:00"));
    // Track 2 starts at t1 samples → t1/588 frames.
    let f = t1 / 588;
    let expect = format!("INDEX 01 {:02}:{:02}:{:02}", f / 75 / 60, (f / 75) % 60, f % 75);
    assert!(cue.contains(&expect), "cue missing {expect}:\n{cue}");

    // Per-track measures: both segments carry the same 440 Hz sine at 0.4
    // amplitude → true peak ≈ 20·log10(0.4) ≈ −7.96 dBTP, matching LUFS.
    let tm = &rep.files[0].track_measures;
    assert_eq!(tm.len(), 2, "{tm:?}");
    for m in tm {
        let tp = m.true_peak_db.expect("per-track true peak missing");
        assert!((tp + 7.96).abs() < 1.0, "true peak {tp} dBTP");
        assert!(m.lufs_i.is_some(), "per-track LUFS missing: {m:?}");
    }
    let (l1, l2) = (tm[0].lufs_i.unwrap(), tm[1].lufs_i.unwrap());
    assert!((l1 - l2).abs() < 0.5, "same material, different LUFS: {l1} vs {l2}");
}

/// Multitrack stems export (#7): one folder per track, one file per layer
/// named with the {ln}/{layer} macros; raw cuts are sample-exact copies of
/// the source slice; mix mode skips muted layers; the Source format
/// mirrors each layer's own container.
#[test]
fn export_stems_multitrack() {
    let dir = tempfile::tempdir().unwrap();
    let stereo = dir.path().join("mic-stereo.wav");
    let mono = dir.path().join("input-3.wav");
    write_wav(&stereo, &[(2.0, 0.4)]);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    // The mono layer is only ONE second long: track 2 lies entirely past
    // its end, exercising the silent-stem path.
    let mut w = hound::WavWriter::create(&mono, spec).unwrap();
    for i in 0..(SR as usize) {
        let v = (0.3 * (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / SR as f32).sin()
            * i16::MAX as f32) as i16;
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();

    let (info, pyramids) = still_core::scan_layers(
        &[vec![(stereo.clone(), None)], vec![(mono.clone(), None)]],
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let mut state = ProjectState::new(
        Project::new_layers(vec![
            vec![stereo.display().to_string()],
            vec![mono.display().to_string()],
        ]),
        info,
        pyramids,
    );
    state.add_region(0, SR as u64, None).unwrap();
    state.add_region(SR as u64, 2 * SR as u64, None).unwrap();

    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — stems export test skipped");
        return;
    };
    use still_core::export::{plan_export_stems, resolve_source_format, StemLayer};
    let stem_layers = vec![
        StemLayer {
            name: "Room mic".into(),
            source_path: stereo.display().to_string(),
        },
        StemLayer {
            name: "input-3".into(),
            source_path: mono.display().to_string(),
        },
    ];
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 16,
        template: "{n} - {title}/{ln} - {layer}".into(),
        stems: true,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let meta = still_core::AlbumMeta::default();

    // Raw cuts: one job per (track × layer), folder per track.
    let jobs =
        plan_export_stems(&state.tracks(), &cfg, &stereo, &meta, &stem_layers, false).unwrap();
    assert_eq!(jobs.len(), 4, "2 tracks × 2 layers");
    let rel = |j: usize| {
        jobs[j]
            .out_path
            .strip_prefix(dir.path().join("out"))
            .unwrap()
            .display()
            .to_string()
    };
    assert_eq!(rel(0), "01 - Track 01/01 - Room mic.wav");
    assert_eq!(rel(1), "01 - Track 01/02 - input-3.wav");
    assert_eq!(jobs[0].layer_volumes, vec![1.0, 0.0]);
    assert_eq!(jobs[1].layer_volumes, vec![0.0, 1.0]);

    let mix = mix_of(&state);
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &mix, 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // The stereo raw stem is a sample-exact slice of its source.
    let out: Vec<i16> = hound::WavReader::open(&jobs[0].out_path)
        .unwrap()
        .samples::<i16>()
        .map(|s| s.unwrap())
        .collect();
    let src: Vec<i16> = hound::WavReader::open(&stereo)
        .unwrap()
        .samples::<i16>()
        .take(SR as usize * 2)
        .map(|s| s.unwrap())
        .collect();
    assert_eq!(out.len(), src.len());
    let max_err = out
        .iter()
        .zip(&src)
        .map(|(a, b)| (a - b).abs())
        .max()
        .unwrap_or(0);
    assert!(max_err <= 1, "raw stem differs from source (max err {max_err} LSB)");

    // Track 2 is past the mono layer's end: its mono stem must still exist,
    // FULL length and silent, so the stem set stays time-aligned in a DAW.
    let silent_job = &jobs[3];
    assert!(silent_job.out_path.display().to_string().contains("input-3"));
    let mut r = hound::WavReader::open(&silent_job.out_path).unwrap();
    assert_eq!(r.duration() as u64, SR as u64, "silent stem must span the window");
    let peak = r
        .samples::<i16>()
        .map(|v| v.unwrap().abs())
        .max()
        .unwrap_or(0);
    assert_eq!(peak, 0, "expected pure silence");

    // Mix mode skips a muted layer entirely.
    let mono_id = state.project.layers[1].id;
    state.set_layer_muted(mono_id, true).unwrap();
    let jobs_mix =
        plan_export_stems(&state.tracks(), &cfg, &stereo, &meta, &stem_layers, true).unwrap();
    assert_eq!(jobs_mix.len(), 2, "muted layer must not produce stems");
    assert!(jobs_mix.iter().all(|j| j.out_path.display().to_string().contains("Room mic")));

    // Source format: each stem mirrors its own container.
    assert_eq!(resolve_source_format(&stereo), (ExportFormat::Wav, 16, Some(SR)));
    let cfg_src = ExportConfig {
        format: ExportFormat::Source,
        ..cfg.clone()
    };
    let jobs_src =
        plan_export_stems(&state.tracks(), &cfg_src, &stereo, &meta, &stem_layers, false)
            .unwrap();
    assert_eq!(jobs_src[0].source_fmt, Some((ExportFormat::Wav, 16, Some(SR))));
    assert!(jobs_src[0].out_path.extension().unwrap() == "wav");
}

/// Tier-3 pro export (#5): the DDP 2.00 fileset. Structure parses, the
/// PQ stream agrees with the map to the frame, per-track ISRCs and the
/// EAN land in the subcode, and the image's program area null-compares
/// against the WAV produced by the cue path from the same session.
#[test]
fn export_ddp_fileset() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — DDP test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("src.wav");
    write_wav(&wav, &[(3.0, 0.4)]);
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state =
        ProjectState::new(Project::new(vec![wav.display().to_string()]), info, vec![peaks]);
    state.add_region(0, SR as u64 + 123, None).unwrap();
    state.add_region(SR as u64 + 200, 2 * SR as u64 + 777, None).unwrap();
    let t1 = state.tracks()[0].id;
    state.set_track_isrc(t1, "fr-ab1-26-00001").unwrap();
    assert_eq!(state.tracks()[0].isrc, "FRAB12600001");
    assert!(state.set_track_isrc(t1, "not an isrc").is_err());

    let meta = still_core::AlbumMeta {
        album: "Barn Sessions".into(),
        album_artist: "The Copper Stills".into(),
        catalog_ean: "1234567890128".into(),
        ..Default::default()
    };
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 16,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let jobs = still_core::export::plan_export_with_meta(&state.tracks(), &cfg, &wav, &meta).unwrap();
    assert_eq!(jobs[0].isrc, "FRAB12600001");

    let rep = still_core::export::run_export_ddp(
        &ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &[], &[], &meta, &cancel, |_| {},
    );
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    let ddp_dir = std::path::PathBuf::from(&rep.files[0].path);
    assert!(ddp_dir.is_dir());

    // Expected geometry: same frame-padded lengths as the cue path.
    let t1_len = (SR as u64 + 123).div_ceil(588);
    let t2_len = (SR as u64 + 577).div_ceil(588);
    let program = t1_len + t2_len;

    // DDPID: level + EAN + CD.
    let id = std::fs::read(ddp_dir.join("DDPID")).unwrap();
    assert_eq!(id.len(), 128);
    assert_eq!(&id[..8], b"DDP 2.00");
    assert_eq!(&id[8..21], b"1234567890128");
    assert_eq!(&id[87..89], b"CD");

    // DDPMS: D0 length covers pause + program; S0 points at PQDESCR.
    let ms = std::fs::read(ddp_dir.join("DDPMS")).unwrap();
    assert_eq!(ms.len(), 256);
    assert_eq!(&ms[..4], b"VVVM");
    let dsl: u64 = String::from_utf8_lossy(&ms[14..22]).trim().parse().unwrap();
    assert_eq!(dsl, 150 + program, "map disagrees with the image length");
    assert_eq!(&ms[38..40], b"DA");
    assert_eq!(String::from_utf8_lossy(&ms[128 + 30..128 + 38]), "PQ DESCR");

    // PQ stream: lead-in EAN, track 1 ISRC, positions to the frame.
    let pq = std::fs::read(ddp_dir.join("PQDESCR")).unwrap();
    assert_eq!(pq.len() % 64, 0);
    let pk: Vec<&[u8]> = pq.chunks(64).collect();
    // lead-in, 01/00, 01/01, 02/01, AA/01.
    assert_eq!(pk.len(), 5);
    assert_eq!(String::from_utf8_lossy(&pk[0][32..45]), "1234567890128");
    assert_eq!(String::from_utf8_lossy(&pk[1][20..32]), "FRAB12600001");
    assert_eq!(String::from_utf8_lossy(&pk[2][10..16]), "000200");
    let msf = |sector: u64| {
        format!(
            "{:02}{:02}{:02}",
            sector / 75 / 60,
            (sector / 75) % 60,
            sector % 75
        )
    };
    assert_eq!(String::from_utf8_lossy(&pk[3][10..16]), msf(150 + t1_len));
    assert_eq!(&pk[4][4..6], b"AA");
    assert_eq!(String::from_utf8_lossy(&pk[4][10..16]), msf(150 + program));

    // Image: pause is digital black, program null-compares against the
    // cue path's WAV rendered from the very same session.
    let image = std::fs::read(ddp_dir.join("IMAGE.DAT")).unwrap();
    assert_eq!(image.len() as u64, (150 + program) * 2352);
    assert!(image[..150 * 2352].iter().all(|b| *b == 0), "pause not silent");

    let cfg_cue = ExportConfig { cd_image: true, ..cfg.clone() };
    let rep2 = still_core::export::run_export_cd_image(
        &ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg_cue, &[], &[], &meta, &cancel, |_| {},
    );
    assert!(rep2.errors.is_empty(), "{:?}", rep2.errors);
    let mut r = hound::WavReader::open(&rep2.files[0].path).unwrap();
    let wav_samples: Vec<i16> = r.samples::<i16>().map(|v| v.unwrap()).collect();
    let img_samples: Vec<i16> = image[150 * 2352..]
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    assert_eq!(img_samples.len(), wav_samples.len());
    assert_eq!(img_samples, wav_samples, "DDP image and WAV image differ");

    // Cue sheet carries the ISRC too.
    let cue = std::fs::read_to_string(&rep2.files[1].path).unwrap();
    assert!(cue.contains("ISRC FRAB12600001"), "{cue}");

    // Checksums verify.
    let chk = std::fs::read_to_string(ddp_dir.join("CHECKSUM.MD5")).unwrap();
    assert!(chk.lines().count() >= 5);
    // PQ sheet exists and quotes the same lead-out time.
    let sheet = std::fs::read_to_string(ddp_dir.join("PQ_SHEET.TXT")).unwrap();
    let lo = 150 + program;
    let expect = format!("{:02}:{:02}:{:02} (lead-out)", lo / 75 / 60, (lo / 75) % 60, lo % 75);
    assert!(sheet.contains(&expect), "{sheet}");
    assert!(sheet.contains("FRAB12600001"), "{sheet}");
}

/// Metering (#2): the export report measures the DELIVERED files. A
/// −20 dBFS 997 Hz correlated stereo sine has a known integrated
/// loudness (≈ −20.7 LUFS: −23.01 dB RMS/ch, +3.01 dB stereo sum,
/// −0.691 dB BS.1770 offset, K-weighting ≈ 0 dB at 997 Hz) and a true
/// peak at ≈ −20 dBTP.
#[test]
fn export_report_measures_loudness() {
    let Ok(ffmpeg) = resolve_ffmpeg(&[]) else {
        eprintln!("ffmpeg not found — loudness report test skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("cal.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    let amp = 10f32.powf(-20.0 / 20.0);
    for i in 0..SR * 4 {
        let s = (2.0 * std::f32::consts::PI * 997.0 * i as f32 / SR as f32).sin() * amp;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
    let (info, peaks) = scan_file(&wav, |_| {}).unwrap();
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, vec![peaks]);
    state.add_region(0, 4 * SR as u64, None).unwrap();
    let cfg = ExportConfig {
        format: ExportFormat::Wav,
        bit_depth: 24,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let jobs = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    let rep = run_export(&ffmpeg, &mix_of(&state), 2, SR, &jobs, &cfg, &cancel, |_| {});
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    let f = &rep.files[0];
    let i = f.lufs_i.expect("report should carry LUFS-I");
    assert!((i - (-20.7)).abs() < 1.0, "LUFS-I {i}");
    let tp = f.true_peak_db.expect("report should carry true peak");
    assert!((tp - (-20.0)).abs() < 1.0, "TP {tp}");
}
