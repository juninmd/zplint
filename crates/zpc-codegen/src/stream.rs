//! The Pawn assembly stream: the intermediate the C compiler writes with
//! `stgwrite()` before `sc6.c` assembles it.
//!
//! `libpc300` emits text (`"\tload.s.pri 8\n"`) and tracks a parallel byte counter
//! `code_idx`. Emitting a structured [`Item`] stream instead keeps the two in sync
//! by construction and lets forward jumps stay symbolic until [`assemble`] resolves
//! them - which is what `sc6.c`'s two-pass `lbltab[]` does.
//!
//! Every helper below is named after the `sc4.c` function it ports, so the two can
//! be diffed line by line.

use std::collections::HashMap;
use std::fmt::Write as _;

use zpc_asm::{Opcode, Operands};

/// Bytes per cell. AMX Mod X is always `PAWN_CELL_SIZE==32`.
pub const CELL: i32 = 4;

/// `sCHARBITS/8` from `sc.h`: bytes per packed character.
pub const CHAR_BYTES: i32 = 1;

/// The shift that turns a cell index into a byte offset (`cell2addr()` in `sc4.c`
/// emits `shl.c.pri 2` for a 32-bit cell).
pub const CELL_SHIFT: i32 = 2;

/// A code label, the Rust equivalent of `getlabel()`'s integer in `sc1.c`.
pub type LabelId = u32;

/// The two AMX general registers. `regid` in `sc.h`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reg {
    /// The primary register: every expression result lands here.
    Pri,
    /// The alternate register: scratch, and the left operand of a binary operator.
    Alt,
}

/// One operand cell, either a literal or a not-yet-resolved code address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operand {
    Imm(i32),
    Label(LabelId),
}

/// One entry of the assembly stream.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Item {
    /// `setlabel()` - defines where a [`Operand::Label`] points. Occupies no space.
    Label(LabelId),
    Insn { opcode: Opcode, operands: Vec<Operand> },
    /// The `casetbl` header plus its `2 + 2n` cells, written by `ffcase()`.
    ///
    /// The C compiler emits these as separate `case` pseudo-instructions that
    /// `sc6.c`'s `do_case` turns into bare cells; keeping the table as one item
    /// makes it impossible to emit a header without a body.
    CaseTbl { default: LabelId, cases: Vec<(i32, LabelId)> },
}

impl Item {
    /// Encoded size in bytes - the `opcodes(n)+opargs(m)` arithmetic of `sc4.c`.
    pub fn size(&self) -> u32 {
        match self {
            Item::Label(_) => 0,
            Item::Insn { operands, .. } => 4 + 4 * operands.len() as u32,
            Item::CaseTbl { cases, .. } => 4 + 8 + 8 * cases.len() as u32,
        }
    }
}

/// Assembling failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AsmError {
    /// A jump referenced a label that was never placed. In the C compiler this is
    /// an assertion in `sc6.c`; here it is a hard error so a codegen bug cannot
    /// silently produce a plugin that jumps to address 0.
    UndefinedLabel(LabelId),
    /// The same label was placed twice.
    DuplicateLabel(LabelId),
}

/// A growable stream of assembly items with a label allocator.
#[derive(Clone, Debug, Default)]
pub struct AsmStream {
    items: Vec<Item>,
    next_label: LabelId,
}

