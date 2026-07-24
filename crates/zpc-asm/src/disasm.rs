//! AMX bytecode disassembler.
//!
//! This exists to make the differential oracle work at the instruction level
//! (`docs/COMPILER_MIGRATION.md` §2). Comparing `.amxx` files byte-for-byte against
//! amxxpc is not a usable signal: deflate streams differ between zlib implementations,
//! and absolute addresses shift whenever anything upstream of them changes size.
//! Comparing *normalised disassembly* is the signal we actually want - it tells us
//! whether the two compilers agree on the program, not on its layout.

use crate::opcode::{Opcode, Operands};
use std::fmt::Write as _;

/// One decoded instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    /// Byte offset of the opcode within the code segment.
    pub address: u32,
    pub opcode: Opcode,
    /// Operand cells that follow the opcode, in order.
    pub operands: Vec<i32>,
    /// Total encoded size in bytes, opcode cell included.
    pub size: u32,
}

/// Why decoding stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisasmError {
    /// The code segment length is not a whole number of cells.
    UnalignedSegment { len: usize },
    /// An opcode cell held a value that is not a known instruction.
    UnknownOpcode { address: u32, raw: u32 },
    /// Operands ran past the end of the segment.
    Truncated { address: u32, needed: u32 },
    /// A `casetbl` declared a record count that cannot fit in the segment.
    BadCaseTable { address: u32, count: i32 },
}

/// AMX cells are 32-bit in every AMX Mod X build; the container records a cell size
/// but only 4 is supported here, matching what amxxpc emits.
const CELL: u32 = 4;

/// Read a little-endian cell at a byte offset.
fn cell_at(code: &[u8], off: usize) -> Option<i32> {
    let bytes = code.get(off..off + CELL as usize)?;
    Some(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Decode a whole code segment into instructions.
///
/// `code` must be exactly the code segment (the AMX header's `cod`..`dat` range),
/// not the whole file - the decoder has no way to detect where code stops otherwise.
pub fn disassemble(code: &[u8]) -> Result<Vec<Instruction>, DisasmError> {
    if !code.len().is_multiple_of(CELL as usize) {
        return Err(DisasmError::UnalignedSegment { len: code.len() });
    }

    let mut out = Vec::new();
    let mut off = 0usize;

    while off < code.len() {
        let address = off as u32;
        // The opcode occupies a full cell, not a byte.
        let raw = cell_at(code, off).ok_or(DisasmError::Truncated { address, needed: CELL })?;
        let opcode = u8::try_from(raw)
            .ok()
            .and_then(Opcode::from_raw)
            .ok_or(DisasmError::UnknownOpcode { address, raw: raw as u32 })?;
        off += CELL as usize;

        let operands = match opcode.operands() {
            Operands::CaseTable => read_case_table(code, &mut off, address)?,
            Operands::Variable => {
                // Debug records are never emitted by the compiler (they are commented
                // out of sc6.c's table), so their in-stream encoding is not defined
                // here. Stop rather than silently desynchronising the decode.
                return Err(DisasmError::UnknownOpcode { address, raw: raw as u32 });
            }
            fixed => {
                let n = fixed.count().unwrap_or(0);
                read_fixed(code, &mut off, address, n)?
            }
        };

        out.push(Instruction {
            address,
            opcode,
            operands,
            size: off as u32 - address,
        });
    }

    Ok(out)
}

fn read_fixed(
    code: &[u8],
    off: &mut usize,
    address: u32,
    count: u32,
) -> Result<Vec<i32>, DisasmError> {
    let mut v = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let c = cell_at(code, *off).ok_or(DisasmError::Truncated {
            address,
            needed: count * CELL,
        })?;
        v.push(c);
        *off += CELL as usize;
    }
    Ok(v)
}

/// `casetbl` is followed by `(count, default_addr)` and then `count` pairs of
/// `(match_value, jump_addr)` - `2 + 2n` cells with no opcode of their own.
fn read_case_table(
    code: &[u8],
    off: &mut usize,
    address: u32,
) -> Result<Vec<i32>, DisasmError> {
    let count = cell_at(code, *off).ok_or(DisasmError::Truncated { address, needed: CELL })?;
    let default = cell_at(code, *off + CELL as usize)
        .ok_or(DisasmError::Truncated { address, needed: 2 * CELL })?;

    // A negative or absurd count would make the loop below read wildly out of range.
    let records = u32::try_from(count).map_err(|_| DisasmError::BadCaseTable { address, count })?;
    let needed = (2 + 2 * records as usize) * CELL as usize;
    if *off + needed > code.len() + CELL as usize {
        return Err(DisasmError::BadCaseTable { address, count });
    }

    *off += 2 * CELL as usize;
    let mut v = vec![count, default];
    for _ in 0..records {
        let value = cell_at(code, *off).ok_or(DisasmError::BadCaseTable { address, count })?;
        let target = cell_at(code, *off + CELL as usize)
            .ok_or(DisasmError::BadCaseTable { address, count })?;
        v.push(value);
        v.push(target);
        *off += 2 * CELL as usize;
    }
    Ok(v)
}

/// How to render a disassembly for comparison.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Keep absolute addresses. Useful when debugging one binary on its own.
    Raw,
    /// Drop instruction addresses and rewrite every jump target to a symbolic label
    /// (`L0`, `L1`, ...) numbered in order of first appearance. Two builds that differ
    /// only in where code landed produce identical text under this style, so a diff
    /// shows real disagreement about the program.
    Normalised,
}

