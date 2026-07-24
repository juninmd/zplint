//! AMX image construction: the `AMX_HEADER`, the metadata tables, the name table,
//! and the code and data segments.
//!
//! Ported from `assemble()` in the AMX Mod X compiler's `libpc300/sc6.c` (Pawn,
//! ITB CompuPhase, zlib-style licence - see ATTRIBUTION.md). Line references below
//! are to that file unless another is named. The header field *offsets* are not
//! restated here: the fields are written in `AMX_HEADER` declaration order, which
//! is what [`zpc_amxx::amx_header`] already documents and parses.
//!
//! ## Layout
//!
//! ```text
//! 0                       AMX_HEADER                     (56 bytes)
//! hdr.publics             publics    [numpublics]        (8 bytes each)
//! hdr.natives             natives    [numnatives]
//! hdr.libraries           libraries  [numlibraries]
//! hdr.pubvars             pubvars    [numpubvars]
//! hdr.tags                tags       [numtags]
//! hdr.nametable           int16 max-name-length, then NUL-terminated names
//!                         padding to a cell boundary
//! hdr.cod                 code segment
//! hdr.dat                 data segment
//! hdr.hea                 (= end of file) heap start
//! hdr.stp                 stack top
//! ```
//!
//! Each table is `defsize` bytes per entry, `defsize == sizeof(AMX_FUNCSTUBNT) == 8`
//! (sc6.c:680; amx.h:225-228 - `ucell address; ucell nameofs;`). The record holds an
//! *offset* to the name, not the name itself, which is why the name table is a
//! separate region shared by all five tables.

use crate::assemble::CELL;
use zpc_amxx::amx_header::{HEADER_LEN, MAGIC_AMX};

/// `sizeof(AMX_FUNCSTUBNT)`, written to `hdr.defsize` (sc6.c:680, amx.h:225-228).
pub const DEFSIZE: i16 = 8;

/// `CUR_FILE_VERSION` (amx.h:146), written to `hdr.file_version` (sc6.c:673).
pub const FILE_VERSION: u8 = 8;

/// `MIN_AMX_VERSION` (amx.h:148), written to `hdr.amx_version` (sc6.c:674).
pub const AMX_VERSION: u8 = 8;

/// `sNAMEMAX` (amx.h:218). Written as an `int16_t` at the head of the name table
/// (sc6.c:836-843) and the longest name any table entry may carry.
pub const NAME_MAX: usize = 63;

/// `sc_dataalign`. Defaults to `sizeof(cell)`; the header plus name table is padded
/// up to this so the code segment - and therefore the data segment and the stack
/// top - are all cell aligned (sc6.c:660-668).
pub const DATA_ALIGN: usize = CELL as usize;

/// `AMX_FLAG_DEBUG` (amx.h:322).
pub const FLAG_DEBUG: i16 = 0x02;
/// `AMX_FLAG_NOCHECKS` (amx.h:325): set when compiling without debug info
/// (sc6.c:678-679).
pub const FLAG_NOCHECKS: i16 = 0x10;

/// One entry of a metadata table: an address plus a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// `AMX_FUNCSTUBNT.address`. Its meaning differs per table - see [`Image`].
    pub address: i32,
}

impl Symbol {
    pub fn new(name: impl Into<String>, address: i32) -> Symbol {
        Symbol {
            name: name.into(),
            address,
        }
    }
}

