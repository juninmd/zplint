//! The `.amxx` plugin container: writer and reader.
//!
//! An `.amxx` file is a thin envelope around one or more compiled AMX images
//! (one per cell size). The envelope is written by amxxpc; this crate
//! reimplements it in Rust from the format description below. The description
//! was derived by reading the layout of the AMX Mod X container (magic values,
//! field widths and write order) and restating it as a specification; no
//! compiler source was copied.
//!
//! # Container layout
//!
//! All integers are **little-endian** and **packed** (no padding between
//! fields). Offsets are absolute byte offsets from the start of file.
//!
//! ## File header (7 bytes, offset 0)
//!
//! | Offset | Width | Field    | Value |
//! |--------|-------|----------|-------|
//! | 0      | u32   | magic    | `0x414D5858`, which little-endian on disk is the ASCII bytes `XXMA` |
//! | 4      | u16   | version  | `0x0300` (bytes `00 03`) |
//! | 6      | u8    | sections | number of section entries that follow |
//!
//! ## Section table (17 bytes per entry, starting at offset 7)
//!
//! Fields are stored in this exact order, which is *not* the declaration order
//! of the C struct in the reference implementation:
//!
//! | Offset | Width | Field      | Meaning |
//! |--------|-------|------------|---------|
//! | +0     | u8    | cell_size  | cell width in bytes (`4` for the 32-bit build, `8` for 64-bit) |
//! | +1     | u32   | disk_size  | length in bytes of the compressed payload |
//! | +5     | u32   | image_size | length in bytes of the payload after decompression |
//! | +9     | u32   | mem_size   | runtime memory footprint (the AMX header's `stp`) |
//! | +13    | u32   | offset     | absolute file offset of the compressed payload |
//!
//! ## Payloads
//!
//! Each payload is the raw AMX image (AMX header + code + data, plus the
//! debug-info chunk when the plugin was built with debug symbols) compressed
//! with **zlib** — RFC 1950 zlib format with its 2-byte header and Adler-32
//! trailer, *not* raw deflate and not gzip — at the default compression
//! level (6).
//!
//! The writer here lays payloads out back-to-back immediately after the
//! section table, so the first payload starts at `7 + 17 * sections`. The
//! reader does not assume that and honours each entry's `offset`.
//!
//! # AMX image facts used by [`amx_image_len`] and [`amx_mem_size`]
//!
//! Within the AMX image header (also little-endian and packed): `size: i32` at
//! offset 0, `magic: u16` (`0xF1E0`) at 4, `flags: i16` at 8, `stp: i32` at 24.
//! `flags & 0x02` marks the presence of debug information, stored as a separate
//! chunk starting at file offset `size` whose own first field is an `i32` byte
//! length of that chunk. The container payload must cover image plus chunk.

use std::fmt;
use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

/// Container magic, `XXMA` on disk. Little-endian encoding of `0x414D5858`.
pub mod amx_header;

pub use amx_header::AmxHeader;

pub const MAGIC: u32 = 0x414D_5858;
/// Container magic as it appears on disk, for byte-level assertions.
pub const MAGIC_BYTES: [u8; 4] = *b"XXMA";
/// Container format version. Written as the bytes `00 03`.
pub const VERSION: u16 = 0x0300;
/// Cell size of the 32-bit build, in bytes.
pub const CELL_SIZE_32: u8 = 4;

/// Size of the file header: magic + version + section count.
pub const FILE_HEADER_LEN: usize = 4 + 2 + 1;
/// Size of one section table entry: one byte plus four 32-bit fields.
pub const SECTION_ENTRY_LEN: usize = 1 + 4 * 4;

/// Magic of an AMX image header.
const AMX_MAGIC: u16 = 0xF1E0;
/// AMX header flag bit meaning "debug information follows the image".
const AMX_FLAG_DEBUG: i16 = 0x02;
/// Byte length of the AMX image header, used to reject truncated images.
const AMX_HEADER_LEN: usize = 60;

/// One plugin image inside the container, held uncompressed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Section {
    /// Cell width in bytes, e.g. [`CELL_SIZE_32`].
    pub cell_size: u8,
    /// Runtime memory footprint of the image (the AMX header's `stp`).
    pub mem_size: u32,
    /// The raw AMX image bytes.
    pub image: Vec<u8>,
}

