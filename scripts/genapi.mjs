// Parse amxmodx .inc files -> src/api.rs signature table.
import fs from 'node:fs';
import path from 'node:path';

const INC = process.argv[2];
const OUT = process.argv[3];

// Strip comments but keep string/char literals intact (Pawn escape char is ^).
function stripComments(src) {
  let out = '', i = 0, n = src.length;
  while (i < n) {
    const c = src[i];
    if (c === '"' || c === "'") {
      const q = c; out += c; i++;
      while (i < n && src[i] !== q) {
        if (src[i] === '^') { out += src[i]; i++; if (i < n) { out += src[i]; i++; } continue; }
        if (src[i] === '\n') break;
        out += src[i]; i++;
      }
      if (i < n) { out += src[i]; i++; }
      continue;
    }
    if (c === '/' && src[i + 1] === '/') { while (i < n && src[i] !== '\n') i++; continue; }
    if (c === '/' && src[i + 1] === '*') {
      i += 2;
      while (i < n && !(src[i] === '*' && src[i + 1] === '/')) { if (src[i] === '\n') out += '\n'; i++; }
      i += 2; continue;
    }
    out += c; i++;
  }
  return out;
}

// Split on top-level commas, respecting (), [], {}, strings and chars.
function splitParams(s) {
  const parts = []; let depth = 0, cur = '', i = 0;
  while (i < s.length) {
    const c = s[i];
    if (c === '"' || c === "'") {
      const q = c; cur += c; i++;
      while (i < s.length && s[i] !== q) {
        if (s[i] === '^') { cur += s[i] + (s[i + 1] ?? ''); i += 2; continue; }
        cur += s[i]; i++;
      }
      cur += s[i] ?? ''; i++; continue;
    }
    if (c === '(' || c === '[' || c === '{') depth++;
    if (c === ')' || c === ']' || c === '}') depth--;
    if (c === ',' && depth === 0) { parts.push(cur); cur = ''; i++; continue; }
    cur += c; i++;
  }
  if (cur.trim() !== '') parts.push(cur);
  return parts.map(p => p.trim()).filter(p => p !== '');
}

function parseParam(raw) {
  let s = raw.trim();
  // Variadic tail, written bare (`...`) or tagged (`any:...`, `Float:...`).
  const vm = s.match(/^(?:([A-Za-z_][A-Za-z0-9_]*)\s*:\s*)?\.\.\.$/);
  if (vm) return { tag: vm[1] ?? 'any', byRef: false, isArray: false, optional: true, variadic: true };
  let optional = false, def = null;
  // default value: first top-level '='  (not '==', not '>=' etc.)
  const eq = findTopLevelEq(s);
  if (eq >= 0) { optional = true; def = s.slice(eq + 1).trim(); s = s.slice(0, eq).trim(); }
  const isConst = /^const\s/.test(s);
  if (isConst) s = s.replace(/^const\s+/, '');
  let isArray = false;
  // array dims: name[..] or name[]
  if (/\[[^\]]*\]\s*$/.test(s)) { isArray = true; s = s.replace(/(\s*\[[^\]]*\])+\s*$/, ''); }
  let tag = '_';
  const tm = s.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*:\s*/);
  if (tm) { tag = tm[1]; s = s.slice(tm[0].length); }
  const byRef = s.trimStart().startsWith('&');
  s = s.replace(/^\s*&\s*/, '');
  const name = s.trim();
  return { name, tag, byRef, isArray, optional, isConst, def, variadic: false };
}

function findTopLevelEq(s) {
  let depth = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === '"' || c === "'") { const q = c; i++; while (i < s.length && s[i] !== q) { if (s[i] === '^') i++; i++; } continue; }
    if (c === '(' || c === '[' || c === '{') depth++;
    else if (c === ')' || c === ']' || c === '}') depth--;
    else if (c === '=' && depth === 0 && s[i + 1] !== '=' && s[i - 1] !== '!' && s[i - 1] !== '<' && s[i - 1] !== '>' && s[i - 1] !== '=') return i;
  }
  return -1;
}

