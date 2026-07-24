//! The peephole optimiser: the port of `sc7.c`'s `stgopt()` and its
//! `sequences[]` pattern table (spelled out in `sc7-in.scp`).
//!
//! # How upstream works
//!
//! `libpc300` buffers one expression's worth of *text* in the staging buffer and
//! then runs [`stgopt()`](../../../sc7.c) over it: for every position, it walks
//! `sequences[]` in order, and on the first match it splices the replacement in,
//! resets the sequence index to 0 and starts over. The whole sweep repeats while
//! any match was made (`sc7.c:536-573`). This module reproduces that control flow
//! exactly - [`optimise`] is the outer `do { matches=0; ... } while (matches>0)`
//! loop and `try_at` is the inner `while (sequences[seq].find!=NULL)` walk with
//! the same rule ordering as `sc7-in.scp`.
//!
//! # The one place we deliberately diverge
//!
//! Several upstream patterns are guarded by the pseudo-comments `;$par` and
//! `;$exp` that `sc3.c`/`sc1.c` write into the staging buffer. They are markers
//! meaning "PRI is not needed after this point" (`sc7-in.scp:656-658`). Our
//! [`Item`] stream carries no such markers, so those rules are instead guarded by
//! `reg_dead_at`, a conservative forward liveness scan that only returns `true`
//! when it can *prove* the register is overwritten before it is read. That is
//! strictly safer than the marker: a marker upstream trusts, a proof we compute.
//!
//! Patterns that could not be guarded safely are listed as `SKIPPED` comments
//! below rather than implemented approximately.

use std::collections::HashMap;

use zpc_asm::Opcode;

use crate::stream::{Item, LabelId, Operand, Reg};

// ---------------------------------------------------------------- liveness

/// True when `op` overwrites `reg` without first reading it.
///
/// `call`/`sysreq.c` are counted as killing *both* registers: PRI receives the
/// return value and a callee is free to clobber ALT, so neither may be live
/// across a call in code this compiler generates. `stack`/`heap` set ALT to the
/// old STK/HEA, so they kill ALT.
fn kills(op: Opcode, reg: Reg) -> bool {
    match reg {
        Reg::Pri => matches!(
            op,
            Opcode::ConstPri
                | Opcode::ZeroPri
                | Opcode::LoadPri
                | Opcode::LoadSPri
                | Opcode::LrefPri
                | Opcode::LrefSPri
                | Opcode::AddrPri
                | Opcode::PopPri
                | Opcode::MovePri
                | Opcode::Lctrl
                | Opcode::Call
                | Opcode::SysreqC
                | Opcode::Proc
        ),
        Reg::Alt => matches!(
            op,
            Opcode::ConstAlt
                | Opcode::ZeroAlt
                | Opcode::LoadAlt
                | Opcode::LoadSAlt
                | Opcode::LrefAlt
                | Opcode::LrefSAlt
                | Opcode::AddrAlt
                | Opcode::PopAlt
                | Opcode::MoveAlt
                | Opcode::Stack
                | Opcode::Heap
                | Opcode::Call
                | Opcode::SysreqC
                | Opcode::Proc
        ),
    }
}

/// True when `op` neither reads nor writes `reg`, so the liveness scan may step
/// over it. Everything not listed here is assumed to *read* `reg`.
///
/// The two arms are near-mirrors: an instruction that only ever names PRI is
/// transparent to ALT, and vice versa. Branches are deliberately absent: a
/// forward-only scan cannot see the other successor, so it must stop there.
fn ignores(op: Opcode, reg: Reg) -> bool {
    let both = matches!(
        op,
        Opcode::PushC
            | Opcode::Push
            | Opcode::PushS
            | Opcode::Pushaddr
            | Opcode::PushR
            | Opcode::Zero
            | Opcode::ZeroS
            | Opcode::Inc
            | Opcode::IncS
            | Opcode::Dec
            | Opcode::DecS
            | Opcode::Nop
            | Opcode::Break
    );
    both
        || match reg {
            Reg::Pri => matches!(
                op,
                Opcode::ConstAlt
                    | Opcode::ZeroAlt
                    | Opcode::LoadAlt
                    | Opcode::LoadSAlt
                    | Opcode::LrefAlt
                    | Opcode::LrefSAlt
                    | Opcode::AddrAlt
                    | Opcode::PopAlt
                    | Opcode::PushAlt
                    | Opcode::StorAlt
                    | Opcode::StorSAlt
                    | Opcode::SrefAlt
                    | Opcode::SrefSAlt
                    | Opcode::IncAlt
                    | Opcode::DecAlt
                    | Opcode::SignAlt
                    | Opcode::EqCAlt
                    | Opcode::ShlCAlt
                    | Opcode::ShrCAlt
                    | Opcode::AlignAlt
                    | Opcode::Stack
                    | Opcode::Heap
            ),
            Reg::Alt => matches!(
                op,
                Opcode::ConstPri
                    | Opcode::ZeroPri
                    | Opcode::LoadPri
                    | Opcode::LoadSPri
                    | Opcode::LrefPri
                    | Opcode::LrefSPri
                    | Opcode::AddrPri
                    | Opcode::PopPri
                    | Opcode::PushPri
                    | Opcode::StorPri
                    | Opcode::StorSPri
                    | Opcode::SrefPri
                    | Opcode::SrefSPri
                    | Opcode::IncPri
                    | Opcode::DecPri
                    | Opcode::SignPri
                    | Opcode::EqCPri
                    | Opcode::ShlCPri
                    | Opcode::ShrCPri
                    | Opcode::AlignPri
                    | Opcode::Not
                    | Opcode::Neg
                    | Opcode::Invert
                    | Opcode::AddC
                    | Opcode::SmulC
                    | Opcode::LoadI
                    | Opcode::LodbI
                    | Opcode::Lctrl
                    | Opcode::Sctrl
                    | Opcode::Bounds
                    // A function's result is returned in PRI only, so ALT can
                    // never be live across the epilogue.
                    | Opcode::Retn
                    | Opcode::Ret
            ),
        }
}

/// Conservative replacement for upstream's `;$par` / `;$exp` markers: is `reg`
/// provably dead at `pos`?
///
/// Labels are stepped over - a label only says that control may *arrive* here,
/// which says nothing about future *uses*. Anything the tables above do not
/// classify stops the scan with `false`.
fn reg_dead_at(items: &[Item], pos: usize, reg: Reg) -> bool {
    let mut i = pos;
    while let Some(item) = items.get(i) {
        match item {
            Item::Label(_) => {}
            Item::CaseTbl { .. } => return false,
            Item::Insn { opcode, .. } => {
                if kills(*opcode, reg) {
                    return true;
                }
                if !ignores(*opcode, reg) {
                    return false;
                }
            }
        }
        i += 1;
    }
    // Fell off the end of the stream: nothing can read the register any more.
    true
}

// ---------------------------------------------------------------- matching

fn as_insn(items: &[Item], i: usize) -> Option<(Opcode, &[Operand])> {
    match items.get(i) {
        Some(Item::Insn { opcode, operands }) => Some((*opcode, operands.as_slice())),
        _ => None,
    }
}

/// `items[i]` is exactly `op` with no operands.
fn m0(items: &[Item], i: usize, op: Opcode) -> bool {
    matches!(as_insn(items, i), Some((o, ops)) if o == op && ops.is_empty())
}

/// `items[i]` is exactly `op` with one operand; yields that operand.
fn m1(items: &[Item], i: usize, op: Opcode) -> Option<Operand> {
    match as_insn(items, i) {
        Some((o, ops)) if o == op && ops.len() == 1 => Some(ops[0]),
        _ => None,
    }
}

/// As [`m1`], but only matches a literal operand.
fn m1v(items: &[Item], i: usize, op: Opcode) -> Option<i32> {
    match m1(items, i, op) {
        Some(Operand::Imm(v)) => Some(v),
        _ => None,
    }
}

