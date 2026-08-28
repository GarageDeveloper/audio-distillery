//! CD-Text stream (`CDTEXT.BIN`) — the optional lead-in text of a Red
//! Book disc, carried in a DDP fileset as a raw sequence of 18-byte
//! packs declared by an `S0`/`CDTEXT` map packet.
//!
//! Pack layout: type (0x80 title, 0x81 performer, 0x8E UPC/ISRC, 0x8F
//! size info), track number, sequence number, flags (DBCS bit, block
//! number, character position), 12 text bytes, CRC-16 (X.25 polynomial,
//! result inverted, big-endian). Strings are NUL-terminated Latin-1,
//! packed back-to-back across consecutive packs of the same type; track
//! 0 holds the album-level value.

use crate::Disc;

const PACK_LEN: usize = 18;
const TEXT_LEN: usize = 12;
/// EBU language code for English (block 0's declared language).
const LANG_ENGLISH: u8 = 0x09;

/// CRC-16, polynomial x^16+x^12+x^5+1, init 0, no reflection — the
/// CD-Text field stores the complement, big-endian.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Latin-1 encoding with '_' for anything outside the charset.
fn latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| {
            let n = c as u32;
            if (0x20..=0x7E).contains(&n) || (0xA0..=0xFF).contains(&n) {
                n as u8
            } else {
                b'_'
            }
        })
        .collect()
}

struct Pack {
    pack_type: u8,
    track: u8,
    /// Character position of the pack's first byte within its string
    /// (capped at 15), OR the element number for size-info packs.
    char_pos: u8,
    text: [u8; TEXT_LEN],
}

/// Split the NUL-terminated concatenation of `(track, text)` strings
/// into 12-byte packs of `pack_type`.
fn text_packs(pack_type: u8, strings: &[(u8, Vec<u8>)]) -> Vec<Pack> {
    // Flat byte stream, remembering which string each byte belongs to
    // and its offset within that string.
    let mut bytes: Vec<(u8, usize, u8)> = Vec::new(); // (byte, char-pos, track)
    for (track, text) in strings {
        for (i, b) in text.iter().chain(std::iter::once(&0u8)).enumerate() {
            bytes.push((*b, i.min(15), *track));
        }
    }
    let mut packs = Vec::new();
    for chunk in bytes.chunks(TEXT_LEN) {
        let mut text = [0u8; TEXT_LEN];
        for (i, (b, _, _)) in chunk.iter().enumerate() {
            text[i] = *b;
        }
        packs.push(Pack {
            pack_type,
            track: chunk[0].2,
            char_pos: chunk[0].1 as u8,
            text,
        });
    }
    packs
}

/// Whether the disc carries any CD-Text worth writing.
pub fn has_text(disc: &Disc) -> bool {
    !disc.title.trim().is_empty()
        || !disc.performer.trim().is_empty()
        || disc.tracks.iter().any(|t| !t.title.trim().is_empty())
}