const funcs = new Map(); // name -> record
const includes = new Map(); // include name -> Set of directly included names
const files = fs.readdirSync(INC).filter(f => f.endsWith('.inc')).sort();

for (const file of files) {
  const incName = path.basename(file, '.inc');
  const rawSrc = fs.readFileSync(path.join(INC, file), 'utf8');
  const src = stripComments(rawSrc);

  const direct = new Set();
  for (const im of src.matchAll(/^\s*#include\s*[<"]([^>"]+)[>"]/gm)) {
    direct.add(im[1].replace(/\.inc$/, '').trim());
  }
  includes.set(incName, direct);
  const lines = src.split(/\r?\n/);
  const rawLines = rawSrc.split(/\r?\n/);

  for (let li = 0; li < lines.length; li++) {
    const m = lines[li].match(/^\s*(native|forward|stock)\s+(.*)$/);
    if (!m) continue;
    const kind = m[1];
    // Accumulate until the parameter list closes.
    let buf = m[2], j = li;
    while (!/\(/.test(buf) && j + 1 < lines.length && !/[;={]/.test(buf)) { j++; buf += ' ' + lines[j].trim(); }
    if (!/\(/.test(buf)) continue;              // global var like `stock Float:x = 0.0;`
    if (findTopLevelEq(buf.split('(')[0]) >= 0) continue;
    // read to balanced close paren
    let depth = 0, done = false, k = j;
    let acc = '';
    outer: while (true) {
      for (const ch of buf) {
        acc += ch;
        if (ch === '(') depth++;
        else if (ch === ')') { depth--; if (depth === 0) { done = true; break outer; } }
      }
      k++; if (k >= lines.length) break;
      acc += ' '; buf = lines[k].trim();
    }
    if (!done) continue;
    const open = acc.indexOf('(');
    let head = acc.slice(0, open).trim();
    const body = acc.slice(open + 1, acc.length - 1);

    // head = [Tag:]name  (operators/ctor forms skipped)
    const hm = head.match(/^(?:([A-Za-z_][A-Za-z0-9_]*)\s*:\s*)?([A-Za-z_][A-Za-z0-9_]*)$/);
    if (!hm) continue;
    const retTag = hm[1] ?? '_';
    const name = hm[2];

    const params = splitParams(body).map(parseParam);
    const variadic = params.some(p => p.variadic);
    const minArgs = params.filter(p => !p.optional && !p.variadic).length;
    const maxArgs = variadic ? 255 : params.length;

    // deprecation: #pragma deprecated within 3 lines above (in raw, un-stripped source)
    let deprecated = null;
    for (let d = Math.max(0, li - 3); d < li; d++) {
      const dm = rawLines[d]?.match(/#pragma\s+deprecated\s*(.*)$/);
      if (dm) deprecated = dm[1].trim().replace(/\s+/g, ' ');
    }

    const rec = { name, include: incName, kind, retTag, params, minArgs, maxArgs, variadic, deprecated };
    // Prefer native over stock/forward if a name appears twice; first include wins otherwise.
    const prev = funcs.get(name);
    if (!prev || (prev.kind !== 'native' && kind === 'native')) funcs.set(name, rec);
  }
}

const all = [...funcs.values()].sort((a, b) => a.name < b.name ? -1 : 1);
const esc = s => s == null ? null : s.replace(/\\/g, '\\\\').replace(/"/g, '\\"');

const kindOf = k => k === 'native' ? 'Native' : k === 'stock' ? 'Stock' : 'Forward';

let rs = `//! GENERATED FILE - do not edit by hand.
//! Source: alliedmodders/amxmodx \`plugins/include/*.inc\` (${files.length} include files),
//! the same declarations that back https://www.amxmodx.org/api.
//! Regenerate with \`scripts/genapi.mjs\` (see docs/KNOWLEDGE.md).

/// What kind of declaration a symbol is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Native,
    Stock,
    Forward,
}

/// One parameter of an API function.
/// \`optional\` and \`ret_tag\` are part of the transcribed signature and kept for
/// completeness even where no detector reads them yet.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct Param {
    /// Tag name without the colon (\`Float\`, \`bool\`, \`_\` for untagged).
    pub tag: &'static str,
    /// Passed by reference (\`&x\`) - the caller must supply a writable variable.
    pub by_ref: bool,
    /// Declared as an array (\`x[]\`, \`x[3]\`).
    pub is_array: bool,
    /// Has a default value, so it may be omitted.
    pub optional: bool,
}

/// A function exposed by an AMX Mod X include file.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct ApiFunc {
    pub name: &'static str,
    /// Include file that declares it, without the \`.inc\` suffix.
    pub include: &'static str,
    pub kind: Kind,
    /// Return tag without the colon (\`_\` when untagged).
    pub ret_tag: &'static str,
    pub params: &'static [Param],
    /// Number of parameters that have no default value.
    pub min_args: u8,
    /// Maximum accepted arguments; \`u8::MAX\` when the function is variadic.
    pub max_args: u8,
    /// Replacement text from a preceding \`#pragma deprecated\`, if any.
    pub deprecated: Option<&'static str>,
}

/// Every native, stock and forward declared by the bundled AMX Mod X includes.
/// Sorted by name so it can be binary-searched.
pub static API: &[ApiFunc] = &[
`;

for (const f of all) {
  const ps = f.params.filter(p => !p.variadic)
    .map(p => `Param{tag:"${p.tag}",by_ref:${p.byRef},is_array:${p.isArray},optional:${p.optional}}`)
    .join(",");
  const dep = f.deprecated == null ? 'None' : `Some("${esc(f.deprecated)}")`;
  rs += `    ApiFunc{name:"${f.name}",include:"${f.include}",kind:Kind::${kindOf(f.kind)},ret_tag:"${f.retTag}",params:&[${ps}],min_args:${f.minArgs},max_args:${f.maxArgs},deprecated:${dep}},\n`;
}

rs += `];

/// For each bundled include, every include it pulls in transitively (itself first).
/// Used to decide whether a plugin's \`#include\` list actually provides a symbol.
pub static INCLUDE_CLOSURE: &[(&str, &[&str])] = &[
__CLOSURE__
];

/// Transitive includes of \`name\`, or \`None\` if \`name\` is not a bundled include.
pub fn include_closure(name: &str) -> Option<&'static [&'static str]> {
    INCLUDE_CLOSURE
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| INCLUDE_CLOSURE[i].1)
}

/// Look up an API function by exact name. \`None\` means the symbol is not part of
/// the bundled AMX Mod X API (it may still be plugin-local or from a third-party include).
pub fn lookup(name: &str) -> Option<&'static ApiFunc> {
    API.binary_search_by(|f| f.name.cmp(name)).ok().map(|i| &API[i])
}
`;

// Transitive include closure (cycle-safe).
function closure(name, seen = new Set()) {
  if (seen.has(name)) return seen;
  seen.add(name);
  for (const dep of includes.get(name) ?? []) {
    if (includes.has(dep)) closure(dep, seen);
  }
  return seen;
}
const closureRows = [...includes.keys()].sort().map(n => {
  const list = [...closure(n)].sort();
  return `    ("${n}", &[${list.map(x => `"${x}"`).join(',')}]),`;
}).join('\n');
rs = rs.replace('__CLOSURE__', closureRows);

fs.writeFileSync(OUT, rs);

const byKind = k => all.filter(f => f.kind === k).length;
console.log(`includes=${files.length} funcs=${all.length} native=${byKind('native')} stock=${byKind('stock')} forward=${byKind('forward')} deprecated=${all.filter(f => f.deprecated).length}`);
console.log(`variadic=${all.filter(f => f.variadic).length}`);
