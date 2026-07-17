#!/usr/bin/env node

import { execSync } from 'child_process';
import { createRequire } from 'module';
import { existsSync, mkdirSync } from 'fs';
import { resolve, dirname, basename } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const WIT_DIR = resolve(__dirname);

const args = process.argv.slice(2);

if (args.length === 0 || args.includes('--help')) {
  console.log(`Usage: workforce-build <config.ts> [config2.ts ...] [-o <outdir>]`);
  console.log(`\nCompiles TypeScript employee configs to WASM components.`);
  process.exit(0);
}

let outDir = './build';
const files = [];

for (let i = 0; i < args.length; i++) {
  if (args[i] === '-o' && args[i + 1]) {
    outDir = args[++i];
  } else {
    files.push(args[i]);
  }
}

if (files.length === 0) {
  console.error('Error: no input files');
  process.exit(1);
}

mkdirSync(outDir, { recursive: true });

const inputDir = dirname(resolve(files[0]));
const req = createRequire(resolve(inputDir, 'node_modules', '_'));
const { build } = await import(req.resolve('esbuild'));

const jcoCandidates = [
  resolve(inputDir, 'node_modules', '.bin', 'jco'),
  resolve(__dirname, 'node_modules', '.bin', 'jco'),
];
const jcoCmd = jcoCandidates.find(existsSync) || 'jco';

for (const file of files) {
  const name = basename(file, '.ts');
  const jsOut = resolve(outDir, `${name}.js`);
  const wasmOut = resolve(outDir, `${name}.wasm`);

  console.log(`==> Building: ${name}`);

  await build({
    entryPoints: [file],
    bundle: true,
    outfile: jsOut,
    format: 'esm',
    target: 'es2020',
    external: ['adi:workforce/*'],
  });

  execSync(
    `"${jcoCmd}" componentize "${jsOut}" --wit "${WIT_DIR}" --world-name loop-script -o "${wasmOut}" -d all`,
    { stdio: 'inherit' },
  );

  console.log(`    → ${wasmOut}\n`);
}

console.log(`==> Built ${files.length} employee(s)`);