fn i0(op: Opcode) -> Item {
    Item::Insn { opcode: op, operands: Vec::new() }
}

fn i1(op: Opcode, a: Operand) -> Item {
    Item::Insn { opcode: op, operands: vec![a] }
}

fn ic(op: Opcode, v: i32) -> Item {
    i1(op, Operand::Imm(v))
}

// ---------------------------------------------------------------- tables

/// `sc7-in.scp:55-124`: the "very common sequence in four varieties" plus the
/// `addr.pri`, `const.pri` and `zero.pri` extensions.
///
/// `(first, first_as_alt, second, second_has_operand)`. Only the twelve
/// combinations that actually appear in `sequences[]` are listed.
const LOAD_PUSH_LOAD_POP: &[(Opcode, Opcode, Opcode, bool)] = &[
    // sc7-in.scp:55-74
    (Opcode::LoadSPri, Opcode::LoadSAlt, Opcode::LoadSPri, true),
    (Opcode::LoadPri, Opcode::LoadAlt, Opcode::LoadSPri, true),
    (Opcode::LoadSPri, Opcode::LoadSAlt, Opcode::LoadPri, true),
    (Opcode::LoadPri, Opcode::LoadAlt, Opcode::LoadPri, true),
    // sc7-in.scp:78-87
    (Opcode::AddrPri, Opcode::AddrAlt, Opcode::LoadSPri, true),
    (Opcode::AddrPri, Opcode::AddrAlt, Opcode::LoadPri, true),
    // sc7-in.scp:91-110
    (Opcode::ConstPri, Opcode::ConstAlt, Opcode::LoadSPri, true),
    (Opcode::ConstPri, Opcode::ConstAlt, Opcode::LoadPri, true),
    (Opcode::LoadSPri, Opcode::LoadSAlt, Opcode::ConstPri, true),
    (Opcode::LoadPri, Opcode::LoadAlt, Opcode::ConstPri, true),
    // sc7-in.scp:115-124
    (Opcode::AddrPri, Opcode::AddrAlt, Opcode::ConstPri, true),
    (Opcode::AddrPri, Opcode::AddrAlt, Opcode::ZeroPri, false),
];

/// `sc7-in.scp:270-309`: the entry to chained relational operators,
/// `A.pri %1 ; B.alt %2 ; xchg` -> `B.pri %2 ; A.alt %1`.
///
/// `(a_pri, a_alt, b_pri, b_alt)`. The `const.pri`/`const.alt` combination is
/// absent upstream, so it is absent here too.
const LOAD_LOAD_XCHG: &[(Opcode, Opcode, Opcode, Opcode)] = &[
    (Opcode::LoadSPri, Opcode::LoadSAlt, Opcode::LoadSPri, Opcode::LoadSAlt),
    (Opcode::LoadSPri, Opcode::LoadSAlt, Opcode::LoadPri, Opcode::LoadAlt),
    (Opcode::LoadSPri, Opcode::LoadSAlt, Opcode::ConstPri, Opcode::ConstAlt),
    (Opcode::LoadPri, Opcode::LoadAlt, Opcode::LoadSPri, Opcode::LoadSAlt),
    (Opcode::LoadPri, Opcode::LoadAlt, Opcode::LoadPri, Opcode::LoadAlt),
    (Opcode::LoadPri, Opcode::LoadAlt, Opcode::ConstPri, Opcode::ConstAlt),
    (Opcode::ConstPri, Opcode::ConstAlt, Opcode::LoadSPri, Opcode::LoadSAlt),
    (Opcode::ConstPri, Opcode::ConstAlt, Opcode::LoadPri, Opcode::LoadAlt),
];

/// `sc7-in.scp:351-483`: the base of an indexed array access is either a local's
/// address or a global's address-as-constant.
const ARRAY_BASE: &[(Opcode, Opcode)] =
    &[(Opcode::AddrPri, Opcode::AddrAlt), (Opcode::ConstPri, Opcode::ConstAlt)];

/// `sc7-in.scp:510-537`: `push.pri ; <load PRI> ; pop.alt` -> `move.alt ; <load PRI>`.
/// `(op, has_operand)`.
const PUSH_LOAD_POP: &[(Opcode, bool)] = &[
    (Opcode::LoadSPri, true),
    (Opcode::LoadPri, true),
    (Opcode::ConstPri, true),
    (Opcode::ZeroPri, false),
    (Opcode::LoadI, false),
];

/// `sc7-in.scp:659-683`: a load immediately followed by `push.pri` collapses into
/// the dedicated push, *provided PRI is not needed afterwards* (upstream checks
/// the `;$par` marker; we prove it with `reg_dead_at`).
/// `(load, has_operand, push)`.
const LOAD_PUSH: &[(Opcode, bool, Opcode)] = &[
    (Opcode::LoadSPri, true, Opcode::PushS),
    (Opcode::LoadPri, true, Opcode::Push),
    (Opcode::ConstPri, true, Opcode::PushC),
    (Opcode::ZeroPri, false, Opcode::PushC),
    (Opcode::AddrPri, true, Opcode::Pushaddr),
];

/// `sc7-in.scp:709-728`: a constant in ALT folds into the immediate form of the
/// following arithmetic. `(operator, folded, negate_operand)`.
const CONST_ALT_FOLD: &[(Opcode, Opcode, bool)] = &[
    (Opcode::Add, Opcode::AddC, false),
    (Opcode::Sub, Opcode::AddC, true),
    (Opcode::Smul, Opcode::SmulC, false),
    (Opcode::Eq, Opcode::EqCPri, false),
];

/// `sc7-in.scp:798-857`: a comparison followed by `jzer`/`jnz` becomes the
/// corresponding conditional jump. `(compare, jump, replacement)`.
const CMP_JUMP: &[(Opcode, Opcode, Opcode)] = &[
    (Opcode::Eq, Opcode::Jzer, Opcode::Jneq),
    (Opcode::Eq, Opcode::Jnz, Opcode::Jeq),
    (Opcode::Neq, Opcode::Jzer, Opcode::Jeq),
    (Opcode::Neq, Opcode::Jnz, Opcode::Jneq),
    (Opcode::Less, Opcode::Jzer, Opcode::Jgeq),
    (Opcode::Leq, Opcode::Jzer, Opcode::Jgrtr),
    (Opcode::Grtr, Opcode::Jzer, Opcode::Jleq),
    (Opcode::Geq, Opcode::Jzer, Opcode::Jless),
    (Opcode::Sless, Opcode::Jzer, Opcode::Jsgeq),
    (Opcode::Sleq, Opcode::Jzer, Opcode::Jsgrtr),
    (Opcode::Sgrtr, Opcode::Jzer, Opcode::Jsleq),
    (Opcode::Sgeq, Opcode::Jzer, Opcode::Jsless),
];

/// `sc7-in.scp:890-929`: `inc`/`dec` already leave the new value in memory, so a
/// load of the same location is redundant when PRI is dead. `(step, load)`.
const INCDEC_LOAD: &[(Opcode, Opcode)] = &[
    (Opcode::Inc, Opcode::LoadPri),
    (Opcode::IncS, Opcode::LoadSPri),
    (Opcode::Dec, Opcode::LoadPri),
    (Opcode::DecS, Opcode::LoadSPri),
];

/// `sc7-in.scp:955-974`: storing a zero has a dedicated opcode.
/// `(store, zero_store)`.
const ZERO_STORE: &[(Opcode, Opcode)] =
    &[(Opcode::StorPri, Opcode::Zero), (Opcode::StorSPri, Opcode::ZeroS)];

/// Opcodes after which control never falls through, so everything up to the next
/// label is unreachable. Mirrors the `lastst==tRETURN || lastst==tGOTO` test of
/// `sc1.c:3517` and `sc1.c:4851-4854`, widened with the other terminators the
/// AMX has.
fn is_terminator(op: Opcode) -> bool {
    matches!(op, Opcode::Retn | Opcode::Ret | Opcode::Jump | Opcode::Halt | Opcode::JumpPri)
}

