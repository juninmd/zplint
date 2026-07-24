//! The assembler: an instruction/label stream in, an AMX code segment out.
//!
//! Ported from `assemble()` and the `do_*` emit functions in the AMX Mod X
//! compiler's `libpc300/sc6.c` (Pawn, ITB CompuPhase, zlib-style licence - see
//! ATTRIBUTION.md). Where `sc6.c` re-reads a textual `.asm` file twice, this takes
//! an already-parsed [`Item`] stream and walks it twice; the algorithm is the same.
//!
//! Encoding rules, all from `sc6.c`:
//!
//! * The opcode occupies a full cell, not a byte: `parm0` (sc6.c:254) writes
//!   `write_encoded(fbin,&opcode,1)`, and `write_encoded` (sc6.c:192) writes
//!   `sizeof(cell)` bytes per item when compaction is off. `AMX_FLAG_BYTEOPC` is
//!   never set by this compiler.
//! * Operands follow as whole cells, little-endian on the x86 targets amxxpc
//!   supports (`aligncell` is a no-op unless `BYTE_ORDER==BIG_ENDIAN`, sc6.c:170).
//! * `casetbl` (sc6.c:401) is `parm0` - a bare opcode. The table after it is written
//!   by `do_case` (sc6.c:367), which emits two cells and *no* opcode per record; the
//!   first record is `(count, default)`. See [`Item::CaseTbl`].
//!
//! Compact encoding (`sc_compress` / `AMX_FLAG_COMPACT`) is deliberately not ported:
//! amxxpc disables it, and the `.amxx` container compresses the whole image anyway.

use crate::opcode::{Opcode, Operands};
use std::collections::BTreeMap;

/// Cell size in bytes. Only 32-bit cells exist in AMX Mod X (`PAWN_CELL_SIZE 32`,
/// amx.h:150).
pub const CELL: u32 = 4;

/// A label identifier. `sc6.c` spells these `l.<hex>` in the assembly text and
/// indexes `lbltab[]` with them (sc6.c:866-869); here they are plain integers.
pub type LabelId = u32;

/// An instruction operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand {
    /// A literal cell, written as-is.
    Num(i32),
    /// A code address, resolved from the label table in the second pass
    /// (`do_call` / `do_jump` / `do_switch`, sc6.c:290-362).
    Label(LabelId),
}

/// One element of the assembly stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// Defines `label` at the current code address (sc6.c:866-869).
    Label(LabelId),
    /// A normal instruction with its fixed number of operand cells.
    Op {
        opcode: Opcode,
        operands: Vec<Operand>,
    },
    /// A `casetbl` plus the whole table that follows it.
    ///
    /// Modelled as one item because `sc6.c` cannot emit the two independently and
    /// stay valid: `ffcase(..., newtable=TRUE)` (sc4.c:729) writes the `casetbl`
    /// opcode and immediately the `(count, default)` record, then one record per
    /// case. Encoded as `2 + 2n` cells after the opcode.
    ///
    /// `cases` must be sorted by ascending value: the compiler keeps the case list
    /// sorted "so that advanced abstract machines can sift the case table with a
    /// binary search" (sc1.c:5218-5221).
    CaseTbl {
        default: Operand,
        cases: Vec<(i32, Operand)>,
    },
}

impl Item {
    /// Convenience constructor for [`Item::Op`].
    pub fn op(opcode: Opcode, operands: impl Into<Vec<Operand>>) -> Item {
        Item::Op {
            opcode,
            operands: operands.into(),
        }
    }

    /// Convenience constructor for an operand-less instruction.
    pub fn op0(opcode: Opcode) -> Item {
        Item::Op {
            opcode,
            operands: Vec::new(),
        }
    }
}

