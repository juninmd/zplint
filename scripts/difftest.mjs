// Differential oracle: compile a corpus with the reference amxxpc and with zpc,
// then diff the results. This is the harness the whole migration is validated by
// (docs/COMPILER_MIGRATION.md section 2).
//
// Usage:
//   node scripts/difftest.mjs --amxxpc <path-to-amxxpc.exe> --include <dir> [--zpc <path>] [--corpus <dir>] [--record]
//
//   --record   only run amxxpc and save its output as the baseline (useful before
//              zpc can compile anything, so the oracle exists from day one)
//
// Exit code is non-zero when any file diverges, so this can gate CI.

import fs from 'node:fs';
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

if (!AMXXPC) {
  console.error('error: --amxxpc <path> is required (the reference compiler is the oracle)');
  console.error('       amxxpc ships with AMX Mod X; point at its scripting/amxxpc.exe');
  process.exit(2);
}
if (!fs.existsSync(AMXXPC)) {
  console.error(`error: amxxpc not found at ${AMXXPC}`);
  process.exit(2);
}

/** Collect .sma files recursively. */
function collect(dir) {
  const out = [];
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...collect(p));
    else if (e.name.endsWith('.sma')) out.push(p);
  }
  return out;
}

/**
 * amxxpc bakes absolute paths and a timestamp into its output. Normalise both so a
 * diff shows real semantic differences instead of environment noise.
 */
function normalise(text, file) {
  return text
    .replaceAll('\\', '/')
    .replaceAll(path.resolve(file).replaceAll('\\', '/'), '<SRC>')
    .replace(/^.*?([^/]+\.(?:sma|inc))\(/gm, '$1(')
    .replace(/\d{4}-\d{2}-\d{2}|\d{2}:\d{2}:\d{2}/g, '<TIME>')
    .split('\n').map(l => l.trimEnd()).filter(l => l !== '')
    .join('\n');
}

function runCompiler(exe, args, cwd) {
  const r = spawnSync(exe, args, { cwd, encoding: 'utf8', timeout: 60_000 });
  return {
    stdout: r.stdout ?? '',
    stderr: r.stderr ?? '',
    code: r.status ?? -1,
    failed: r.error ? String(r.error) : null,
  };
}

const files = collect(CORPUS);
if (files.length === 0) {
  console.error(`error: no .sma files under ${CORPUS}`);
  process.exit(2);
}

fs.mkdirSync(BASELINE, { recursive: true });

let diverged = 0, recorded = 0, compared = 0;
const report = [];

for (const file of files) {
  const outAmx = path.join(BASELINE, path.basename(file, '.sma') + '.ref.amxx');
  const refArgs = ['-o' + outAmx];
  if (INCLUDE) refArgs.push('-i' + INCLUDE);
  refArgs.push(file);

  const ref = runCompiler(AMXXPC, refArgs, process.cwd());
  const refText = normalise(ref.stdout + ref.stderr, file);
  const refPath = path.join(BASELINE, path.basename(file, '.sma') + '.ref.txt');

  if (RECORD) {
    fs.writeFileSync(refPath, refText);
    recorded++;
    console.log(`recorded  ${path.basename(file)}  (exit ${ref.code}, ${refText.split('\n').length} diag line(s))`);
    continue;
  }

  if (!fs.existsSync(ZPC)) {
    console.error(`error: zpc binary not found at ${ZPC} - build it or pass --zpc, or use --record`);
    process.exit(2);
  }

  const ours = runCompiler(ZPC, ['compile', file, '--include', INCLUDE ?? ''], process.cwd());
  const ourText = normalise(ours.stdout + ours.stderr, file);
  compared++;

  if (ourText !== refText) {
    diverged++;
    report.push({ file, refText, ourText, refCode: ref.code, ourCode: ours.code });
  }
}

if (RECORD) {
  console.log(`\nbaseline recorded for ${recorded} file(s) in ${BASELINE}`);
  process.exit(0);
}

for (const r of report) {
  console.log(`\n=== DIVERGED: ${r.file} (amxxpc exit ${r.refCode}, zpc exit ${r.ourCode}) ===`);
  console.log('--- amxxpc ---');
  console.log(r.refText || '(no output)');
  console.log('--- zpc ---');
  console.log(r.ourText || '(no output)');
}

console.log(`\n${compared - diverged}/${compared} matched, ${diverged} diverged`);
process.exit(diverged === 0 ? 0 : 1);