/// Everything needed to lay out an `.amx` image.
///
/// The five tables carry different address meanings and different orderings; both
/// are enforced by [`Image::build`], because the AMX runtime binary-searches three
/// of them and a wrong order fails silently at load time rather than loudly here.
#[derive(Clone, Debug, Default)]
pub struct Image {
    /// The assembled code segment (`cod..dat`).
    pub code: Vec<u8>,
    /// The initialised data segment (`dat..hea`), `glb_declared` cells (sc6.c:689).
    pub data: Vec<u8>,
    /// `sc_stksize`: the combined stack/heap size in cells (sc6.c:690).
    pub stack_cells: i32,
    /// Code address of `main`, or `None` for a plugin with no entry point.
    /// Becomes `hdr.cip` (sc6.c:691); `mainaddr` is preset to -1 (sc6.c:609).
    pub main_address: Option<i32>,
    /// `hdr.flags` (sc6.c:675-679).
    pub flags: i16,
    /// Public functions. `address` is the function's code address (sc6.c:707).
    /// **Sorted by name** on build.
    pub publics: Vec<Symbol>,
    /// Used natives, **in native-id order** - i.e. index in this vector *is* the id
    /// the `sysreq.c` operand refers to. `address` is forced to 0 (sc6.c:756).
    pub natives: Vec<Symbol>,
    /// Required libraries, in declaration order. `address` is forced to 0
    /// (sc6.c:778).
    pub libraries: Vec<Symbol>,
    /// Public variables. `address` is the data offset (sc6.c:801). **Sorted by
    /// name** on build.
    pub pubvars: Vec<Symbol>,
    /// Public tags. `address` is `constptr->value & TAGMASK`, the tag id
    /// (sc6.c:821). **Sorted by tag id** on build.
    pub tags: Vec<Symbol>,
}

/// Why the image could not be laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// A segment length is not a whole number of cells. `write_encoded` asserts
    /// `pc_lengthbin(fbin) % sizeof(cell) == 0` (sc6.c:236).
    NotCellAligned { segment: &'static str, len: usize },
    /// A name exceeds `sNAMEMAX` (amx.h:218, asserted at sc6.c:630 and :752).
    NameTooLong { name: String },
    /// A name contains a NUL, which would truncate it in the name table.
    NameHasNul { name: String },
    /// Two tag entries share an id, so the runtime's binary search over tag ids
    /// (amx.cpp:1462-1473 asserts strict ascent) would be ill-defined.
    DuplicateTagId { id: i32 },
    /// Two entries of a name-sorted table share a name, so `amx_FindPublic`'s
    /// binary search (amx.cpp:1320) could not distinguish them.
    DuplicateName { table: &'static str, name: String },
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn check_names(table: &[Symbol]) -> Result<(), ImageError> {
    for s in table {
        if s.name.len() > NAME_MAX {
            return Err(ImageError::NameTooLong {
                name: s.name.clone(),
            });
        }
        if s.name.as_bytes().contains(&0) {
            return Err(ImageError::NameHasNul {
                name: s.name.clone(),
            });
        }
    }
    Ok(())
}

fn reject_duplicate_names(table: &[Symbol], name: &'static str) -> Result<(), ImageError> {
    for pair in table.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(ImageError::DuplicateName {
                table: name,
                name: pair[0].name.clone(),
            });
        }
    }
    Ok(())
}

impl Image {
    /// An image holding just this code segment: no tables, no data, a 4096-cell
    /// stack and no entry point.
    pub fn new(code: Vec<u8>) -> Image {
        Image {
            code,
            stack_cells: 4096,
            flags: FLAG_NOCHECKS,
            ..Image::default()
        }
    }