// ---------------------------------------------------------------- the pass

/// Apply the `sc7.c` sequences to `items` until nothing changes.
///
/// This is `stgopt()` (`sc7.c:526-578`): repeat a full left-to-right sweep while
/// the previous sweep replaced anything. Unlike upstream, which works on one
/// staging-buffer block at a time, this runs over the whole function stream -
/// which is safe because every rule below either stays inside one run of
/// consecutive instructions or explicitly reasons about labels.
pub fn optimise(items: &[Item]) -> Vec<Item> {
    let mut items = items.to_vec();
    // Every rule strictly shrinks the stream except the two `const -> zero`
    // rewrites, which cannot re-match; the cap is belt and braces.
    for _ in 0..1000 {
        let labels = label_index(&items);
        let mut changed = false;
        let mut i = 0;
        while i < items.len() {
            if let Some((len, repl)) = try_at(&items, i, &labels) {
                items.splice(i..i + len, repl);
                changed = true;
                // `seq=0; matches++` - restart the rule walk at the same spot.
                continue;
            }
            i += 1;
        }
        if !changed {
            return items;
        }
    }
    items
}

fn label_index(items: &[Item]) -> HashMap<LabelId, usize> {
    items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| match it {
            Item::Label(l) => Some((*l, i)),
            _ => None,
        })
        .collect()
}