impl AsmStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn into_items(self) -> Vec<Item> {
        self.items
    }

    /// Current length of the stream, for use with [`AsmStream::truncate`].
    pub fn mark(&self) -> usize {
        self.items.len()
    }

    /// `stgdel()`: discard everything emitted since [`AsmStream::mark`].
    ///
    /// The C compiler buffers code in a staging area precisely so it can throw
    /// away a sub-expression once it turns out to be constant, or once a
    /// commutative operator makes the `push`/`pop` pair unnecessary
    /// (`plnge2()` in `sc3.c`). Label allocation is *not* rolled back, matching
    /// upstream, where `getlabel()` has no undo.
    pub fn truncate(&mut self, mark: usize) {
        self.items.truncate(mark);
    }

    /// `getlabel()` in `sc1.c`.
    pub fn label(&mut self) -> LabelId {
        let l = self.next_label;
        self.next_label += 1;
        l
    }

    /// `setlabel()` in `sc4.c`.
    pub fn place(&mut self, label: LabelId) {
        self.items.push(Item::Label(label));
    }

    pub fn emit0(&mut self, opcode: Opcode) {
        self.items.push(Item::Insn { opcode, operands: Vec::new() });
    }

    pub fn emit1(&mut self, opcode: Opcode, val: i32) {
        self.items.push(Item::Insn { opcode, operands: vec![Operand::Imm(val)] });
    }

    /// Emit a jump-like instruction whose operand is a code address.
    pub fn emit_jump(&mut self, opcode: Opcode, target: LabelId) {
        debug_assert!(opcode.is_jump(), "{} is not a jump", opcode.mnemonic());
        self.items.push(Item::Insn { opcode, operands: vec![Operand::Label(target)] });
    }

    // ---------------------------------------------------------------- sc4.c

    /// `ldconst()`: `zero.pri`/`zero.alt` for 0, `const.pri`/`const.alt` otherwise.
    pub fn ldconst(&mut self, val: i32, reg: Reg) {
        match (val, reg) {
            (0, Reg::Pri) => self.emit0(Opcode::ZeroPri),
            (0, Reg::Alt) => self.emit0(Opcode::ZeroAlt),
            (v, Reg::Pri) => self.emit1(Opcode::ConstPri, v),
            (v, Reg::Alt) => self.emit1(Opcode::ConstAlt, v),
        }
    }

    /// `moveto1()`: PRI = ALT.
    pub fn moveto1(&mut self) {
        self.emit0(Opcode::MovePri);
    }

    /// `move_alt()`: ALT = PRI.
    pub fn move_alt(&mut self) {
        self.emit0(Opcode::MoveAlt);
    }

    /// `pushreg()`.
    pub fn pushreg(&mut self, reg: Reg) {
        self.emit0(match reg {
            Reg::Pri => Opcode::PushPri,
            Reg::Alt => Opcode::PushAlt,
        });
    }

    /// `popreg()`.
    pub fn popreg(&mut self, reg: Reg) {
        self.emit0(match reg {
            Reg::Pri => Opcode::PopPri,
            Reg::Alt => Opcode::PopAlt,
        });
    }

    /// `pushval()`.
    pub fn pushval(&mut self, val: i32) {
        self.emit1(Opcode::PushC, val);
    }

    /// `swap1()`.
    pub fn swap1(&mut self) {
        self.emit0(Opcode::SwapPri);
    }

    /// `load_i()`.
    pub fn load_i(&mut self) {
        self.emit0(Opcode::LoadI);
    }

    /// `addconst()` - emits nothing for a zero delta, exactly like the original.
    pub fn addconst(&mut self, val: i32) {
        if val != 0 {
            self.emit1(Opcode::AddC, val);
        }
    }

    /// `modstk()` - emits nothing for a zero delta.
    pub fn modstk(&mut self, delta: i32) {
        if delta != 0 {
            self.emit1(Opcode::Stack, delta);
        }
    }

    /// `modheap()` - emits nothing for a zero delta.
    pub fn modheap(&mut self, delta: i32) {
        if delta != 0 {
            self.emit1(Opcode::Heap, delta);
        }
    }

    /// `setheap_pri()`: allocate one cell on the heap and store PRI in it, leaving
    /// the cell's address in PRI.
    pub fn setheap_pri(&mut self) {
        self.emit1(Opcode::Heap, CELL);
        self.emit0(Opcode::StorI);
        self.moveto1();
    }

    /// `setstk()`: force STK to a hard offset from FRM. Used when a `goto` crosses
    /// a declaration (`sc1.c` `dolabel()`).
    pub fn setstk(&mut self, value: i32) {
        debug_assert!(value <= 0, "STK must always become <= FRM");
        self.emit1(Opcode::Lctrl, 5);
        self.addconst(value);
        self.emit1(Opcode::Sctrl, 4);
    }

    /// `cell2addr()`.
    pub fn cell2addr(&mut self) {
        self.emit1(Opcode::ShlCPri, CELL_SHIFT);
    }

    /// `cell2addr_alt()`.
    pub fn cell2addr_alt(&mut self) {
        self.emit1(Opcode::ShlCAlt, CELL_SHIFT);
    }

    /// `addr2cell()`.
    pub fn addr2cell(&mut self) {
        self.emit1(Opcode::ShrCPri, CELL_SHIFT);
    }

    /// `char2addr()` - a no-op for 8-bit characters, which is what AMX Mod X uses.
    pub fn char2addr(&mut self) {}

    /// `charalign()`.
    pub fn charalign(&mut self) {
        self.emit1(Opcode::AlignPri, CHAR_BYTES);
    }

    /// `memcopy()`: source in PRI, destination in ALT, `size` in bytes.
    pub fn memcopy(&mut self, size: i32) {
        self.emit1(Opcode::Movs, size);
    }

    /// `ffbounds()`: only emitted when bounds checking is on.
    pub fn ffbounds(&mut self, size: i32) {
        self.emit1(Opcode::Bounds, size);
    }

    /// `ffret()`.
    pub fn ffret(&mut self) {
        self.emit0(Opcode::Retn);
    }

    /// `ffabort()`.
    pub fn ffabort(&mut self, reason: i32) {
        self.emit1(Opcode::Halt, reason);
    }

    /// `jumplabel()`.
    pub fn jumplabel(&mut self, target: LabelId) {
        self.emit_jump(Opcode::Jump, target);
    }

    /// `jmp_eq0()`.
    pub fn jmp_eq0(&mut self, target: LabelId) {
        self.emit_jump(Opcode::Jzer, target);
    }

    /// `jmp_ne0()`.
    pub fn jmp_ne0(&mut self, target: LabelId) {
        self.emit_jump(Opcode::Jnz, target);
    }

    /// `ffswitch()`: the operand is the address of the case table.
    pub fn ffswitch(&mut self, table: LabelId) {
        self.emit_jump(Opcode::Switch, table);
    }

    /// `ffcase(count, default, TRUE)` followed by one `ffcase(value, label, FALSE)`
    /// per record. The C compiler keeps the records sorted by value so an abstract
    /// machine can binary-search them; the caller must pass them sorted.
    pub fn case_table(&mut self, default: LabelId, cases: Vec<(i32, LabelId)>) {
        debug_assert!(cases.windows(2).all(|w| w[0].0 <= w[1].0), "case table must be sorted");
        self.items.push(Item::CaseTbl { default, cases });
    }

    /// `relop_prefix()` - see the comment above `os_lt()` in `sc4.c`.
    pub fn relop_prefix(&mut self) {
        self.pushreg(Reg::Pri);
        self.moveto1();
    }

    /// `relop_suffix()`.
    pub fn relop_suffix(&mut self) {
        self.emit0(Opcode::SwapAlt);
        self.emit0(Opcode::And);
        self.popreg(Reg::Alt);
    }
}

