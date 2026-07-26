// Differential acceptance oracle: compile one corpus with reference amxxpc and
// zplint, then compare accept/reject decisions and validate every emitted zplint
// artifact through its disassembler. Outputs stay in a temporary directory.
//
// Usage:
//   node scripts/difftest.mjs --amxxpc <path> --include <dir> [--zpc <path>] [--corpus <dir>]
//
// Optional:
//   --strict-diagnostics  also require identical diagnostic tuples
//   --record              only run amxxpc and save normalised output as baseline

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

function arg(name, fallback = null) {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 && i + 1 < process.argv.length ? process.argv[i + 1] : fallback;
}
const flag = name => process.argv.includes(`--${name}`);

const AMXXPC = arg('amxxpc');
const INCLUDE = arg('include');
const ZPC = arg('zpc', 'target/release/zplint.exe');
const CORPUS = arg('corpus', 'crates/zpc/tests/fixtures');
const BASELINE = arg('baseline', 'crates/zpc/tests/baseline');
const RECORD = flag('record');
const STRICT_DIAGNOSTICS = flag('strict-diagnostics');

if (!AMXXPC) {
  console.error('error: --amxxpc <path> is required (reference compiler oracle)');
  process.exit(2);
}
for (const [label, value] of [['amxxpc', AMXXPC], ['corpus', CORPUS]]) {
  if (!fs.existsSync(value)) {
    console.error(`error: ${label} not found at ${value}`);
    process.exit(2);
  }
}
if (!RECORD && !fs.existsSync(ZPC)) {
  console.error(`error: zplint binary not found at ${ZPC}; build release or pass --zpc`);
  process.exit(2);
}

function collect(dir) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const candidate = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...collect(candidate));
    else if (entry.name.endsWith('.sma')) out.push(candidate);
  }
  return out.sort();
}

function normalise(text, file) {
  return text
    .replaceAll('\\', '/')
    .replaceAll(path.resolve(file).replaceAll('\\', '/'), '<SRC>')
    .replace(/^.*?([^/]+\.(?:sma|inc))\(/gm, '$1(')
    .replace(/\d{4}-\d{2}-\d{2}|\d{2}:\d{2}:\d{2}/g, '<TIME>')
    .split('\n')
    .map(line => line.trimEnd())
    .filter(Boolean)
    .join('\n');
}

function diagnostics(text, file) {
  return normalise(text, file)
    .split('\n')
    .filter(line => /\(\d+\) : (?:fatal )?(?:error|warning) \d{3}:/.test(line));
}

function run(exe, args) {
  const result = spawnSync(exe, args, {
    cwd: process.cwd(),
    encoding: 'utf8',
    timeout: 60_000,
  });
  return {
    text: (result.stdout ?? '') + (result.stderr ?? ''),
    code: result.status ?? -1,
    failed: result.error ? String(result.error) : null,
  };
}

const files = collect(CORPUS);
if (files.length === 0) {
  console.error(`error: no .sma files under ${CORPUS}`);
  process.exit(2);
}

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'zplint-difftest-'));
process.on('exit', () => fs.rmSync(work, { recursive: true, force: true }));
if (RECORD) fs.mkdirSync(BASELINE, { recursive: true });

let diverged = 0;
let recorded = 0;
const report = [];

for (const [index, file] of files.entries()) {
  const stem = `${String(index).padStart(4, '0')}-${path.basename(file, '.sma')}`;
  const refOutput = path.join(work, `${stem}.ref.amxx`);
  const ourOutput = path.join(work, `${stem}.ours.amxx`);
  const refArgs = [`-o${refOutput}`];
  if (INCLUDE) refArgs.push(`-i${INCLUDE}`);
  refArgs.push(file);

  const ref = run(AMXXPC, refArgs);
  if (RECORD) {
    fs.writeFileSync(
      path.join(BASELINE, `${stem}.ref.txt`),
      normalise(ref.text, file),
    );
    recorded++;
    continue;
  }

  const ourArgs = ['compile', file, '--output', ourOutput];
  if (INCLUDE) ourArgs.push('--include', INCLUDE);
  const ours = run(ZPC, ourArgs);
  const reasons = [];
  const refAccepted = ref.code === 0;
  const ourAccepted = ours.code === 0;

  if (ref.failed) reasons.push(`amxxpc process failed: ${ref.failed}`);
  if (ours.failed) reasons.push(`zplint process failed: ${ours.failed}`);
  if (refAccepted !== ourAccepted) {
    reasons.push(`acceptance differs: amxxpc=${ref.code}, zplint=${ours.code}`);
  }

  const refArtifact = fs.existsSync(refOutput) && fs.statSync(refOutput).size > 0;
  const ourArtifact = fs.existsSync(ourOutput) && fs.statSync(ourOutput).size > 0;
  if (refArtifact !== refAccepted) {
    reasons.push(`amxxpc artifact invariant failed: accepted=${refAccepted}, artifact=${refArtifact}`);
  }
  if (ourArtifact !== ourAccepted) {
    reasons.push(`zplint artifact invariant failed: accepted=${ourAccepted}, artifact=${ourArtifact}`);
  }

  if (ourAccepted) {
    const disasm = run(ZPC, ['disasm', ourOutput, '--normalised']);
    if (disasm.code !== 0) {
      reasons.push(`zplint artifact does not disassemble: exit=${disasm.code}`);
    }
  }

  if (STRICT_DIAGNOSTICS) {
    const refDiags = diagnostics(ref.text, file);
    const ourDiags = diagnostics(ours.text, file);
    if (JSON.stringify(refDiags) !== JSON.stringify(ourDiags)) {
      reasons.push('diagnostics differ');
    }
  }

  if (reasons.length > 0) {
    diverged++;
    report.push({ file, reasons, ref, ours });
  }
}

if (RECORD) {
  console.log(`baseline recorded for ${recorded} file(s) in ${BASELINE}`);
  process.exit(0);
}

for (const result of report) {
  console.log(`\n=== DIVERGED: ${result.file} ===`);
  for (const reason of result.reasons) console.log(`- ${reason}`);
  console.log(`  amxxpc exit ${result.ref.code}; zplint exit ${result.ours.code}`);
  if (
    result.reasons.some(reason => reason.startsWith('acceptance differs')) ||
    (STRICT_DIAGNOSTICS && result.reasons.includes('diagnostics differ'))
  ) {
    console.log('--- amxxpc diagnostics ---');
    console.log(diagnostics(result.ref.text, result.file).join('\n') || '(none)');
    console.log('--- zplint diagnostics ---');
    console.log(diagnostics(result.ours.text, result.file).join('\n') || '(none)');
  }
}

const gate = STRICT_DIAGNOSTICS ? 'strict differential cases' : 'acceptance cases';
console.log(`\n${files.length - diverged}/${files.length} ${gate} matched; ${diverged} diverged`);
process.exit(diverged === 0 ? 0 : 1);
