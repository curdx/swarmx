// End-to-end test for the release-asset naming pipeline. Drives the REAL
// rename-release-assets.mjs and gen-updater-manifest.mjs as subprocesses over
// the exact filenames the Tauri bundler produced for v0.1.2, then asserts the
// friendly names and the latest.json the app's updater consumes. This is the
// safety net for the updater (the v0.1.1/v0.1.2 footgun): a wrong url or a
// dropped .sig here fails the build instead of shipping a dead auto-update.
//
// Run: node --test scripts/release-naming.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, readdir } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { canonicalName, suffixOf, TARGETS } from "./release-naming.mjs";

const HERE = import.meta.dirname;
const RENAME = path.join(HERE, "rename-release-assets.mjs");
const MANIFEST = path.join(HERE, "gen-updater-manifest.mjs");
const VERSION = "0.1.2";
const TAG = "v0.1.2";
const REPO = "curdx/flockmux-core";

// Exactly what the bundler emits per target (the v0.1.2 assets, with the old
// Rust-triple prefix stripped — i.e. the raw bundler basenames).
const RAW = {
  "aarch64-apple-darwin": [
    "flockmux.app.tar.gz",
    "flockmux.app.tar.gz.sig",
    "flockmux_0.1.2_aarch64.dmg",
  ],
  "x86_64-apple-darwin": [
    "flockmux.app.tar.gz",
    "flockmux.app.tar.gz.sig",
    "flockmux_0.1.2_x64.dmg",
  ],
  "x86_64-unknown-linux-gnu": [
    "flockmux-0.1.2-1.x86_64.rpm",
    "flockmux-0.1.2-1.x86_64.rpm.sig",
    "flockmux_0.1.2_amd64.AppImage",
    "flockmux_0.1.2_amd64.AppImage.sig",
    "flockmux_0.1.2_amd64.deb",
    "flockmux_0.1.2_amd64.deb.sig",
  ],
  "x86_64-pc-windows-msvc": [
    "flockmux_0.1.2_x64-setup.exe",
    "flockmux_0.1.2_x64-setup.exe.sig",
    "flockmux_0.1.2_x64_en-US.msi",
    "flockmux_0.1.2_x64_en-US.msi.sig",
  ],
};

const EXPECTED = {
  "aarch64-apple-darwin": [
    "flockmux_0.1.2_macos-arm64.app.tar.gz",
    "flockmux_0.1.2_macos-arm64.app.tar.gz.sig",
    "flockmux_0.1.2_macos-arm64.dmg",
  ],
  "x86_64-apple-darwin": [
    "flockmux_0.1.2_macos-x64.app.tar.gz",
    "flockmux_0.1.2_macos-x64.app.tar.gz.sig",
    "flockmux_0.1.2_macos-x64.dmg",
  ],
  "x86_64-unknown-linux-gnu": [
    "flockmux_0.1.2_linux-x86_64.rpm",
    "flockmux_0.1.2_linux-x86_64.rpm.sig",
    "flockmux_0.1.2_linux-x86_64.AppImage",
    "flockmux_0.1.2_linux-x86_64.AppImage.sig",
    "flockmux_0.1.2_linux-x86_64.deb",
    "flockmux_0.1.2_linux-x86_64.deb.sig",
  ],
  "x86_64-pc-windows-msvc": [
    "flockmux_0.1.2_windows-x64-setup.exe",
    "flockmux_0.1.2_windows-x64-setup.exe.sig",
    "flockmux_0.1.2_windows-x64.msi",
    "flockmux_0.1.2_windows-x64.msi.sig",
  ],
};

async function tmp(prefix) {
  return mkdtemp(path.join(os.tmpdir(), prefix));
}

test("canonicalName maps every real v0.1.2 raw name correctly", () => {
  for (const triple of Object.keys(RAW)) {
    const got = RAW[triple].map((n) => canonicalName(n, triple, VERSION));
    assert.deepEqual(
      got.sort(),
      [...EXPECTED[triple]].sort(),
      `canonical names for ${triple}`,
    );
  }
});

test("no 'unknown' or Rust triple leaks into any canonical name", () => {
  for (const triple of Object.keys(RAW)) {
    for (const n of EXPECTED[triple]) {
      assert.doesNotMatch(n, /unknown|apple-darwin|pc-windows|unknown-linux|amd64|_en-US|aarch64\.dmg/);
    }
  }
});

test("suffixOf prefers the longest match (.sig pairs disambiguated)", () => {
  assert.equal(suffixOf("flockmux.app.tar.gz.sig"), ".app.tar.gz.sig");
  assert.equal(suffixOf("flockmux.app.tar.gz"), ".app.tar.gz");
  assert.equal(suffixOf("x_en-US.msi.sig"), ".msi.sig");
  assert.equal(suffixOf("x_en-US.msi"), ".msi");
  assert.equal(suffixOf("x-setup.exe.sig"), "-setup.exe.sig");
  assert.equal(suffixOf("x-setup.exe"), "-setup.exe");
  assert.equal(suffixOf("mystery.tar.zst"), null);
});

test("rename-release-assets.mjs produces the friendly set for every target", async () => {
  for (const triple of Object.keys(RAW)) {
    const raw = await tmp("fm-raw-");
    const out = await tmp("fm-out-");
    for (const n of RAW[triple]) await writeFile(path.join(raw, n), `body:${n}`);
    execFileSync("node", [RENAME, raw, out, triple, VERSION], { stdio: "pipe" });
    const got = (await readdir(out)).sort();
    assert.deepEqual(got, [...EXPECTED[triple]].sort(), `renamed set for ${triple}`);
  }
});

