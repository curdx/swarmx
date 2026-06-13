#!/usr/bin/env node
// Aggregate the per-platform updater signatures into a single `latest.json`
// (Tauri's static updater manifest). The app's `endpoints` points at
// `releases/latest/download/latest.json`; on launch it fetches this, compares
// `version`, and if newer downloads the matching platform `url` + verifies it
// against the `.sig` `signature` with the bundled public key.
//
// The Tauri updater matches a platform by its `url` (NOT by filename), so the
// url here MUST point at a file that was actually uploaded under that exact
// name. We find each platform's signature by the stable, version-independent
// tail from release-naming.mjs (the same module rename-release-assets.mjs uses
// to produce the names) and derive the bundle url from the matched file — so
// the manifest and the uploaded assets can't drift apart.
//
// Usage: node scripts/gen-updater-manifest.mjs <assets-dir> <tag> <repo> [date]
//   <assets-dir>  dir holding the downloaded release assets (with `.sig` files)
//   <tag>         release tag, e.g. v0.2.0
//   <repo>        owner/name, e.g. curdx/flockmux-core
//   [date]        RFC3339 pub_date (pass from the workflow; Date.now isn't used)
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { TARGETS, updaterSigTail } from "./release-naming.mjs";

const [assetsDir, tag, repo, date] = process.argv.slice(2);
if (!assetsDir || !tag || !repo) {
  console.error("usage: gen-updater-manifest.mjs <assets-dir> <tag> <repo> [date]");
  process.exit(1);
}

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
const broken = [];

for (const t of TARGETS) {
  const tail = updaterSigTail(t); // e.g. "macos-arm64.app.tar.gz.sig"
  const sig = files.find((f) => path.basename(f).endsWith(tail));
  if (sig) {
    const signature = (await readFile(sig, "utf8")).trim();
    const assetName = path.basename(sig).replace(/\.sig$/, "");
    platforms[t.platform] = {
      signature,
      url: `https://github.com/${repo}/releases/download/${tag}/${assetName}`,
    };
    continue;
  }
  // No signature for this platform. If the platform's updater BUNDLE itself
  // shipped (e.g. the .app.tar.gz / .AppImage / .msi is present) but its .sig
  // didn't, the signature silently went missing — that leg can still exit 0
  // because the manual-download .dmg/.deb was produced. Publishing latest.json
  // without this platform would strand every installed user on it ("no update
  // available" forever — the v0.1.1-class symptom). Fail loudly instead.
  const bundleTail = `${t.label}${t.updaterSuffix}`; // e.g. "macos-arm64.app.tar.gz"
  const bundle = files.find((f) => path.basename(f).endsWith(bundleTail));
  if (bundle) broken.push(`${t.platform}: have ${path.basename(bundle)} but no ${path.basename(bundle)}.sig`);
  // else: platform genuinely not built in this run (partial dispatch) — skip.
}

if (broken.length) {
  console.error(
    "updater bundle present but its signature is missing — refusing to publish a\n" +
      "partial latest.json that would strand those users:\n  " +
      broken.join("\n  "),
  );
  process.exit(1);
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