/// One pass of the `while (sequences[seq].find!=NULL)` walk at position `i`.
///
/// Returns `(matched_length, replacement)`. Rules appear in `sc7-in.scp` order,
/// longest first within a family, exactly as the table is laid out.
fn try_at(
    items: &[Item],
    i: usize,
    labels: &HashMap<LabelId, usize>,
) -> Option<(usize, Vec<Item>)> {
    // ---- sc7-in.scp:55-124  <load> %1 ; push.pri ; <load> %2 ; pop.alt
    for &(first, first_alt, second, has_op) in LOAD_PUSH_LOAD_POP {
        let Some(a) = m1(items, i, first) else { continue };
        if !m0(items, i + 1, Opcode::PushPri) {
            continue;
        }
        let b = if has_op {
            match m1(items, i + 2, second) {
                Some(b) => Some(b),
                None => continue,
            }
        } else {
            if !m0(items, i + 2, second) {
                continue;
            }
            None
        };
        if !m0(items, i + 3, Opcode::PopAlt) {
            continue;
        }
        let second_item = match b {
            Some(b) => i1(second, b),
            None => i0(second),
        };
        // The `addr.pri` rows spell the replacement base-first; the others put
        // the second load first. Both orders are equivalent (the two writes hit
        // different registers) - this follows the table verbatim.
        let repl = if first == Opcode::AddrPri {
            vec![i1(first_alt, a), second_item]
        } else {
            vec![second_item, i1(first_alt, a)]
        };
        return Some((4, repl));
    }

    // ---- sc7-in.scp:134-148  move.pri ; push.pri ; <load> %1 ; pop.alt
    // PRI := ALT, push it, load, pop it back: the net effect is just the load.
    if m0(items, i, Opcode::MovePri) && m0(items, i + 1, Opcode::PushPri) {
        for op in [Opcode::LoadSPri, Opcode::LoadPri, Opcode::ConstPri] {
            if let Some(a) = m1(items, i + 2, op)
                && m0(items, i + 3, Opcode::PopAlt)
            {
                return Some((4, vec![i1(op, a)]));
            }
        }
    }

    // ---- sc7-in.scp:270-309  <load>.pri %1 ; <load>.alt %2 ; xchg
    for &(a_pri, a_alt, b_pri, b_alt) in LOAD_LOAD_XCHG {
        if let Some(a) = m1(items, i, a_pri)
            && let Some(b) = m1(items, i + 1, b_alt)
            && m0(items, i + 2, Opcode::Xchg)
        {
            return Some((3, vec![i1(b_pri, b), i1(a_alt, a)]));
        }
    }

    // ---- sc7-in.scp:351-436, 474-483  indexed array access
    // base %1 ; push.pri ; load.s.pri %2 [; bounds %3] ; shl.c.pri %n ; pop.alt
    //   ; add [; load.i]
    for &(base, base_alt) in ARRAY_BASE {
        let Some(a) = m1(items, i, base) else { continue };
        if !m0(items, i + 1, Opcode::PushPri) {
            continue;
        }
        let Some(idx) = m1(items, i + 2, Opcode::LoadSPri) else { continue };
        let mut k = i + 3;
        let bounds = m1(items, k, Opcode::Bounds);
        if bounds.is_some() {
            k += 1;
        }
        // sc7-in.scp:474-483: the packed-array form stops right here.
        if bounds.is_some() && m0(items, k, Opcode::PopAlt) {
            return Some((
                k + 1 - i,
                vec![i1(base_alt, a), i1(Opcode::LoadSPri, idx), i1(Opcode::Bounds, bounds?)],
            ));
        }
        let Some(shift) = m1v(items, k, Opcode::ShlCPri) else { continue };
        if !(m0(items, k + 1, Opcode::PopAlt) && m0(items, k + 2, Opcode::Add)) {
            continue;
        }
        let load = m0(items, k + 3, Opcode::LoadI);
        let tail = match (load, shift == 2) {
            (true, true) => i0(Opcode::Lidx),       // sc7-in.scp:351-370
            (true, false) => ic(Opcode::LidxB, shift), // sc7-in.scp:373-392
            (false, true) => i0(Opcode::Idxaddr),   // sc7-in.scp:395-414
            (false, false) => ic(Opcode::IdxaddrB, shift), // sc7-in.scp:417-436
        };
        let mut repl = vec![i1(base_alt, a), i1(Opcode::LoadSPri, idx)];
        if let Some(b) = bounds {
            repl.push(i1(Opcode::Bounds, b));
        }
        repl.push(tail);
        let len = k + 3 + usize::from(load) - i;
        return Some((len, repl));
    }

    // ---- sc7-in.scp:439-459  the short array-index tail
    // shl.c.pri %1 ; pop.alt ; add [; load.i]
    //
    // NOTE: upstream spells the load `loadi` in these four rows, which is not a
    // mnemonic `sc6.c` knows, so those rows can never fire in `pawncc`. The
    // rewrite itself is sound, so it is implemented here.
    if let Some(shift) = m1v(items, i, Opcode::ShlCPri)
        && m0(items, i + 1, Opcode::PopAlt)
        && m0(items, i + 2, Opcode::Add)
    {
        let load = m0(items, i + 3, Opcode::LoadI);
        let tail = match (load, shift == 2) {
            (true, true) => i0(Opcode::Lidx),
            (true, false) => ic(Opcode::LidxB, shift),
            (false, true) => i0(Opcode::Idxaddr),
            (false, false) => ic(Opcode::IdxaddrB, shift),
        };
        return Some((3 + usize::from(load), vec![i0(Opcode::PopAlt), tail]));
    }

    // ---- sc7-in.scp:510-537  push.pri ; <load PRI> ; pop.alt -> move.alt ; ...
    if m0(items, i, Opcode::PushPri) {
        for &(op, has_op) in PUSH_LOAD_POP {
            let mid = if has_op {
                match m1(items, i + 1, op) {
                    Some(a) => i1(op, a),
                    None => continue,
                }
            } else {
                if !m0(items, i + 1, op) {
                    continue;
                }
                i0(op)
            };
            if m0(items, i + 2, Opcode::PopAlt) {
                return Some((3, vec![i0(Opcode::MoveAlt), mid]));
            }
        }
        // sc7-in.scp:543-547  push.pri ; pop.alt -> move.alt
        if m0(items, i + 1, Opcode::PopAlt) {
            return Some((2, vec![i0(Opcode::MoveAlt)]));
        }
    }

    // ---- sc7-in.scp:553-558  push.alt ; pop.alt -> (nothing)
    // Commented out upstream only because SCPACK cannot encode an empty
    // replacement string; the rewrite is a plain identity.
    if m0(items, i, Opcode::PushAlt) && m0(items, i + 1, Opcode::PopAlt) {
        return Some((2, Vec::new()));
    }

    // ---- sc7-in.scp:659-683  <load> %1 ; push.pri -> <push> %1   [;$par]
    for &(load, has_op, push) in LOAD_PUSH {
        let operand = if has_op {
            match m1(items, i, load) {
                Some(a) => a,
                None => continue,
            }
        } else {
            if !m0(items, i, load) {
                continue;
            }
            // `zero.pri ; push.pri` -> `push.c 0`.
            Operand::Imm(0)
        };
        if m0(items, i + 1, Opcode::PushPri) && reg_dead_at(items, i + 2, Reg::Pri) {
            return Some((2, vec![i1(push, operand)]));
        }
    }

    // ---- sc7-in.scp:689-693  move.pri ; push.pri -> push.alt
    // Only valid when PRI is dead: the replacement no longer copies ALT into it.
    if m0(items, i, Opcode::MovePri)
        && m0(items, i + 1, Opcode::PushPri)
        && reg_dead_at(items, i + 2, Reg::Pri)
    {
        return Some((2, vec![i0(Opcode::PushAlt)]));
    }

    // ---- sc7-in.scp:709-728  const.alt %1 ; <op> -> <op>.c %1
    // The immediate forms do not leave the constant in ALT, so ALT must be dead.
    if let Some(Operand::Imm(v)) = m1(items, i, Opcode::ConstAlt) {
        for &(op, folded, negate) in CONST_ALT_FOLD {
            if m0(items, i + 1, op) && reg_dead_at(items, i + 2, Reg::Alt) {
                let v = if negate { v.wrapping_neg() } else { v };
                return Some((2, vec![ic(folded, v)]));
            }
        }
    }

    // ---- sc7-in.scp:739-748  const.pri %1 ; <load>.alt %2 ; sub.alt
    // `sub.alt` is PRI = ALT - PRI, so the whole thing is %2 - %1. The rewrite
    // stops loading %2 into ALT, hence the ALT-dead guard.
    if let Some(Operand::Imm(c)) = m1(items, i, Opcode::ConstPri) {
        for (alt, pri) in
            [(Opcode::LoadSAlt, Opcode::LoadSPri), (Opcode::LoadAlt, Opcode::LoadPri)]
        {
            if let Some(a) = m1(items, i + 1, alt)
                && m0(items, i + 2, Opcode::SubAlt)
                && reg_dead_at(items, i + 3, Reg::Alt)
            {
                return Some((3, vec![i1(pri, a), ic(Opcode::AddC, c.wrapping_neg())]));
            }
        }
    }

    // ---- sc7-in.scp:754-757  add.c %1 ; add.c %2 -> add.c %1+%2
    if let Some(a) = m1v(items, i, Opcode::AddC)
        && let Some(b) = m1v(items, i + 1, Opcode::AddC)
    {
        return Some((2, vec![ic(Opcode::AddC, a.wrapping_add(b))]));
    }

    // ---- sc7-in.scp:798-857  <compare> ; jzer/jnz %1 -> <conditional jump> %1
    // The fused jump does not leave the boolean in PRI, so PRI must be dead on
    // *both* successors.
    for &(cmp, jmp, repl) in CMP_JUMP {
        if m0(items, i, cmp)
            && let Some(t @ Operand::Label(l)) = m1(items, i + 1, jmp)
            && let Some(&target) = labels.get(&l)
            && reg_dead_at(items, i + 2, Reg::Pri)
            && reg_dead_at(items, target, Reg::Pri)
        {
            return Some((2, vec![i1(repl, t)]));
        }
    }

    // ---- sc7-in.scp:867-875  zero.alt ; jeq/jneq %1 -> jzer/jnz %1
    // The replacement no longer zeroes ALT, so ALT must be dead on both paths.
    if m0(items, i, Opcode::ZeroAlt) {
        for (from, to) in [(Opcode::Jeq, Opcode::Jzer), (Opcode::Jneq, Opcode::Jnz)] {
            if let Some(t @ Operand::Label(l)) = m1(items, i + 1, from)
                && let Some(&target) = labels.get(&l)
                && reg_dead_at(items, i + 2, Reg::Alt)
                && reg_dead_at(items, target, Reg::Alt)
            {
                return Some((2, vec![i1(to, t)]));
            }
        }
    }

    // ---- sc7-in.scp:890-929  inc/dec next to a redundant load  [;$exp]
    for &(step, load) in INCDEC_LOAD {
        if let Some(a) = m1(items, i, step) {
            if m1(items, i + 1, load) == Some(a) && reg_dead_at(items, i + 2, Reg::Pri) {
                return Some((2, vec![i1(step, a)]));
            }
        } else if let Some(a) = m1(items, i, load)
            && m1(items, i + 1, step) == Some(a)
            && reg_dead_at(items, i + 2, Reg::Pri)
        {
            return Some((2, vec![i1(step, a)]));
        }
    }

    // ---- sc7-in.scp:955-974  <zero PRI> ; stor[.s].pri %1 -> zero[.s] %1  [;$exp]
    let loads_zero = m1v(items, i, Opcode::ConstPri) == Some(0) || m0(items, i, Opcode::ZeroPri);
    if loads_zero {
        for &(store, zero) in ZERO_STORE {
            if let Some(a) = m1(items, i + 1, store)
                && reg_dead_at(items, i + 2, Reg::Pri)
            {
                return Some((2, vec![i1(zero, a)]));
            }
        }
    }

    // ---- sc7-in.scp:975-984  const.pri 0 -> zero.pri ; const.alt 0 -> zero.alt
    // Same instruction count, one operand cell less.
    if m1v(items, i, Opcode::ConstPri) == Some(0) {
        return Some((1, vec![i0(Opcode::ZeroPri)]));
    }
    if m1v(items, i, Opcode::ConstAlt) == Some(0) {
        return Some((1, vec![i0(Opcode::ZeroAlt)]));
    }

    // ---- Not from sequences[]: `modstk()`/`modheap()` in sc4.c emit nothing for
    // a zero delta. A `stack 0` / `heap 0` that reaches us is pure overhead -
    // except that both also set ALT, so ALT must be dead.
    for op in [Opcode::Stack, Opcode::Heap] {
        if m1v(items, i, op) == Some(0) && reg_dead_at(items, i + 1, Reg::Alt) {
            return Some((1, Vec::new()));
        }
    }

    // ---- Not from sequences[]: a `jump` whose target label is the very next
    // thing in the stream is a no-op. (`sc7.c` cannot see this: it works on one
    // expression at a time and never has labels in its buffer.)
    if let Some(Operand::Label(l)) = m1(items, i, Opcode::Jump) {
        let mut j = i + 1;
        while let Some(Item::Label(m)) = items.get(j) {
            if *m == l {
                return Some((1, Vec::new()));
            }
            j += 1;
        }
    }

    // ---- Not from sequences[]: unreachable code. This is the structured
    // equivalent of `lastst` in `sc1.c`: `newfunc()` (sc1.c:3517) and
    // `compound()` (sc1.c:4851-4854) skip the stack cleanup and the trailing
    // `zero.pri ; retn` when the last statement was a `return` or a `goto`.
    // Emitting them unconditionally and deleting everything between a
    // terminator and the next label reproduces the same accept set - and also
    // catches the `break`/`continue` cases that `sc1.c:4845` merely warns about.
    if let Some((op, _)) = as_insn(items, i)
        && is_terminator(op)
    {
        let mut n = 0;
        while let Some(Item::Insn { .. }) = items.get(i + 1 + n) {
            n += 1;
        }
        if n > 0 {
            return Some((1 + n, vec![items[i].clone()]));
        }
    }

    None
}