/// Resolve labels and encode the stream into a code segment.
///
/// This is the job `sc6.c` does in two passes: collect `lbltab[]`, then write the
/// opcodes with the addresses substituted.
pub fn assemble(items: &[Item]) -> Result<Vec<u8>, AsmError> {
    let mut addr: HashMap<LabelId, i32> = HashMap::new();
    let mut pc: u32 = 0;
    for item in items {
        if let Item::Label(l) = item
            && addr.insert(*l, pc as i32).is_some()
        {
            return Err(AsmError::DuplicateLabel(*l));
        }
        pc += item.size();
    }

    let resolve = |op: &Operand| -> Result<i32, AsmError> {
        match op {
            Operand::Imm(v) => Ok(*v),
            Operand::Label(l) => addr.get(l).copied().ok_or(AsmError::UndefinedLabel(*l)),
        }
    };

    let mut out = Vec::with_capacity(pc as usize);
    let cell = |out: &mut Vec<u8>, v: i32| out.extend_from_slice(&v.to_le_bytes());
    for item in items {
        match item {
            Item::Label(_) => {}
            Item::Insn { opcode, operands } => {
                cell(&mut out, opcode.raw() as i32);
                for op in operands {
                    let v = resolve(op)?;
                    cell(&mut out, v);
                }
            }
            Item::CaseTbl { default, cases } => {
                cell(&mut out, Opcode::Casetbl.raw() as i32);
                cell(&mut out, cases.len() as i32);
                cell(&mut out, resolve(&Operand::Label(*default))?);
                for (value, target) in cases {
                    cell(&mut out, *value);
                    cell(&mut out, resolve(&Operand::Label(*target))?);
                }
            }
        }
    }
    Ok(out)
}