/// A parsed `.amxx` container.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct AmxxFile {
    pub sections: Vec<Section>,
}

/// Everything that can go wrong reading or writing a container.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AmxxError {
    /// The input is shorter than the structure being read requires.
    Truncated { need: usize, got: usize },
    /// The first four bytes are not [`MAGIC_BYTES`].
    BadMagic { found: u32 },
    /// The version field is not [`VERSION`].
    UnsupportedVersion { found: u16 },
    /// A section's payload range lies outside the file.
    PayloadOutOfBounds { index: usize },
    /// A payload is not valid zlib data.
    Decompress { index: usize },
    /// A payload decompressed to a length other than its `image_size`.
    ImageSizeMismatch {
        index: usize,
        expected: u32,
        actual: usize,
    },
    /// More sections than the u8 count field can express.
    TooManySections { count: usize },
    /// A length does not fit in the u32 field that must hold it.
    LengthOverflow,
    /// The bytes handed to an AMX-image helper are not a valid image.
    NotAnAmxImage,
}

impl fmt::Display for AmxxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AmxxError::Truncated { need, got } => {
                write!(f, "truncated container: need {need} bytes, got {got}")
            }
            AmxxError::BadMagic { found } => {
                write!(f, "bad magic {found:#010x}, expected {MAGIC:#010x}")
            }
            AmxxError::UnsupportedVersion { found } => {
                write!(f, "unsupported version {found:#06x}, expected {VERSION:#06x}")
            }
            AmxxError::PayloadOutOfBounds { index } => {
                write!(f, "section {index} payload lies outside the file")
            }
            AmxxError::Decompress { index } => {
                write!(f, "section {index} payload is not valid zlib data")
            }
            AmxxError::ImageSizeMismatch {
                index,
                expected,
                actual,
            } => write!(
                f,
                "section {index} decompressed to {actual} bytes, header says {expected}"
            ),
            AmxxError::TooManySections { count } => {
                write!(f, "{count} sections exceeds the container limit of 255")
            }
            AmxxError::LengthOverflow => write!(f, "image or payload length exceeds 4 GiB"),
            AmxxError::NotAnAmxImage => write!(f, "input is not a valid AMX image"),
        }
    }
}

impl std::error::Error for AmxxError {}

/// The plugin image length of a compiled `.amx` file: the AMX image itself
/// plus its debug-information chunk when one is present.
///
/// This is the length to slice before building a [`Section`], since the
/// container payload must cover both parts.
pub fn amx_image_len(amx: &[u8]) -> Result<usize, AmxxError> {
    if amx.len() < AMX_HEADER_LEN || u16_at(amx, 4) != Some(AMX_MAGIC) {
        return Err(AmxxError::NotAnAmxImage);
    }
    let size = u32_at(amx, 0).ok_or(AmxxError::NotAnAmxImage)? as usize;
    if size < AMX_HEADER_LEN || size > amx.len() {
        return Err(AmxxError::NotAnAmxImage);
    }
    let flags = u16_at(amx, 8).ok_or(AmxxError::NotAnAmxImage)? as i16;
    if flags & AMX_FLAG_DEBUG == 0 {
        return Ok(size);
    }
    let dbg_size = u32_at(amx, size).ok_or(AmxxError::NotAnAmxImage)? as usize;
    size.checked_add(dbg_size)
        .filter(|total| *total <= amx.len())
        .ok_or(AmxxError::NotAnAmxImage)
}

/// The runtime memory footprint (`stp`) declared by an AMX image header.
pub fn amx_mem_size(amx: &[u8]) -> Result<u32, AmxxError> {
    if amx.len() < AMX_HEADER_LEN || u16_at(amx, 4) != Some(AMX_MAGIC) {
        return Err(AmxxError::NotAnAmxImage);
    }
    u32_at(amx, 24).ok_or(AmxxError::NotAnAmxImage)
}