/// Build the full CD-Text stream (block 0, English, Latin-1): titles,
/// performers, UPC/ISRC when present, and the 3 size-information packs.
pub fn cdtext_stream(disc: &Disc) -> Vec<u8> {
    let mut packs: Vec<Pack> = Vec::new();

    // 0x80 — titles: album, then every track.
    let mut titles: Vec<(u8, Vec<u8>)> = vec![(0, latin1(disc.title.trim()))];
    for t in &disc.tracks {
        titles.push((t.number as u8, latin1(t.title.trim())));
    }
    packs.extend(text_packs(0x80, &titles));

    // 0x81 — performers: the album performer, replicated per track.
    let performer = latin1(disc.performer.trim());
    let mut performers: Vec<(u8, Vec<u8>)> = vec![(0, performer.clone())];
    for t in &disc.tracks {
        performers.push((t.number as u8, performer.clone()));
    }
    packs.extend(text_packs(0x81, &performers));

    // 0x8E — UPC/EAN (track 0) and ISRCs, only when any exists.
    if disc.ean.is_some() || disc.tracks.iter().any(|t| t.isrc.is_some()) {
        let mut codes: Vec<(u8, Vec<u8>)> =
            vec![(0, latin1(disc.ean.as_deref().unwrap_or("")))];
        for t in &disc.tracks {
            codes.push((t.number as u8, latin1(t.isrc.as_deref().unwrap_or(""))));
        }
        packs.extend(text_packs(0x8E, &codes));
    }

    // 0x8F — size information: 3 packs describing the block.
    let mut counts = [0u8; 16];
    for p in &packs {
        counts[(p.pack_type - 0x80) as usize] += 1;
    }
    counts[0x0F] = 3;
    let total = packs.len() + 3;
    let last_track = disc.tracks.iter().map(|t| t.number).max().unwrap_or(0) as u8;
    let mut info = [0u8; 36];
    info[0] = 0x00; // character code: ISO 8859-1
    info[1] = 1; // first track
    info[2] = last_track;
    info[3] = 0x00; // copyright flags
    info[4..20].copy_from_slice(&counts);
    info[20] = (total - 1) as u8; // last sequence number, block 0
    info[28] = LANG_ENGLISH; // language code, block 0
    for (i, chunk) in info.chunks(TEXT_LEN).enumerate() {
        let mut text = [0u8; TEXT_LEN];
        text.copy_from_slice(chunk);
        packs.push(Pack {
            pack_type: 0x8F,
            track: i as u8, // element number, not a track
            char_pos: 0,
            text,
        });
    }

    // Serialize: sequence numbers, flags, CRC.
    let mut out = Vec::with_capacity(packs.len() * PACK_LEN);
    for (seq, p) in packs.iter().enumerate() {
        let mut raw = [0u8; PACK_LEN];
        raw[0] = p.pack_type;
        raw[1] = p.track;
        raw[2] = seq as u8;
        raw[3] = p.char_pos & 0x0F; // block 0, single-byte charset
        raw[4..16].copy_from_slice(&p.text);
        let crc = !crc16(&raw[..16]);
        raw[16] = (crc >> 8) as u8;
        raw[17] = crc as u8;
        out.extend_from_slice(&raw);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Track;

    /// Known-answer: CRC-CCITT/XModem of "123456789" is 0x31C3.
    #[test]
    fn crc_known_vector() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
    }

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
                    length_sectors: 100,
                    isrc: Some("FRAB12600001".into()),
                    pregap_sectors: 0,
                },
                Track {
                    number: 2,
                    title: "Éléphant".into(), // Latin-1 accents survive
                    start_sector: 100,
                    length_sectors: 50,
                    isrc: None,
                    pregap_sectors: 0,
                },
            ],
        }
    }

    /// Decode the stream the way a reader does: accumulate text bytes of
    /// one pack type, split on NUL, attribute per track.
    fn decode(stream: &[u8], pack_type: u8) -> Vec<String> {
        let mut buf = Vec::new();
        for p in stream.chunks(PACK_LEN) {
            if p[0] == pack_type && p[3] & 0x70 == 0 {
                buf.extend_from_slice(&p[4..16]);
            }
        }
        buf.split(|b| *b == 0)
            .map(|s| s.iter().map(|b| *b as char).collect::<String>())
            .collect()
    }

    #[test]
    fn stream_is_valid_and_readable() {
        let s = cdtext_stream(&disc());
        assert_eq!(s.len() % PACK_LEN, 0);

        for (i, p) in s.chunks(PACK_LEN).enumerate() {
            assert_eq!(p[2] as usize, i, "sequence must be continuous");
            let crc = !crc16(&p[..16]);
            assert_eq!(
                ((p[16] as u16) << 8) | p[17] as u16,
                crc,
                "pack {i} CRC"
            );
        }

        let titles = decode(&s, 0x80);
        assert_eq!(titles[0], "Barn Sessions");
        assert_eq!(titles[1], "Opener");
        assert!(titles[2].starts_with('É'), "{titles:?}");
        let performers = decode(&s, 0x81);
        assert_eq!(performers[0], "The Copper Stills");
        assert_eq!(performers[1], "The Copper Stills");
        let codes = decode(&s, 0x8E);
        assert_eq!(codes[0], "1234567890128");
        assert_eq!(codes[1], "FRAB12600001");
        assert_eq!(codes[2], "");

        // Size info: 3 packs, charset Latin-1, last track 2, counts add up.
        let info: Vec<&[u8]> = s
            .chunks(PACK_LEN)
            .filter(|p| p[0] == 0x8F)
            .collect();
        assert_eq!(info.len(), 3);
        let payload: Vec<u8> = info.iter().flat_map(|p| p[4..16].to_vec()).collect();
        assert_eq!(payload[0], 0x00, "ISO 8859-1");
        assert_eq!(payload[1], 1);
        assert_eq!(payload[2], 2);
        let total = s.len() / PACK_LEN;
        assert_eq!(payload[20] as usize, total - 1, "last sequence number");
        assert_eq!(payload[28], LANG_ENGLISH);
        let declared: usize = payload[4..20].iter().map(|c| *c as usize).sum();
        assert_eq!(declared, total, "declared pack counts must cover the block");
    }

    #[test]
    fn empty_disc_has_no_text() {
        let d = Disc::default();
        assert!(!has_text(&d));
        assert!(has_text(&disc()));
    }
}
