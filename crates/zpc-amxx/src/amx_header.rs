//! Parser for the AMX image header (the `.amx` inside a `.amxx` section).
//!
//! Field layout transcribed from `AMX_HEADER` in `libpc300/amx.h` (Pawn, ITB
//! CompuPhase, zlib-style licence - see ATTRIBUTION.md). The struct is `PACKED`,
//! so every field sits at a fixed byte offset with no alignment padding.
//!
//! This exists so the disassembler can find the code segment: the differential
//! oracle needs `cod..dat`, and guessing that range would silently produce
//! garbage instructions rather than an error.

use crate::AmxxError;

/// Byte offsets of each `AMX_HEADER` field, in declaration order.
mod offset {
    pub const SIZE: usize = 0; // i32  total image size
    pub const MAGIC: usize = 4; // u16
    pub const FILE_VERSION: usize = 6; // u8
    pub const AMX_VERSION: usize = 7; // u8
    pub const FLAGS: usize = 8; // i16
    pub const DEFSIZE: usize = 10; // i16
    pub const COD: usize = 12; // i32  start of code
    pub const DAT: usize = 16; // i32  start of data (code ends here)
    pub const HEA: usize = 20; // i32
    pub const STP: usize = 24; // i32
    pub const CIP: usize = 28; // i32
    pub const PUBLICS: usize = 32; // i32
    pub const NATIVES: usize = 36; // i32
    pub const LIBRARIES: usize = 40; // i32
    pub const PUBVARS: usize = 44; // i32
    pub const TAGS: usize = 48; // i32
    pub const NAMETABLE: usize = 52; // i32
}

/// Size of `AMX_HEADER`, counted field by field from the packed struct:
/// `4 + 2 + 1 + 1 + 2 + 2` for the fixed-width prefix, then eleven `int32_t`
/// offsets, i.e. `12 + 44 = 56`.
///
/// Note: `lib.rs` carries a private `AMX_HEADER_LEN = 60`, used only as a
/// minimum-length guard, so the difference is harmless there. 56 is the value
/// derived from the struct and is what the offsets above depend on.
pub const HEADER_LEN: usize = 56;

/// `AMX_HEADER.magic` for a 32-bit cell image.
pub const MAGIC_AMX: u16 = 0xF1E0;

/// `AMX_FLAG_DEBUG` - a debug chunk follows the image.
pub const FLAG_DEBUG: i16 = 0x02;

/// The decoded AMX image header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmxHeader {
    pub size: i32,
    pub magic: u16,
    pub file_version: u8,
    pub amx_version: u8,
    pub flags: i16,
    pub defsize: i16,
    pub cod: i32,
    pub dat: i32,
    pub hea: i32,
    pub stp: i32,
    pub cip: i32,
    pub publics: i32,
    pub natives: i32,
    pub libraries: i32,
    pub pubvars: i32,
    pub tags: i32,
    pub nametable: i32,
}