/// Why assembly failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsmError {
    /// The same label id was defined twice.
    DuplicateLabel { label: LabelId, first: u32 },
    /// A label was referenced but never defined. `sc6.c` asserts this away
    /// (`assert(i>=0 && i<sc_labnum)`); an assembler that silently emitted a wrong
    /// address would produce a plugin that jumps into the middle of an instruction.
    UndefinedLabel { label: LabelId },
    /// The operand count does not match the opcode's arity.
    BadArity {
        opcode: Opcode,
        expected: u32,
        got: usize,
    },
    /// A [`Operand::Label`] was used where the AMX expects a plain number. Only the
    /// single operand of a jump/call/switch is relocated (sc6.c `do_call`,
    /// `do_jump`, `do_switch`); every other operand is written verbatim.
    LabelNotAnAddress { opcode: Opcode, index: usize },
    /// `casetbl` was passed as [`Item::Op`], or a variable-length debug record was
    /// requested. Use [`Item::CaseTbl`] for the former; the latter is not emitted by
    /// this compiler (the rows are commented out of `opcodelist[]`, sc6.c:416).
    UnsupportedOpcode { opcode: Opcode },
    /// Case values are not strictly ascending (sc1.c:5218).
    UnsortedCaseTable { at: usize },
}

/// A successfully assembled code segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assembled {
    /// The code segment bytes, i.e. exactly the `cod..dat` range of the image.
    pub code: Vec<u8>,
    /// Resolved address of every label, relative to `cod`. This is `lbltab[]`.
    pub labels: BTreeMap<LabelId, u32>,
}

/// Encoded size in bytes of one item, opcode cell included.
fn item_size(item: &Item) -> u32 {
    match item {
        Item::Label(_) => 0,
        Item::Op { opcode, .. } => {
            CELL + CELL * opcode.operands().count().unwrap_or(0)
        }
        // opcode + (count, default) + one (value, address) pair per case
        Item::CaseTbl { cases, .. } => CELL + 2 * CELL * (1 + cases.len() as u32),
    }
}

/// Reject items the encoder cannot represent, before any address is assigned.
fn validate(item: &Item) -> Result<(), AsmError> {
    match item {
        Item::Label(_) => Ok(()),
        Item::Op { opcode, operands } => {
            let expected = match opcode.operands() {
                Operands::CaseTable | Operands::Variable => {
                    return Err(AsmError::UnsupportedOpcode { opcode: *opcode });
                }
                fixed => fixed.count().unwrap_or(0),
            };
            if operands.len() as u32 != expected {
                return Err(AsmError::BadArity {
                    opcode: *opcode,
                    expected,
                    got: operands.len(),
                });
            }
            for (i, op) in operands.iter().enumerate() {
                // Only operand 0 of a relocated instruction may be a label.
                if matches!(op, Operand::Label(_)) && !(opcode.is_jump() && i == 0) {
                    return Err(AsmError::LabelNotAnAddress {
                        opcode: *opcode,
                        index: i,
                    });
                }
            }
            Ok(())
        }
        Item::CaseTbl { cases, .. } => {
            for i in 1..cases.len() {
                if cases[i].0 <= cases[i - 1].0 {
                    return Err(AsmError::UnsortedCaseTable { at: i });
                }
            }
            Ok(())
        }
    }
}

