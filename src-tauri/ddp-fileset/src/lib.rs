//! Writer for **DDP 2.00 filesets** — the deliverable pressing plants
//! ingest for audio CD replication (Red Book).
//!
//! A fileset is a folder of flat files:
//!
//! - `IMAGE.DAT` — the raw audio image: 2352-byte sectors of 44.1 kHz
//!   16-bit **little-endian** stereo PCM. By this crate's convention the
//!   image *includes* the initial 150-sector (2 s) pause, so absolute
//!   disc time equals byte position (`DSS` is left blank).
//! - `DDPID` — one 128-byte packet identifying the DDP level ("DDP 2.00").
//! - `DDPMS` — the map stream: 128-byte packets describing each data
//!   stream (one `D0` audio packet, one `S0` subcode packet here).
//! - `PQDESCR` — the PQ subcode stream: 64-byte packets giving track and
//!   index positions in absolute disc time, ISRCs and the UPC/EAN.
//! - `CHECKSUM.MD5` — `md5sum -c`-compatible digests of every file.
//!
//! The audio image itself is streamed by the caller through
//! [`Fileset::write_audio`]; this crate owns the metadata files, the
//! human-readable PQ sheet and the checksums.
//!
//! Field layouts follow the packet structure as documented by multiple
//! independent readers of the format (packet sizes, offsets and value
//! conventions cross-checked against real-world filesets); all fields are
//! space-padded ASCII.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

/// Sectors ("frames") per second on an audio CD.
pub const SECTORS_PER_SECOND: u64 = 75;
/// Bytes per audio sector: 588 stereo samples × 2 ch × 2 bytes.
pub const SECTOR_BYTES: u64 = 2352;
/// The mandatory pause before track 1 (2 seconds), included in the image.
pub const PAUSE_SECTORS: u64 = 150;

/// One track of the program area. Positions are **program-relative**
/// sectors: track 1 starts at 0; the initial pause is handled by the
/// writer.
#[derive(Debug, Clone)]
pub struct Track {
    /// 1-based track number (Red Book: 1..=99).
    pub number: u32,
    /// Display title — used in the PQ sheet only (subcode carries no
    /// titles; CD-Text is a separate, optional stream).
    pub title: String,
    /// First sector of this track's audio within the program area.
    pub start_sector: u64,
    /// Track length in sectors.
    pub length_sectors: u64,
    /// 12-character ISRC (e.g. `FRXXX2600001`), already normalized.
    pub isrc: Option<String>,
}

/// The disc description handed to the writer.
#[derive(Debug, Clone, Default)]
pub struct Disc {
    pub title: String,
    pub performer: String,
    /// 12/13-digit UPC/EAN, written to the DDPID and the PQ lead-in.
    pub ean: Option<String>,
    pub tracks: Vec<Track>,
}

impl Disc {
    /// Program-area length: end of the last track, in sectors.
    pub fn program_sectors(&self) -> u64 {
        self.tracks
            .iter()
            .map(|t| t.start_sector + t.length_sectors)
            .max()
            .unwrap_or(0)
    }
}

/// `mm:ss:ff` from an absolute sector count.
pub fn msf(sector: u64) -> (u64, u64, u64) {
    let s = sector / SECTORS_PER_SECOND;
    (s / 60, s % 60, sector % SECTORS_PER_SECOND)
}

/// Validate + normalize an ISRC: strips separators and uppercases;
/// `Ok(None)` for empty input. The strict shape is 2 letters (country),
/// 3 alphanumerics (registrant), 2 digits (year), 5 digits (designation).
pub fn normalize_isrc(raw: &str) -> Result<Option<String>, String> {
    let s: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if s.is_empty() {
        return Ok(None);
    }
    let b = s.as_bytes();
    let ok = s.len() == 12
        && b[..2].iter().all(u8::is_ascii_alphabetic)
        && b[2..5].iter().all(u8::is_ascii_alphanumeric)
        && b[5..].iter().all(u8::is_ascii_digit);
    if ok {
        Ok(Some(s))
    } else {
        Err(format!(
            "\"{raw}\" is not a valid ISRC (expected CC-XXX-YY-NNNNN: \
             2 letters, 3 alphanumerics, 7 digits)"
        ))
    }
}

// ---------------------------------------------------------------------------
// Packet building. All packets are space-padded ASCII records.

struct Packet(Vec<u8>);

impl Packet {
    fn new(len: usize) -> Self {
        Packet(vec![b' '; len])
    }
    /// Left-aligned string at `off` (truncated to `width`).
    fn put(&mut self, off: usize, width: usize, s: &str) {
        for (i, b) in s.bytes().take(width).enumerate() {
            self.0[off + i] = b;
        }
    }
    /// Zero-padded number, exactly `width` digits.
    fn put_num0(&mut self, off: usize, width: usize, n: u64) {
        self.put(off, width, &format!("{n:0width$}"));
    }
    /// Right-aligned space-padded number.
    fn put_num(&mut self, off: usize, width: usize, n: u64) {
        self.put(off, width, &format!("{n:>width$}"));
    }
}

