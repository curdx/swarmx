// Single source of truth for release-asset naming.
//
// Raw Tauri bundler output is named with bundler-specific noise — Rust target
// triples on the updater bundles plus inconsistent arch tokens (`aarch64`,
// `x64`, `amd64`, `x86_64`) and a `_en-US` locale on the MSI. Left alone, the
// release publishes scary names like `x86_64-unknown-linux-gnu-swarmx_0.1.2_
// amd64.deb` (the "unknown" is the vendor field of the Rust triple — normal,
// but it reads like a bug to users).
//
// Both the rename step (rename-release-assets.mjs) and the updater manifest
// generator (gen-updater-manifest.mjs) import THIS module, so the friendly
// names and the `latest.json` urls can never drift apart. Drift here silently
// breaks auto-update for every installed user — the Tauri updater fetches the
// bundle by the manifest `url`, so a url that doesn't match an uploaded file is
// a dead update (the v0.1.1 / v0.1.2 class of bug). Keep the mapping in one
// place and both sides stay honest.
//
// Canonical name: <product>_<version>_<label><suffix>
//   e.g. swarmx_0.1.2_macos-arm64.app.tar.gz
//        swarmx_0.1.2_linux-x86_64.AppImage
//        swarmx_0.1.2_windows-x64-setup.exe

export const PRODUCT = "swarmx";

// Rust target triple → { label used in file names, Tauri updater platform key,
// the updater-bundle suffix for that platform }.
//
// `platform` keys are Tauri's `{os}-{arch}` updater identifiers and MUST stay
// exactly as the app's bundled updater expects (darwin/linux/windows +
// x86_64/aarch64) — they are what installed apps match against in latest.json.
// `label` is purely cosmetic (what humans read in the filename) and is free to
// be friendly (macos/windows/linux + arm64/x64/x86_64).
export const TARGETS = [
  {
    triple: "aarch64-apple-darwin",
    label: "macos-arm64",
    platform: "darwin-aarch64",
    updaterSuffix: ".app.tar.gz",
  },
  {
    triple: "x86_64-apple-darwin",
    label: "macos-x64",
    platform: "darwin-x86_64",
    updaterSuffix: ".app.tar.gz",
  },
  {
    triple: "x86_64-unknown-linux-gnu",
    label: "linux-x86_64",
    platform: "linux-x86_64",
    updaterSuffix: ".AppImage",
  },
  {
    triple: "x86_64-pc-windows-msvc",
    label: "windows-x64",
    platform: "windows-x86_64",
    updaterSuffix: ".msi",
  },
];

export function targetFor(triple) {
  const t = TARGETS.find((x) => x.triple === triple);
  if (!t) {
    throw new Error(
      `unknown target triple: ${triple} (known: ${TARGETS.map((x) => x.triple).join(", ")})`,
    );
  }
  return t;
}

// Recognized suffixes on raw bundler output, LONGEST FIRST so e.g.
// `.app.tar.gz.sig` wins over `.app.tar.gz` and `.sig`. Everything before the
// matched suffix (the bundler's own version/arch/locale noise) is discarded and
// rebuilt from <product>_<version>_<label>. The release workflow only collects
// these extensions, so this list is the full universe of inputs.
const SUFFIXES = [
  ".app.tar.gz.sig",
  ".app.tar.gz",
  ".AppImage.sig",
  ".AppImage",
  ".msi.sig",
  ".msi",
  "-setup.exe.sig",
  "-setup.exe",
  ".deb.sig",
  ".deb",
  ".rpm.sig",
  ".rpm",
  ".dmg",
];

// Return the canonical suffix this raw filename ends with, or null if none.
export function suffixOf(filename) {
  for (const s of SUFFIXES) {
    if (filename.endsWith(s)) return s;
  }
  return null;
}

// Raw bundler basename → canonical asset name, or null if the suffix is
// unrecognized (caller should treat null as a hard error, not skip silently).
export function canonicalName(rawBasename, triple, version) {
  const suffix = suffixOf(rawBasename);
  if (!suffix) return null;
  const { label } = targetFor(triple);
  return `${PRODUCT}_${version}_${label}${suffix}`;
}

// The stable tail of a platform's updater-bundle signature file, independent of
// version. gen-updater-manifest matches on this (so a tag/version skew can't
// make it miss) and derives the bundle url from the actual matched file.
//   e.g. "macos-arm64.app.tar.gz.sig", "linux-x86_64.AppImage.sig"
export function updaterSigTail(target) {
  return `${target.label}${target.updaterSuffix}.sig`;
}