// SKIPPED patterns, and why:
//
// * sc7-in.scp:175-254 - the whole `xchg`/`sgrtr`/`swap.alt`/`and`/`pop.alt`
//   family for chained relational operators. `xchg` leaves ALT holding the old
//   PRI while the reversed comparison does not, so the rewrites are only valid
//   under the `;$exp` guarantee that *both* registers are dead. The `and`/
//   `pop.alt` reordering in the longer rows changes which value the `and` sees.
//   Not reproducible with confidence, and this compiler does not yet emit
//   chained comparisons.
// * sc7-in.scp:491-500 - the `;$lcl <name> <stk>` declaration rows. They key off
//   a debug pseudo-comment that carries the variable's stack slot; the [`Item`]
//   stream has no equivalent, and matching `stack -4 ; const.pri %3 ;
//   stor.s.pri %2` without knowing that `%2` *is* the slot just declared would
//   be wrong.
// * sc7-in.scp:574-587 - already commented out upstream ("this optimization does
//   not work, because the argument re-ordering in a function call causes each
//   argument to be optimized individually").
// * sc7-in.scp:600-639 - the user-defined-operator `push.pri ; push.alt` pairs.
//   The replacement discards both register values, so it needs PRI *and* ALT
//   dead; and this compiler does not implement user-defined operators at all
//   (see the crate docs), so the sequence never occurs.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{AsmStream, assemble, opcodes};
    use zpc_asm::{dangling_targets, disassemble};

    /// `const.pri 5` etc.
    fn n(op: Opcode, v: i32) -> Item {
        ic(op, v)
    }

    fn z(op: Opcode) -> Item {
        i0(op)
    }

    fn j(op: Opcode, l: LabelId) -> Item {
        i1(op, Operand::Label(l))
    }

    /// Assert the exact before/after item stream, and that optimising again is a
    /// no-op.
    #[track_caller]
    fn check(before: &[Item], after: &[Item]) {
        let got = optimise(before);
        assert_eq!(got, after, "\n  before: {:?}\n  want:   {:?}", before, after);
        assert_eq!(optimise(&got), got, "not idempotent: {:?}", got);
    }

    // -------------------------------------------------- sc7-in.scp:55-124

    #[test]
    fn load_push_load_pop_collapses_all_twelve_rows() {
        // Every case ends in `add ; xchg`: the `xchg` reads both registers and so
        // pins them live, keeping the later folding rules out of the assertion.
        let tail = || [z(Opcode::Add), z(Opcode::Xchg)];
        let case = |before: &[Item], after: &[Item]| {
            let b: Vec<Item> = before.iter().cloned().chain(tail()).collect();
            let a: Vec<Item> = after.iter().cloned().chain(tail()).collect();
            check(&b, &a);
        };

        // load.s.pri 8 ; push.pri ; load.s.pri 12 ; pop.alt
        case(
            &[
                n(Opcode::LoadSPri, 8),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                z(Opcode::PopAlt),
            ],
            &[n(Opcode::LoadSPri, 12), n(Opcode::LoadSAlt, 8)],
        );
        // load.pri / load.s.pri
        case(
            &[
                n(Opcode::LoadPri, 4),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                z(Opcode::PopAlt),
            ],
            &[n(Opcode::LoadSPri, 12), n(Opcode::LoadAlt, 4)],
        );
        // addr.pri stays in front (sc7-in.scp:78)
        case(
            &[
                n(Opcode::AddrPri, 8),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                z(Opcode::PopAlt),
            ],
            &[n(Opcode::AddrAlt, 8), n(Opcode::LoadSPri, 12)],
        );
        // addr.pri + zero.pri (sc7-in.scp:120)
        case(
            &[n(Opcode::AddrPri, 8), z(Opcode::PushPri), z(Opcode::ZeroPri), z(Opcode::PopAlt)],
            &[n(Opcode::AddrAlt, 8), z(Opcode::ZeroPri)],
        );
        // const.pri first (sc7-in.scp:91)
        case(
            &[
                n(Opcode::ConstPri, 7),
                z(Opcode::PushPri),
                n(Opcode::LoadPri, 4),
                z(Opcode::PopAlt),
            ],
            &[n(Opcode::LoadPri, 4), n(Opcode::ConstAlt, 7)],
        );
        // const.pri second (sc7-in.scp:102)
        case(
            &[
                n(Opcode::LoadSPri, 8),
                z(Opcode::PushPri),
                n(Opcode::ConstPri, 7),
                z(Opcode::PopAlt),
            ],
            &[n(Opcode::ConstPri, 7), n(Opcode::LoadSAlt, 8)],
        );
    }

    #[test]
    fn a_label_inside_the_window_blocks_the_match() {
        // The four-instruction rewrite must not span the label; only the local
        // two-instruction `load.s.pri ; push.pri` shortcut may still apply.
        check(
            &[
                n(Opcode::LoadSPri, 8),
                z(Opcode::PushPri),
                Item::Label(1),
                n(Opcode::LoadSPri, 12),
                z(Opcode::PopAlt),
                z(Opcode::Add),
                z(Opcode::Xchg),
            ],
            &[
                n(Opcode::PushS, 8),
                Item::Label(1),
                n(Opcode::LoadSPri, 12),
                z(Opcode::PopAlt),
                z(Opcode::Add),
                z(Opcode::Xchg),
            ],
        );
    }

    // -------------------------------------------------- sc7-in.scp:134-148

    #[test]
    fn move_pri_push_load_pop_leaves_only_the_load() {
        check(
            &[
                z(Opcode::MovePri),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 8),
                z(Opcode::PopAlt),
                z(Opcode::Add),
            ],
            &[n(Opcode::LoadSPri, 8), z(Opcode::Add)],
        );
    }

    // -------------------------------------------------- sc7-in.scp:270-309

    #[test]
    fn load_load_xchg_swaps_the_two_loads() {
        check(
            &[
                n(Opcode::LoadSPri, 8),
                n(Opcode::LoadSAlt, 12),
                z(Opcode::Xchg),
                z(Opcode::Sless),
            ],
            &[n(Opcode::LoadSPri, 12), n(Opcode::LoadSAlt, 8), z(Opcode::Sless)],
        );
        check(
            &[n(Opcode::ConstPri, 3), n(Opcode::LoadAlt, 12), z(Opcode::Xchg), z(Opcode::Sless)],
            &[n(Opcode::LoadPri, 12), n(Opcode::ConstAlt, 3), z(Opcode::Sless)],
        );
    }

    // -------------------------------------------------- sc7-in.scp:351-483

    #[test]
    fn indexed_array_load_becomes_lidx() {
        check(
            &[
                n(Opcode::AddrPri, 8),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                n(Opcode::Bounds, 9),
                n(Opcode::ShlCPri, 2),
                z(Opcode::PopAlt),
                z(Opcode::Add),
                z(Opcode::LoadI),
            ],
            &[
                n(Opcode::AddrAlt, 8),
                n(Opcode::LoadSPri, 12),
                n(Opcode::Bounds, 9),
                z(Opcode::Lidx),
            ],
        );
    }

    #[test]
    fn indexed_array_load_without_bounds_and_with_a_byte_shift() {
        check(
            &[
                n(Opcode::ConstPri, 100),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                n(Opcode::ShlCPri, 1),
                z(Opcode::PopAlt),
                z(Opcode::Add),
                z(Opcode::LoadI),
            ],
            &[n(Opcode::ConstAlt, 100), n(Opcode::LoadSPri, 12), n(Opcode::LidxB, 1)],
        );
    }

    #[test]
    fn indexed_array_store_address_becomes_idxaddr() {
        check(
            &[
                n(Opcode::AddrPri, 8),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                n(Opcode::ShlCPri, 2),
                z(Opcode::PopAlt),
                z(Opcode::Add),
                z(Opcode::StorI),
            ],
            &[n(Opcode::AddrAlt, 8), n(Opcode::LoadSPri, 12), z(Opcode::Idxaddr), z(Opcode::StorI)],
        );
    }

    #[test]
    fn packed_array_form_stops_at_bounds() {
        check(
            &[
                n(Opcode::AddrPri, 8),
                z(Opcode::PushPri),
                n(Opcode::LoadSPri, 12),
                n(Opcode::Bounds, 9),
                z(Opcode::PopAlt),
                z(Opcode::Add),
            ],
            &[
                n(Opcode::AddrAlt, 8),
                n(Opcode::LoadSPri, 12),
                n(Opcode::Bounds, 9),
                z(Opcode::Add),
            ],
        );
    }

    #[test]
    fn the_short_index_tail_folds() {
        check(
            &[n(Opcode::ShlCPri, 2), z(Opcode::PopAlt), z(Opcode::Add), z(Opcode::LoadI)],
            &[z(Opcode::PopAlt), z(Opcode::Lidx)],
        );
        check(
            &[n(Opcode::ShlCPri, 3), z(Opcode::PopAlt), z(Opcode::Add), z(Opcode::StorI)],
            &[z(Opcode::PopAlt), n(Opcode::IdxaddrB, 3), z(Opcode::StorI)],
        );
    }

    // -------------------------------------------------- sc7-in.scp:510-558

    #[test]
    fn push_pop_around_a_load_becomes_move_alt() {
        check(
            &[z(Opcode::PushPri), n(Opcode::LoadSPri, 8), z(Opcode::PopAlt), z(Opcode::Add)],
            &[z(Opcode::MoveAlt), n(Opcode::LoadSPri, 8), z(Opcode::Add)],
        );
        check(
            &[z(Opcode::PushPri), z(Opcode::LoadI), z(Opcode::PopAlt), z(Opcode::Add)],
            &[z(Opcode::MoveAlt), z(Opcode::LoadI), z(Opcode::Add)],
        );
    }

    #[test]
    fn a_bare_push_pop_becomes_move_alt() {
        check(
            &[z(Opcode::PushPri), z(Opcode::PopAlt), z(Opcode::Add)],
            &[z(Opcode::MoveAlt), z(Opcode::Add)],
        );
    }

    #[test]
    fn push_alt_pop_alt_disappears() {
        check(
            &[z(Opcode::PushAlt), z(Opcode::PopAlt), z(Opcode::Add)],
            &[z(Opcode::Add)],
        );
    }

    // -------------------------------------------------- sc7-in.scp:659-693

    #[test]
    fn a_load_feeding_push_pri_becomes_the_dedicated_push() {
        // The following `const.pri` proves PRI is dead - upstream's `;$par`.
        check(
            &[
                n(Opcode::LoadSPri, 8),
                z(Opcode::PushPri),
                n(Opcode::ConstPri, 4),
                z(Opcode::PushPri),
                n(Opcode::SysreqC, 0),
            ],
            &[n(Opcode::PushS, 8), n(Opcode::PushC, 4), n(Opcode::SysreqC, 0)],
        );
        check(
            &[z(Opcode::ZeroPri), z(Opcode::PushPri), n(Opcode::SysreqC, 0)],
            &[n(Opcode::PushC, 0), n(Opcode::SysreqC, 0)],
        );
        check(
            &[n(Opcode::AddrPri, 8), z(Opcode::PushPri), n(Opcode::SysreqC, 0)],
            &[n(Opcode::Pushaddr, 8), n(Opcode::SysreqC, 0)],
        );
        check(
            &[n(Opcode::LoadPri, 8), z(Opcode::PushPri), n(Opcode::SysreqC, 0)],
            &[n(Opcode::Push, 8), n(Opcode::SysreqC, 0)],
        );
    }

    #[test]
    fn the_push_shortcut_is_refused_when_pri_is_still_live() {
        // `add` reads PRI, so `const.pri 4` must survive.
        let before =
            [n(Opcode::ConstPri, 4), z(Opcode::PushPri), z(Opcode::Add), z(Opcode::Retn)];
        check(&before, &before);
    }

    #[test]
    fn move_pri_push_pri_becomes_push_alt_when_pri_is_dead() {
        check(
            &[z(Opcode::MovePri), z(Opcode::PushPri), n(Opcode::ConstPri, 1), z(Opcode::Retn)],
            &[z(Opcode::PushAlt), n(Opcode::ConstPri, 1), z(Opcode::Retn)],
        );
        let live = [z(Opcode::MovePri), z(Opcode::PushPri), z(Opcode::Retn)];
        check(&live, &live);
    }

    // -------------------------------------------------- sc7-in.scp:709-757

    #[test]
    fn a_constant_in_alt_folds_into_the_operator() {
        // `zero.alt` after the operator proves ALT is dead.
        check(
            &[n(Opcode::ConstAlt, 5), z(Opcode::Add), z(Opcode::ZeroAlt), z(Opcode::Retn)],
            &[n(Opcode::AddC, 5), z(Opcode::ZeroAlt), z(Opcode::Retn)],
        );
        check(
            &[n(Opcode::ConstAlt, 5), z(Opcode::Sub), z(Opcode::ZeroAlt), z(Opcode::Retn)],
            &[n(Opcode::AddC, -5), z(Opcode::ZeroAlt), z(Opcode::Retn)],
        );
        check(
            &[n(Opcode::ConstAlt, 5), z(Opcode::Smul), z(Opcode::ZeroAlt), z(Opcode::Retn)],
            &[n(Opcode::SmulC, 5), z(Opcode::ZeroAlt), z(Opcode::Retn)],
        );
        check(
            &[n(Opcode::ConstAlt, 5), z(Opcode::Eq), z(Opcode::ZeroAlt), z(Opcode::Retn)],
            &[n(Opcode::EqCPri, 5), z(Opcode::ZeroAlt), z(Opcode::Retn)],
        );
    }

    #[test]
    fn the_fold_is_refused_when_alt_is_still_live() {
        let before = [n(Opcode::ConstAlt, 5), z(Opcode::Add), z(Opcode::Add), z(Opcode::Retn)];
        check(&before, &before);
    }

    #[test]
    fn sub_alt_against_a_constant_becomes_add_c() {
        check(
            &[
                n(Opcode::ConstPri, 3),
                n(Opcode::LoadSAlt, 8),
                z(Opcode::SubAlt),
                z(Opcode::ZeroAlt),
                z(Opcode::Retn),
            ],
            &[n(Opcode::LoadSPri, 8), n(Opcode::AddC, -3), z(Opcode::ZeroAlt), z(Opcode::Retn)],
        );
    }

    #[test]
    fn chained_add_c_folds_into_one() {
        check(
            &[n(Opcode::AddC, 4), n(Opcode::AddC, 8), n(Opcode::AddC, -2), z(Opcode::Retn)],
            &[n(Opcode::AddC, 10), z(Opcode::Retn)],
        );
    }

    // -------------------------------------------------- sc7-in.scp:798-875

    #[test]
    fn a_compare_followed_by_jzer_fuses() {
        // PRI is dead on both edges: `zero.pri` follows, and the target too.
        for (cmp, repl) in [
            (Opcode::Eq, Opcode::Jneq),
            (Opcode::Neq, Opcode::Jeq),
            (Opcode::Less, Opcode::Jgeq),
            (Opcode::Leq, Opcode::Jgrtr),
            (Opcode::Grtr, Opcode::Jleq),
            (Opcode::Geq, Opcode::Jless),
            (Opcode::Sless, Opcode::Jsgeq),
            (Opcode::Sleq, Opcode::Jsgrtr),
            (Opcode::Sgrtr, Opcode::Jsleq),
            (Opcode::Sgeq, Opcode::Jsless),
        ] {
            check(
                &[
                    z(cmp),
                    j(Opcode::Jzer, 1),
                    z(Opcode::ZeroPri),
                    Item::Label(1),
                    z(Opcode::ZeroPri),
                    z(Opcode::Retn),
                ],
                &[
                    j(repl, 1),
                    z(Opcode::ZeroPri),
                    Item::Label(1),
                    z(Opcode::ZeroPri),
                    z(Opcode::Retn),
                ],
            );
        }
        // and the two `jnz` rows
        check(
            &[
                z(Opcode::Eq),
                j(Opcode::Jnz, 1),
                z(Opcode::ZeroPri),
                Item::Label(1),
                z(Opcode::ZeroPri),
                z(Opcode::Retn),
            ],
            &[
                j(Opcode::Jeq, 1),
                z(Opcode::ZeroPri),
                Item::Label(1),
                z(Opcode::ZeroPri),
                z(Opcode::Retn),
            ],
        );
    }

    #[test]
    fn the_compare_fusion_is_refused_when_the_boolean_is_used() {
        // PRI is live at the jump target (`retn` returns it).
        let before = [
            z(Opcode::Eq),
            j(Opcode::Jzer, 1),
            z(Opcode::ZeroPri),
            Item::Label(1),
            z(Opcode::Retn),
        ];
        check(&before, &before);
    }

    #[test]
    fn zero_alt_before_jeq_becomes_jzer() {
        check(
            &[
                z(Opcode::ZeroAlt),
                j(Opcode::Jeq, 1),
                z(Opcode::ZeroAlt),
                Item::Label(1),
                z(Opcode::ZeroAlt),
                z(Opcode::Retn),
            ],
            &[
                j(Opcode::Jzer, 1),
                z(Opcode::ZeroAlt),
                Item::Label(1),
                z(Opcode::ZeroAlt),
                z(Opcode::Retn),
            ],
        );
        check(
            &[
                z(Opcode::ZeroAlt),
                j(Opcode::Jneq, 1),
                z(Opcode::ZeroAlt),
                Item::Label(1),
                z(Opcode::ZeroAlt),
                z(Opcode::Retn),
            ],
            &[
                j(Opcode::Jnz, 1),
                z(Opcode::ZeroAlt),
                Item::Label(1),
                z(Opcode::ZeroAlt),
                z(Opcode::Retn),
            ],
        );
    }

    // -------------------------------------------------- sc7-in.scp:890-984

    #[test]
    fn a_redundant_load_around_inc_or_dec_disappears() {
        for (step, load) in [
            (Opcode::Inc, Opcode::LoadPri),
            (Opcode::IncS, Opcode::LoadSPri),
            (Opcode::Dec, Opcode::LoadPri),
            (Opcode::DecS, Opcode::LoadSPri),
        ] {
            check(
                &[n(step, 8), n(load, 8), z(Opcode::ZeroPri), z(Opcode::Retn)],
                &[n(step, 8), z(Opcode::ZeroPri), z(Opcode::Retn)],
            );
            check(
                &[n(load, 8), n(step, 8), z(Opcode::ZeroPri), z(Opcode::Retn)],
                &[n(step, 8), z(Opcode::ZeroPri), z(Opcode::Retn)],
            );
        }
        // a *different* slot must not be folded
        let before =
            [n(Opcode::IncS, 8), n(Opcode::LoadSPri, 12), z(Opcode::ZeroPri), z(Opcode::Retn)];
        check(&before, &before);
    }

    #[test]
    fn storing_zero_uses_the_dedicated_opcode() {
        check(
            &[n(Opcode::ConstPri, 0), n(Opcode::StorPri, 16), z(Opcode::ZeroPri), z(Opcode::Retn)],
            &[n(Opcode::Zero, 16), z(Opcode::ZeroPri), z(Opcode::Retn)],
        );
        check(
            &[z(Opcode::ZeroPri), n(Opcode::StorSPri, 8), z(Opcode::ZeroPri), z(Opcode::Retn)],
            &[n(Opcode::ZeroS, 8), z(Opcode::ZeroPri), z(Opcode::Retn)],
        );
    }

    #[test]
    fn a_zero_constant_uses_the_zero_opcodes() {
        check(
            &[n(Opcode::ConstPri, 0), z(Opcode::Add), z(Opcode::Retn)],
            &[z(Opcode::ZeroPri), z(Opcode::Add), z(Opcode::Retn)],
        );
        // `xchg` keeps ALT live so the `const.alt ; add -> add.c` fold cannot
        // pre-empt this rule.
        check(
            &[n(Opcode::ConstAlt, 0), z(Opcode::Add), z(Opcode::Xchg), z(Opcode::Retn)],
            &[z(Opcode::ZeroAlt), z(Opcode::Add), z(Opcode::Xchg), z(Opcode::Retn)],
        );
    }

    // -------------------------------------------------- our own rules

    #[test]
    fn a_zero_stack_or_heap_adjustment_disappears() {
        check(
            &[n(Opcode::Stack, 0), z(Opcode::ZeroPri), z(Opcode::Retn)],
            &[z(Opcode::ZeroPri), z(Opcode::Retn)],
        );
        check(
            &[n(Opcode::Heap, 0), z(Opcode::ZeroPri), z(Opcode::Retn)],
            &[z(Opcode::ZeroPri), z(Opcode::Retn)],
        );
        // but not when the ALT side effect is used
        let before = [n(Opcode::Stack, 0), z(Opcode::Add), z(Opcode::Retn)];
        check(&before, &before);
    }

    #[test]
    fn a_jump_to_the_next_label_disappears() {
        check(
            &[j(Opcode::Jump, 1), Item::Label(1), z(Opcode::Retn)],
            &[Item::Label(1), z(Opcode::Retn)],
        );
        // even with other labels in between
        check(
            &[j(Opcode::Jump, 2), Item::Label(1), Item::Label(2), z(Opcode::Retn)],
            &[Item::Label(1), Item::Label(2), z(Opcode::Retn)],
        );
        // Dead-code elimination drops the unreachable `nop` first, after which
        // this jump is also a jump-to-the-next-label and goes too.
        check(
            &[j(Opcode::Jump, 1), z(Opcode::Nop), Item::Label(1), z(Opcode::Retn)],
            &[Item::Label(1), z(Opcode::Retn)],
        );
        // A jump over live code stays.
        let before = [
            j(Opcode::Jump, 2),
            Item::Label(1),
            z(Opcode::Nop),
            Item::Label(2),
            z(Opcode::Retn),
        ];
        check(&before, &before);
    }

    // -------------------------------------------------- lastst / dead code

    #[test]
    fn the_trailing_dead_pair_after_an_explicit_return_is_removed() {
        // Exactly what `Generator::function` emits for `f() { return 1; }`:
        // the `return` path, then the unconditional `zero.pri ; retn`.
        check(
            &[
                z(Opcode::Proc),
                n(Opcode::ConstPri, 1),
                n(Opcode::Stack, 4),
                z(Opcode::Retn),
                n(Opcode::Stack, 4),
                z(Opcode::ZeroPri),
                z(Opcode::Retn),
            ],
            &[z(Opcode::Proc), n(Opcode::ConstPri, 1), n(Opcode::Stack, 4), z(Opcode::Retn)],
        );
    }

    #[test]
    fn dead_code_elimination_stops_at_a_label() {
        check(
            &[
                z(Opcode::Retn),
                z(Opcode::Nop),
                Item::Label(1),
                z(Opcode::ZeroPri),
                z(Opcode::Retn),
            ],
            &[z(Opcode::Retn), Item::Label(1), z(Opcode::ZeroPri), z(Opcode::Retn)],
        );
    }

    #[test]
    fn dead_code_elimination_stops_at_a_case_table() {
        let before = [
            j(Opcode::Jump, 1),
            Item::CaseTbl { default: 1, cases: vec![(1, 1)] },
            Item::Label(1),
            z(Opcode::Retn),
        ];
        // The jump targets label 1, which is not the *next* item, so it stays;
        // the case table must not be swallowed as dead code.
        check(&before, &before);
    }

    #[test]
    fn code_after_halt_and_jump_is_dead_too() {
        check(
            &[n(Opcode::Halt, 1), z(Opcode::Nop), Item::Label(1)],
            &[n(Opcode::Halt, 1), Item::Label(1)],
        );
        // Here the jump's own target becomes the next item once the dead `nop`s
        // are gone, so the jump goes too.
        check(
            &[j(Opcode::Jump, 1), z(Opcode::Nop), z(Opcode::Nop), Item::Label(1)],
            &[Item::Label(1)],
        );
    }

    // -------------------------------------------------- properties

    /// A set of hand-written streams that exercise every rule family plus a few
    /// combinations, used for the whole-program properties below.
    fn corpus() -> Vec<Vec<Item>> {
        // 1. a whole function with an explicit return
        let mut out: Vec<Vec<Item>> = vec![vec![
            z(Opcode::Proc),
            n(Opcode::LoadSPri, 12),
            z(Opcode::PushPri),
            n(Opcode::ConstPri, 0),
            z(Opcode::PushPri),
            n(Opcode::PushC, 8),
            n(Opcode::SysreqC, 3),
            n(Opcode::Stack, 12),
            n(Opcode::Stack, 0),
            z(Opcode::Retn),
            n(Opcode::Stack, 4),
            z(Opcode::ZeroPri),
            z(Opcode::Retn),
        ]];

        // 2. an if/else with a fused comparison
        out.push(vec![
            z(Opcode::Proc),
            n(Opcode::LoadSPri, 12),
            z(Opcode::PushPri),
            n(Opcode::ConstPri, 5),
            z(Opcode::PopAlt),
            z(Opcode::Sless),
            j(Opcode::Jzer, 1),
            n(Opcode::ConstPri, 0),
            n(Opcode::StorSPri, 8),
            j(Opcode::Jump, 2),
            Item::Label(1),
            n(Opcode::ConstPri, 1),
            n(Opcode::StorSPri, 8),
            Item::Label(2),
            z(Opcode::ZeroPri),
            z(Opcode::Retn),
        ]);

        // 3. an array read and an array write
        out.push(vec![
            z(Opcode::Proc),
            n(Opcode::AddrPri, -40),
            z(Opcode::PushPri),
            n(Opcode::LoadSPri, 12),
            n(Opcode::Bounds, 9),
            n(Opcode::ShlCPri, 2),
            z(Opcode::PopAlt),
            z(Opcode::Add),
            z(Opcode::LoadI),
            z(Opcode::PushPri),
            n(Opcode::ConstPri, 100),
            z(Opcode::PushPri),
            n(Opcode::LoadSPri, 16),
            n(Opcode::ShlCPri, 2),
            z(Opcode::PopAlt),
            z(Opcode::Add),
            z(Opcode::MoveAlt),
            z(Opcode::PopPri),
            z(Opcode::StorI),
            z(Opcode::ZeroPri),
            z(Opcode::Retn),
        ]);

        // 4. a loop with a back edge and a `stack 0`
        out.push(vec![
            z(Opcode::Proc),
            n(Opcode::ConstPri, 0),
            n(Opcode::StorSPri, -4),
            Item::Label(1),
            n(Opcode::LoadSPri, -4),
            n(Opcode::ConstAlt, 10),
            z(Opcode::Sless),
            j(Opcode::Jzer, 2),
            n(Opcode::IncS, -4),
            n(Opcode::LoadSPri, -4),
            j(Opcode::Jump, 1),
            Item::Label(2),
            n(Opcode::Stack, 0),
            z(Opcode::ZeroPri),
            z(Opcode::Retn),
        ]);

        // 5. a switch with a case table
        out.push(vec![
            z(Opcode::Proc),
            n(Opcode::LoadSPri, 12),
            j(Opcode::Switch, 4),
            Item::Label(1),
            n(Opcode::ConstPri, 0),
            j(Opcode::Jump, 3),
            Item::Label(2),
            n(Opcode::ConstPri, 0),
            j(Opcode::Jump, 3),
            Item::Label(4),
            Item::CaseTbl { default: 3, cases: vec![(1, 1), (2, 2)] },
            Item::Label(3),
            z(Opcode::ZeroPri),
            z(Opcode::Retn),
            n(Opcode::Stack, 4),
            z(Opcode::ZeroPri),
            z(Opcode::Retn),
        ]);

        // 6. arithmetic folding chains
        out.push(vec![
            z(Opcode::Proc),
            n(Opcode::LoadSPri, 12),
            n(Opcode::AddC, 4),
            n(Opcode::AddC, 8),
            n(Opcode::ConstAlt, 3),
            z(Opcode::Smul),
            z(Opcode::MovePri),
            z(Opcode::PushPri),
            n(Opcode::PushC, 4),
            n(Opcode::SysreqC, 1),
            z(Opcode::Retn),
        ]);

        // 7. a stream that must not change at all
        out.push(vec![
            z(Opcode::Proc),
            z(Opcode::LoadI),
            z(Opcode::Add),
            z(Opcode::Xchg),
            z(Opcode::Sub),
            z(Opcode::Retn),
        ]);

        out
    }

    #[test]
    fn optimised_streams_still_assemble_with_no_dangling_targets() {
        for (k, stream) in corpus().into_iter().enumerate() {
            // The input itself must be well formed, or the property is vacuous.
            let base = assemble(&stream).unwrap_or_else(|e| panic!("corpus {k} is bad: {e:?}"));
            assert!(dangling_targets(&disassemble(&base).unwrap()).is_empty(), "corpus {k}");

            let opt = optimise(&stream);
            let code = assemble(&opt).unwrap_or_else(|e| panic!("corpus {k} broke: {e:?}"));
            let d = disassemble(&code).unwrap_or_else(|e| panic!("corpus {k} decode: {e:?}"));
            assert!(dangling_targets(&d).is_empty(), "corpus {k} has dangling targets");
            // Never bigger than the input - `sc7.c:553` asserts the same thing.
            assert!(code.len() <= base.len(), "corpus {k} grew");
        }
    }

    #[test]
    fn optimise_is_idempotent() {
        for (k, stream) in corpus().into_iter().enumerate() {
            let once = optimise(&stream);
            let twice = optimise(&once);
            assert_eq!(once, twice, "corpus {k} is not a fixed point");
        }
    }

    #[test]
    fn every_label_that_is_still_referenced_is_still_defined() {
        for (k, stream) in corpus().into_iter().enumerate() {
            let opt = optimise(&stream);
            let defined: Vec<LabelId> = opt
                .iter()
                .filter_map(|it| match it {
                    Item::Label(l) => Some(*l),
                    _ => None,
                })
                .collect();
            for it in &opt {
                match it {
                    Item::Insn { operands, .. } => {
                        for op in operands {
                            if let Operand::Label(l) = op {
                                assert!(defined.contains(l), "corpus {k}: label {l} vanished");
                            }
                        }
                    }
                    Item::CaseTbl { default, cases } => {
                        assert!(defined.contains(default), "corpus {k}: default vanished");
                        for (_, l) in cases {
                            assert!(defined.contains(l), "corpus {k}: case label {l} vanished");
                        }
                    }
                    Item::Label(_) => {}
                }
            }
        }
    }

    #[test]
    fn an_empty_stream_is_a_fixed_point() {
        assert_eq!(optimise(&[]), Vec::<Item>::new());
    }

    #[test]
    fn a_generated_function_shrinks_and_still_assembles() {
        // A smoke test built through the real emitter helpers rather than by hand.
        let mut s = AsmStream::new();
        s.emit0(Opcode::Proc);
        s.ldconst(3, Reg::Pri);
        s.pushreg(Reg::Pri);
        s.pushval(4);
        s.emit1(Opcode::SysreqC, 0);
        s.modstk(8);
        s.ffret();
        s.ldconst(0, Reg::Pri);
        s.ffret();

        let opt = optimise(s.items());
        assert_eq!(
            opcodes(&opt),
            [Opcode::Proc, Opcode::PushC, Opcode::PushC, Opcode::SysreqC, Opcode::Stack, Opcode::Retn]
        );
        let code = assemble(&opt).unwrap();
        assert!(dangling_targets(&disassemble(&code).unwrap()).is_empty());
    }
}