/// DDPID: one 128-byte packet.
fn ddpid_packet(disc: &Disc, map_bytes: u64) -> Vec<u8> {
    let mut p = Packet::new(128);
    p.put(0, 8, "DDP 2.00");
    if let Some(ean) = &disc.ean {
        p.put(8, 13, ean); // UPC/EAN
    }
    // 21..29 MSS (map stream start): blank for a fileset on disk.
    p.put_num(29, 8, map_bytes); // MSL: map stream length in bytes
    // 37 media number: blank (single media).
    // 38..86 Master ID — the album title is a helpful identifier.
    p.put(38, 48, &ascii_only(&disc.title));
    // 86 book specifier: blank (Red Book).
    p.put(87, 2, "CD"); // type of disc
    // 89..93 sides/layers: reserved, blank. 93..95 user text length: blank.
    p.0
}

/// The two DDPMS map packets: `D0` (audio image) + `S0` (PQ stream).
fn ddpms_packets(disc: &Disc, image_name: &str, pq_bytes: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);

    // D0 — the audio data stream, pause included.
    let mut p = Packet::new(128);
    p.put(0, 4, "VVVM");
    p.put(4, 2, "D0");
    // 6..14 DSP blank; DSL in sectors; 22..30 DSS blank (stream starts at
    // disc sector 0 — the pause is IN the image).
    p.put_num0(14, 8, PAUSE_SECTORS + disc.program_sectors());
    p.put(38, 2, "DA"); // CDM: CD-DA
    p.put(40, 1, "0"); // SSM: user data only (2352-byte audio sectors)
    p.put(41, 1, "0"); // SCR: not scrambled
    p.put_num(46, 4, PAUSE_SECTORS); // PG2: pause included in the stream
    p.put_num0(71, 3, 17); // DSI size
    p.put(74, 17, image_name);
    out.extend_from_slice(&p.0);

    // S0 — the PQ subcode descriptor stream.
    let mut p = Packet::new(128);
    p.put(0, 4, "VVVM");
    p.put(4, 2, "S0");
    p.put_num0(14, 8, pq_bytes); // DSL in bytes for subcode streams
    p.put(30, 8, "PQ DESCR"); // SUB
    p.put_num0(71, 3, 17);
    p.put(74, 17, "PQDESCR");
    out.extend_from_slice(&p.0);

    out
}

/// One 64-byte PQ packet. `time`: absolute disc MSF (pause included);
/// None leaves the time fields blank (lead-in packet convention: zeros).
fn pq_packet(
    trk: &str,
    idx: u32,
    time: (u64, u64, u64),
    isrc: Option<&str>,
    upc: Option<&str>,
) -> Vec<u8> {
    let mut p = Packet::new(64);
    p.put(0, 4, "VVVS");
    p.put(4, 2, trk);
    p.put_num0(6, 2, idx as u64);
    // 8..10 hours: blank by convention.
    p.put_num0(10, 2, time.0);
    p.put_num0(12, 2, time.1);
    p.put_num0(14, 2, time.2);
    // Control/ADR: 2-channel audio, no pre-emphasis, ADR 1 (position).
    p.put(16, 2, "01");
    if let Some(i) = isrc {
        p.put(20, 12, i);
    }
    if let Some(u) = upc {
        p.put(32, 13, u);
    }
    p.0
}

/// The full PQ stream for a gapless program (tracks are contiguous; only
/// track 1 has an index 00, covering the initial pause).
fn pq_stream(disc: &Disc) -> Vec<u8> {
    let mut out = Vec::new();
    // Lead-in: carries the UPC/EAN.
    out.extend_from_slice(&pq_packet("00", 0, (0, 0, 0), None, disc.ean.as_deref()));
    for (k, t) in disc.tracks.iter().enumerate() {
        let trk = format!("{:02}", t.number);
        if k == 0 {
            // Track 1 index 00: start of the pause, absolute 00:00:00.
            out.extend_from_slice(&pq_packet(&trk, 0, (0, 0, 0), t.isrc.as_deref(), None));
            out.extend_from_slice(&pq_packet(
                &trk,
                1,
                msf(PAUSE_SECTORS + t.start_sector),
                None,
                None,
            ));
        } else {
            out.extend_from_slice(&pq_packet(
                &trk,
                1,
                msf(PAUSE_SECTORS + t.start_sector),
                t.isrc.as_deref(),
                None,
            ));
        }
    }
    // Lead-out.
    out.extend_from_slice(&pq_packet(
        "AA",
        1,
        msf(PAUSE_SECTORS + disc.program_sectors()),
        None,
        None,
    ));
    out
}

fn ascii_only(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '_' })
        .collect()
}