test("rename fails loudly on an unrecognized artifact", async () => {
  const raw = await tmp("fm-raw-bad-");
  const out = await tmp("fm-out-bad-");
  await writeFile(path.join(raw, "flockmux_0.1.2_amd64.snap"), "x");
  assert.throws(() =>
    execFileSync("node", [RENAME, raw, out, "x86_64-unknown-linux-gnu", VERSION], {
      stdio: "pipe",
    }),
  );
});

test("gen-updater-manifest builds latest.json matching the uploaded files", async () => {
  // Lay out the renamed assets like actions/download-artifact does:
  // release-artifacts/flockmux-<triple>/<canonical files>
  const root = await tmp("fm-rel-");
  const sigBody = {}; // canonical .sig name → its signature content
  for (const triple of Object.keys(EXPECTED)) {
    const dir = path.join(root, `flockmux-${triple}`);
    await mkdir(dir, { recursive: true });
    for (const n of EXPECTED[triple]) {
      const body = n.endsWith(".sig") ? `SIGNATURE(${n})` : `bin(${n})`;
      if (n.endsWith(".sig")) sigBody[n] = body;
      await writeFile(path.join(dir, n), body);
    }
  }

  const stdout = execFileSync(
    "node",
    [MANIFEST, root, TAG, REPO, "2026-06-13T00:00:00Z"],
    { encoding: "utf8" },
  );
  const manifest = JSON.parse(stdout);

  assert.equal(manifest.version, "0.1.2");
  assert.equal(manifest.pub_date, "2026-06-13T00:00:00Z");

  // Every Tauri platform key present, each pointing at the real uploaded file.
  const base = `https://github.com/${REPO}/releases/download/${TAG}/`;
  const want = {
    "darwin-aarch64": "flockmux_0.1.2_macos-arm64.app.tar.gz",
    "darwin-x86_64": "flockmux_0.1.2_macos-x64.app.tar.gz",
    "linux-x86_64": "flockmux_0.1.2_linux-x86_64.AppImage",
    "windows-x86_64": "flockmux_0.1.2_windows-x64.msi",
  };
  assert.deepEqual(Object.keys(manifest.platforms).sort(), Object.keys(want).sort());
  for (const [key, file] of Object.entries(want)) {
    assert.equal(manifest.platforms[key].url, base + file, `url for ${key}`);
    assert.equal(
      manifest.platforms[key].signature,
      sigBody[file + ".sig"],
      `signature for ${key} comes from ${file}.sig`,
    );
  }

  // Windows must pick the MSI updater bundle, never the NSIS setup.exe.
  assert.ok(
    !manifest.platforms["windows-x86_64"].url.includes("setup.exe"),
    "windows updater url must be the .msi, not the setup.exe",
  );
});

test("gen-updater-manifest FAILS LOUDLY if a built bundle is missing its .sig", async () => {
  // macOS arm64 shipped its .app.tar.gz (and .dmg) but signing flaked, so the
  // .app.tar.gz.sig is absent. The leg still exited 0 (the .dmg exists), so a
  // naive generator would silently drop darwin-aarch64. We must refuse instead.
  const root = await tmp("fm-broken-");
  // linux is fully signed (so the run isn't entirely empty)
  const lin = path.join(root, "flockmux-x86_64-unknown-linux-gnu");
  await mkdir(lin, { recursive: true });
  await writeFile(path.join(lin, "flockmux_0.1.2_linux-x86_64.AppImage"), "bin");
  await writeFile(path.join(lin, "flockmux_0.1.2_linux-x86_64.AppImage.sig"), "SIG");
  // macOS arm64: bundle present, signature MISSING
  const mac = path.join(root, "flockmux-aarch64-apple-darwin");
  await mkdir(mac, { recursive: true });
  await writeFile(path.join(mac, "flockmux_0.1.2_macos-arm64.app.tar.gz"), "bin");
  await writeFile(path.join(mac, "flockmux_0.1.2_macos-arm64.dmg"), "bin");

  assert.throws(
    () =>
      execFileSync("node", [MANIFEST, root, TAG, REPO, "2026-06-13T00:00:00Z"], {
        stdio: "pipe",
      }),
    /signature is missing|macos-arm64/,
    "manifest gen must reject a run where a built updater bundle has no .sig",
  );
});

test("gen-updater-manifest is fine when a platform is genuinely not built", async () => {
  // Partial dispatch: only linux built. No macOS bundle present at all → that
  // platform is legitimately absent (not 'broken'), so the manifest succeeds
  // with just the platforms that ran.
  const root = await tmp("fm-partial-");
  const lin = path.join(root, "flockmux-x86_64-unknown-linux-gnu");
  await mkdir(lin, { recursive: true });
  await writeFile(path.join(lin, "flockmux_0.1.2_linux-x86_64.AppImage"), "bin");
  await writeFile(path.join(lin, "flockmux_0.1.2_linux-x86_64.AppImage.sig"), "SIG");

  const out = JSON.parse(
    execFileSync("node", [MANIFEST, root, TAG, REPO, "2026-06-13T00:00:00Z"], {
      encoding: "utf8",
    }),
  );
  assert.deepEqual(Object.keys(out.platforms), ["linux-x86_64"]);
});

test("TARGETS platform keys are the Tauri updater identifiers (must not change)", () => {
  const keys = TARGETS.map((t) => t.platform).sort();
  assert.deepEqual(keys, [
    "darwin-aarch64",
    "darwin-x86_64",
    "linux-x86_64",
    "windows-x86_64",
  ]);
});
