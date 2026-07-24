//! The AMX `.dbg` (debug information) chunk: types, writer and reader.
//!
//! # Provenance
//!
//! Every field width, order and constant below is transcribed from
//! `libpc300/amxdbg.h` and `libpc300/sc6.c` of the AMX Mod X compiler (Pawn,
//! ITB CompuPhase, zlib-style licence - see ATTRIBUTION.md). Line citations
//! refer to those files. `amxdbg.h` declares every record under `#pragma
//! pack(1)` / `__attribute__((packed))` (`amxdbg.h:39-61`), so **there is no
//! alignment padding anywhere in the chunk**, and all integers are
//! **little-endian** (the format is only ever produced/consumed on
//! little-endian AMX builds; the compiler writes raw structs with `fwrite`).
//!
//! `ucell` is 32 bits for the 4-byte-cell build this crate targets, so every
//! `ucell` field below is a `u32`.
//!
//! # Placement
//!
//! The chunk is appended directly after the AMX image, i.e. at file offset
//! `AMX_HEADER.size`, and the image header's `AMX_FLAG_DEBUG` (`0x02`) bit is
//! set (`sc6.c:925`, `append_dbginfo`). The chunk's own first field repeats its
//! total byte length, which is how [`crate::amx_image_len`] finds the end of
//! the payload. Use [`append_debug_chunk`] to do both steps.
//!
//! # Chunk layout
//!
//! ## Header - `AMX_DBG_HDR`, 22 bytes (`amxdbg.h:63-75`)
//!
//! | Offset | Width | Field | Meaning |
//! |--------|-------|-------|---------|
//! | 0  | i32 | `size`       | size **of the whole debug chunk**, header included (`sc6.c:974` seeds it with `sizeof dbghdr` then adds every record) |
//! | 4  | u16 | `magic`      | [`DBG_MAGIC`] = `0xf1ef` (`amxdbg.h:76`) |
//! | 6  | u8  | `file_version` | `CUR_FILE_VERSION` = 8 (`amx.h:146`, `sc6.c:976`) |
//! | 7  | u8  | `amx_version`  | `MIN_AMX_VERSION` = 8 (`amx.h:148`, `sc6.c:977`) |
//! | 8  | i16 | `flags`      | unused, written as 0 (`amxdbg.h:68`, zeroed at `sc6.c:973`) |
//! | 10 | i16 | `files`      | number of file records |
//! | 12 | i16 | `lines`      | number of line records |
//! | 14 | i16 | `symbols`    | number of symbol records |
//! | 16 | i16 | `tags`       | number of tag records |
//! | 18 | i16 | `automatons` | number of automaton records |
//! | 20 | i16 | `states`     | number of state records |
//!
//! The six tables then follow **in that same order**, back to back, with no
//! padding and no per-table header (`sc6.c:1057-1161`).
//!
//! ## File table - `AMX_DBG_FILE` (`amxdbg.h:78-81`, written at `sc6.c:1063-1084`)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | u32 (`ucell`) | `address` | code address where this file's generated code starts |
//! | bytes | `name`    | NUL-terminated string |
//!
//! ## Line table - `AMX_DBG_LINE`, 8 bytes fixed (`amxdbg.h:83-86`, `sc6.c:1087-1095`)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | u32 (`ucell`) | `address` | code address the line starts at |
//! | i32 | `line` | **0-based** line number as emitted by the compiler |
//!
//! ## Symbol table - `AMX_DBG_SYMBOL`, 18 fixed bytes + name + dims
//! (`amxdbg.h:88-97`; note `sc6.c:1127` writes `sizeof dbgsym - 1`, i.e. the
//! struct minus the one-byte `name[1]` placeholder, then the NUL-terminated
//! name, then `dim` `AMX_DBG_SYMDIM` records)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | u32 (`ucell`) | `address`   | data-segment address, or frame-relative offset for locals |
//! | i16 | `tag`       | tag id of the symbol |
//! | u32 (`ucell`) | `codestart` | first code address where the symbol is in scope |
//! | u32 (`ucell`) | `codeend`   | one past the last code address where it is in scope |
//! | u8  | `ident`     | kind: 1 `iVARIABLE`, 2 `iREFERENCE`, 3 `iARRAY`, 4 `iREFARRAY`, 9 `iFUNCTN` (`amxdbg.h:131-137`) |
//! | u8  | `vclass`    | storage class: 0 global, 1 local/argument, 2 static (see [`VCLASS_GLOBAL`]) |
//! | i16 | `dim`       | number of array dimensions that follow the name |
//! | bytes | `name`    | NUL-terminated string |
//! | `dim` x 6 | dims | `AMX_DBG_SYMDIM` records |
//!
//! ### `AMX_DBG_SYMDIM`, 6 bytes (`amxdbg.h:99-102`, `sc6.c:1130`)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | i16 | `tag`  | tag of this dimension's index |
//! | u32 (`ucell`) | `size` | number of elements in this dimension |
//!
//! ## Tag table - `AMX_DBG_TAG` (`amxdbg.h:104-107`, `sc6.c:1135-1140`)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | i16 | `tag`  | tag id, masked with `TAGMASK` by the compiler |
//! | bytes | `name` | NUL-terminated string |
//!
//! ## Automaton table - `AMX_DBG_MACHINE` (`amxdbg.h:109-113`, `sc6.c:1143-1150`)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | i16 | `automaton` | automaton id (0 is the implicit unnamed automaton) |
//! | u32 (`ucell`) | `address` | data address of the automaton's state variable |
//! | bytes | `name` | NUL-terminated string (empty for automaton 0) |
//!
//! ## State table - `AMX_DBG_STATE` (`amxdbg.h:115-119`, `sc6.c:1153-1160`)
//!
//! | Width | Field | Meaning |
//! |-------|-------|---------|
//! | i16 | `state`     | state id |
//! | i16 | `automaton` | id of the automaton the state belongs to |
//! | bytes | `name`    | NUL-terminated string |
//!
//! # Strings
//!
//! Names are written as raw bytes followed by one `0x00`; there is no string
//! pool and **no prefix sharing** - two records with a common prefix each store
//! their own full copy. This module encodes [`String`] as UTF-8 and rejects
//! interior NUL bytes, which is a superset of the ASCII the compiler emits.

