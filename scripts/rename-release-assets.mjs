#!/usr/bin/env node
// Copy raw Tauri bundler output into friendly, collision-free release names.
//
// Usage: node scripts/rename-release-assets.mjs <raw-dir> <out-dir> <triple> <version>
//   <raw-dir>  dir holding this target's raw bundler files (original names)
//   <out-dir>  dir to write the renamed assets into (created if missing)
//   <triple>   Rust target triple of this matrix job, e.g. aarch64-apple-darwin
//   <version>  app version, e.g. 0.1.2 (from tauri.conf.json)
//
// Fails loudly (non-zero exit) on an unrecognized suffix or a name collision —
// a release that silently dropped or clobbered an artifact is worse than one
// that stops the workflow.
import { readdir, mkdir, copyFile } from "node:fs/promises";
import path from "node:path";
import { canonicalName, targetFor } from "./release-naming.mjs";

const [rawDir, outDir, triple, version] = process.argv.slice(2);
if (!rawDir || !outDir || !triple || !version) {
  console.error(
    "usage: rename-release-assets.mjs <raw-dir> <out-dir> <triple> <version>",
  );
  process.exit(1);
}

targetFor(triple); // validate the triple before touching the filesystem
await mkdir(outDir, { recursive: true });

const entries = (await readdir(rawDir, { withFileTypes: true }))
  .filter((e) => e.isFile())
  .map((e) => e.name)
  .sort();

const taken = new Map(); // canonical name → source name, to catch collisions
let renamed = 0;

for (const name of entries) {
  const canon = canonicalName(name, triple, version);
  if (!canon) {
    console.error(`✗ unrecognized bundle artifact (no known suffix): ${name}`);
    process.exit(1);
  }
  if (taken.has(canon)) {
    console.error(
      `✗ name collision: ${name} and ${taken.get(canon)} both map to ${canon}`,
    );
    process.exit(1);
  }
  taken.set(canon, name);
  await copyFile(path.join(rawDir, name), path.join(outDir, canon));
  console.log(`✓ ${name} → ${canon}`);
  renamed++;
}

if (renamed === 0) {
  console.error(`✗ no bundle artifacts found in ${rawDir}`);
  process.exit(1);
}

console.log(`\nRenamed ${renamed} asset(s) for ${triple} into ${outDir}`);
