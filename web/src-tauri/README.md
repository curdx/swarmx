# flockmux-tauri

Tauri 2.x desktop shell for flockmux. Bundles the React frontend (`web/`)
and the `flockmux-server` binary into a single .app / .exe / .AppImage.

## Layout

```
web/src-tauri/
├── Cargo.toml          ← independent sub-workspace (NOT part of the
│                          repo root cargo workspace)
├── tauri.conf.json     ← productName, devUrl, externalBin, icons
├── src/
│   ├── main.rs         ← thin wrapper, calls flockmux_tauri_lib::run()
│   └── lib.rs          ← Tauri Builder + tray + (release) sidecar spawn
├── capabilities/
│   └── default.json    ← shell:allow-spawn for the flockmux-server sidecar
├── scripts/
│   └── build-sidecar.sh
├── binaries/           ← (.gitignore) staged sidecar binaries per target
│                          triple, e.g. flockmux-server-aarch64-apple-darwin
└── icons/              ← Tauri-generated app icons
```

## Dev workflow

Two processes, dev never spawns the sidecar (so backend changes hot-reload
in their own shell):

```bash
# terminal 1 — backend
cargo run -p flockmux-server

# terminal 2 — Tauri shell + vite
cd web
npm run tauri:dev
```

The frontend talks to `127.0.0.1:7777` via vite's `/api` + `/ws` proxy.

## Production build

Sidecar must be staged once before `tauri build` so it gets bundled:

```bash
cd web
npm run sidecar:release   # builds flockmux-server in release mode,
                          # copies to binaries/flockmux-server-<host triple>
npm run tauri:build       # bundles .app / .dmg / etc.
```

Requires Xcode (macOS) for code-signing the .app. On a fresh machine:

```bash
xcode-select --install   # CLT only is not enough for full bundle signing
```

## Why src-tauri is its own workspace

The repo root `Cargo.toml` is a workspace with 9 crates; adding `src-tauri`
as a 10th member caused Tauri CLI's `cargo run` (which doesn't pass `-p`)
to fail with `tauri project package doesn't exist in cargo metadata output`.
Marking `src-tauri/Cargo.toml` with a top-level `[workspace]` makes it a
self-contained micro-workspace so the CLI can unambiguously resolve the
single binary inside it. The root workspace `exclude = ["web/src-tauri"]`
keeps `cargo build` at the root from descending into it.

The price: dependencies are not shared via `workspace.dependencies` — but
`src-tauri` only depends on the `tauri-*` family, none of which are used
elsewhere, so duplication is zero.