use crate::amx_header::{FLAG_DEBUG, HEADER_LEN, MAGIC_AMX};

/// `AMX_DBG_MAGIC`, the debug chunk signature (`amxdbg.h:76`).
pub const DBG_MAGIC: u16 = 0xf1ef;
/// `CUR_FILE_VERSION` (`amx.h:146`), written to `AMX_DBG_HDR.file_version`.
pub const DBG_FILE_VERSION: u8 = 8;
/// `MIN_AMX_VERSION` (`amx.h:148`), written to `AMX_DBG_HDR.amx_version`.
pub const DBG_AMX_VERSION: u8 = 8;

/// Byte length of `AMX_DBG_HDR`: `4 + 2 + 1 + 1 + 7 * 2`.
pub const HDR_LEN: usize = 22;
/// Byte length of `AMX_DBG_LINE`: `4 + 4`.
pub const LINE_LEN: usize = 8;
/// Fixed part of `AMX_DBG_SYMBOL`, excluding the name: `4 + 2 + 4 + 4 + 1 + 1 + 2`.
pub const SYMBOL_FIXED_LEN: usize = 18;
/// Byte length of `AMX_DBG_SYMDIM`: `2 + 4`.
pub const SYMDIM_LEN: usize = 6;

/// `iVARIABLE` (`amxdbg.h:132`).
pub const IDENT_VARIABLE: u8 = 1;
/// `iREFERENCE` (`amxdbg.h:133`).
pub const IDENT_REFERENCE: u8 = 2;
/// `iARRAY` (`amxdbg.h:134`).
pub const IDENT_ARRAY: u8 = 3;
/// `iREFARRAY` (`amxdbg.h:135`).
pub const IDENT_REFARRAY: u8 = 4;
/// `iFUNCTN` (`amxdbg.h:136`).
pub const IDENT_FUNCTION: u8 = 9;

