//! Integration tests over the full pipeline with a programmatically generated
//! WAV file, including the decisive non-destructive test from SPEC §3 bis:
//! after a full load → mark → export scenario, the source file must be
//! byte-for-byte identical (checksum) to its initial state.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use still_core::project::{ExportConfig, ExportFormat, Project};
use still_core::{plan_export, resolve_ffmpeg, run_export, scan_file, ProjectState, SilenceParams};

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
        peaks,
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
    let report = run_export(&ffmpeg, &state.info.clips, SR, &jobs, &cfg, &cancel, |_| {});
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
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, peaks);
    // Region starting at 2 s — far from the file start.
    state.add_region(2 * SR as u64, 3 * SR as u64, None).unwrap();
    let cfg = ExportConfig {
        format: ExportFormat::Aac,
        dest_dir: dir.path().join("out").display().to_string(),
        ..Default::default()
    };
    let jobs = plan_export(&state.tracks(), &cfg, &wav).unwrap();
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &state.info.clips, SR, &jobs, &cfg, &cancel, |_| {});
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
        peaks,
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
    let report = run_export(&ffmpeg, &state.info.clips, SR, &jobs, &cfg, &cancel, |_| {});
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
    let mut state = ProjectState::new(Project::new(vec![wav.display().to_string()]), info, peaks);
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
    let bad_clips = vec![still_core::ClipInfo {
        path: "/nonexistent/audio.wav".into(),
        name: "audio.wav".into(),
        start_sample: 0,
        duration_samples: SR as u64,
    }];
    let cancel = AtomicBool::new(false);
    let report = run_export(&ffmpeg, &bad_clips, SR, &jobs, &cfg, &cancel, |_| {});
    assert_eq!(report.files.len(), 0);
    assert!(!report.errors.is_empty());
}
