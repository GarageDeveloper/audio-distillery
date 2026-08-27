use super::*;

fn disc() -> Disc {
    Disc {
        title: "Barn Sessions".into(),
        performer: "The Copper Stills".into(),
        ean: Some("1234567890128".into()),
        tracks: vec![
            Track {
                number: 1,
                title: "Opener".into(),
                start_sector: 0,
                length_sectors: 14816,
                isrc: Some("FRAB12600001".into()),
            },
            Track {
                number: 2,
                title: "Closer".into(),
                start_sector: 14816,
                length_sectors: 10883,
                isrc: None,
            },
        ],
    }
}

fn field(bytes: &[u8], off: usize, width: usize) -> String {
    String::from_utf8_lossy(&bytes[off..off + width]).to_string()
}

#[test]
fn msf_math() {
    assert_eq!(msf(0), (0, 0, 0));
    assert_eq!(msf(150), (0, 2, 0));
    assert_eq!(msf(14816 + 150), (3, 19, 41));
}

#[test]
fn isrc_validation() {
    assert_eq!(
        normalize_isrc("fr-ab1-26-00001").unwrap().as_deref(),
        Some("FRAB12600001")
    );
    assert_eq!(normalize_isrc("  ").unwrap(), None);
    assert!(normalize_isrc("123456789012").is_err()); // starts with digits
    assert!(normalize_isrc("FRAB126001").is_err()); // too short
    assert!(normalize_isrc("FRAB1260000A").is_err()); // letter in designation
}

/// DDPID: 128 bytes, level at 0, EAN at 8, MSL at 29, "CD" at 87.
#[test]
fn ddpid_layout() {
    let p = ddpid_packet(&disc(), 256);
    assert_eq!(p.len(), 128);
    assert_eq!(field(&p, 0, 8), "DDP 2.00");
    assert_eq!(field(&p, 8, 13), "1234567890128");
    assert_eq!(field(&p, 29, 8).trim(), "256");
    assert_eq!(field(&p, 87, 2), "CD");
    assert!(p.iter().all(|b| b.is_ascii() && !b.is_ascii_control()));
}

/// DDPMS: two 128-byte packets — D0 audio (sectors, CDM=DA, pause
/// included as PG2) then S0 subcode (bytes, SUB="PQ DESCR").
#[test]
fn ddpms_layout() {
    let d = disc();
    let ms = ddpms_packets(&d, IMAGE_NAME, 6 * 64);
    assert_eq!(ms.len(), 256);
    let d0 = &ms[..128];
    assert_eq!(field(d0, 0, 4), "VVVM");
    assert_eq!(field(d0, 4, 2), "D0");
    assert_eq!(field(d0, 6, 8), "        ", "DSP blank");
    assert_eq!(field(d0, 14, 8), format!("{:08}", 150 + 14816 + 10883));
    assert_eq!(field(d0, 22, 8), "        ", "DSS blank: pause in image");
    assert_eq!(field(d0, 38, 2), "DA");
    assert_eq!(field(d0, 40, 1), "0");
    assert_eq!(field(d0, 41, 1), "0");
    assert_eq!(field(d0, 46, 4), " 150");
    assert_eq!(field(d0, 71, 3), "017");
    assert_eq!(field(d0, 74, 17).trim(), "IMAGE.DAT");

    let s0 = &ms[128..];
    assert_eq!(field(s0, 0, 4), "VVVM");
    assert_eq!(field(s0, 4, 2), "S0");
    assert_eq!(field(s0, 14, 8), format!("{:08}", 6 * 64));
    assert_eq!(field(s0, 30, 8), "PQ DESCR");
    assert_eq!(field(s0, 74, 17).trim(), "PQDESCR");
}