/// `sGLOBAL` storage class: address is absolute in the data segment.
pub const VCLASS_GLOBAL: u8 = 0;
/// `sLOCAL` storage class: address is relative to the frame pointer.
pub const VCLASS_LOCAL: u8 = 1;
/// `sSTATIC` storage class.
pub const VCLASS_STATIC: u8 = 2;

/// Everything that can go wrong writing or parsing a debug chunk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DebugError {
    /// The chunk (or a record inside it) is shorter than required.
    Truncated { need: usize, got: usize },
    /// `AMX_DBG_HDR.magic` is not [`DBG_MAGIC`].
    BadMagic { found: u16 },
    /// A NUL-terminated string ran off the end of the chunk.
    UnterminatedString { at: usize },
    /// A name is not valid UTF-8.
    NameNotUtf8 { at: usize },
    /// A name handed to the writer contains an interior NUL byte.
    InteriorNul,
    /// A table has more entries than the `i16` count field can express.
    TooManyEntries { table: &'static str, count: usize },
    /// The chunk does not fit in the `i32` `size` field.
    ChunkTooLarge,
    /// `AMX_DBG_HDR.size` disagrees with the bytes actually consumed.
    SizeMismatch { declared: usize, actual: usize },
    /// [`append_debug_chunk`] was handed something that is not an AMX image,
    /// or whose header `size` does not match the image length.
    NotAnAmxImage,
}

impl std::fmt::Display for DebugError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugError::Truncated { need, got } => {
                write!(f, "truncated debug chunk: need {need} bytes, got {got}")
            }
            DebugError::BadMagic { found } => {
                write!(f, "bad debug magic {found:#06x}, expected {DBG_MAGIC:#06x}")
            }
            DebugError::UnterminatedString { at } => {
                write!(f, "unterminated string at offset {at}")
            }
            DebugError::NameNotUtf8 { at } => write!(f, "name at offset {at} is not UTF-8"),
            DebugError::InteriorNul => write!(f, "name contains an interior NUL byte"),
            DebugError::TooManyEntries { table, count } => {
                write!(f, "{count} entries in the {table} table exceed the i16 limit")
            }
            DebugError::ChunkTooLarge => write!(f, "debug chunk exceeds 2 GiB"),
            DebugError::SizeMismatch { declared, actual } => write!(
                f,
                "debug header declares {declared} bytes but the tables span {actual}"
            ),
            DebugError::NotAnAmxImage => write!(f, "input is not a self-consistent AMX image"),
        }
    }
}

impl std::error::Error for DebugError {}

/// `AMX_DBG_FILE` (`amxdbg.h:78-81`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DebugFile {
    /// Code address where this file's generated code starts.
    pub address: u32,
    pub name: String,
}

/// `AMX_DBG_LINE` (`amxdbg.h:83-86`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DebugLine {
    /// Code address the line starts at.
    pub address: u32,
    /// Line number, 0-based as emitted by the compiler.
    pub line: i32,
}

/// `AMX_DBG_SYMDIM` (`amxdbg.h:99-102`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SymDim {
    pub tag: i16,
    pub size: u32,
}

/// `AMX_DBG_SYMBOL` (`amxdbg.h:88-97`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DebugSymbol {
    /// Data address, or frame-relative offset when `vclass` is local.
    pub address: u32,
    pub tag: i16,
    /// First code address where the symbol is in scope.
    pub codestart: u32,
    /// One past the last code address where the symbol is in scope.
    pub codeend: u32,
    /// One of the `IDENT_*` constants.
    pub ident: u8,
    /// One of the `VCLASS_*` constants.
    pub vclass: u8,
    pub name: String,
    /// Array dimensions; its length is the on-disk `dim` field.
    pub dims: Vec<SymDim>,
}

/// `AMX_DBG_TAG` (`amxdbg.h:104-107`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DebugTag {
    pub tag: i16,
    pub name: String,
}

