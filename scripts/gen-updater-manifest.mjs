#!/usr/bin/env node
// Aggregate the per-platform updater signatures into a single `latest.json`
// (Tauri's static updater manifest). The app's `endpoints` points at
// `releases/latest/download/latest.json`; on launch it fetches this, compares
// `version`, and if newer downloads the matching platform `url` + verifies it
// against the `.sig` `signature` with the bundled public key.
//
// Usage: node scripts/gen-updater-manifest.mjs <assets-dir> <tag> <repo> [date]
//   <assets-dir>  dir holding the downloaded release assets (with `.sig` files)
//   <tag>         release tag, e.g. v0.2.0
//   <repo>        owner/name, e.g. curdx/flockmux-core
//   [date]        RFC3339 pub_date (pass from the workflow; Date.now isn't used)
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";

const [assetsDir, tag, repo, date] = process.argv.slice(2);
if (!assetsDir || !tag || !repo) {
  console.error("usage: gen-updater-manifest.mjs <assets-dir> <tag> <repo> [date]");
  process.exit(1);
}

// Rust target triple → Tauri updater platform key + updater-bundle suffix.
// (prepare-assets prefixes every asset filename with "<triple>-".)
const TARGETS = [
  { triple: "aarch64-apple-darwin", platform: "darwin-aarch64", suffix: ".app.tar.gz" },
  { triple: "x86_64-apple-darwin", platform: "darwin-x86_64", suffix: ".app.tar.gz" },
  { triple: "x86_64-unknown-linux-gnu", platform: "linux-x86_64", suffix: ".AppImage" },
  { triple: "x86_64-pc-windows-msvc", platform: "windows-x86_64", suffix: ".msi" },
];

async function walk(dir) {
  const out = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...(await walk(full)));
    else out.push(full);
  }
  return out;
}

const files = await walk(assetsDir);
const platforms = {};

for (const t of TARGETS) {
  const sig = files.find(
    (f) => path.basename(f).includes(t.triple) && f.endsWith(`${t.suffix}.sig`),
  );
  if (!sig) continue; // platform not built in this run
  const signature = (await readFile(sig, "utf8")).trim();
  const assetName = path.basename(sig).replace(/\.sig$/, "");
  platforms[t.platform] = {
    signature,
    url: `https://github.com/${repo}/releases/download/${tag}/${assetName}`,
  };
}

if (Object.keys(platforms).length === 0) {
  console.error("no updater .sig artifacts found — did createUpdaterArtifacts + signing run?");
  process.exit(1);
}

process.stdout.write(
  JSON.stringify(
    {
      version: tag.replace(/^v/, ""),
      notes: `flockmux ${tag}`,
      pub_date: date || tag, // workflow passes an RFC3339 timestamp
      platforms,
    },
    null,
    2,
  ),
);