/// Render instructions as text, one per line.
pub fn render(instrs: &[Instruction], style: Style) -> String {
    let labels = if style == Style::Normalised {
        label_map(instrs)
    } else {
        Vec::new()
    };
    let label_of = |addr: i32| -> Option<usize> {
        labels.iter().position(|a| *a == addr)
    };

    let mut out = String::new();
    for ins in instrs {
        if style == Style::Normalised
            && let Some(i) = label_of(ins.address as i32)
        {
            let _ = writeln!(out, "L{i}:");
        } else if style == Style::Raw {
            let _ = write!(out, "{:08x}  ", ins.address);
        }

        let _ = write!(out, "{}", ins.opcode.mnemonic());
        for (i, op) in ins.operands.iter().enumerate() {
            // Only the operand that is genuinely a code address becomes a label.
            let is_target = style == Style::Normalised && is_address_operand(ins, i);
            match is_target.then(|| label_of(*op)).flatten() {
                Some(l) => {
                    let _ = write!(out, " L{l}");
                }
                Option::None => {
                    let _ = write!(out, " {op}");
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Whether operand `i` of `ins` holds a code address.
fn is_address_operand(ins: &Instruction, i: usize) -> bool {
    if ins.opcode.operands() == Operands::CaseTable {
        // Layout is (count, default, [value, target]*): index 1 and every odd index
        // from 3 upwards are addresses; the counts and match values are not.
        return i == 1 || (i >= 3 && i % 2 == 1);
    }
    ins.opcode.is_jump() && i == 0
}

/// Every address referenced as a jump target, deduplicated and ordered by first
/// appearance so numbering is stable and independent of absolute layout.
fn label_map(instrs: &[Instruction]) -> Vec<i32> {
    let mut labels: Vec<i32> = Vec::new();
    for ins in instrs {
        for (i, op) in ins.operands.iter().enumerate() {
            if is_address_operand(ins, i) && !labels.contains(op) {
                labels.push(*op);
            }
        }
    }
    labels
}

/// Jump targets that do not land on any instruction boundary.
///
/// A dangling target means the decode desynchronised, the segment bounds were
/// wrong, or the operand is not really an address. Normalised output would render
/// it as a label that is never defined, and the oracle would then be comparing two
/// listings that only look alike - so callers should treat a non-empty result as a
/// hard failure rather than a cosmetic issue.
pub fn dangling_targets(instrs: &[Instruction]) -> Vec<i32> {
    let valid: Vec<i32> = instrs.iter().map(|i| i.address as i32).collect();
    label_map(instrs)
        .into_iter()
        .filter(|t| !valid.contains(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a code segment from (opcode, operands) pairs.
    fn asm(items: &[(Opcode, &[i32])]) -> Vec<u8> {
        let mut v = Vec::new();
        for (op, args) in items {
            v.extend_from_slice(&(op.raw() as i32).to_le_bytes());
            for a in *args {
                v.extend_from_slice(&a.to_le_bytes());
            }
        }
        v
    }

    #[test]
    fn decodes_fixed_arity_instructions() {
        let code = asm(&[
            (Opcode::Proc, &[]),
            (Opcode::ConstPri, &[42]),
            (Opcode::Retn, &[]),
        ]);
        let d = disassemble(&code).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].opcode, Opcode::Proc);
        assert_eq!(d[0].size, 4);
        assert_eq!(d[1].operands, vec![42]);
        assert_eq!(d[1].size, 8);
        assert_eq!(d[2].address, 12);
    }

    #[test]
    fn rejects_an_unaligned_segment() {
        assert_eq!(
            disassemble(&[0, 1, 2]),
            Err(DisasmError::UnalignedSegment { len: 3 })
        );
    }

    #[test]
    fn rejects_an_unknown_opcode() {
        let code = 0xFFFF_i32.to_le_bytes().to_vec();
        assert!(matches!(
            disassemble(&code),
            Err(DisasmError::UnknownOpcode { address: 0, .. })
        ));
    }

    #[test]
    fn detects_truncated_operands() {
        // const.pri claims an operand but the segment ends
        let code = (Opcode::ConstPri.raw() as i32).to_le_bytes().to_vec();
        assert!(matches!(
            disassemble(&code),
            Err(DisasmError::Truncated { address: 0, .. })
        ));
    }

    #[test]
    fn decodes_a_case_table() {
        // casetbl with 2 records: (count=2, default), (1, addr1), (2, addr2)
        let code = asm(&[(Opcode::Casetbl, &[2, 100, 1, 200, 2, 300])]);
        let d = disassemble(&code).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].operands, vec![2, 100, 1, 200, 2, 300]);
        // opcode + 6 cells
        assert_eq!(d[0].size, 7 * 4);
    }

    #[test]
    fn rejects_a_case_table_with_a_negative_count() {
        let code = asm(&[(Opcode::Casetbl, &[-1, 0])]);
        assert!(matches!(
            disassemble(&code),
            Err(DisasmError::BadCaseTable { count: -1, .. })
        ));
    }

    #[test]
    fn normalised_render_is_layout_independent() {
        // Same program, but the second build has an extra leading instruction, so
        // every address shifts. Normalised output must still match.
        let a = disassemble(&asm(&[
            (Opcode::Proc, &[]),
            (Opcode::Jump, &[12]),
            (Opcode::ConstPri, &[1]),
            (Opcode::Retn, &[]),
        ]))
        .unwrap();
        // jump target shifts from 12 to 16 because of the added nop
        let b = disassemble(&asm(&[
            (Opcode::Nop, &[]),
            (Opcode::Proc, &[]),
            (Opcode::Jump, &[16]),
            (Opcode::ConstPri, &[1]),
            (Opcode::Retn, &[]),
        ]))
        .unwrap();

        let ra = render(&a, Style::Normalised);
        let rb = render(&b, Style::Normalised);
        // strip b's extra first line to compare the common part
        let rb_tail = rb.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(ra.trim_end(), rb_tail.trim_end());
        assert!(ra.contains("jump L0"), "jump should be symbolic, got:\n{ra}");
    }

    #[test]
    fn dangling_jump_targets_are_reported() {
        // 999 is not the address of any instruction: normalised output would emit
        // "jump L0" with no L0: line anywhere, which must not pass silently.
        let d = disassemble(&asm(&[(Opcode::Jump, &[999]), (Opcode::Retn, &[])])).unwrap();
        assert_eq!(dangling_targets(&d), vec![999]);

        // a target that does land on an instruction is fine
        let ok = disassemble(&asm(&[(Opcode::Jump, &[8]), (Opcode::Nop, &[]), (Opcode::Retn, &[])]))
            .unwrap();
        assert!(dangling_targets(&ok).is_empty());
    }

    #[test]
    fn raw_render_keeps_addresses() {
        let d = disassemble(&asm(&[(Opcode::Proc, &[]), (Opcode::Retn, &[])])).unwrap();
        let text = render(&d, Style::Raw);
        assert!(text.starts_with("00000000  proc"), "got:\n{text}");
        assert!(text.contains("00000004  retn"), "got:\n{text}");
    }

    #[test]
    fn only_real_address_operands_become_labels() {
        // const.pri 12 must NOT become a label even though 12 is also a jump target.
        let d = disassemble(&asm(&[
            (Opcode::Jump, &[8]),
            (Opcode::ConstPri, &[8]),
            (Opcode::Retn, &[]),
        ]))
        .unwrap();
        let text = render(&d, Style::Normalised);
        assert!(text.contains("jump L0"), "got:\n{text}");
        assert!(text.contains("const.pri 8"), "operand must stay numeric:\n{text}");
    }
}