/// `AMX_DBG_MACHINE` (`amxdbg.h:109-113`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DebugAutomaton {
    pub automaton: i16,
    /// Data address of the automaton's state variable.
    pub address: u32,
    pub name: String,
}

/// `AMX_DBG_STATE` (`amxdbg.h:115-119`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DebugState {
    pub state: i16,
    pub automaton: i16,
    pub name: String,
}

/// A whole `.dbg` chunk, decoded.
///
/// The header's `size`, the six count fields, `magic` and the two version
/// bytes are all derived on write, so they are not stored here; `flags` is
/// documented as unused and always written as 0.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DebugInfo {
    pub files: Vec<DebugFile>,
    pub lines: Vec<DebugLine>,
    pub symbols: Vec<DebugSymbol>,
    pub tags: Vec<DebugTag>,
    pub automatons: Vec<DebugAutomaton>,
    pub states: Vec<DebugState>,
}

impl DebugInfo {
    /// Total byte length of the chunk this value would serialise to, header
    /// included - the value written to `AMX_DBG_HDR.size`.
    pub fn byte_len(&self) -> usize {
        let names = |acc: usize, len: usize| acc + len + 1;
        let mut size = HDR_LEN;
        size += self
            .files
            .iter()
            .fold(0, |a, f| names(a, f.name.len()) + size_of::<u32>());
        size += self.lines.len() * LINE_LEN;
        size += self.symbols.iter().fold(0, |a, s| {
            names(a, s.name.len()) + SYMBOL_FIXED_LEN + s.dims.len() * SYMDIM_LEN
        });
        size += self.tags.iter().fold(0, |a, t| names(a, t.name.len()) + 2);
        size += self
            .automatons
            .iter()
            .fold(0, |a, m| names(a, m.name.len()) + 6);
        size += self.states.iter().fold(0, |a, s| names(a, s.name.len()) + 4);
        size
    }