    /// Lay out the complete `.amx` image.
    ///
    /// Sorting, and why each table is sorted the way it is:
    ///
    /// * **publics, pubvars** - by name, `strcmp` order (raw bytes). `sc6.c` walks
    ///   `glbtab` (sc6.c:702, :796), which `add_symbol` keeps in `strcmp` order
    ///   (sc2.c:2507-2509). The runtime relies on it: `amx_FindPublic`
    ///   (amx.cpp:1320) and `amx_FindPubVar` (amx.cpp:1377) are binary searches
    ///   over `strcmp`.
    /// * **natives** - by native id, which is the caller's vector order.
    ///   `sc6.c:722-732` says so explicitly: "The native functions must be written
    ///   in sorted order. (They are sorted on their id, not on their name)". The
    ///   `sysreq.c` operand is an index into this table, so reordering it would
    ///   redirect every native call.
    /// * **tags** - by tag id ascending, since `amx_FindTagId` binary-searches on
    ///   the id and asserts the table is sorted on it (amx.cpp:1457-1474).
    /// * **libraries** - not sorted. `sc6.c:775` walks `libname_tab` in order and
    ///   the runtime only ever scans this table linearly.
    ///
    /// Names are written into the name table in table order - publics, natives,
    /// libraries, pubvars, tags - and are not deduplicated: `sc6.c` appends
    /// `strlen(name)+1` per entry as it writes each table (sc6.c:717, :766, :788,
    /// :811, :831).
    pub fn build(&self) -> Result<Vec<u8>, ImageError> {
        if !self.code.len().is_multiple_of(CELL as usize) {
            return Err(ImageError::NotCellAligned {
                segment: "code",
                len: self.code.len(),
            });
        }
        if !self.data.len().is_multiple_of(CELL as usize) {
            return Err(ImageError::NotCellAligned {
                segment: "data",
                len: self.data.len(),
            });
        }

        let mut publics = self.publics.clone();
        let mut pubvars = self.pubvars.clone();
        let mut tags = self.tags.clone();
        publics.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        pubvars.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        tags.sort_by_key(|t| t.address);
        reject_duplicate_names(&publics, "publics")?;
        reject_duplicate_names(&pubvars, "pubvars")?;
        for pair in tags.windows(2) {
            if pair[0].address == pair[1].address {
                return Err(ImageError::DuplicateTagId {
                    id: pair[0].address,
                });
            }
        }

        // Natives and libraries keep the caller's order but never carry an address.
        let natives: Vec<Symbol> = self
            .natives
            .iter()
            .map(|s| Symbol::new(s.name.clone(), 0))
            .collect();
        let libraries: Vec<Symbol> = self
            .libraries
            .iter()
            .map(|s| Symbol::new(s.name.clone(), 0))
            .collect();

        for t in [&publics, &natives, &libraries, &pubvars, &tags] {
            check_names(t)?;
        }

        // sc6.c:605 - the name table opens with an int16_t, then one NUL-terminated
        // name per entry of every table (sc6.c:633, :645, :656).
        let order = [&publics, &natives, &libraries, &pubvars, &tags];
        let nametablesize: usize = size_of::<i16>()
            + order
                .iter()
                .flat_map(|t| t.iter())
                .map(|s| s.name.len() + 1)
                .sum::<usize>();

        // sc6.c:666-668
        let mut padding = DATA_ALIGN - (HEADER_LEN + nametablesize) % DATA_ALIGN;
        if padding == DATA_ALIGN {
            padding = 0;
        }

        let stub = DEFSIZE as usize;
        // sc6.c:681-692
        let h_publics = HEADER_LEN;
        let h_natives = h_publics + publics.len() * stub;
        let h_libraries = h_natives + natives.len() * stub;
        let h_pubvars = h_libraries + libraries.len() * stub;
        let h_tags = h_pubvars + pubvars.len() * stub;
        let h_nametable = h_tags + tags.len() * stub;
        let cod = h_nametable + nametablesize + padding;
        let dat = cod + self.code.len();
        let hea = dat + self.data.len();
        let stp = hea + self.stack_cells as usize * CELL as usize;
        let cip = self.main_address.unwrap_or(-1);
        // sc6.c:692 - without compact encoding hdr.size is hea, which is also the
        // exact image length.
        let size = hea;

        let mut out = Vec::with_capacity(stp);
        // AMX_HEADER, in declaration order (amx.h; offsets in zpc_amxx::amx_header).
        put_i32(&mut out, size as i32);
        out.extend_from_slice(&MAGIC_AMX.to_le_bytes()); // sc6.c:672
        out.push(FILE_VERSION); // sc6.c:673
        out.push(AMX_VERSION); // sc6.c:674
        out.extend_from_slice(&self.flags.to_le_bytes()); // sc6.c:675-679
        out.extend_from_slice(&DEFSIZE.to_le_bytes()); // sc6.c:680
        put_i32(&mut out, cod as i32);
        put_i32(&mut out, dat as i32);
        put_i32(&mut out, hea as i32);
        put_i32(&mut out, stp as i32);
        put_i32(&mut out, cip);
        put_i32(&mut out, h_publics as i32);
        put_i32(&mut out, h_natives as i32);
        put_i32(&mut out, h_libraries as i32);
        put_i32(&mut out, h_pubvars as i32);
        put_i32(&mut out, h_tags as i32);
        put_i32(&mut out, h_nametable as i32);
        debug_assert_eq!(out.len(), HEADER_LEN);

        // sc6.c:695-697 zeroes everything up to hdr.cod and then "seeks" back to
        // fill it in; we simply append in offset order instead.
        for table in order {
            for s in table {
                put_i32(&mut out, s.address);
                // nameofs is patched below, once every name has a home.
                put_i32(&mut out, 0);
            }
        }
        debug_assert_eq!(out.len(), h_nametable);

        // sc6.c:836-843 - the "maximum name length" field opens the name table.
        out.extend_from_slice(&(NAME_MAX as i16).to_le_bytes());
        let mut nameofs = h_nametable + size_of::<i16>(); // sc6.c:698
        let mut stub_at = h_publics;
        for table in order {
            for s in table {
                let ofs = (nameofs as i32).to_le_bytes();
                out[stub_at + 4..stub_at + 8].copy_from_slice(&ofs);
                stub_at += stub;
                out.extend_from_slice(s.name.as_bytes());
                out.push(0);
                nameofs += s.name.len() + 1;
            }
        }
        // sc6.c:837 - assert(nameofs==hdr.nametable+nametablesize)
        debug_assert_eq!(nameofs, h_nametable + nametablesize);

        out.resize(cod, 0); // the alignment padding
        out.extend_from_slice(&self.code);
        out.extend_from_slice(&self.data);
        debug_assert_eq!(out.len(), size);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::{Item, Operand, assemble};
    use crate::disasm::{dangling_targets, disassemble};
    use crate::opcode::Opcode;
    use zpc_amxx::amx_header::AmxHeader;

    fn names_at(amx: &[u8], mut ofs: usize, n: usize) -> Vec<String> {
        let mut v = Vec::new();
        for _ in 0..n {
            let end = ofs + amx[ofs..].iter().position(|b| *b == 0).unwrap();
            v.push(String::from_utf8(amx[ofs..end].to_vec()).unwrap());
            ofs = end + 1;
        }
        v
    }

    /// Read one table's (address, name) entries.
    fn table(amx: &[u8], start: usize, count: usize) -> Vec<(i32, String)> {
        (0..count)
            .map(|i| {
                let at = start + i * DEFSIZE as usize;
                let addr = i32::from_le_bytes(amx[at..at + 4].try_into().unwrap());
                let ofs =
                    i32::from_le_bytes(amx[at + 4..at + 8].try_into().unwrap()) as usize;
                (addr, names_at(amx, ofs, 1).pop().unwrap())
            })
            .collect()
    }

    #[test]
    fn header_fields_match_hand_computed_values() {
        // Tiny program: proc; retn  -> 8 bytes of code. One public "main" at 0.
        let code = assemble(&[Item::op0(Opcode::Proc), Item::op0(Opcode::Retn)])
            .unwrap()
            .code;
        assert_eq!(code.len(), 8);

        let img = Image {
            code,
            data: 3i32.to_le_bytes().to_vec(), // one cell of globals
            stack_cells: 4096,
            main_address: Some(0),
            flags: FLAG_NOCHECKS,
            publics: vec![Symbol::new("main", 0)],
            ..Image::default()
        };
        let amx = img.build().unwrap();
        let h = AmxHeader::parse(&amx).unwrap();

        // Hand computation:
        //   publics   = 56                      (right after the header)
        //   natives   = 56 + 1*8 = 64
        //   libraries = pubvars = tags = nametable = 64  (all empty)
        //   nametablesize = 2 + len("main")+1 = 7
        //   padding   = 4 - (56 + 7) % 4 = 4 - 3 = 1
        //   cod       = 64 + 7 + 1 = 72
        //   dat       = 72 + 8 = 80
        //   hea       = 80 + 4 = 84
        //   stp       = 84 + 4096*4 = 16468
        assert_eq!(h.publics, 56);
        assert_eq!(h.natives, 64);
        assert_eq!(h.libraries, 64);
        assert_eq!(h.pubvars, 64);
        assert_eq!(h.tags, 64);
        assert_eq!(h.nametable, 64);
        assert_eq!(h.cod, 72);
        assert_eq!(h.dat, 80);
        assert_eq!(h.hea, 84);
        assert_eq!(h.stp, 16468);
        assert_eq!(h.cip, 0);
        assert_eq!(h.size, 84);
        assert_eq!(h.size as usize, amx.len());
        assert_eq!(h.defsize, 8);
        assert_eq!(h.magic, MAGIC_AMX);
        assert_eq!(h.file_version, 8);
        assert_eq!(h.amx_version, 8);
        assert!(!h.has_debug());

        // The name table opens with sNAMEMAX and cod is cell aligned.
        assert_eq!(
            i16::from_le_bytes(amx[64..66].try_into().unwrap()),
            NAME_MAX as i16
        );
        assert_eq!(h.cod % 4, 0);
    }

    #[test]
    fn no_padding_when_already_aligned() {
        // nametablesize = 2 + len("ab")+1 = 5; 56+5 = 61; padding = 4 - 1 = 3.
        let a = Image {
            publics: vec![Symbol::new("ab", 0)],
            ..Image::new(Vec::new())
        }
        .build()
        .unwrap();
        assert_eq!(AmxHeader::parse(&a).unwrap().cod, 56 + 8 + 5 + 3);

        // nametablesize = 2 + len("a")+1 = 4; 56+4 = 60, already aligned: no padding.
        let b = Image {
            publics: vec![Symbol::new("a", 0)],
            ..Image::new(Vec::new())
        }
        .build()
        .unwrap();
        assert_eq!(AmxHeader::parse(&b).unwrap().cod, 56 + 8 + 4);
    }

    #[test]
    fn tables_are_sorted_and_named_correctly() {
        let img = Image {
            publics: vec![Symbol::new("zulu", 40), Symbol::new("alpha", 8)],
            // native ids are positions: server_print is 0, client_print is 1
            natives: vec![Symbol::new("server_print", 0), Symbol::new("client_print", 0)],
            libraries: vec![Symbol::new("zlib", 0), Symbol::new("alib", 0)],
            pubvars: vec![Symbol::new("gz", 12), Symbol::new("ga", 4)],
            tags: vec![Symbol::new("Float", 7), Symbol::new("Bool", 2)],
            ..Image::new(Vec::new())
        };
        let amx = img.build().unwrap();
        let h = AmxHeader::parse(&amx).unwrap();

        // publics / pubvars: strcmp order, addresses travel with the name
        assert_eq!(
            table(&amx, h.publics as usize, 2),
            vec![(8, "alpha".into()), (40, "zulu".into())]
        );
        assert_eq!(
            table(&amx, h.pubvars as usize, 2),
            vec![(4, "ga".into()), (12, "gz".into())]
        );
        // natives: id order preserved, address always 0
        assert_eq!(
            table(&amx, h.natives as usize, 2),
            vec![(0, "server_print".into()), (0, "client_print".into())]
        );
        // libraries: declaration order, address always 0
        assert_eq!(
            table(&amx, h.libraries as usize, 2),
            vec![(0, "zlib".into()), (0, "alib".into())]
        );
        // tags: ascending tag id
        assert_eq!(
            table(&amx, h.tags as usize, 2),
            vec![(2, "Bool".into()), (7, "Float".into())]
        );

        // Names appear in table order in the name table.
        assert_eq!(
            names_at(&amx, h.nametable as usize + 2, 10),
            vec![
                "alpha",
                "zulu",
                "server_print",
                "client_print",
                "zlib",
                "alib",
                "ga",
                "gz",
                "Bool",
                "Float"
            ]
        );
    }

    #[test]
    fn a_complete_image_parses_and_disassembles() {
        let items = [
            Item::Label(0),
            Item::op0(Opcode::Proc),
            Item::op(Opcode::PushC, [Operand::Num(1)]),
            Item::op(Opcode::SysreqC, [Operand::Num(0)]),
            Item::op(Opcode::Stack, [Operand::Num(8)]),
            Item::op(Opcode::Jzer, [Operand::Label(1)]),
            Item::op(Opcode::Switch, [Operand::Label(2)]),
            Item::Label(1),
            Item::op0(Opcode::ZeroPri),
            Item::op0(Opcode::Retn),
            Item::Label(2),
            Item::CaseTbl {
                default: Operand::Label(1),
                cases: vec![(1, Operand::Label(0)), (2, Operand::Label(1))],
            },
        ];
        let asm = assemble(&items).unwrap();
        let img = Image {
            main_address: Some(asm.labels[&0] as i32),
            publics: vec![Symbol::new("plugin_init", 0)],
            natives: vec![Symbol::new("server_print", 0)],
            libraries: vec![Symbol::new("float", 0)],
            pubvars: vec![Symbol::new("g_state", 0)],
            tags: vec![Symbol::new("Float", 1)],
            data: vec![0; 8],
            ..Image::new(asm.code.clone())
        };

        let amx = img.build().unwrap();
        let h = AmxHeader::parse(&amx).unwrap();
        let code = h.code(&amx).unwrap();
        assert_eq!(code, &asm.code[..]);

        let d = disassemble(code).unwrap();
        assert!(dangling_targets(&d).is_empty(), "{:?}", dangling_targets(&d));
        let got: Vec<Opcode> = d.iter().map(|i| i.opcode).collect();
        assert_eq!(
            got,
            vec![
                Opcode::Proc,
                Opcode::PushC,
                Opcode::SysreqC,
                Opcode::Stack,
                Opcode::Jzer,
                Opcode::Switch,
                Opcode::ZeroPri,
                Opcode::Retn,
                Opcode::Casetbl,
            ]
        );
    }

    #[test]
    fn rejects_a_misaligned_segment() {
        assert_eq!(
            Image::new(vec![0, 1, 2]).build().unwrap_err(),
            ImageError::NotCellAligned {
                segment: "code",
                len: 3
            }
        );
        assert_eq!(
            Image {
                data: vec![0; 5],
                ..Image::new(Vec::new())
            }
            .build()
            .unwrap_err(),
            ImageError::NotCellAligned {
                segment: "data",
                len: 5
            }
        );
    }

    #[test]
    fn rejects_bad_names_and_ambiguous_tables() {
        assert!(matches!(
            Image {
                publics: vec![Symbol::new("x".repeat(64), 0)],
                ..Image::new(Vec::new())
            }
            .build(),
            Err(ImageError::NameTooLong { .. })
        ));
        assert!(matches!(
            Image {
                natives: vec![Symbol::new("a\0b", 0)],
                ..Image::new(Vec::new())
            }
            .build(),
            Err(ImageError::NameHasNul { .. })
        ));
        assert!(matches!(
            Image {
                publics: vec![Symbol::new("dup", 0), Symbol::new("dup", 8)],
                ..Image::new(Vec::new())
            }
            .build(),
            Err(ImageError::DuplicateName {
                table: "publics",
                ..
            })
        ));
        assert_eq!(
            Image {
                tags: vec![Symbol::new("A", 3), Symbol::new("B", 3)],
                ..Image::new(Vec::new())
            }
            .build()
            .unwrap_err(),
            ImageError::DuplicateTagId { id: 3 }
        );
    }

    #[test]
    fn an_empty_image_is_still_valid() {
        let amx = Image::new(Vec::new()).build().unwrap();
        let h = AmxHeader::parse(&amx).unwrap();
        // nametablesize = 2, padding = 4 - 58 % 4 = 2, cod = 56 + 2 + 2 = 60
        assert_eq!(h.cod, 60);
        assert_eq!(h.dat, 60);
        assert_eq!(h.hea, 60);
        assert_eq!(h.cip, -1);
        assert!(h.code(&amx).unwrap().is_empty());
        assert!(disassemble(h.code(&amx).unwrap()).unwrap().is_empty());
    }
}
