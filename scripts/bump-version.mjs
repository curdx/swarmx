#!/usr/bin/env node
// Single source of truth for the app version. Writes one version string into
// ALL FOUR manifests so a release can't ship as vX outside but self-report
// 0.1.0 inside (which would mislabel crash logs / usage / MCP `initialize`).
//
// Usage:  node scripts/bump-version.mjs <x.y.z>
//         node scripts/bump-version.mjs --check   (assert all four agree)
import { readFile, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";

const MANIFESTS = [
  // [file, regex capturing everything up to the value, with the value as "..."]
  [
    "Cargo.toml",
    /(\[workspace\.package\][\s\S]*?\nversion\s*=\s*)"[^"]+"/,
  ],
  ["web/package.json", /("version"\s*:\s*)"[^"]+"/],
  ["web/src-tauri/tauri.conf.json", /("version"\s*:\s*)"[^"]+"/],
  ["web/src-tauri/Cargo.toml", /(^|\n)(version\s*=\s*)"[^"]+"/],
];

const VALUE_RE = {
  "Cargo.toml": /\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/,
  "web/package.json": /"version"\s*:\s*"([^"]+)"/,
  "web/src-tauri/tauri.conf.json": /"version"\s*:\s*"([^"]+)"/,
  "web/src-tauri/Cargo.toml": /(?:^|\n)version\s*=\s*"([^"]+)"/,
};

const arg = process.argv[2];

async function currentVersions() {
  const out = {};
  for (const [file] of MANIFESTS) {
    const text = await readFile(file, "utf8");
    const m = text.match(VALUE_RE[file]);
    out[file] = m ? m[1] : null;
  }
  return out;
}

if (arg === "--check") {
  const vers = await currentVersions();
  const values = Object.values(vers);
  const bad = Object.entries(vers).filter(([, v]) => !v);
  if (bad.length) {
    console.error("✗ version not found in:", bad.map(([f]) => f).join(", "));
    process.exit(1);
  }
  if (new Set(values).size !== 1) {
    console.error("✗ version mismatch across manifests:");
    for (const [f, v] of Object.entries(vers)) console.error(`  ${f}: ${v}`);
    process.exit(1);
  }
  console.log(`✓ all manifests agree on ${values[0]}`);
  process.exit(0);
}

const version = arg;
if (!version || !/^\d+\.\d+\.\d+(-[\w.]+)?$/.test(version)) {
  console.error("Usage: node scripts/bump-version.mjs <x.y.z>  |  --check");
  process.exit(1);
}

for (const [file, re] of MANIFESTS) {
  const text = await readFile(file, "utf8");
  if (!re.test(text)) {
    console.error(`✗ ${file}: version field not found`);
    process.exit(1);
  }
  // Replace only the matched value, preserving the captured prefix.
  const next = text.replace(re, (m) => m.replace(/"[^"]+"$/, `"${version}"`));
  await writeFile(file, next);
  console.log(`✓ ${file} → ${version}`);
}

// Cargo.lock records each workspace crate's version too. A bumped Cargo.toml
// without a synced lock makes the release gate (`cargo test --locked`) fail
// with "cannot update the lock file because --locked was passed". Sync it now —
// `--workspace` only rewrites the swarmx-* member versions, not any deps.
try {
  execFileSync("cargo", ["update", "--workspace"], { stdio: "inherit" });
  console.log("✓ Cargo.lock synced (cargo update --workspace)");
} catch {
  console.error(
    "✗ Cargo.lock NOT synced. Run `cargo update --workspace` before committing,\n" +
      "  otherwise the release gate (cargo test --locked) will fail.",
  );
  process.exit(1);
}

console.log(`\nAll manifests synced to ${version}. Commit, then tag v${version}.`);
