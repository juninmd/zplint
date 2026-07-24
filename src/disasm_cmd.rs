//! `zplint disasm` - disassemble a compiled `.amxx` (or raw `.amx`) plugin.
//!
//! This is the first user-facing piece of the zpc compiler work. It also exercises
//! the exact path the differential oracle depends on: container -> section ->
//! AMX image header -> code segment -> instructions. If this command works on a
//! real plugin, that whole chain is validated against files amxxpc actually produced.

use std::path::Path;

/// Disassemble `path` and print the listing. Returns the process exit code.
pub fn run(path: &Path, normalised: bool) -> i32 {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return 2;
        }
    };

    let amx = match extract_amx(&bytes) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{}: {msg}", path.display());
            return 2;
        }
    };

    let header = match zpc_amxx::AmxHeader::parse(&amx) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{}: not a valid AMX image ({e:?})", path.display());
            return 2;
        }
    };

    let code = match header.code(&amx) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: bad code segment bounds ({e:?})", path.display());
            return 2;
        }
    };

    let instrs = match zpc_asm::disassemble(code) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{}: disassembly failed: {e:?}", path.display());
            return 2;
        }
    };

    let style = if normalised {
        zpc_asm::Style::Normalised
    } else {
        zpc_asm::Style::Raw
    };

    // A dangling target means the decode is not trustworthy; say so loudly rather
    // than printing a listing that looks fine.
    let dangling = zpc_asm::dangling_targets(&instrs);
    if !dangling.is_empty() {
        eprintln!(
            "warning: {} jump target(s) do not land on an instruction ({}); \
             the disassembly may be desynchronised",
            dangling.len(),
            dangling
                .iter()
                .map(|t| format!("{t:#x}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    println!(
        "; {} - {} instruction(s), code {:#x}..{:#x}{}",
        path.display(),
        instrs.len(),
        header.cod,
        header.dat,
        if header.has_debug() { ", debug info present" } else { "" }
    );
    print!("{}", zpc_asm::render(&instrs, style));
    0
}

/// Pull the AMX image out of a `.amxx` container, or accept a raw `.amx` as-is.
/// Deciding by content rather than by file extension keeps the command usable on
/// intermediate files that have no conventional name.
fn extract_amx(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() >= 4 && bytes[..4] == zpc_amxx::MAGIC_BYTES {
        let file = zpc_amxx::read(bytes).map_err(|e| format!("bad .amxx container ({e:?})"))?;
        // A 32-bit build has one section; take the first 4-byte-cell one.
        let section = file
            .sections
            .iter()
            .find(|s| s.cell_size == zpc_amxx::CELL_SIZE_32)
            .ok_or("no 32-bit section in this .amxx")?;
        return Ok(section.image.clone());
    }
    Ok(bytes.to_vec())
}
