#!/usr/bin/env node
// Harness check — mechanical, deterministic guards for cross-file invariants
// that the compiler and `cargo test` can NOT catch (they compile fine but fail
// silently at runtime or at package time).
//
// This is NOT an AI reviewer. It is a zero-dependency Node script of "if A
// changed but B didn't / if file X no longer contains Y, then refuse" rules.
// Every rule below encodes a real footgun this repo has already hit (see the
// project memory). Each time we hit a NEW silent-failure footgun, add a rule
// here so it can never be stepped on twice.
//
// Run:        node scripts/harness-check.mjs
// Exit:       0 = all guards pass, 1 = at least one guard tripped (prints why).
//
// All current rules are STATIC (read files + assert), so the script behaves
// identically in a local pre-commit hook and in CI — no git history / PR diff
// needed. If a future rule needs diff-awareness, add a `git diff` helper then.

import { readFile } from "node:fs/promises";
import path from "node:path";

const root = process.cwd();
const failures = [];
const fail = (message) => failures.push(message);

async function readText(relativePath) {
  return readFile(path.join(root, relativePath), "utf8");
}

// Extract the field names of a Rust `struct <name> { ... }` by brace-matching
// its body and collecting every `pub <field>:`. Returns null if not found.
function structFields(text, structName) {
  const decl = text.indexOf(`struct ${structName}`);
  if (decl < 0) return null;
  const open = text.indexOf("{", decl);
  if (open < 0) return null;
  let depth = 0;
  let end = -1;
  for (let i = open; i < text.length; i++) {
    if (text[i] === "{") depth++;
    else if (text[i] === "}") {
      depth--;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) return null;
  const body = text.slice(open + 1, end);
  const fields = [];
  const re = /\bpub\s+(\w+)\s*:/g;
  let m;
  while ((m = re.exec(body))) fields.push(m[1]);
  return fields;
}

// ─────────────────────────────────────────────────────────────────────────
// Rule 1 — the two MessageRecord DTOs + their hand-written mappings must stay
// in sync.
//
// Root cause it guards: `protocol::rest::MessageRecord` (REST out-param) and
// `storage::models::MessageRecord` (DB row) are two separate structs, and
// `routes/swarm.rs` maps DB → REST field-by-field BY HAND in two places
// (list_messages, send_message). Add a field to the structs but forget either
// mapping → compiles fine, field is SILENTLY dropped from the API response.
// ─────────────────────────────────────────────────────────────────────────
{
  const restPath = "crates/flockmux-protocol/src/rest.rs";
  const modelsPath = "crates/flockmux-storage/src/models.rs";
  const swarmRoutePath = "crates/flockmux-server/src/routes/swarm.rs";
  const restText = await readText(restPath);
  const modelsText = await readText(modelsPath);
  const swarmRoute = await readText(swarmRoutePath);

  const restFields = structFields(restText, "MessageRecord");
  const storageFields = structFields(modelsText, "MessageRecord");

  if (!restFields) fail(`规则1: 在 ${restPath} 找不到 struct MessageRecord（解析失败）`);
  if (!storageFields) fail(`规则1: 在 ${modelsPath} 找不到 struct MessageRecord（解析失败）`);

  if (restFields && storageFields) {
    const restSet = new Set(restFields);
    const storageSet = new Set(storageFields);
    const onlyRest = restFields.filter((f) => !storageSet.has(f));
    const onlyStorage = storageFields.filter((f) => !restSet.has(f));
    if (onlyRest.length || onlyStorage.length) {
      fail(
        `规则1: 两个 MessageRecord 字段不一致 —— protocol::rest 独有 [${
          onlyRest.join(", ") || "—"
        }]，storage::models 独有 [${
          onlyStorage.join(", ") || "—"
        }]。给 message 加字段必须 rest.rs 与 models.rs 两个 DTO 同步，否则 REST 出参静默丢字段。`,
      );
    }

    // Both hand-written mappings in routes/swarm.rs must cover every field.
    // list_messages maps from `r.<field>`; send_message maps from `record.<field>`.
    for (const f of restFields) {
      if (!swarmRoute.includes(`${f}: r.${f}`)) {
        fail(
          `规则1: ${swarmRoutePath} 的 list_messages 映射缺字段 \`${f}\`（应有 \`${f}: r.${f},\`）—— 新字段会在 GET /api/message 出参里静默丢失。`,
        );
      }
      if (!swarmRoute.includes(`${f}: record.${f}`)) {
        fail(
          `规则1: ${swarmRoutePath} 的 send_message 映射缺字段 \`${f}\`（应有 \`${f}: record.${f},\`）—— 新字段会在 POST /api/message 出参里静默丢失。`,
        );
      }
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────
// Rule 2 — every public type in flockmux-storage::models must be re-exported
// from its lib.rs.
//
// Root cause it guards: `flockmux-storage/src/lib.rs` re-exports models via an
// EXPLICIT list (`pub use models::{...}`), not `pub use models::*`. Add a
// `pub struct/enum` to models.rs but forget the list → storage compiles, but
// external crates (server/swarm) can't reference the new type, and it's easy
// to miss until something downstream won't build.
// ─────────────────────────────────────────────────────────────────────────
{
  const modelsPath = "crates/flockmux-storage/src/models.rs";
  const libPath = "crates/flockmux-storage/src/lib.rs";
  const modelsText = await readText(modelsPath);
  const libText = await readText(libPath);

  const listStart = libText.indexOf("pub use models::{");
  const listEnd = listStart < 0 ? -1 : libText.indexOf("};", listStart);
  if (listStart < 0 || listEnd < 0) {
    fail(`规则2: ${libPath} 找不到 \`pub use models::{ ... };\` re-export 清单`);
  } else {
    const reExport = libText.slice(listStart, listEnd);
    const re = /^pub\s+(?:struct|enum)\s+(\w+)/gm;
    const missing = [];
    let m;
    while ((m = re.exec(modelsText))) {
      const name = m[1];
      if (!new RegExp(`\\b${name}\\b`).test(reExport)) missing.push(name);
    }
    if (missing.length) {
      fail(
        `规则2: storage 公开类型未 re-export —— models.rs 定义了 [${missing.join(
          ", ",
        )}]，但 ${libPath} 的 \`pub use models::{...}\` 清单里没有。外部 crate（server/swarm）无法引用，请加进清单。`,
      );
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────
// Rule 3 — Tauri externalBin must list all three sidecar binaries.
//
// Root cause it guards: the .app bundles server + shim + mcp as sidecars. Drop
// any one from `bundle.externalBin` and the app starts, then immediately fails
// when it can't find that binary at runtime.
// ─────────────────────────────────────────────────────────────────────────
{
  const confPath = "web/src-tauri/tauri.conf.json";
  const conf = JSON.parse(await readText(confPath));
  const externalBin = conf?.bundle?.externalBin;
  const required = [
    "binaries/flockmux-server",
    "binaries/flockmux-shim",
    "binaries/flockmux-mcp",
  ];
  if (!Array.isArray(externalBin)) {
    fail(`规则3: ${confPath} 的 bundle.externalBin 不是数组（或缺失）`);
  } else {
    for (const bin of required) {
      if (!externalBin.includes(bin)) {
        fail(
          `规则3: ${confPath} 的 bundle.externalBin 缺 \`${bin}\` —— sidecar 三件套（server/shim/mcp）缺一个，.app 启动后找不到该二进制立即崩溃。`,
        );
      }
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────
// Rule 4 — codex worker spawn must keep injecting a per-agent CODEX_HOME.
//
// Root cause it guards: codex workers MUST run with an isolated CODEX_HOME so
// they load ONLY the flockmux-swarm MCP, not the user's personal ~/.codex MCP
// servers (chrome-devtools, pencil, …) which stall a headless worker at
// startup (503 / hangs). If a refactor of the spawn env drops this line, codex
// workers silently fall back to the global ~/.codex and break.
// ─────────────────────────────────────────────────────────────────────────
{
  const spawnPath = "crates/flockmux-server/src/spawn.rs";
  const spawnText = await readText(spawnPath);
  // Match the quoted literal `"CODEX_HOME"` — it only appears in the actual
  // `env.insert("CODEX_HOME", ...)` call, NOT in the surrounding prose comments
  // (which mention CODEX_HOME unquoted). So deleting the injection trips this
  // even if the explanatory comment stays.
  if (!spawnText.includes('"CODEX_HOME"')) {
    fail(
      `规则4: ${spawnPath} 不再注入 CODEX_HOME（找不到 env.insert("CODEX_HOME", ...)）—— codex worker 会回退到用户全局 ~/.codex，被个人 MCP server 卡死/503。务必保留 per-agent CODEX_HOME 注入。`,
    );
  }
  if (!spawnText.includes("McpFormat::CodexGlobalToml")) {
    fail(
      `规则4: ${spawnPath} 缺 McpFormat::CodexGlobalToml 门控 —— 确认 codex per-agent CODEX_HOME 的注入条件还在。`,
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────
if (failures.length > 0) {
  console.error(`❌ harness-check 失败（${failures.length} 项）：`);
  for (const failure of failures) console.error(`  - ${failure}`);
  console.error("\n这些是编译器/cargo test 抓不到的「跨文件静默坑」。修复后再提交。");
  process.exit(1);
}

console.log("✅ harness-check 通过");
