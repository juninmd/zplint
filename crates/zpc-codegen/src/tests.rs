//! Behavioural tests: every one asserts the emitted opcode *sequence*, not just
//! that codegen survived. Where an operand carries meaning (a frame offset, the
//! argument-count cell, a case value) it is asserted too.

use zpc_asm::{Opcode, Style, dangling_targets, disassemble, render};
use zpc_diag::Diagnostics;
use zpc_lex::Scanner;

use crate::emit::{Generator, Unit};
use crate::stream::{Item, Operand, opcodes};

// ------------------------------------------------------------------ helpers

fn compile(src: &str) -> Unit {
    let mut d = Diagnostics::new();
    let tokens = Scanner::new(src, "t.sma").scan(&mut d);
    let (program, pdiags) = zpc_parse::parse(src, &tokens, "t.sma");
    assert!(
        !pdiags.items().iter().any(|i| i.is_error()),
        "source did not parse cleanly:\n{src}\n{:?}",
        pdiags.items()
    );
    Generator::new("t.sma").program(&program)
}

/// The instructions of the *last* function body, i.e. everything strictly after
/// its `proc`. Tests put the function under scrutiny last, so helper callees can
/// be declared above it.
fn body(src: &str) -> Vec<Item> {
    let unit = compile(src);
    let start = unit
        .code
        .iter()
        .rposition(|i| matches!(i, Item::Insn { opcode: Opcode::Proc, .. }))
        .expect("no function was emitted");
    unit.code[start + 1..].to_vec()
}

fn body_ops(src: &str) -> Vec<Opcode> {
    opcodes(&body(src))
}

/// Every immediate operand in order, ignoring labels.
fn immediates(items: &[Item]) -> Vec<i32> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Insn { operands, .. } => Some(operands),
            _ => None,
        })
        .flatten()
        .filter_map(|o| match o {
            Operand::Imm(v) => Some(*v),
            Operand::Label(_) => None,
        })
        .collect()
}