    /// Serialise the chunk exactly as `append_dbginfo` (`sc6.c:958-1164`) does.
    pub fn write(&self) -> Result<Vec<u8>, DebugError> {
        let size = i32::try_from(self.byte_len()).map_err(|_| DebugError::ChunkTooLarge)?;
        let count = |v: usize, table: &'static str| {
            i16::try_from(v).map_err(|_| DebugError::TooManyEntries { table, count: v })
        };

        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&DBG_MAGIC.to_le_bytes());
        out.push(DBG_FILE_VERSION);
        out.push(DBG_AMX_VERSION);
        out.extend_from_slice(&0i16.to_le_bytes()); // flags, unused
        out.extend_from_slice(&count(self.files.len(), "file")?.to_le_bytes());
        out.extend_from_slice(&count(self.lines.len(), "line")?.to_le_bytes());
        out.extend_from_slice(&count(self.symbols.len(), "symbol")?.to_le_bytes());
        out.extend_from_slice(&count(self.tags.len(), "tag")?.to_le_bytes());
        out.extend_from_slice(&count(self.automatons.len(), "automaton")?.to_le_bytes());
        out.extend_from_slice(&count(self.states.len(), "state")?.to_le_bytes());

        for file in &self.files {
            out.extend_from_slice(&file.address.to_le_bytes());
            push_name(&mut out, &file.name)?;
        }
        for line in &self.lines {
            out.extend_from_slice(&line.address.to_le_bytes());
            out.extend_from_slice(&line.line.to_le_bytes());
        }
        for sym in &self.symbols {
            let dim = count(sym.dims.len(), "symbol dimension")?;
            out.extend_from_slice(&sym.address.to_le_bytes());
            out.extend_from_slice(&sym.tag.to_le_bytes());
            out.extend_from_slice(&sym.codestart.to_le_bytes());
            out.extend_from_slice(&sym.codeend.to_le_bytes());
            out.push(sym.ident);
            out.push(sym.vclass);
            out.extend_from_slice(&dim.to_le_bytes());
            push_name(&mut out, &sym.name)?;
            for d in &sym.dims {
                out.extend_from_slice(&d.tag.to_le_bytes());
                out.extend_from_slice(&d.size.to_le_bytes());
            }
        }
        for tag in &self.tags {
            out.extend_from_slice(&tag.tag.to_le_bytes());
            push_name(&mut out, &tag.name)?;
        }
        for machine in &self.automatons {
            out.extend_from_slice(&machine.automaton.to_le_bytes());
            out.extend_from_slice(&machine.address.to_le_bytes());
            push_name(&mut out, &machine.name)?;
        }
        for state in &self.states {
            out.extend_from_slice(&state.state.to_le_bytes());
            out.extend_from_slice(&state.automaton.to_le_bytes());
            push_name(&mut out, &state.name)?;
        }

        debug_assert_eq!(out.len(), size as usize);
        Ok(out)
    }

    /// Parse a debug chunk. `bytes` must start at the chunk; trailing bytes
    /// beyond `AMX_DBG_HDR.size` are ignored.
    pub fn parse(bytes: &[u8]) -> Result<DebugInfo, DebugError> {
        if bytes.len() < HDR_LEN {
            return Err(DebugError::Truncated {
                need: HDR_LEN,
                got: bytes.len(),
            });
        }
        let magic = u16::from_le_bytes([bytes[4], bytes[5]]);
        if magic != DBG_MAGIC {
            return Err(DebugError::BadMagic { found: magic });
        }
        let declared = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let declared = usize::try_from(declared).map_err(|_| DebugError::Truncated {
            need: HDR_LEN,
            got: bytes.len(),
        })?;
        if declared < HDR_LEN || declared > bytes.len() {
            return Err(DebugError::Truncated {
                need: declared.max(HDR_LEN),
                got: bytes.len(),
            });
        }
        let chunk = &bytes[..declared];

        let counts: Vec<usize> = (0..6)
            .map(|i| {
                let at = 10 + i * 2;
                i16::from_le_bytes([chunk[at], chunk[at + 1]]).max(0) as usize
            })
            .collect();

        let mut r = Cursor {
            bytes: chunk,
            at: HDR_LEN,
        };
        let mut info = DebugInfo::default();
        for _ in 0..counts[0] {
            info.files.push(DebugFile {
                address: r.u32()?,
                name: r.name()?,
            });
        }
        for _ in 0..counts[1] {
            info.lines.push(DebugLine {
                address: r.u32()?,
                line: r.u32()? as i32,
            });
        }
        for _ in 0..counts[2] {
            let address = r.u32()?;
            let tag = r.i16()?;
            let codestart = r.u32()?;
            let codeend = r.u32()?;
            let ident = r.u8()?;
            let vclass = r.u8()?;
            let dim = r.i16()?.max(0) as usize;
            let name = r.name()?;
            let mut dims = Vec::with_capacity(dim);
            for _ in 0..dim {
                dims.push(SymDim {
                    tag: r.i16()?,
                    size: r.u32()?,
                });
            }
            info.symbols.push(DebugSymbol {
                address,
                tag,
                codestart,
                codeend,
                ident,
                vclass,
                name,
                dims,
            });
        }
        for _ in 0..counts[3] {
            info.tags.push(DebugTag {
                tag: r.i16()?,
                name: r.name()?,
            });
        }
        for _ in 0..counts[4] {
            info.automatons.push(DebugAutomaton {
                automaton: r.i16()?,
                address: r.u32()?,
                name: r.name()?,
            });
        }
        for _ in 0..counts[5] {
            info.states.push(DebugState {
                state: r.i16()?,
                automaton: r.i16()?,
                name: r.name()?,
            });
        }

        if r.at != declared {
            return Err(DebugError::SizeMismatch {
                declared,
                actual: r.at,
            });
        }
        Ok(info)
    }
}