/// Render the stream the way `stgwrite()` does: one tab-indented mnemonic per
/// line, labels as `l.<hex>`, operands in hexadecimal (`outval()` calls `itoh()`).
pub fn render_asm(items: &[Item]) -> String {
    let mut out = String::new();
    let hex = |v: i32| format!("{:x}", v as u32);
    for item in items {
        match item {
            Item::Label(l) => {
                let _ = writeln!(out, "l.{}", hex(*l as i32));
            }
            Item::Insn { opcode, operands } => {
                let _ = write!(out, "\t{}", opcode.mnemonic());
                for op in operands {
                    match op {
                        Operand::Imm(v) => {
                            let _ = write!(out, " {}", hex(*v));
                        }
                        Operand::Label(l) => {
                            let _ = write!(out, " l.{}", hex(*l as i32));
                        }
                    }
                }
                out.push('\n');
            }
            Item::CaseTbl { default, cases } => {
                let _ = writeln!(out, "\tcasetbl");
                let _ = writeln!(out, "\tcase {} l.{}", cases.len(), hex(*default as i32));
                for (value, target) in cases {
                    let _ = writeln!(out, "\tcase {} l.{}", hex(*value), hex(*target as i32));
                }
            }
        }
    }
    out
}

/// Debug helper: the opcode sequence of a stream, labels skipped.
///
/// `CaseTbl` contributes a single [`Opcode::Casetbl`], matching what a
/// disassembler sees.
pub fn opcodes(items: &[Item]) -> Vec<Opcode> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Label(_) => None,
            Item::Insn { opcode, .. } => Some(*opcode),
            Item::CaseTbl { .. } => Some(Opcode::Casetbl),
        })
        .collect()
}

/// True when the opcode takes exactly the operand count the stream gave it.
///
/// Used by the tests to catch a mis-built [`Item`] before it reaches [`assemble`].
pub fn arity_ok(item: &Item) -> bool {
    match item {
        Item::Label(_) | Item::CaseTbl { .. } => true,
        Item::Insn { opcode, operands } => match opcode.operands() {
            Operands::None => operands.is_empty(),
            Operands::One => operands.len() == 1,
            Operands::Two => operands.len() == 2,
            Operands::CaseTable | Operands::Variable => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zpc_asm::{Style, disassemble, render};

    #[test]
    fn ldconst_uses_zero_forms_for_zero() {
        let mut s = AsmStream::new();
        s.ldconst(0, Reg::Pri);
        s.ldconst(1, Reg::Pri);
        s.ldconst(0, Reg::Alt);
        s.ldconst(1, Reg::Alt);
        assert_eq!(
            opcodes(s.items()),
            [Opcode::ZeroPri, Opcode::ConstPri, Opcode::ZeroAlt, Opcode::ConstAlt]
        );
    }

    #[test]
    fn zero_deltas_emit_nothing() {
        let mut s = AsmStream::new();
        s.modstk(0);
        s.modheap(0);
        s.addconst(0);
        assert!(s.items().is_empty());
    }

    #[test]
    fn labels_resolve_to_byte_addresses() {
        let mut s = AsmStream::new();
        let l = s.label();
        s.jumplabel(l); // 8 bytes
        s.emit0(Opcode::Nop); // 4 bytes
        s.place(l); // address 12
        s.ffret();
        let code = assemble(s.items()).unwrap();
        let d = disassemble(&code).unwrap();
        assert_eq!(d[0].operands, vec![12]);
        assert!(zpc_asm::dangling_targets(&d).is_empty());
    }

    #[test]
    fn undefined_labels_are_an_error() {
        let mut s = AsmStream::new();
        let l = s.label();
        s.jumplabel(l);
        assert_eq!(assemble(s.items()), Err(AsmError::UndefinedLabel(l)));
    }

    #[test]
    fn a_case_table_round_trips_through_the_disassembler() {
        let mut s = AsmStream::new();
        let table = s.label();
        let a = s.label();
        let b = s.label();
        let exit = s.label();
        s.ffswitch(table);
        s.place(a);
        s.jumplabel(exit);
        s.place(b);
        s.jumplabel(exit);
        s.place(table);
        s.case_table(exit, vec![(1, a), (2, b)]);
        s.place(exit);
        s.ffret();

        let code = assemble(s.items()).unwrap();
        let d = disassemble(&code).unwrap();
        assert!(zpc_asm::dangling_targets(&d).is_empty());
        let text = render(&d, Style::Normalised);
        // switch -> the table; the table's 2+2n cells name the default and both arms.
        assert!(text.contains("casetbl 2"), "got:\n{text}");
    }

    #[test]
    fn every_emitted_instruction_has_the_declared_arity() {
        let mut s = AsmStream::new();
        s.setheap_pri();
        s.setstk(-8);
        s.cell2addr();
        s.charalign();
        s.relop_prefix();
        s.relop_suffix();
        assert!(s.items().iter().all(arity_ok));
    }
}
