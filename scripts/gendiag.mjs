// Generate crates/zpc-diag/src/table.rs from the Pawn compiler's sc5-in.scp.
// The message texts are the compiler's own, so zpc diagnostics match amxxpc verbatim.
import fs from 'node:fs';

const SRC = process.argv[2];   // path to sc5-in.scp
const OUT = process.argv[3];

const text = fs.readFileSync(SRC, 'utf8');

// Each table is `static char *NAME[] = { ... };` with entries `/*NNN*/ "text",`
function table(name) {
  const start = text.indexOf(`static char *${name}[] = {`);
  if (start < 0) throw new Error(`table ${name} not found`);
  const end = text.indexOf('};', start);
  const body = text.slice(start, end);
  const out = [];
  // `/*123*/  "message text\n",`  (may span concatenated string literals)
  const re = /\/\*(\d+)\*\/\s*((?:"(?:[^"\\]|\\.)*"\s*)+)/g;
  for (const m of body.matchAll(re)) {
    const code = +m[1];
    // join adjacent C string literals, then unescape
    const lit = [...m[2].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map(x => x[1]).join('');
    const msg = lit
      .replace(/\\n/g, '')
      .replace(/\\t/g, '\t')
      .replace(/\\"/g, '"')
      .replace(/\\\\/g, '\\')
      .trim();
    out.push([code, msg]);
  }
  return out;
}

const errors = table('errmsg');
const fatals = table('fatalmsg');
const warns = table('warnmsg');

const esc = s => s.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
const rows = list => list.map(([c, m]) => `    (${c}, "${esc(m)}"),`).join('\n');

const rs = `//! GENERATED FILE - do not edit by hand.
//! Source: \`compiler/libpc300/sc5-in.scp\` from alliedmodders/amxmodx (Pawn compiler,
//! Copyright (c) ITB CompuPhase, zlib-style licence - see ATTRIBUTION.md).
//! Regenerate with \`node scripts/gendiag.mjs <sc5-in.scp> crates/zpc-diag/src/table.rs\`.
//!
//! The texts are reproduced verbatim so that zpc's diagnostics can be compared
//! byte-for-byte against amxxpc's during differential testing.

/// Non-fatal compile errors (codes 1..=99 in the Pawn numbering).
pub static ERRORS: &[(u16, &str)] = &[
${rows(errors)}
];

/// Fatal errors that abort the compile (codes 100..=199).
pub static FATALS: &[(u16, &str)] = &[
${rows(fatals)}
];

/// Warnings (codes 200..=234; amxxpc defines no code above 234).
pub static WARNINGS: &[(u16, &str)] = &[
${rows(warns)}
];

/// Look up the message template for a diagnostic code.
pub fn message(code: u16) -> Option<&'static str> {
    let table = match code {
        0..=99 => ERRORS,
        100..=199 => FATALS,
        _ => WARNINGS,
    };
    table.iter().find(|(c, _)| *c == code).map(|(_, m)| *m)
}
`;

fs.writeFileSync(OUT, rs);
console.log(`errors=${errors.length} fatals=${fatals.length} warnings=${warns.length}`);
console.log(`warning range: ${Math.min(...warns.map(w => w[0]))}..${Math.max(...warns.map(w => w[0]))}`);