/// Build the single-section container for a 32-bit-cell build from the raw
/// bytes of a compiled `.amx` file.
///
/// Image length and memory size come from the AMX header, so `amx` may carry
/// trailing bytes beyond the plugin image.
pub fn write_amx32(amx: &[u8]) -> Result<Vec<u8>, AmxxError> {
    let len = amx_image_len(amx)?;
    write(&[Section {
        cell_size: CELL_SIZE_32,
        mem_size: amx_mem_size(amx)?,
        image: amx[..len].to_vec(),
    }])
}

/// Serialise sections into `.amxx` container bytes.
pub fn write(sections: &[Section]) -> Result<Vec<u8>, AmxxError> {
    let count = u8::try_from(sections.len()).map_err(|_| AmxxError::TooManySections {
        count: sections.len(),
    })?;

    let payloads: Vec<Vec<u8>> = sections
        .iter()
        .map(|s| zlib_compress(&s.image))
        .collect::<Result<_, _>>()?;

    let table_len = FILE_HEADER_LEN + SECTION_ENTRY_LEN * sections.len();
    let mut out = Vec::with_capacity(table_len + payloads.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.push(count);

    // Payloads follow the section table back-to-back.
    let mut offset = u32::try_from(table_len).map_err(|_| AmxxError::LengthOverflow)?;
    for (section, payload) in sections.iter().zip(&payloads) {
        let disk_size = u32::try_from(payload.len()).map_err(|_| AmxxError::LengthOverflow)?;
        let image_size =
            u32::try_from(section.image.len()).map_err(|_| AmxxError::LengthOverflow)?;
        out.push(section.cell_size);
        out.extend_from_slice(&disk_size.to_le_bytes());
        out.extend_from_slice(&image_size.to_le_bytes());
        out.extend_from_slice(&section.mem_size.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset = offset
            .checked_add(disk_size)
            .ok_or(AmxxError::LengthOverflow)?;
    }

    for payload in &payloads {
        out.extend_from_slice(payload);
    }
    Ok(out)
}

/// Parse `.amxx` container bytes back into their uncompressed sections.
pub fn read(bytes: &[u8]) -> Result<AmxxFile, AmxxError> {
    if bytes.len() < FILE_HEADER_LEN {
        return Err(AmxxError::Truncated {
            need: FILE_HEADER_LEN,
            got: bytes.len(),
        });
    }
    let magic = u32_at(bytes, 0).unwrap_or(0);
    if magic != MAGIC {
        return Err(AmxxError::BadMagic { found: magic });
    }
    let version = u16_at(bytes, 4).unwrap_or(0);
    if version != VERSION {
        return Err(AmxxError::UnsupportedVersion { found: version });
    }
    let count = bytes[6] as usize;

    let table_end = FILE_HEADER_LEN + SECTION_ENTRY_LEN * count;
    if bytes.len() < table_end {
        return Err(AmxxError::Truncated {
            need: table_end,
            got: bytes.len(),
        });
    }

    let mut sections = Vec::with_capacity(count);
    for index in 0..count {
        let at = FILE_HEADER_LEN + SECTION_ENTRY_LEN * index;
        let cell_size = bytes[at];
        let disk_size = u32_at(bytes, at + 1).unwrap_or(0) as usize;
        let image_size = u32_at(bytes, at + 5).unwrap_or(0);
        let mem_size = u32_at(bytes, at + 9).unwrap_or(0);
        let offset = u32_at(bytes, at + 13).unwrap_or(0) as usize;

        let end = offset
            .checked_add(disk_size)
            .ok_or(AmxxError::PayloadOutOfBounds { index })?;
        let payload = bytes
            .get(offset..end)
            .ok_or(AmxxError::PayloadOutOfBounds { index })?;
        let image =
            zlib_decompress(payload, image_size).map_err(|_| AmxxError::Decompress { index })?;
        if image.len() as u64 != u64::from(image_size) {
            return Err(AmxxError::ImageSizeMismatch {
                index,
                expected: image_size,
                actual: image.len(),
            });
        }
        sections.push(Section {
            cell_size,
            mem_size,
            image,
        });
    }
    Ok(AmxxFile { sections })
}

fn zlib_compress(data: &[u8]) -> Result<Vec<u8>, AmxxError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .and_then(|()| encoder.finish())
        .map_err(|_| AmxxError::LengthOverflow)
}

fn zlib_decompress(payload: &[u8], hint: u32) -> Result<Vec<u8>, std::io::Error> {
    let mut out = Vec::with_capacity(hint as usize);
    ZlibDecoder::new(payload).read_to_end(&mut out)?;
    Ok(out)
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(image: Vec<u8>) -> Section {
        Section {
            cell_size: CELL_SIZE_32,
            mem_size: 0x1234,
            image,
        }
    }

    /// A minimal but structurally valid AMX image.
    fn fake_amx(body: usize, flags: i16, stp: i32) -> Vec<u8> {
        let size = AMX_HEADER_LEN + body;
        let mut amx = vec![0u8; size];
        amx[0..4].copy_from_slice(&(size as i32).to_le_bytes());
        amx[4..6].copy_from_slice(&AMX_MAGIC.to_le_bytes());
        amx[8..10].copy_from_slice(&flags.to_le_bytes());
        amx[24..28].copy_from_slice(&stp.to_le_bytes());
        amx
    }

    #[test]
    fn header_bytes_match_the_spec() {
        let bytes = write(&[section(vec![1, 2, 3])]).unwrap();
        assert_eq!(&bytes[0..4], &MAGIC_BYTES);
        assert_eq!(&bytes[0..4], b"XXMA");
        assert_eq!(&bytes[4..6], &[0x00, 0x03]);
        assert_eq!(bytes[6], 1);
        assert_eq!(FILE_HEADER_LEN, 7);
        assert_eq!(SECTION_ENTRY_LEN, 17);
    }

    #[test]
    fn section_entry_fields_are_in_write_order() {
        let image = vec![7u8; 100];
        let bytes = write(&[section(image.clone())]).unwrap();
        let at = FILE_HEADER_LEN;
        let disk_size = u32_at(&bytes, at + 1).unwrap();
        assert_eq!(bytes[at], CELL_SIZE_32);
        assert_eq!(u32_at(&bytes, at + 5).unwrap(), image.len() as u32);
        assert_eq!(u32_at(&bytes, at + 9).unwrap(), 0x1234);
        assert_eq!(u32_at(&bytes, at + 13).unwrap(), 24);
        assert_eq!(bytes.len(), 24 + disk_size as usize);
    }

    #[test]
    fn round_trips_a_plain_payload() {
        let sections = vec![section((0u8..=255).collect())];
        let parsed = read(&write(&sections).unwrap()).unwrap();
        assert_eq!(parsed.sections, sections);
    }

    #[test]
    fn round_trips_an_empty_payload() {
        let sections = vec![section(Vec::new())];
        let bytes = write(&sections).unwrap();
        let parsed = read(&bytes).unwrap();
        assert_eq!(parsed.sections, sections);
        assert_eq!(u32_at(&bytes, FILE_HEADER_LEN + 5).unwrap(), 0);
    }

    #[test]
    fn round_trips_a_payload_that_grows_when_compressed() {
        // A few incompressible bytes: zlib framing exceeds the input length.
        let image = vec![0x1f, 0x8b, 0x42, 0xd7];
        let bytes = write(&[section(image.clone())]).unwrap();
        let disk_size = u32_at(&bytes, FILE_HEADER_LEN + 1).unwrap() as usize;
        assert!(disk_size > image.len(), "expected expansion, got {disk_size}");
        assert_eq!(read(&bytes).unwrap().sections[0].image, image);
    }

    #[test]
    fn round_trips_a_highly_compressible_payload() {
        let image = vec![0xAAu8; 64 * 1024];
        let bytes = write(&[section(image.clone())]).unwrap();
        let disk_size = u32_at(&bytes, FILE_HEADER_LEN + 1).unwrap() as usize;
        assert!(disk_size < image.len() / 100, "poor compression: {disk_size}");
        assert_eq!(read(&bytes).unwrap().sections[0].image, image);
    }

    #[test]
    fn round_trips_multiple_sections_with_distinct_offsets() {
        let sections = vec![
            Section {
                cell_size: 4,
                mem_size: 10,
                image: vec![1u8; 500],
            },
            Section {
                cell_size: 8,
                mem_size: 20,
                image: vec![2u8; 700],
            },
        ];
        let bytes = write(&sections).unwrap();
        assert_eq!(bytes[6], 2);
        let first = u32_at(&bytes, FILE_HEADER_LEN + 13).unwrap();
        let second = u32_at(&bytes, FILE_HEADER_LEN + SECTION_ENTRY_LEN + 13).unwrap();
        assert_eq!(first, 7 + 34);
        assert!(second > first);
        assert_eq!(read(&bytes).unwrap().sections, sections);
    }

    #[test]
    fn write_amx32_takes_size_and_stp_from_the_amx_header() {
        let amx = fake_amx(40, 0, 0x4000);
        let parsed = read(&write_amx32(&amx).unwrap()).unwrap();
        assert_eq!(parsed.sections.len(), 1);
        assert_eq!(parsed.sections[0].cell_size, CELL_SIZE_32);
        assert_eq!(parsed.sections[0].mem_size, 0x4000);
        assert_eq!(parsed.sections[0].image, amx);
    }

    #[test]
    fn write_amx32_includes_the_debug_chunk() {
        let mut amx = fake_amx(40, AMX_FLAG_DEBUG, 0x4000);
        let image_len = amx.len();
        amx.extend_from_slice(&32i32.to_le_bytes());
        amx.extend_from_slice(&[9u8; 28]);
        amx.extend_from_slice(&[0xFF; 16]); // trailing junk, must be excluded

        assert_eq!(amx_image_len(&amx).unwrap(), image_len + 32);
        let parsed = read(&write_amx32(&amx).unwrap()).unwrap();
        assert_eq!(parsed.sections[0].image, amx[..image_len + 32]);
    }

    #[test]
    fn rejects_non_amx_input() {
        assert_eq!(amx_image_len(&[0u8; 8]), Err(AmxxError::NotAnAmxImage));
        assert_eq!(amx_mem_size(&[0u8; 8]), Err(AmxxError::NotAnAmxImage));
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut bytes = write(&[section(vec![1, 2, 3])]).unwrap();
        bytes[0] = b'Y';
        assert!(matches!(read(&bytes), Err(AmxxError::BadMagic { .. })));
    }

    #[test]
    fn rejects_an_unknown_version() {
        let mut bytes = write(&[section(vec![1, 2, 3])]).unwrap();
        bytes[5] = 0x04;
        assert_eq!(
            read(&bytes),
            Err(AmxxError::UnsupportedVersion { found: 0x0400 })
        );
    }

    #[test]
    fn rejects_a_truncated_file() {
        assert!(matches!(read(&[]), Err(AmxxError::Truncated { .. })));
        let bytes = write(&[section(vec![1, 2, 3])]).unwrap();
        assert!(matches!(
            read(&bytes[..10]),
            Err(AmxxError::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_a_payload_pointing_past_the_end() {
        let mut bytes = write(&[section(vec![1, 2, 3])]).unwrap();
        let at = FILE_HEADER_LEN + 13;
        bytes[at..at + 4].copy_from_slice(&9999u32.to_le_bytes());
        assert_eq!(read(&bytes), Err(AmxxError::PayloadOutOfBounds { index: 0 }));
    }

    #[test]
    fn rejects_a_corrupt_payload() {
        let mut bytes = write(&[section(vec![5u8; 200])]).unwrap();
        let last = bytes.len() - 1;
        bytes[24] ^= 0xFF;
        bytes[last] ^= 0xFF;
        assert!(matches!(
            read(&bytes),
            Err(AmxxError::Decompress { .. } | AmxxError::ImageSizeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_a_lying_image_size() {
        let mut bytes = write(&[section(vec![5u8; 200])]).unwrap();
        let at = FILE_HEADER_LEN + 5;
        bytes[at..at + 4].copy_from_slice(&199u32.to_le_bytes());
        assert_eq!(
            read(&bytes),
            Err(AmxxError::ImageSizeMismatch {
                index: 0,
                expected: 199,
                actual: 200
            })
        );
    }
}