/// Append a serialised debug chunk to a finished AMX image and set
/// `AMX_FLAG_DEBUG` in its header, as `sc6.c:925` does.
///
/// `image` must be exactly the AMX image: its header `size` field has to equal
/// `image.len()`, since the chunk is placed at that offset and
/// [`crate::amx_image_len`] reads it back from there.
pub fn append_debug_chunk(image: &mut Vec<u8>, info: &DebugInfo) -> Result<(), DebugError> {
    if image.len() < HEADER_LEN {
        return Err(DebugError::NotAnAmxImage);
    }
    if u16::from_le_bytes([image[4], image[5]]) != MAGIC_AMX {
        return Err(DebugError::NotAnAmxImage);
    }
    let size = i32::from_le_bytes([image[0], image[1], image[2], image[3]]);
    if usize::try_from(size) != Ok(image.len()) {
        return Err(DebugError::NotAnAmxImage);
    }
    let chunk = info.write()?;
    let flags = i16::from_le_bytes([image[8], image[9]]) | FLAG_DEBUG;
    image[8..10].copy_from_slice(&flags.to_le_bytes());
    image.extend_from_slice(&chunk);
    Ok(())
}

fn push_name(out: &mut Vec<u8>, name: &str) -> Result<(), DebugError> {
    if name.as_bytes().contains(&0) {
        return Err(DebugError::InteriorNul);
    }
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    Ok(())
}