fn i32_at(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn i16_at(b: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([b[off], b[off + 1]])
}

impl AmxHeader {
    /// Parse the header from the start of an AMX image.
    pub fn parse(amx: &[u8]) -> Result<Self, AmxxError> {
        if amx.len() < HEADER_LEN {
            return Err(AmxxError::Truncated {
                need: HEADER_LEN,
                got: amx.len(),
            });
        }
        let magic = u16::from_le_bytes([amx[offset::MAGIC], amx[offset::MAGIC + 1]]);
        if magic != MAGIC_AMX {
            return Err(AmxxError::BadMagic {
                found: u32::from(magic),
            });
        }
        Ok(Self {
            size: i32_at(amx, offset::SIZE),
            magic,
            file_version: amx[offset::FILE_VERSION],
            amx_version: amx[offset::AMX_VERSION],
            flags: i16_at(amx, offset::FLAGS),
            defsize: i16_at(amx, offset::DEFSIZE),
            cod: i32_at(amx, offset::COD),
            dat: i32_at(amx, offset::DAT),
            hea: i32_at(amx, offset::HEA),
            stp: i32_at(amx, offset::STP),
            cip: i32_at(amx, offset::CIP),
            publics: i32_at(amx, offset::PUBLICS),
            natives: i32_at(amx, offset::NATIVES),
            libraries: i32_at(amx, offset::LIBRARIES),
            pubvars: i32_at(amx, offset::PUBVARS),
            tags: i32_at(amx, offset::TAGS),
            nametable: i32_at(amx, offset::NAMETABLE),
        })
    }

    /// Whether the image carries a debug chunk after it.
    pub fn has_debug(&self) -> bool {
        self.flags & FLAG_DEBUG != 0
    }

    /// The code segment: everything between `cod` and `dat`.
    ///
    /// Returns an error rather than a truncated slice when the offsets are
    /// inconsistent - feeding a wrong range to the disassembler yields
    /// plausible-looking nonsense, which is worse than failing.
    pub fn code<'a>(&self, amx: &'a [u8]) -> Result<&'a [u8], AmxxError> {
        let oob = || AmxxError::Truncated {
            need: HEADER_LEN,
            got: amx.len(),
        };
        let cod = usize::try_from(self.cod).map_err(|_| oob())?;
        let dat = usize::try_from(self.dat).map_err(|_| oob())?;
        if cod > dat || dat > amx.len() {
            return Err(oob());
        }
        Ok(&amx[cod..dat])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but structurally valid AMX image wrapping `code`.
    fn fake_amx(code: &[u8]) -> Vec<u8> {
        let cod = HEADER_LEN as i32;
        let dat = cod + code.len() as i32;
        let mut v = vec![0u8; HEADER_LEN];
        v[offset::SIZE..offset::SIZE + 4].copy_from_slice(&dat.to_le_bytes());
        v[offset::MAGIC..offset::MAGIC + 2].copy_from_slice(&MAGIC_AMX.to_le_bytes());
        v[offset::COD..offset::COD + 4].copy_from_slice(&cod.to_le_bytes());
        v[offset::DAT..offset::DAT + 4].copy_from_slice(&dat.to_le_bytes());
        v[offset::STP..offset::STP + 4].copy_from_slice(&4096i32.to_le_bytes());
        v.extend_from_slice(code);
        v
    }

    #[test]
    fn parses_the_documented_field_offsets() {
        let amx = fake_amx(&[1, 2, 3, 4]);
        let h = AmxHeader::parse(&amx).unwrap();
        assert_eq!(h.magic, MAGIC_AMX);
        assert_eq!(h.cod, HEADER_LEN as i32);
        assert_eq!(h.dat, HEADER_LEN as i32 + 4);
        assert_eq!(h.stp, 4096);
        assert!(!h.has_debug());
    }

    #[test]
    fn extracts_the_code_segment() {
        let amx = fake_amx(&[10, 20, 30, 40]);
        let h = AmxHeader::parse(&amx).unwrap();
        assert_eq!(h.code(&amx).unwrap(), &[10, 20, 30, 40]);
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut amx = fake_amx(&[]);
        amx[offset::MAGIC] = 0;
        assert!(matches!(
            AmxHeader::parse(&amx),
            Err(AmxxError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_a_truncated_header() {
        assert!(matches!(
            AmxHeader::parse(&[0u8; 8]),
            Err(AmxxError::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_inconsistent_code_bounds() {
        // dat before cod must not silently yield an empty or reversed slice
        let mut amx = fake_amx(&[1, 2, 3, 4]);
        amx[offset::DAT..offset::DAT + 4].copy_from_slice(&0i32.to_le_bytes());
        let h = AmxHeader::parse(&amx).unwrap();
        assert!(matches!(h.code(&amx), Err(AmxxError::Truncated { .. })));
    }

    #[test]
    fn detects_the_debug_flag() {
        let mut amx = fake_amx(&[]);
        amx[offset::FLAGS..offset::FLAGS + 2].copy_from_slice(&FLAG_DEBUG.to_le_bytes());
        assert!(AmxHeader::parse(&amx).unwrap().has_debug());
    }
}