/// Every immediate carried by one particular opcode, in order.
fn imms_of(items: &[Item], opcode: Opcode) -> Vec<i32> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Insn { opcode: o, operands } if *o == opcode => match operands.first() {
                Some(Operand::Imm(v)) => Some(*v),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn first_imm(items: &[Item], opcode: Opcode) -> i32 {
    *imms_of(items, opcode)
        .first()
        .unwrap_or_else(|| panic!("no {} in the stream", opcode.mnemonic()))
}

/// Assemble, disassemble and render with symbolic labels. Also proves that every
/// jump lands on an instruction boundary.
fn normalised(src: &str) -> String {
    let unit = compile(src);
    let code = unit.assemble().expect("labels must all resolve");
    let d = disassemble(&code).expect("the emitted code must decode");
    assert!(dangling_targets(&d).is_empty(), "dangling jump targets: {:?}", dangling_targets(&d));
    render(&d, Style::Normalised)
}

/// Where the index-vector cell at `i` points: the offsets that
/// `adjust_indirectiontables()` stores are relative to the address of the cell
/// that holds them, so cell `i` selects cell `(i*cell + data[i]) / cell`.
fn follows(data: &[i32], i: usize) -> usize {
    (i * 4 + data[i] as usize) / 4
}

// ------------------------------------------------------------ frame layout

#[test]
fn a_program_starts_with_halt_zero() {
    // writeleader(): a subroutine returning to address 0 must find a HALT there.
    let unit = compile("foo() {}");
    assert!(matches!(unit.code[0], Item::Insn { opcode: Opcode::Halt, .. }));
    assert_eq!(immediates(&unit.code[..1]), vec![0]);
}

#[test]
fn an_empty_function_is_proc_zero_retn() {
    assert_eq!(body_ops("foo() {}"), [Opcode::ZeroPri, Opcode::Retn]);
}

#[test]
fn arguments_are_addressed_at_positive_offsets_from_frm() {
    // sc1.c declargs(): doarg(..., (argcnt+3)*sizeof(cell), ...)
    let items = body("foo(a, b, c) { return c; }");
    assert_eq!(first_imm(&items, Opcode::LoadSPri), 5 * 4, "argument 2 lives at FRM+20");
    let items = body("foo(a) { return a; }");
    assert_eq!(first_imm(&items, Opcode::LoadSPri), 3 * 4, "argument 0 lives at FRM+12");
}

#[test]
fn locals_are_allocated_downwards_and_released_on_exit() {
    // declloc(): declared += size; addr = -declared*cell; modstk(-size*cell)
    let items = body("foo() { new a = 5; }");
    assert_eq!(
        opcodes(&items),
        [
            Opcode::Stack,    // reserve one cell
            Opcode::ConstPri, // the initialiser
            Opcode::StorSPri, // store into the slot
            Opcode::Stack,    // newfunc() releases the frame
            Opcode::ZeroPri,
            Opcode::Retn,
        ]
    );
    assert_eq!(immediates(&items), vec![-4, 5, -4, 4]);
}

#[test]
fn an_uninitialised_local_is_zeroed() {
    // "uninitialized variable, set to zero"
    assert_eq!(
        body_ops("foo() { new a; }"),
        [
            Opcode::Stack,
            Opcode::ZeroPri,
            Opcode::StorSPri,
            Opcode::Stack,
            Opcode::ZeroPri,
            Opcode::Retn
        ]
    );
}

#[test]
fn a_nested_block_releases_only_its_own_locals() {
    let items = body("foo() { new a; { new b; } }");
    assert_eq!(
        imms_of(&items, Opcode::Stack),
        vec![-4, -4, 4, 4],
        "the inner block gives back exactly one cell"
    );
}

#[test]
fn globals_live_in_the_data_segment_and_are_reached_with_load_pri() {
    let unit = compile("new g = 7; foo() { return g; }");
    assert_eq!(unit.data, vec![7]);
    let start =
        unit.code.iter().position(|i| matches!(i, Item::Insn { opcode: Opcode::Proc, .. }));
    let items = &unit.code[start.unwrap() + 1..];
    assert_eq!(opcodes(items), [Opcode::LoadPri, Opcode::Retn, Opcode::ZeroPri, Opcode::Retn]);
    assert_eq!(first_imm(items, Opcode::LoadPri), 0, "the first global sits at data offset 0");
}

// ------------------------------------------------------------------- calls

#[test]
fn arguments_are_pushed_right_to_left() {
    // sc3.c:1930 stgmark(sSTARTREORDER) .. sc3.c:2280 stgmark(sENDREORDER) wrap
    // one sEXPRSTART+argpos block per argument (sc3.c:1976); sc7.c:266-270 then
    // flushes them with `while (argc) { argc--; stgstring(stack[argc]...) }`,
    // i.e. in descending argument index. So the LAST argument is pushed FIRST.
    let items = body("bar(a, b, c) {} foo() { bar(1, 2, 3); }");
    assert_eq!(
        imms_of(&items, Opcode::ConstPri),
        vec![3, 2, 1],
        "argument 3 is loaded and pushed first"
    );
    assert_eq!(
        opcodes(&items)[..7],
        [
            Opcode::ConstPri,
            Opcode::PushPri,
            Opcode::ConstPri,
            Opcode::PushPri,
            Opcode::ConstPri,
            Opcode::PushPri,
            Opcode::PushC, // the argument count cell
        ]
    );
}

#[test]
fn the_argument_count_cell_holds_the_byte_count() {
    // sc3.c:2281 pushval((cell)nargs*sizeof(cell))
    let items = body("bar(a, b, c) {} foo() { bar(1, 2, 3); }");
    assert_eq!(first_imm(&items, Opcode::PushC), 3 * 4);

    let items = body("bar() {} foo() { bar(); }");
    assert_eq!(first_imm(&items, Opcode::PushC), 0, "a no-argument call still pushes the count");
}

#[test]
fn a_native_call_uses_sysreq_c_and_the_caller_cleans_the_stack() {
    // ffcall(): "sysreq.c <id>" then "stack (numargs+1)*sizeof(cell)"
    let items = body("native n(a); foo() { n(7); }");
    assert_eq!(
        opcodes(&items)[..5],
        [Opcode::ConstPri, Opcode::PushPri, Opcode::PushC, Opcode::SysreqC, Opcode::Stack]
    );
    assert_eq!(first_imm(&items, Opcode::SysreqC), 0, "the first native gets index 0");
    assert_eq!(first_imm(&items, Opcode::Stack), 2 * 4, "one argument plus the count cell");
}

#[test]
fn a_reference_argument_is_passed_by_address() {
    let ops = body_ops("bar(&a) {} foo() { new x; bar(x); }");
    assert!(ops.windows(2).any(|w| w == [Opcode::AddrPri, Opcode::PushPri]), "got {ops:?}");
}

#[test]
fn an_array_argument_passes_the_base_address() {
    let ops = body_ops("bar(a[]) {} foo() { new arr[4]; bar(arr); }");
    assert!(ops.windows(2).any(|w| w == [Opcode::AddrPri, Opcode::PushPri]), "got {ops:?}");
}

#[test]
fn an_omitted_argument_takes_its_declared_default() {
    let items = body("bar(a, b = 9) {} foo() { bar(1); }");
    assert_eq!(
        imms_of(&items, Opcode::ConstPri),
        vec![9, 1],
        "the default fills slot 1 and is therefore pushed first"
    );
    assert_eq!(first_imm(&items, Opcode::PushC), 2 * 4);
}

#[test]
fn a_named_argument_lands_in_its_declared_slot() {
    let items = body("bar(a, b) {} foo() { bar(.b = 2, .a = 1); }");
    assert_eq!(
        imms_of(&items, Opcode::ConstPri),
        vec![2, 1],
        "b is argument 1, so it is still pushed first"
    );
}

// --------------------------------------------------------------- operators

#[test]
fn a_binary_operator_finds_its_left_operand_in_alt() {
    // plnge2(): push.pri the left operand, evaluate the right, pop.alt.
    assert_eq!(
        body_ops("foo(a, b) { return a - b; }")[..5],
        [Opcode::LoadSPri, Opcode::PushPri, Opcode::LoadSPri, Opcode::PopAlt, Opcode::SubAlt]
    );
}

#[test]
fn a_commutative_operator_with_a_constant_skips_the_push_and_pop() {
    // plnge2() calls stgdel() to scratch the pushreg when commutative() holds.
    assert_eq!(
        body_ops("foo(a) { return a + 1; }")[..3],
        [Opcode::LoadSPri, Opcode::ConstAlt, Opcode::Add]
    );
    // Subtraction is not commutative, so the pair stays.
    assert_eq!(
        body_ops("foo(a) { return a - 1; }")[..5],
        [Opcode::LoadSPri, Opcode::PushPri, Opcode::ConstPri, Opcode::PopAlt, Opcode::SubAlt]
    );
}

#[test]
fn every_arithmetic_operator_maps_to_its_sc4_emitter() {
    let cases: &[(&str, &[Opcode])] = &[
        ("a * b", &[Opcode::Smul]),
        ("a / b", &[Opcode::SdivAlt]),
        // os_mod(): sdiv.alt leaves the remainder in ALT, so move it across.
        ("a % b", &[Opcode::SdivAlt, Opcode::MovePri]),
        ("a << b", &[Opcode::Xchg, Opcode::Shl]),
        ("a >> b", &[Opcode::Xchg, Opcode::Sshr]),
        ("a >>> b", &[Opcode::Xchg, Opcode::Shr]),
        ("a | b", &[Opcode::Or]),
        ("a ^ b", &[Opcode::Xor]),
        ("a & b", &[Opcode::And]),
        ("a == b", &[Opcode::Eq]),
        ("a != b", &[Opcode::Neq]),
        ("a < b", &[Opcode::Xchg, Opcode::Sless]),
        ("a <= b", &[Opcode::Xchg, Opcode::Sleq]),
        ("a > b", &[Opcode::Xchg, Opcode::Sgrtr]),
        ("a >= b", &[Opcode::Xchg, Opcode::Sgeq]),
    ];
    for (expr, tail) in cases {
        let src = format!("foo(a, b) {{ return {expr}; }}");
        let ops = body_ops(&src);
        let n = tail.len();
        // The operator instructions sit just before the `retn` of the `return`.
        let at = ops.iter().position(|o| *o == Opcode::Retn).unwrap();
        assert_eq!(&ops[at - n..at], *tail, "for `{expr}` the stream was {ops:?}");
    }
}

#[test]
fn unary_operators_use_the_single_cell_forms() {
    assert!(body_ops("foo(a) { return -a; }").contains(&Opcode::Neg));
    assert!(body_ops("foo(a) { return !a; }").contains(&Opcode::Not));
    assert!(body_ops("foo(a) { return ~a; }").contains(&Opcode::Invert));
}

#[test]
fn chained_relational_operators_evaluate_the_middle_operand_once() {
    // plnge_rel(): `a < b < c` is `a < b && b < c` with `b` evaluated once and
    // kept in ALT across the relop_prefix/relop_suffix pair.
    let ops = body_ops("foo(a, b, c) { return a < b < c; }");
    assert!(ops.contains(&Opcode::SwapAlt), "relop_suffix emits swap.alt: {ops:?}");
    assert_eq!(ops.iter().filter(|o| **o == Opcode::Sless).count(), 2);
    assert_eq!(
        ops.iter().filter(|o| **o == Opcode::LoadSPri).count(),
        3,
        "three operands, three loads: {ops:?}"
    );
}

// ----------------------------------------------------------- short circuit

#[test]
fn logical_and_short_circuits_to_zero() {
    // skim(list11, jmp_eq0, dropval=0, endval=1, ...)
    let ops = body_ops("foo(a, b) { return a && b; }");
    assert_eq!(
        &ops[..7],
        [
            Opcode::LoadSPri,
            Opcode::Jzer, // drop out on the first false operand
            Opcode::LoadSPri,
            Opcode::Jzer,
            Opcode::ConstPri, // endval = 1
            Opcode::Jump,
            Opcode::ZeroPri, // dropval = 0
        ]
    );
}

#[test]
fn logical_or_short_circuits_to_one() {
    // skim(list12, jmp_ne0, dropval=1, endval=0, ...)
    let ops = body_ops("foo(a, b) { return a || b; }");
    assert_eq!(
        &ops[..7],
        [
            Opcode::LoadSPri,
            Opcode::Jnz,
            Opcode::LoadSPri,
            Opcode::Jnz,
            Opcode::ZeroPri, // endval = 0
            Opcode::Jump,
            Opcode::ConstPri, // dropval = 1
        ]
    );
}

#[test]
fn a_three_operand_and_chain_shares_one_drop_out_label() {
    let text = normalised("foo(a, b, c) { return a && b && c; }");
    assert_eq!(text.matches("jzer L0").count(), 3, "all three tests jump to one label:\n{text}");
}

#[test]
fn the_right_operand_of_and_is_not_evaluated_unconditionally() {
    let ops = body_ops("side() {} foo(a) { return a && side(); }");
    let jzer = ops.iter().position(|o| *o == Opcode::Jzer).unwrap();
    let call = ops.iter().position(|o| *o == Opcode::Call).unwrap();
    assert!(jzer < call, "the call must be guarded: {ops:?}");
}

// ------------------------------------------------------------- assignment

#[test]
fn a_simple_scalar_assignment_needs_no_push_or_pop() {
    // "if direct fetch and simple assignment: no push and pop needed"
    assert_eq!(
        body_ops("foo() { new a; a = 3; }")[3..6],
        [Opcode::ConstPri, Opcode::StorSPri, Opcode::Stack]
    );
}

#[test]
fn a_compound_assignment_reads_then_operates_then_stores() {
    let ops = body_ops("foo() { new a; a += 2; }");
    assert_eq!(&ops[3..7], [Opcode::LoadSPri, Opcode::ConstAlt, Opcode::Add, Opcode::StorSPri]);
}

#[test]
fn assigning_to_an_array_cell_keeps_the_address_in_alt() {
    // hier14(): the indirect case pushes the address and pops it into ALT, which
    // is where stor.i expects it.
    let ops = body_ops("foo(a[4], i) { a[i] = 5; }");
    let stor = ops.iter().position(|o| *o == Opcode::StorI).unwrap();
    assert_eq!(ops[stor - 2..=stor], [Opcode::ConstPri, Opcode::PopAlt, Opcode::StorI]);
}

#[test]
fn increment_forms_differ_in_where_the_read_happens() {
    // hier2 (prefix): inc first, then rvalue. hier1 (postfix): rvalue, then inc.
    assert_eq!(body_ops("foo() { new a; ++a; }")[3..5], [Opcode::IncS, Opcode::LoadSPri]);
    assert_eq!(body_ops("foo() { new a; a++; }")[3..5], [Opcode::LoadSPri, Opcode::IncS]);
    assert!(body_ops("foo() { new a; a--; }").contains(&Opcode::DecS));
    // A global uses the non-stack-relative form.
    assert!(body_ops("new g; foo() { g++; }").contains(&Opcode::Inc));
}

// ---------------------------------------------------------------- indexing

#[test]
fn a_constant_index_folds_into_an_offset_and_scratches_the_push() {
    // hier1(): stgdel() removes the pushreg, then ldconst(idx<<2, sALT); ob_add().
    let items = body("foo(a[4]) { return a[2]; }");
    assert_eq!(
        opcodes(&items)[..5],
        [Opcode::LoadSPri, Opcode::ConstAlt, Opcode::Add, Opcode::LoadI, Opcode::Retn]
    );
    assert_eq!(first_imm(&items, Opcode::ConstAlt), 2 * 4, "the index is scaled to bytes");
}

#[test]
fn a_zero_index_adds_nothing() {
    assert_eq!(
        body_ops("foo(a[4]) { return a[0]; }")[..3],
        [Opcode::LoadSPri, Opcode::LoadI, Opcode::Retn]
    );
}

#[test]
fn a_variable_index_emits_a_bounds_check_and_scales_at_run_time() {
    let items = body("foo(a[4], i) { return a[i]; }");
    assert_eq!(
        opcodes(&items)[..7],
        [
            Opcode::LoadSPri, // base address
            Opcode::PushPri,
            Opcode::LoadSPri, // the index
            Opcode::Bounds,
            Opcode::ShlCPri, // cell2addr
            Opcode::PopAlt,
            Opcode::Add,
        ]
    );
    assert_eq!(first_imm(&items, Opcode::Bounds), 3, "bounds takes the last valid index");
}

#[test]
fn an_out_of_range_constant_index_is_error_32() {
    let unit = compile("foo() { new a[4]; new b = a[9]; }");
    assert!(unit.diags.items().iter().any(|d| d.code == 32), "{:?}", unit.diags.items());
}

#[test]
fn a_packed_character_index_uses_the_byte_forms() {
    let ops = body_ops("foo(a[4], i) { return a{i}; }");
    assert!(ops.contains(&Opcode::AlignPri), "charalign() must run: {ops:?}");
    assert!(ops.contains(&Opcode::LodbI), "a packed character is read with lodb.i: {ops:?}");
}

// ------------------------------------------------------------- statements

#[test]
fn if_without_else_jumps_over_the_body() {
    let text = normalised("foo(a) { if (a) { return 1; } }");
    assert!(text.contains("jzer L0"), "{text}");
}

#[test]
fn if_else_emits_both_labels() {
    let ops = body_ops("foo(a) { if (a) { new x; } else { new y; } }");
    assert_eq!(ops[0], Opcode::LoadSPri);
    assert_eq!(ops[1], Opcode::Jzer);
    assert!(ops.contains(&Opcode::Jump), "the true arm jumps over the false arm: {ops:?}");
}

#[test]
fn a_while_loop_tests_at_the_top_and_jumps_back() {
    let src = "foo(a) { while (a) { new x; } }";
    let text = normalised(src);
    // L0 is the exit (named first, by the jzer), L1 the loop top.
    assert!(text.contains("jzer L0"), "{text}");
    assert!(text.contains("jump L1"), "{text}");
    assert!(text.contains("L1:
load.s.pri"), "the test is at the top:
{text}");
    assert_eq!(body_ops(src)[0], Opcode::LoadSPri, "the test comes first");
}

#[test]
fn a_do_while_loop_tests_at_the_bottom() {
    let ops = body_ops("foo(a) { do { new x; } while (a); }");
    assert_eq!(ops[0], Opcode::Stack, "the body runs before the test: {ops:?}");
    let jzer = ops.iter().position(|o| *o == Opcode::Jzer).unwrap();
    let jump = ops.iter().position(|o| *o == Opcode::Jump).unwrap();
    assert!(jzer < jump, "the back-edge follows the test: {ops:?}");
}

#[test]
fn a_for_loop_emits_the_step_before_the_test() {
    // dofor(): "Expressions 2 and 3 are reversed in the generated code:
    // expression 3 precedes expression 2."
    let ops = body_ops("foo() { for (new i = 0; i < 3; i++) { new x; } }");
    let jump_to_skip = ops.iter().position(|o| *o == Opcode::Jump).unwrap();
    let inc = ops.iter().position(|o| *o == Opcode::IncS).unwrap();
    let test = ops.iter().position(|o| *o == Opcode::Sless).unwrap();
    assert!(jump_to_skip < inc, "the first pass skips the step: {ops:?}");
    assert!(inc < test, "the step is emitted before the test: {ops:?}");
}

#[test]
fn a_for_loop_releases_its_own_declaration() {
    let items = body("foo() { for (new i = 0; i < 3; i++) {} }");
    assert_eq!(
        imms_of(&items, Opcode::Stack),
        vec![-4, 4],
        "`i` is reserved once and released once"
    );
}

#[test]
fn break_and_continue_restore_the_stack_of_the_loop() {
    // dobreak(): modstk((declared - wq[wqBRK]) * cell)
    let items = body("foo(a) { while (a) { new x; break; } }");
    assert!(
        imms_of(&items, Opcode::Stack).contains(&4),
        "break gives back the block's cell: {:?}",
        imms_of(&items, Opcode::Stack)
    );

    let ops = body_ops("foo(a) { while (a) { new x; continue; } }");
    let stack_at = ops.iter().position(|o| *o == Opcode::Stack).unwrap();
    assert!(stack_at < ops.len());
    assert!(ops.windows(2).any(|w| w == [Opcode::Stack, Opcode::Jump]));
}

#[test]
fn break_outside_a_loop_is_error_24() {
    let unit = compile("foo() { break; }");
    assert!(unit.diags.items().iter().any(|d| d.code == 24));
}

#[test]
fn return_drops_the_whole_frame() {
    // doreturn(): modstk(declared*cell) before ffret()
    let items = body("foo() { new a; new b; return 1; }");
    let ops = opcodes(&items);
    let retn = ops.iter().position(|o| *o == Opcode::Retn).unwrap();
    assert_eq!(ops[retn - 2..retn], [Opcode::ConstPri, Opcode::Stack]);
    assert!(
        imms_of(&items, Opcode::Stack).contains(&8),
        "both locals are released at once: {:?}",
        imms_of(&items, Opcode::Stack)
    );
}

#[test]
fn goto_and_its_label_resolve() {
    let text = normalised("foo() { goto done; done: return 1; }");
    assert!(text.contains("jump L0"), "{text}");
}

#[test]
fn assert_halts_with_the_assertion_code() {
    // doassert(): test(flab1, FALSE, TRUE) then ffabort(xASSERTION)
    let items = body("foo(a) { assert a; }");
    assert_eq!(opcodes(&items)[..3], [Opcode::LoadSPri, Opcode::Jnz, Opcode::Halt]);
    assert_eq!(first_imm(&items, Opcode::Halt), 2);
}

#[test]
fn exit_and_sleep_pass_the_value_in_pri_and_the_tag_in_alt() {
    let items = body("foo() { exit 3; }");
    assert_eq!(opcodes(&items)[..3], [Opcode::ConstPri, Opcode::ZeroAlt, Opcode::Halt]);
    assert_eq!(first_imm(&items, Opcode::Halt), 1, "xEXIT");
    let items = body("foo() { sleep 0; }");
    assert_eq!(first_imm(&items, Opcode::Halt), 12, "xSLEEP");
}

// ----------------------------------------------------------------- switch

#[test]
fn a_switch_emits_the_table_after_the_clauses() {
    let unit = compile("foo(a) { switch (a) { case 1: return 1; case 2: return 2; } }");
    let ops = opcodes(&unit.code);
    let switch = ops.iter().position(|o| *o == Opcode::Switch).unwrap();
    let table = ops.iter().position(|o| *o == Opcode::Casetbl).unwrap();
    assert!(switch < table, "the table follows every clause: {ops:?}");
}

#[test]
fn the_case_table_is_two_plus_two_n_cells_and_sorted() {
    let unit = compile("foo(a) { switch (a) { case 5: return 5; case 1: return 1; } }");
    let Some(Item::CaseTbl { cases, .. }) =
        unit.code.iter().find(|i| matches!(i, Item::CaseTbl { .. }))
    else {
        panic!("no case table");
    };
    assert_eq!(cases.iter().map(|(v, _)| *v).collect::<Vec<_>>(), vec![1, 5]);

    // The encoded form: opcode cell + (count, default) + n*(value, address).
    let code = unit.assemble().unwrap();
    let d = disassemble(&code).unwrap();
    let tbl = d.iter().find(|i| i.opcode == Opcode::Casetbl).unwrap();
    assert_eq!(tbl.operands.len(), 2 + 2 * 2);
    assert_eq!(tbl.operands[0], 2, "the header cell holds the record count");
    assert!(dangling_targets(&d).is_empty());
}

#[test]
fn a_case_range_is_expanded_to_one_record_per_value() {
    // doswitch(): `while (++val <= end)` inserts a record for every value.
    let unit = compile("foo(a) { switch (a) { case 1 .. 4: return 1; } }");
    let Some(Item::CaseTbl { cases, .. }) =
        unit.code.iter().find(|i| matches!(i, Item::CaseTbl { .. }))
    else {
        panic!("no case table");
    };
    assert_eq!(cases.iter().map(|(v, _)| *v).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert!(cases.windows(2).all(|w| w[0].1 == w[1].1), "all four share one clause label");
}

#[test]
fn a_switch_without_a_default_falls_through_to_the_exit_label() {
    let unit = compile("foo(a) { switch (a) { case 1: return 1; } }");
    let code = unit.assemble().unwrap();
    let d = disassemble(&code).unwrap();
    let tbl = d.iter().find(|i| i.opcode == Opcode::Casetbl).unwrap();
    let default = tbl.operands[1];
    assert!(d.iter().any(|i| i.address as i32 == default), "the default is a real address");
    assert!(default > tbl.address as i32, "with no default clause it is the exit label");
}

#[test]
fn a_duplicate_case_label_is_error_40() {
    let unit = compile("foo(a) { switch (a) { case 1: return 1; case 1: return 2; } }");
    assert!(unit.diags.items().iter().any(|d| d.code == 40));
}

// ------------------------------------------------------------------- data

#[test]
fn a_string_literal_is_placed_in_the_data_segment() {
    // A global first, so the literal does not land at offset 0 and the address
    // is visible as a const.pri operand rather than a zero.pri.
    let unit = compile("new g; native n(s[]); foo() { n(\"hi\"); }");
    assert_eq!(unit.data, vec![0, b'h' as i32, b'i' as i32, 0]);
    assert_eq!(first_imm(&unit.code, Opcode::ConstPri), 4, "addressed by its data offset");
}

#[test]
fn a_global_array_initialiser_is_written_straight_into_the_data_segment() {
    let unit = compile("new g[3] = {1, 2, 3}; foo() { return g[0]; }");
    assert_eq!(unit.data, vec![1, 2, 3]);
    // declglb() emits no code for an initialiser.
    assert!(!opcodes(&unit.code).contains(&Opcode::Fill));
    assert!(!opcodes(&unit.code).contains(&Opcode::Movs));
}

#[test]
fn an_ellipsis_initialiser_extrapolates_rather_than_repeating() {
    // initvector() (sc1.c) keeps the previous two values, computes
    // `step = prev1 - prev2`, then fills with `prev1 += step`.
    let unit = compile("new g[6] = {1, 3, ...}; foo() { return g[0]; }");
    assert_eq!(unit.data, vec![1, 3, 5, 7, 9, 11], "must extrapolate by the step");

    // A single value leaves step 0, so it does repeat - which is why the
    // repeat-the-last reading looks correct until a second value appears.
    let one = compile("new g[4] = {7, ...}; foo() { return g[0]; }");
    assert_eq!(one.data, vec![7, 7, 7, 7]);

    // A descending pair extrapolates downwards.
    let down = compile("new g[4] = {10, 8, ...}; foo() { return g[0]; }");
    assert_eq!(down.data, vec![10, 8, 6, 4]);
}

// ------------------------------------------------ functions returning arrays

#[test]
fn calling_an_array_returning_function_reserves_a_hidden_heap_parameter() {
    // callfunction() (sc3.c:1899-1912):
    //   modheap(retsize*sizeof(cell));  /* address is in ALT */
    //   pushreg(sALT);                  /* pass ALT as the last (hidden) param */
    // ...and after the call (sc3.c:2289-2290):
    //   if (symret!=NULL) popreg(sPRI); /* pop hidden param as function result */
    let src = "make() { new v[3]; return v; } foo() { make(); }";
    let items = body(src);
    assert_eq!(
        opcodes(&items),
        [
            Opcode::Heap,    // reserve 3 cells, old HEA -> ALT
            Opcode::PushAlt, // the hidden destination, pushed *first*
            Opcode::PushC,   // argument count: 0 arguments
            Opcode::Call,
            Opcode::PopPri, // the hidden parameter *is* the result
            Opcode::Heap,   // expression(): scrap the array left on the heap
            Opcode::ZeroPri,
            Opcode::Retn,
        ]
    );
    assert_eq!(imms_of(&items, Opcode::Heap), vec![3 * 4, -3 * 4]);
    assert_eq!(first_imm(&items, Opcode::PushC), 0, "the hidden parameter is not counted");
}

#[test]
fn a_native_returning_an_array_uses_the_same_convention() {
    // funcstub() attaches the return array to natives too (sc1.c:3333-3337), so
    // finddepend() finds it in callfunction() and the convention is identical.
    // The `stack` that cleans up after a sysreq covers nargs+1 cells only - the
    // hidden parameter is left for the pop.pri.
    let items = body("native Float:[3] vel(id); foo() { vel(1); }");
    assert_eq!(
        opcodes(&items),
        [
            Opcode::Heap,
            Opcode::PushAlt,
            Opcode::ConstPri,
            Opcode::PushPri,
            Opcode::PushC,
            Opcode::SysreqC,
            Opcode::Stack,
            Opcode::PopPri,
            Opcode::Heap,
            Opcode::ZeroPri,
            Opcode::Retn,
        ]
    );
    assert_eq!(first_imm(&items, Opcode::Stack), 2 * 4, "one real argument plus the count cell");
    assert_eq!(imms_of(&items, Opcode::Heap), vec![3 * 4, -3 * 4]);
}

#[test]
fn returning_an_array_copies_into_the_hidden_parameter() {
    // doreturn() (sc1.c:5521-5531): ALT = the hidden parameter, PRI already holds
    // the source address, then `memcopy(arraysize*sizeof(cell))`. PRI is left
    // alone because callfunction() supplies the result with its popreg().
    let items = body("make() { new v[3]; return v; }");
    let ops = opcodes(&items);
    let movs = ops.iter().position(|o| *o == Opcode::Movs).expect("the copy");
    assert_eq!(
        ops[movs - 2..=movs],
        [Opcode::AddrPri, Opcode::LoadSAlt, Opcode::Movs],
        "source address in PRI, hidden destination in ALT"
    );
    assert_eq!(first_imm(&items, Opcode::Movs), 3 * 4);
    // The hidden parameter is one slot past the last declared argument:
    // base + (n+3)*cell, with n == 0 here (sc1.c:5493-5502).
    assert_eq!(first_imm(&items, Opcode::LoadSAlt), 3 * 4);

    // With two declared arguments it moves to FRM + (2+3)*cell.
    let items = body("make(a, b) { new v[4]; return v; }");
    assert_eq!(first_imm(&items, Opcode::LoadSAlt), 5 * 4);
    assert_eq!(first_imm(&items, Opcode::Movs), 4 * 4);
}

#[test]
fn an_array_returning_function_copies_its_index_vectors_too() {
    // calc_arraysize({2,3}) == 8, not 6: the returned block includes the index
    // vector, so both the caller's heap reservation and the callee's memcopy
    // must be 8 cells (sc1.c:5530).
    let items = body("make() { new v[2][3]; return v; }");
    // The first `movs` is declloc() copying `v`'s own index vector in; the last
    // one is the return copy.
    assert_eq!(*imms_of(&items, Opcode::Movs).last().unwrap(), 8 * 4);
    let items = body("make() { new v[2][3]; return v; } foo() { make(); }");
    assert_eq!(imms_of(&items, Opcode::Heap), vec![8 * 4, -8 * 4]);
}

#[test]
fn the_result_of_an_array_returning_call_can_be_subscripted() {
    // lval_result->ident=iREFARRAY (sc3.c:1910), so hier1() may index it.
    let items = body("make() { new v[3]; return v; } foo() { return make()[1]; }");
    let ops = opcodes(&items);
    let pop = ops.iter().position(|o| *o == Opcode::PopPri).expect("the hidden result");
    assert_eq!(
        ops[pop..pop + 4],
        [Opcode::PopPri, Opcode::ConstAlt, Opcode::Add, Opcode::LoadI],
        "index 1 is a constant offset from the returned address"
    );
    assert_eq!(imms_of(&items, Opcode::ConstAlt), vec![4], "one cell in");
}

#[test]
fn the_heap_is_scrapped_once_per_full_expression_not_per_call() {
    // expression() (sc3.c:674-683) snapshots decl_heap and gives back the whole
    // delta at the end, so two calls in one expression share one `heap -N`.
    let items = body("make() { new v[3]; return v; } foo() { make()[0] + make()[1]; }");
    assert_eq!(
        imms_of(&items, Opcode::Heap),
        vec![3 * 4, 3 * 4, -6 * 4],
        "two reservations, one release"
    );
}

#[test]
fn a_return_dimension_must_have_a_known_size() {
    // funcstub() (sc1.c:3250-3252): `if (size==0) error(9)`.
    // A literal `[]` is caught by the parser; a size that *folds* to zero is
    // codegen's to catch, exactly as `needsub()` returning 0 is in funcstub().
    let unit = compile("const N = 0; native Float:[N] make();");
    assert!(unit.diags.items().iter().any(|d| d.code == 9), "{:?}", unit.diags.items());
}

#[test]
fn returning_a_mismatched_array_is_error_47() {
    let unit = compile("forward [3] make(); make() { new v[4]; return v; }");
    assert!(unit.diags.items().iter().any(|d| d.code == 47), "{:?}", unit.diags.items());
}

// --------------------------------------- multi-dimensional array index vectors

#[test]
fn a_two_dimensional_global_is_an_index_vector_followed_by_the_rows() {
    // initials2() (sc1.c:2356) reserves calc_arraysize(dim,numdim-1,0) cells,
    // initarray() (sc1.c:2410) appends the rows row-major, and
    // adjust_indirectiontables() (sc1.c:2244-2247) then writes
    //     litq[base++] = (size*dim[cur] + (dim[cur+1]-1)*(dim[cur]*i+d)) * cell
    // For dim={2,3}, cur=0, size=1:
    //     d=0 -> (1*2 + 2*0)*4 ==  8
    //     d=1 -> (1*2 + 2*1)*4 == 16
    // and calc_arraysize({2,3}) == 2 + 2*3 == 8 cells in total.
    let unit = compile("new g[2][3] = {{1,2,3},{4,5,6}}; foo() { return g[0][0]; }");
    assert_eq!(unit.data, vec![8, 16, 1, 2, 3, 4, 5, 6]);

    // The offsets are relative to the vector cell itself: cell 0 is at byte 0 and
    // 0+8 == byte 8 == cell 2, the start of row 0. Cell 1 is at byte 4 and
    // 4+16 == byte 20 == cell 5, the start of row 1.
    assert_eq!(follows(&unit.data, 0), 2);
    assert_eq!(follows(&unit.data, 1), 5);
}

#[test]
fn short_rows_of_a_two_dimensional_array_are_zero_padded() {
    // initarray() calls initvector(..., dim[numdim-1], TRUE, ...) - the TRUE is
    // `fillzero` (sc1.c:2443, sc1.c:2561), so a row shorter than the declared
    // minor dimension is padded rather than shifting the next row up. Without the
    // padding the fixed offsets 8/16 would point at the wrong cells.
    let unit = compile("new g[2][3] = {{1},{2}}; foo() { return g[0][0]; }");
    assert_eq!(unit.data, vec![8, 16, 1, 0, 0, 2, 0, 0]);

    // An omitted major row stays entirely zero.
    let unit = compile("new g[2][3] = {{1,2,3}}; foo() { return g[0][0]; }");
    assert_eq!(unit.data, vec![8, 16, 1, 2, 3, 0, 0, 0]);
}

#[test]
fn an_uninitialised_multidimensional_array_still_gets_its_index_vector() {
    // sc1.c:2311-2330: when there is no `=`, initials2() still reserves the
    // vectors and calls adjust_indirectiontables() on them.
    let unit = compile("new g[2][3]; foo() { return g[1][0]; }");
    assert_eq!(unit.data, vec![8, 16, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn a_three_dimensional_array_puts_every_level_of_vector_first() {
    // dim={2,2,2}: calc_arraysize({2,2}) == 6 vector cells, then 8 data cells.
    //   cur=0, size=1: (1*2 + (2-1)*(2*0+d))*4          -> { 8, 12}
    //   cur=1, size=2: (2*2 + (2-1)*(2*i+d))*4          -> {16,20, 24,28}
    let unit = compile(
        "new g[2][2][2] = {{{1,2},{3,4}},{{5,6},{7,8}}}; foo() { return g[0][0][0]; }",
    );
    assert_eq!(unit.data, vec![8, 12, 16, 20, 24, 28, 1, 2, 3, 4, 5, 6, 7, 8]);
    // Cell 0 (byte 0) + 8 == cell 2, the first level-1 vector (cells 2..3).
    assert_eq!(follows(&unit.data, 0), 2);
    // Cell 1 (byte 4) + 12 == byte 16 == cell 4, the second level-1 vector.
    assert_eq!(follows(&unit.data, 1), 4);
    // Cell 2 (byte 8) + 16 == byte 24 == cell 6, the first data row.
    assert_eq!(follows(&unit.data, 2), 6);
    // Cell 5 (byte 20) + 28 == byte 48 == cell 12, the last data row.
    assert_eq!(follows(&unit.data, 5), 12);
}

#[test]
fn a_two_dimensional_string_table_lays_its_rows_out_padded() {
    // A common Pawn idiom: an array of fixed-width strings.
    let unit = compile("new g[2][4] = {\"ab\", \"cd\"}; foo() { return g[0][0]; }");
    assert_eq!(
        unit.data,
        vec![8, 20, b'a' as i32, b'b' as i32, 0, 0, b'c' as i32, b'd' as i32, 0, 0]
    );
    // dim={2,4}: (1*2 + 3*d)*4 -> {8, 20}; cell 0 + 8 == cell 2, cell 1 + 20 == cell 6.
    assert_eq!(follows(&unit.data, 0), 2);
    assert_eq!(follows(&unit.data, 1), 6);
}

#[test]
fn a_two_dimensional_local_copies_the_whole_image_including_the_vector() {
    // The index vector is part of the array's storage, so declloc()'s memcopy
    // covers calc_arraysize() cells, not just the elements.
    let unit = compile("foo() { new a[2][3] = {{1,2,3},{4,5,6}}; }");
    assert_eq!(unit.data, vec![8, 16, 1, 2, 3, 4, 5, 6], "the literal block is the full image");
    let items = body("foo() { new a[2][3] = {{1,2,3},{4,5,6}}; }");
    assert_eq!(first_imm(&items, Opcode::Stack), -8 * 4, "calc_arraysize of [2][3] is 8 cells");
    assert_eq!(first_imm(&items, Opcode::Movs), 8 * 4, "all 8 cells are copied");
}

#[test]
fn a_local_multidimensional_array_without_an_initialiser_still_writes_its_vector() {
    let unit = compile("foo() { new a[2][3]; }");
    assert_eq!(unit.data, vec![8, 16], "the vector alone becomes the literal block");
    let ops = opcodes(&unit.code);
    let fill = ops.iter().position(|o| *o == Opcode::Fill).expect("zero-fill first");
    let movs = ops.iter().position(|o| *o == Opcode::Movs).expect("then the vector");
    assert!(fill < movs, "{ops:?}");
}

#[test]
fn a_local_array_of_one_repeated_value_uses_fill() {
    // declloc(): "if it is [all one value], more compact code can be generated"
    let ops = body_ops("foo() { new a[3] = {7, 7, 7}; }");
    assert!(ops.contains(&Opcode::Fill), "{ops:?}");
    assert!(!ops.contains(&Opcode::Movs));
}

#[test]
fn a_local_array_with_mixed_values_is_copied_with_movs() {
    let unit = compile("foo() { new a[3] = {1, 2, 3}; }");
    assert_eq!(unit.data, vec![1, 2, 3]);
    assert!(opcodes(&unit.code).contains(&Opcode::Movs));
}

#[test]
fn a_partially_initialised_local_array_is_zeroed_first() {
    let unit = compile("foo() { new a[4] = {1, 2}; }");
    let ops = opcodes(&unit.code);
    let fill = ops.iter().position(|o| *o == Opcode::Fill).expect("zero-fill");
    let movs = ops.iter().position(|o| *o == Opcode::Movs).expect("copy");
    assert!(fill < movs, "{ops:?}");
}

// --------------------------------------------------------------- constants

#[test]
fn enum_members_fold_to_successive_constants() {
    let items = body("enum { A, B, C } foo() { return C; }");
    assert_eq!(first_imm(&items, Opcode::ConstPri), 2);
}

#[test]
fn a_shifting_enum_folds_to_bit_flags() {
    let items = body("enum (<<= 1) { F0 = 1, F1, F2 } foo() { return F2; }");
    assert_eq!(first_imm(&items, Opcode::ConstPri), 4);
}

#[test]
fn a_constant_expression_is_folded_and_never_emitted() {
    // plnge1()/stgdel(): a constant subexpression generates no code at all.
    assert_eq!(
        body_ops("foo() { return 2 * 3 + 4; }"),
        [Opcode::ConstPri, Opcode::Retn, Opcode::ZeroPri, Opcode::Retn]
    );
    assert_eq!(first_imm(&body("foo() { return 2 * 3 + 4; }"), Opcode::ConstPri), 10);
}

// -------------------------------------------------------------- round trip

#[test]
fn a_representative_program_round_trips_through_the_disassembler() {
    let src = r#"
        native log(msg[]);
        new g_count;

        stock helper(a, b) {
            if (a > b) {
                return a;
            }
            return b;
        }

        public plugin_init() {
            new total = 0;
            for (new i = 0; i < 10; i++) {
                switch (i) {
                    case 0 .. 3: total += i;
                    case 7: total = helper(total, i);
                    default: total -= 1;
                }
            }
            while (total > 0 && g_count < 5) {
                total--;
                g_count++;
            }
            log("done");
            return total;
        }
    "#;
    let text = normalised(src);
    assert!(text.starts_with("halt 0\n"), "{text}");
    for expected in ["proc", "casetbl", "sysreq.c", "call", "switch", "retn"] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }

    let unit = compile(src);
    assert_eq!(unit.natives, vec!["log".to_owned()]);
    assert_eq!(unit.publics.len(), 1);
    assert_eq!(unit.publics[0].0, "plugin_init");
    assert!(
        !unit.diags.items().iter().any(|d| d.is_error()),
        "unexpected errors: {:?}",
        unit.diags.items()
    );
}

#[test]
fn every_emitted_instruction_has_its_declared_operand_count() {
    let unit = compile(
        "native n(a); new g[2] = {1,2}; foo(x, &y, z[]) { new q = x; y = q; z[0] = q; n(q); }",
    );
    assert!(
        unit.code.iter().all(crate::stream::arity_ok),
        "an instruction was built with the wrong arity"
    );
}

// ------------------------------------------------- user-defined operators

/// The four `native` overloads `float.inc` declares, in the same order, so that
/// the native indices in the assertions below are stable.
const FLOAT_OPS: &str = r#"
    native Float:operator*(Float:a, Float:b) = floatmul;
    native Float:operator/(Float:a, Float:b) = floatdiv;
    native Float:operator+(Float:a, Float:b) = floatadd;
    native Float:operator-(Float:a, Float:b) = floatsub;
"#;

#[test]
fn an_operator_overload_is_registered_under_its_mangled_name() {
    // `native Float:operator*(...) = floatmul;` has no legal exported name of
    // its own, so the *alias* is what reaches the native table.
    let unit = compile(FLOAT_OPS);
    assert_eq!(unit.natives, ["floatmul", "floatdiv", "floatadd", "floatsub"]);
    assert!(
        !unit.diags.items().iter().any(|d| d.is_error()),
        "declaring an operator must no longer be error 7: {:?}",
        unit.diags.items()
    );
}

#[test]
fn multiplying_two_floats_calls_the_overload_instead_of_smul() {
    let src = format!("{FLOAT_OPS} foo(Float:a, Float:b) {{ return _:(a * b); }}");
    let ops = body_ops(&src);
    assert!(ops.contains(&Opcode::SysreqC), "no call was emitted:\n{ops:?}");
    assert!(!ops.contains(&Opcode::Smul), "the integer opcode was emitted anyway:\n{ops:?}");

    // check_userop(): "the left operand is in the secondary register and the
    // right operand is in the primary register", pushed in reversed order.
    let items = body(&src);
    assert_eq!(
        opcodes(&items),
        [
            Opcode::LoadSPri, // a  -> PRI
            Opcode::PushPri,
            Opcode::LoadSPri, // b  -> PRI
            Opcode::PopAlt,   // a  -> ALT
            Opcode::PushPri,  // right operand first
            Opcode::PushAlt,  // then the left one
            Opcode::PushC,    // 2*cell
            Opcode::SysreqC,
            Opcode::Stack, // the caller drops the arguments plus the count cell
            Opcode::Retn,
            Opcode::ZeroPri, // the unconditional tail, removed by the peephole
            Opcode::Retn,
        ]
    );
    // `operator*` is native 0, and the argument-count cell says two arguments.
    assert_eq!(first_imm(&items, Opcode::SysreqC), 0);
    assert_eq!(first_imm(&items, Opcode::PushC), 2 * 4);
}

#[test]
fn untagged_multiplication_still_uses_the_integer_opcode() {
    // check_userop()'s quick exit: "user-defined operators on untagged operands
    // are forbidden".
    let src = format!("{FLOAT_OPS} foo(a, b) {{ return a * b; }}");
    let ops = body_ops(&src);
    assert!(ops.contains(&Opcode::Smul), "{ops:?}");
    assert!(!ops.contains(&Opcode::SysreqC), "{ops:?}");
}

#[test]
fn a_tag_cast_changes_which_operator_is_dispatched() {
    let src = format!("{FLOAT_OPS} foo(a, b) {{ return _:(Float:a * Float:b); }}");
    let ops = body_ops(&src);
    assert!(ops.contains(&Opcode::SysreqC), "the cast must select the overload:\n{ops:?}");
    assert!(!ops.contains(&Opcode::Smul), "{ops:?}");

    // ... and `_:` strips it again.
    let src = format!("{FLOAT_OPS} foo(Float:a, Float:b) {{ return _:a * _:b; }}");
    let ops = body_ops(&src);
    assert!(ops.contains(&Opcode::Smul), "{ops:?}");
    assert!(!ops.contains(&Opcode::SysreqC), "{ops:?}");
}

#[test]
fn a_commutative_operator_dispatches_with_the_operands_swapped() {
    // Only `operator+(Float:, _:)` exists, so `1 + f` must find it by swapping.
    let src = "
        native Float:operator+(Float:a, b) = floatadd;
        foo(Float:f, n) { return _:(n + f); }
    ";
    let items = body(src);
    let ops = opcodes(&items);
    assert!(ops.contains(&Opcode::SysreqC), "{ops:?}");
    assert!(!ops.contains(&Opcode::Add), "{ops:?}");
    // swapparams: `pushreg(sALT); pushreg(sPRI)` - the Float: operand goes first.
    let pushes: Vec<Opcode> =
        ops.iter().copied().filter(|o| matches!(o, Opcode::PushPri | Opcode::PushAlt)).collect();
    assert_eq!(pushes.last().copied(), Some(Opcode::PushPri));
}

#[test]
fn a_float_comparison_saves_pri_across_the_call() {
    // binoper_savepri is TRUE for the relational operators: the chained form
    // needs ALT untouched, so PRI is pushed before the operands and popped into
    // ALT afterwards.
    let src = "
        native bool:operator<(Float:a, Float:b) = floatcmp;
        foo(Float:a, Float:b) { return _:(a < b); }
    ";
    let ops = body_ops(src);
    assert!(ops.contains(&Opcode::SysreqC), "{ops:?}");
    assert!(!ops.contains(&Opcode::Sless), "{ops:?}");
    assert_eq!(ops.iter().filter(|o| **o == Opcode::PopAlt).count(), 2, "{ops:?}");
}

#[test]
fn incrementing_a_float_calls_the_stock_and_stores_the_result_back() {
    let src = "
        native Float:operator+(Float:a, Float:b) = floatadd;
        stock Float:operator++(Float:oper) return oper + 1.0;
        foo() { new Float:f; f++; }
    ";
    let items = body(src);
    let ops = opcodes(&items);
    // `operator++` is an ordinary stock, so it is reached with `call`.
    assert!(ops.contains(&Opcode::Call), "no call to the stock:\n{ops:?}");
    assert!(!ops.contains(&Opcode::IncS), "the integer increment was emitted:\n{ops:?}");
    // check_userop(): rvalue, push, call, store back, moveto1.
    assert!(ops.contains(&Opcode::StorSPri), "the result is not stored back:\n{ops:?}");
    assert!(ops.contains(&Opcode::MovePri), "moveto1() is missing:\n{ops:?}");
    assert_eq!(first_imm(&items, Opcode::PushC), 4, "one argument");
}

#[test]
fn an_operator_body_does_not_recurse_into_itself() {
    // "we don't want to use the redefined operator in the function that
    // redefines the operator itself" (sc3.c:202-209).
    let src = "stock Float:operator+(Float:a, Float:b) return _:(a + b);";
    let unit = compile(src);
    let ops = crate::stream::opcodes(&unit.code);
    assert!(ops.contains(&Opcode::Add), "the built-in opcode must stand:\n{ops:?}");
    assert!(!ops.contains(&Opcode::Call), "{ops:?}");
}

#[test]
fn a_forward_only_operator_is_rejected_rather_than_compiled() {
    // float.inc declares `forward operator%(Float:, Float:)` precisely so that
    // the modulus of two floats does not silently become an integer sdiv.
    let src = "
        forward operator%(Float:a, Float:b);
        foo(Float:a, Float:b) { return _:(a % b); }
    ";
    let unit = compile(src);
    let codes: Vec<u16> = unit.diags.items().iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(codes.contains(&4), "expected error 4, got {codes:?}");
    let ops = crate::stream::opcodes(&unit.code);
    assert!(!ops.contains(&Opcode::SdivAlt), "the integer opcode must not stand in:\n{ops:?}");
}

#[test]
fn an_operator_on_untagged_operands_is_error_64() {
    let unit = compile("stock operator+(a, b) return a + b;");
    let codes: Vec<u16> = unit.diags.items().iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(codes.contains(&64), "expected error 64, got {codes:?}");
}

#[test]
fn the_assignment_and_destructor_operators_are_still_error_7() {
    for src in ["stock Float:operator=(oper) return Float:oper;", "stock operator~(Float:a[]) {}"] {
        let unit = compile(src);
        let codes: Vec<u16> =
            unit.diags.items().iter().filter(|d| d.is_error()).map(|d| d.code).collect();
        assert!(codes.contains(&7), "expected error 7 for `{src}`, got {codes:?}");
    }
}