/// PQ stream: lead-in with EAN, track 1 index 00 at 00:00:00 and index
/// 01 at 00:02:00, later tracks at absolute time, lead-out at AA.
#[test]
fn pq_stream_layout() {
    let pq = pq_stream(&disc());
    assert_eq!(pq.len() % 64, 0);
    let packets: Vec<&[u8]> = pq.chunks(64).collect();
    assert_eq!(packets.len(), 5);

    for p in &packets {
        assert_eq!(field(p, 0, 4), "VVVS");
        assert_eq!(field(p, 8, 2), "  ", "hours stay blank");
        assert_eq!(field(p, 16, 2), "01", "control/ADR: audio, position");
    }
    // Lead-in carries the UPC/EAN.
    assert_eq!(field(packets[0], 4, 2), "00");
    assert_eq!(field(packets[0], 32, 13), "1234567890128");
    // Track 1: index 00 at absolute zero, index 01 after the pause.
    assert_eq!(field(packets[1], 4, 4), "0100");
    assert_eq!(field(packets[1], 10, 6), "000000");
    assert_eq!(field(packets[1], 20, 12), "FRAB12600001");
    assert_eq!(field(packets[2], 4, 4), "0101");
    assert_eq!(field(packets[2], 10, 6), "000200");
    // Track 2 index 01 at 150+14816 sectors = 03:19:41.
    assert_eq!(field(packets[3], 4, 4), "0201");
    assert_eq!(field(packets[3], 10, 6), "031941");
    // Lead-out at 150+25699 = 05:44:49.
    assert_eq!(field(packets[4], 4, 2), "AA");
    assert_eq!(field(packets[4], 10, 6), "054449");
}

/// PQ sheet and PQ stream agree to the frame.
#[test]
fn pq_sheet_agrees_with_stream() {
    let d = disc();
    let sheet = pq_sheet(&d);
    assert!(sheet.contains(" 01  01  00:02:00"), "{sheet}");
    assert!(sheet.contains(" 02  01  03:19:41"), "{sheet}");
    assert!(sheet.contains(" AA  01  05:44:49"), "{sheet}");
    assert!(sheet.contains("FRAB12600001"), "{sheet}");
    assert!(sheet.contains("1234567890128"), "{sheet}");
}

/// Full fileset: pause written, sector accounting enforced, checksums
/// verifiable.
#[test]
fn writes_a_complete_fileset() {
    let dir = std::env::temp_dir().join(format!("ddp-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let mut d = disc();
    // Tiny program: 2 tracks of 5 and 3 sectors.
    d.tracks[0].length_sectors = 5;
    d.tracks[1].start_sector = 5;
    d.tracks[1].length_sectors = 3;

    let mut fs_w = Fileset::create(&dir).unwrap();
    let audio = vec![0x55u8; (8 * SECTOR_BYTES) as usize];
    fs_w.write_audio(&audio).unwrap();
    let files = fs_w.finish(&d).unwrap();
    assert_eq!(files.len(), 6);

    let image = fs::read(dir.join(IMAGE_NAME)).unwrap();
    assert_eq!(image.len() as u64, (150 + 8) * SECTOR_BYTES);
    assert!(image[..(150 * SECTOR_BYTES) as usize].iter().all(|b| *b == 0));
    assert_eq!(image[(150 * SECTOR_BYTES) as usize], 0x55);

    // Checksums match the files on disk.
    let chk = fs::read_to_string(dir.join("CHECKSUM.MD5")).unwrap();
    for line in chk.lines() {
        let (digest, name) = line.split_once("  ").unwrap();
        let data = fs::read(dir.join(name)).unwrap();
        assert_eq!(digest, format!("{:x}", Md5::digest(&data)), "{name}");
    }

    // A short image must be refused, not silently mis-described.
    let mut d2 = disc();
    d2.tracks.truncate(1);
    d2.tracks[0].length_sectors = 5;
    let mut w = Fileset::create(&dir.join("bad")).unwrap();
    w.write_audio(&vec![0u8; (3 * SECTOR_BYTES) as usize]).unwrap();
    assert!(w.finish(&d2).is_err());

    let _ = fs::remove_dir_all(&dir);
}
