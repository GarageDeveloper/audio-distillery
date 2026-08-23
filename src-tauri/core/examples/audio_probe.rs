//! Console probe for the playback engine: generates a 44.1 kHz WAV, plays
//! it through PlayerHandle for two seconds and reports what happened.
//! Used to verify device-rate negotiation on foreign audio stacks (WASAPI).

use std::sync::atomic::AtomicBool;

fn main() {
    // 1. Default device, as cpal sees it.
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.default_output_device() {
        Some(d) => {
            println!("device: {}", d.name().unwrap_or_else(|_| "?".into()));
            match d.default_output_config() {
                Ok(c) => println!(
                    "device default config: {} Hz, {} ch, {:?}",
                    c.sample_rate().0,
                    c.channels(),
                    c.sample_format()
                ),
                Err(e) => println!("default_output_config error: {e}"),
            }
        }
        None => println!("NO OUTPUT DEVICE"),
    }

    // 2. A 3 s 44.1 kHz stereo test tone.
    let dir = std::env::temp_dir();
    let wav = dir.join("still_probe_44k1.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..(44_100 * 3) {
        let s = ((2.0 * std::f64::consts::PI * 440.0 * i as f64 / 44_100.0).sin() * 8000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();

    // 3. Play it through the engine (session rate 44.1 kHz).
    let player = still_core::PlayerHandle::spawn();
    player
        .load_session(
            vec![still_core::LayerPlay {
                playlist: vec![(Some(wav.clone()), 3.0)],
            }],
            3.0,
            still_core::VolumeAutomation {
                default: vec![1.0],
                spans: vec![],
            },
            44_100,
            2,
        )
        .unwrap();
    let _ = AtomicBool::new(false);
    player.play().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let st = player.state();
    println!(
        "after 2s: playing={} position={:.2}s ready={} device_error={:?}",
        st.playing, st.position_seconds, st.ready, st.device_error
    );
    let ok = st.device_error.is_none() && st.position_seconds > 1.5 && st.position_seconds < 2.5;
    println!("PROBE {}", if ok { "PASS" } else { "FAIL" });
    let _ = std::fs::remove_file(&wav);
    std::process::exit(if ok { 0 } else { 1 });
}