/// The human-readable PQ sheet: the master's paper trail, agreeing with
/// the PQ stream to the frame.
pub fn pq_sheet(disc: &Disc) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "PQ SHEET — {}", disc.title);
    if !disc.performer.is_empty() {
        let _ = writeln!(s, "Performer: {}", disc.performer);
    }
    if let Some(ean) = &disc.ean {
        let _ = writeln!(s, "UPC/EAN (catalog): {ean}");
    }
    let n = disc.tracks.len();
    let (tm, ts, tf) = msf(disc.program_sectors());
    let _ = writeln!(s, "Tracks: {n} — total program: {tm:02}:{ts:02}:{tf:02}");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "TRK IDX  START (mm:ss:ff)  LENGTH (mm:ss:ff)  ISRC          TITLE"
    );
    let _ = writeln!(
        s,
        "--- ---  ----------------  -----------------  ------------  -----"
    );
    let _ = writeln!(
        s,
        " 01  00  00:00:00 (pause)  00:02:00           {:<12}  ",
        ""
    );
    for t in &disc.tracks {
        let (m, sec, f) = msf(PAUSE_SECTORS + t.start_sector);
        let (lm, ls, lf) = msf(t.length_sectors);
        let _ = writeln!(
            s,
            " {:02}  01  {m:02}:{sec:02}:{f:02}          {lm:02}:{ls:02}:{lf:02}           {:<12}  {}",
            t.number,
            t.isrc.as_deref().unwrap_or(""),
            t.title,
        );
    }
    let (om, os, of) = msf(PAUSE_SECTORS + disc.program_sectors());
    let _ = writeln!(s, " AA  01  {om:02}:{os:02}:{of:02} (lead-out)");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "Times are absolute disc positions (the 2-second pause before \
         track 1 is included, as on the pressed disc)."
    );
    s
}

/// Streams the audio image while hashing it, then writes the whole
/// fileset. Create with [`Fileset::create`], feed PCM through
/// [`Fileset::write_audio`], then [`Fileset::finish`].
pub struct Fileset {
    dir: PathBuf,
    image: fs::File,
    hasher: Md5,
    audio_bytes: u64,
}

pub const IMAGE_NAME: &str = "IMAGE.DAT";

impl Fileset {
    /// Creates the fileset folder and opens the image, writing the
    /// initial 150-sector digital-black pause. The disc description is
    /// only needed at [`Fileset::finish`], once track lengths are known.
    pub fn create(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut image = fs::File::create(dir.join(IMAGE_NAME))?;
        let mut hasher = Md5::new();
        let pause = vec![0u8; (PAUSE_SECTORS * SECTOR_BYTES) as usize];
        image.write_all(&pause)?;
        hasher.update(&pause);
        Ok(Self {
            dir: dir.to_path_buf(),
            image,
            hasher,
            audio_bytes: 0,
        })
    }

    /// Append raw program audio: 44.1 kHz 16-bit little-endian stereo.
    pub fn write_audio(&mut self, pcm: &[u8]) -> io::Result<()> {
        self.image.write_all(pcm)?;
        self.hasher.update(pcm);
        self.audio_bytes += pcm.len() as u64;
        Ok(())
    }

    /// Validates the program length, writes DDPID/DDPMS/PQDESCR, the PQ
    /// sheet and the checksum file. Returns the paths written.
    pub fn finish(mut self, disc: &Disc) -> io::Result<Vec<PathBuf>> {
        if self.audio_bytes % SECTOR_BYTES != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "audio image is not sector-aligned ({} bytes is not a multiple of {})",
                    self.audio_bytes, SECTOR_BYTES
                ),
            ));
        }
        let audio_sectors = self.audio_bytes / SECTOR_BYTES;
        let program = disc.program_sectors();
        if audio_sectors != program {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "audio image is {audio_sectors} sectors but the track list \
                     describes {program}"
                ),
            ));
        }
        self.image.flush()?;
        drop(self.image);

        let pq = pq_stream(disc);
        let ms = ddpms_packets(disc, IMAGE_NAME, pq.len() as u64);
        let id = ddpid_packet(disc, ms.len() as u64);
        let sheet = pq_sheet(disc);

        let mut written = vec![self.dir.join(IMAGE_NAME)];
        let mut sums: Vec<(String, String)> =
            vec![(IMAGE_NAME.into(), format!("{:x}", self.hasher.finalize()))];
        for (name, bytes) in [
            ("DDPID", id.as_slice()),
            ("DDPMS", ms.as_slice()),
            ("PQDESCR", pq.as_slice()),
            ("PQ_SHEET.TXT", sheet.as_bytes()),
        ] {
            let path = self.dir.join(name);
            fs::write(&path, bytes)?;
            sums.push((name.into(), format!("{:x}", Md5::digest(bytes))));
            written.push(path);
        }
        // md5sum -c compatible: "<digest>  <name>".
        let mut chk = String::new();
        for (name, digest) in &sums {
            let _ = writeln!(chk, "{digest}  {name}");
        }
        let chk_path = self.dir.join("CHECKSUM.MD5");
        fs::write(&chk_path, chk)?;
        written.push(chk_path);
        Ok(written)
    }
}

#[cfg(test)]
mod tests;