/// Assemble an item stream into a code segment.
///
/// Two passes, exactly as `sc6.c` does it (sc6.c:846 "First pass: relocate all
/// labels", sc6.c:888 "Second pass"): the first assigns an address to every label
/// by accumulating instruction sizes, the second emits bytes with every label
/// reference replaced by its address. A single pass cannot work because forward
/// jumps reference labels that do not have an address yet.
pub fn assemble(items: &[Item]) -> Result<Assembled, AsmError> {
    // Pass 1: sizes and label addresses.
    let mut labels: BTreeMap<LabelId, u32> = BTreeMap::new();
    let mut code_index: u32 = 0;
    for item in items {
        validate(item)?;
        if let Item::Label(id) = item
            && let Some(first) = labels.insert(*id, code_index)
        {
            return Err(AsmError::DuplicateLabel {
                label: *id,
                first,
            });
        }
        code_index += item_size(item);
    }

    // Pass 2: emit.
    let resolve = |op: &Operand| -> Result<i32, AsmError> {
        match op {
            Operand::Num(v) => Ok(*v),
            Operand::Label(id) => labels
                .get(id)
                .map(|a| *a as i32)
                .ok_or(AsmError::UndefinedLabel { label: *id }),
        }
    };

    fn push(v: i32, code: &mut Vec<u8>) {
        code.extend_from_slice(&v.to_le_bytes());
    }

    let mut code = Vec::with_capacity(code_index as usize);
    for item in items {
        match item {
            Item::Label(_) => {}
            Item::Op { opcode, operands } => {
                push(i32::from(opcode.raw()), &mut code);
                for op in operands {
                    push(resolve(op)?, &mut code);
                }
            }
            Item::CaseTbl { default, cases } => {
                push(i32::from(Opcode::Casetbl.raw()), &mut code);
                push(cases.len() as i32, &mut code);
                push(resolve(default)?, &mut code);
                for (value, target) in cases {
                    push(*value, &mut code);
                    push(resolve(target)?, &mut code);
                }
            }
        }
    }

    debug_assert_eq!(code.len() as u32, code_index);
    Ok(Assembled { code, labels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disasm::{Style, dangling_targets, disassemble, render};

    fn cells(bytes: &[u8]) -> Vec<i32> {
        bytes
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn opcode_occupies_a_whole_cell() {
        let a = assemble(&[Item::op0(Opcode::Proc)]).unwrap();
        assert_eq!(a.code, vec![46, 0, 0, 0]);
    }

    #[test]
    fn operands_follow_as_little_endian_cells() {
        let a = assemble(&[Item::op(Opcode::ConstPri, [Operand::Num(-2)])]).unwrap();
        assert_eq!(cells(&a.code), vec![11, -2]);
        assert_eq!(&a.code[4..8], &[0xFE, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn resolves_forward_and_backward_jumps() {
        // L0: proc            @0
        //     jump L1         @4   (forward)
        //     nop             @12
        // L1: jump L0         @16  (backward)
        //     retn            @24
        let items = [
            Item::Label(0),
            Item::op0(Opcode::Proc),
            Item::op(Opcode::Jump, [Operand::Label(1)]),
            Item::op0(Opcode::Nop),
            Item::Label(1),
            Item::op(Opcode::Jump, [Operand::Label(0)]),
            Item::op0(Opcode::Retn),
        ];
        let a = assemble(&items).unwrap();
        assert_eq!(a.labels[&0], 0);
        assert_eq!(a.labels[&1], 16);
        assert_eq!(cells(&a.code), vec![46, 51, 16, 134, 51, 0, 48]);

        let d = disassemble(&a.code).unwrap();
        assert!(dangling_targets(&d).is_empty());
    }

    #[test]
    fn round_trips_through_the_disassembler() {
        let items = [
            Item::op0(Opcode::Proc),
            Item::op(Opcode::PushC, [Operand::Num(7)]),
            Item::op(Opcode::SysreqC, [Operand::Num(3)]),
            Item::op(Opcode::Stack, [Operand::Num(8)]),
            Item::Label(9),
            Item::op(Opcode::Jzer, [Operand::Label(9)]),
            Item::op0(Opcode::Add),
            Item::op(Opcode::Halt, [Operand::Num(0)]),
            Item::op0(Opcode::Retn),
        ];
        let a = assemble(&items).unwrap();
        let d = disassemble(&a.code).unwrap();

        let want: Vec<(Opcode, Vec<i32>)> = vec![
            (Opcode::Proc, vec![]),
            (Opcode::PushC, vec![7]),
            (Opcode::SysreqC, vec![3]),
            (Opcode::Stack, vec![8]),
            // label 9 sits after proc(4) + push.c(8) + sysreq.c(8) + stack(8)
            (Opcode::Jzer, vec![28]),
            (Opcode::Add, vec![]),
            (Opcode::Halt, vec![0]),
            (Opcode::Retn, vec![]),
        ];
        let got: Vec<(Opcode, Vec<i32>)> =
            d.iter().map(|i| (i.opcode, i.operands.clone())).collect();
        assert_eq!(got, want);
        assert!(dangling_targets(&d).is_empty());
    }

    #[test]
    fn assembles_a_case_table_with_several_records() {
        // switch -> casetbl with 3 cases and a default, all relocated.
        let items = [
            Item::op(Opcode::Switch, [Operand::Label(10)]),
            Item::Label(1),
            Item::op(Opcode::ConstPri, [Operand::Num(1)]),
            Item::Label(2),
            Item::op(Opcode::ConstPri, [Operand::Num(2)]),
            Item::Label(3),
            Item::op(Opcode::ConstPri, [Operand::Num(3)]),
            Item::Label(4),
            Item::op0(Opcode::Retn),
            Item::Label(10),
            Item::CaseTbl {
                default: Operand::Label(4),
                cases: vec![
                    (10, Operand::Label(1)),
                    (20, Operand::Label(2)),
                    (30, Operand::Label(3)),
                ],
            },
        ];
        let a = assemble(&items).unwrap();
        // switch(8) + 3 * const.pri(8) + retn(4) = 36
        assert_eq!(a.labels[&10], 36);
        assert_eq!(a.labels[&1], 8);
        assert_eq!(a.labels[&2], 16);
        assert_eq!(a.labels[&3], 24);
        assert_eq!(a.labels[&4], 32);

        let d = disassemble(&a.code).unwrap();
        assert_eq!(d.len(), 6);
        let tbl = d.last().unwrap();
        assert_eq!(tbl.opcode, Opcode::Casetbl);
        // (count, default), then (value, address) per case
        assert_eq!(tbl.operands, vec![3, 32, 10, 8, 20, 16, 30, 24]);
        assert_eq!(tbl.size, (1 + 2 + 2 * 3) * CELL);
        assert!(dangling_targets(&d).is_empty());

        // Every address in the table renders as a label. Numbering follows first
        // appearance: the switch operand (the table itself) is L0, then the
        // default, then each case target.
        let text = render(&d, Style::Normalised);
        assert!(text.contains("switch L0"), "got:\n{text}");
        assert!(text.contains("casetbl 3 L1 10 L2 20 L3 30 L4"), "got:\n{text}");
    }

    #[test]
    fn labels_may_share_an_address() {
        let a = assemble(&[Item::Label(1), Item::Label(2), Item::op0(Opcode::Retn)]).unwrap();
        assert_eq!(a.labels[&1], 0);
        assert_eq!(a.labels[&2], 0);
    }

    #[test]
    fn rejects_a_duplicate_label() {
        let e = assemble(&[Item::Label(1), Item::op0(Opcode::Nop), Item::Label(1)]).unwrap_err();
        assert_eq!(e, AsmError::DuplicateLabel { label: 1, first: 0 });
    }

    #[test]
    fn rejects_an_undefined_label() {
        let e = assemble(&[Item::op(Opcode::Jump, [Operand::Label(7)])]).unwrap_err();
        assert_eq!(e, AsmError::UndefinedLabel { label: 7 });
    }

    #[test]
    fn rejects_a_wrong_operand_count() {
        let e = assemble(&[Item::op0(Opcode::ConstPri)]).unwrap_err();
        assert_eq!(
            e,
            AsmError::BadArity {
                opcode: Opcode::ConstPri,
                expected: 1,
                got: 0
            }
        );
    }

    #[test]
    fn rejects_a_label_where_a_number_is_expected() {
        // const.pri is not relocated; a label here would silently become a constant.
        let e = assemble(&[
            Item::Label(0),
            Item::op(Opcode::ConstPri, [Operand::Label(0)]),
        ])
        .unwrap_err();
        assert_eq!(
            e,
            AsmError::LabelNotAnAddress {
                opcode: Opcode::ConstPri,
                index: 0
            }
        );
    }

    #[test]
    fn rejects_casetbl_as_a_plain_instruction() {
        let e = assemble(&[Item::op0(Opcode::Casetbl)]).unwrap_err();
        assert_eq!(
            e,
            AsmError::UnsupportedOpcode {
                opcode: Opcode::Casetbl
            }
        );
        // debug records are not emitted by this compiler either
        assert_eq!(
            assemble(&[Item::op0(Opcode::File)]).unwrap_err(),
            AsmError::UnsupportedOpcode {
                opcode: Opcode::File
            }
        );
    }

    #[test]
    fn rejects_an_unsorted_case_table() {
        let e = assemble(&[Item::CaseTbl {
            default: Operand::Num(0),
            cases: vec![(5, Operand::Num(0)), (5, Operand::Num(0))],
        }])
        .unwrap_err();
        assert_eq!(e, AsmError::UnsortedCaseTable { at: 1 });
    }
}