/// Sequential little-endian reader over the chunk.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DebugError> {
        let end = self.at.checked_add(n).ok_or(DebugError::Truncated {
            need: usize::MAX,
            got: self.bytes.len(),
        })?;
        let slice = self.bytes.get(self.at..end).ok_or(DebugError::Truncated {
            need: end,
            got: self.bytes.len(),
        })?;
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, DebugError> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> Result<i16, DebugError> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, DebugError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn name(&mut self) -> Result<String, DebugError> {
        let start = self.at;
        let len = self.bytes[start..]
            .iter()
            .position(|b| *b == 0)
            .ok_or(DebugError::UnterminatedString { at: start })?;
        let raw = self.take(len + 1)?;
        String::from_utf8(raw[..len].to_vec()).map_err(|_| DebugError::NameNotUtf8 { at: start })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DebugInfo {
        DebugInfo {
            files: vec![
                DebugFile {
                    address: 0,
                    name: "plugin.sma".into(),
                },
                DebugFile {
                    address: 0x120,
                    name: "include/amxmodx.inc".into(),
                },
            ],
            lines: vec![
                DebugLine {
                    address: 0,
                    line: 0,
                },
                DebugLine {
                    address: 0x18,
                    line: 12,
                },
                DebugLine {
                    address: 0x40,
                    line: 13,
                },
            ],
            symbols: vec![
                DebugSymbol {
                    address: 0x10,
                    tag: 0,
                    codestart: 0,
                    codeend: 0x200,
                    ident: IDENT_FUNCTION,
                    vclass: VCLASS_GLOBAL,
                    name: "plugin_init".into(),
                    dims: vec![],
                },
                DebugSymbol {
                    address: 0xfffffff0,
                    tag: 1,
                    codestart: 0x20,
                    codeend: 0x80,
                    ident: IDENT_ARRAY,
                    vclass: VCLASS_LOCAL,
                    name: "buffer".into(),
                    dims: vec![
                        SymDim { tag: 0, size: 4 },
                        SymDim { tag: 2, size: 33 },
                    ],
                },
            ],
            tags: vec![
                DebugTag {
                    tag: 0,
                    name: "_".into(),
                },
                DebugTag {
                    tag: 1,
                    name: "Float".into(),
                },
            ],
            automatons: vec![DebugAutomaton {
                automaton: 0,
                address: 0x400,
                name: String::new(),
            }],
            states: vec![DebugState {
                state: 1,
                automaton: 0,
                name: "idle".into(),
            }],
        }
    }

    /// A minimal but self-consistent AMX image header.
    fn fake_amx() -> Vec<u8> {
        let mut amx = vec![0u8; 64];
        amx[0..4].copy_from_slice(&64i32.to_le_bytes());
        amx[4..6].copy_from_slice(&MAGIC_AMX.to_le_bytes());
        amx
    }

    #[test]
    fn constants_match_amxdbg_h() {
        // amxdbg.h:76  #define AMX_DBG_MAGIC 0xf1ef
        assert_eq!(DBG_MAGIC, 0xf1ef);
        // amx.h:146 CUR_FILE_VERSION 8 / amx.h:148 MIN_AMX_VERSION 8, sc6.c:976-977
        assert_eq!(DBG_FILE_VERSION, 8);
        assert_eq!(DBG_AMX_VERSION, 8);
        // Packed struct sizes, amxdbg.h:63-102.
        assert_eq!(HDR_LEN, 4 + 2 + 1 + 1 + 7 * 2);
        assert_eq!(LINE_LEN, 4 + 4);
        assert_eq!(SYMBOL_FIXED_LEN, 4 + 2 + 4 + 4 + 1 + 1 + 2);
        assert_eq!(SYMDIM_LEN, 2 + 4);
        // amxdbg.h:131-137
        assert_eq!(
            (
                IDENT_VARIABLE,
                IDENT_REFERENCE,
                IDENT_ARRAY,
                IDENT_REFARRAY,
                IDENT_FUNCTION
            ),
            (1, 2, 3, 4, 9)
        );
    }

    #[test]
    fn header_bytes_match_the_spec() {
        let bytes = sample().write().unwrap();
        assert_eq!(&bytes[0..4], &(bytes.len() as i32).to_le_bytes());
        assert_eq!(&bytes[4..6], &[0xef, 0xf1]); // little-endian 0xf1ef
        assert_eq!(bytes[6], 8);
        assert_eq!(bytes[7], 8);
        assert_eq!(&bytes[8..10], &[0, 0]); // flags unused
        assert_eq!(&bytes[10..12], &2i16.to_le_bytes()); // files
        assert_eq!(&bytes[12..14], &3i16.to_le_bytes()); // lines
        assert_eq!(&bytes[14..16], &2i16.to_le_bytes()); // symbols
        assert_eq!(&bytes[16..18], &2i16.to_le_bytes()); // tags
        assert_eq!(&bytes[18..20], &1i16.to_le_bytes()); // automatons
        assert_eq!(&bytes[20..22], &1i16.to_le_bytes()); // states
    }

    #[test]
    fn byte_len_agrees_with_write() {
        let info = sample();
        assert_eq!(info.byte_len(), info.write().unwrap().len());
    }

    #[test]
    fn first_file_record_layout() {
        let bytes = sample().write().unwrap();
        assert_eq!(&bytes[HDR_LEN..HDR_LEN + 4], &0u32.to_le_bytes());
        assert_eq!(&bytes[HDR_LEN + 4..HDR_LEN + 15], b"plugin.sma\0");
    }

    #[test]
    fn round_trips_a_full_chunk() {
        let info = sample();
        assert_eq!(DebugInfo::parse(&info.write().unwrap()).unwrap(), info);
    }

    #[test]
    fn round_trips_an_empty_chunk() {
        let info = DebugInfo::default();
        let bytes = info.write().unwrap();
        assert_eq!(bytes.len(), HDR_LEN);
        assert_eq!(&bytes[8..22], &[0u8; 14]);
        assert_eq!(DebugInfo::parse(&bytes).unwrap(), info);
    }

    #[test]
    fn round_trips_names_with_shared_prefixes() {
        // No string pool: each record stores its own full copy.
        let info = DebugInfo {
            files: vec![
                DebugFile {
                    address: 1,
                    name: "a/b/c.inc".into(),
                },
                DebugFile {
                    address: 2,
                    name: "a/b/c.inc.bak".into(),
                },
                DebugFile {
                    address: 3,
                    name: "a/b".into(),
                },
                DebugFile {
                    address: 4,
                    name: String::new(),
                },
            ],
            tags: vec![
                DebugTag {
                    tag: 1,
                    name: "Float".into(),
                },
                DebugTag {
                    tag: 2,
                    name: "FloatEx".into(),
                },
            ],
            ..DebugInfo::default()
        };
        let bytes = info.write().unwrap();
        assert_eq!(bytes.len(), info.byte_len());
        assert_eq!(DebugInfo::parse(&bytes).unwrap(), info);
    }

    #[test]
    fn round_trips_a_symbol_with_many_dimensions() {
        let info = DebugInfo {
            symbols: vec![DebugSymbol {
                address: 4,
                tag: -1,
                codestart: 0,
                codeend: u32::MAX,
                ident: IDENT_REFARRAY,
                vclass: VCLASS_STATIC,
                name: "grid".into(),
                dims: (0..8)
                    .map(|i| SymDim {
                        tag: i,
                        size: 1 + i as u32,
                    })
                    .collect(),
            }],
            ..DebugInfo::default()
        };
        assert_eq!(DebugInfo::parse(&info.write().unwrap()).unwrap(), info);
    }

    #[test]
    fn parse_ignores_bytes_past_the_declared_size() {
        let mut bytes = sample().write().unwrap();
        let len = bytes.len();
        bytes.extend_from_slice(&[0xAB; 32]);
        assert_eq!(DebugInfo::parse(&bytes).unwrap(), sample());
        assert_eq!(&bytes[0..4], &(len as i32).to_le_bytes());
    }

    #[test]
    fn rejects_a_bad_magic() {
        let mut bytes = sample().write().unwrap();
        bytes[4] = 0;
        assert_eq!(
            DebugInfo::parse(&bytes),
            Err(DebugError::BadMagic { found: 0xf100 })
        );
    }

    #[test]
    fn rejects_a_truncated_chunk() {
        assert!(matches!(
            DebugInfo::parse(&[]),
            Err(DebugError::Truncated { .. })
        ));
        let bytes = sample().write().unwrap();
        assert!(matches!(
            DebugInfo::parse(&bytes[..bytes.len() - 3]),
            Err(DebugError::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_a_size_that_does_not_match_the_tables() {
        let mut bytes = sample().write().unwrap();
        bytes.push(0); // one byte the tables do not account for
        let grown = bytes.len();
        bytes[0..4].copy_from_slice(&(grown as i32).to_le_bytes());
        assert_eq!(
            DebugInfo::parse(&bytes),
            Err(DebugError::SizeMismatch {
                declared: grown,
                actual: grown - 1
            })
        );

        // A size that stops short of the tables is a truncation.
        let mut short = sample().write().unwrap();
        short[0..4].copy_from_slice(&(HDR_LEN as i32 + 2).to_le_bytes());
        assert!(matches!(
            DebugInfo::parse(&short),
            Err(DebugError::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_an_unterminated_name() {
        let mut info = DebugInfo::default();
        info.tags.push(DebugTag {
            tag: 1,
            name: "abc".into(),
        });
        let mut bytes = info.write().unwrap();
        let last = bytes.len() - 1;
        bytes[last] = b'd'; // clobber the terminator
        assert!(matches!(
            DebugInfo::parse(&bytes),
            Err(DebugError::UnterminatedString { .. })
        ));
    }

    #[test]
    fn rejects_an_interior_nul_in_a_name() {
        let info = DebugInfo {
            tags: vec![DebugTag {
                tag: 0,
                name: "a\0b".into(),
            }],
            ..DebugInfo::default()
        };
        assert_eq!(info.write(), Err(DebugError::InteriorNul));
    }

    #[test]
    fn append_sets_the_debug_flag_and_places_the_chunk_at_size() {
        let mut image = fake_amx();
        let at = image.len();
        append_debug_chunk(&mut image, &sample()).unwrap();
        assert_eq!(i16::from_le_bytes([image[8], image[9]]) & FLAG_DEBUG, 2);
        assert_eq!(DebugInfo::parse(&image[at..]).unwrap(), sample());
        // The container writer must now cover image + chunk.
        assert_eq!(crate::amx_image_len(&image).unwrap(), image.len());
    }

    #[test]
    fn append_rejects_a_non_image() {
        assert_eq!(
            append_debug_chunk(&mut vec![0u8; 8], &DebugInfo::default()),
            Err(DebugError::NotAnAmxImage)
        );
        let mut wrong_size = fake_amx();
        wrong_size.push(0);
        assert_eq!(
            append_debug_chunk(&mut wrong_size, &DebugInfo::default()),
            Err(DebugError::NotAnAmxImage)
        );
    }
}
