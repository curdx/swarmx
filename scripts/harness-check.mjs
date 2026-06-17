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
//
// Since the per-CLI adapter split, the two halves of this invariant live in
// separate files: the actual `env.insert("CODEX_HOME", ...)` is in the codex
// adapter's `contribute_env`, and the routing gate that selects that adapter
// for a codex plugin (`McpFormat::CodexGlobalToml`) is in the dispatch hub.
// Both must stay put.
// ─────────────────────────────────────────────────────────────────────────
{
  const codexAdapterPath = "crates/flockmux-server/src/cli/codex.rs";
  const dispatchPath = "crates/flockmux-server/src/cli/mod.rs";
  const codexAdapterText = await readText(codexAdapterPath);
  const dispatchText = await readText(dispatchPath);
  // Match the quoted literal `"CODEX_HOME"` — it only appears in the actual
  // `env.insert("CODEX_HOME", ...)` call, NOT in the surrounding prose comments
  // (which mention CODEX_HOME unquoted). So deleting the injection trips this
  // even if the explanatory comment stays.
  if (!codexAdapterText.includes('"CODEX_HOME"')) {
    fail(
      `规则4: ${codexAdapterPath} 不再注入 CODEX_HOME（找不到 env.insert("CODEX_HOME", ...)）—— codex worker 会回退到用户全局 ~/.codex，被个人 MCP server 卡死/503。务必保留 per-agent CODEX_HOME 注入。`,
    );
  }
  // The dispatch must still route codex plugins (mcp_format = codex-global-toml)
  // to CodexAdapter; without it, codex agents get the generic no-op adapter and
  // never receive CODEX_HOME.
  if (!dispatchText.includes("McpFormat::CodexGlobalToml")) {
    fail(
      `规则4: ${dispatchPath} 缺 McpFormat::CodexGlobalToml 门控 —— 确认 adapter_for 仍把 codex 路由到 CodexAdapter（其 contribute_env 注入 per-agent CODEX_HOME）。`,
    );
  }
}

// ─────────────────────────────────────────────────────────────────────────
// Rule 5 — every migration file must be BOTH registered (include_str! const)
// AND applied (apply(conn, N, ...)) in schema.rs.
//
// Root cause it guards: adding `migrations/00NN_*.sql` requires two manual
// edits in schema.rs — a `const MIGRATION_00NN = include_str!(...)` and a
// `(N, MIGRATION_00NN)` entry in the `MIGRATIONS` array. Miss either and the
// crate compiles fine but the migration SILENTLY never runs, so the new
// table/column doesn't exist at runtime (a query fails far from the cause).
// ─────────────────────────────────────────────────────────────────────────
{
  const { readdir } = await import("node:fs/promises");
  const migDir = "crates/flockmux-storage/migrations";
  const schemaPath = "crates/flockmux-storage/src/schema.rs";
  const schema = await readText(schemaPath);
  let files = [];
  try {
    files = (await readdir(path.join(root, migDir))).filter((f) =>
      /^\d{4}_.*\.sql$/.test(f),
    );
  } catch (e) {
    fail(`规则5: 读不到 migrations 目录 ${migDir}：${e.message}`);
  }
  for (const f of files.sort()) {
    const four = f.slice(0, 4); // "0016"
    const n = parseInt(four, 10); // 16
    const constName = `MIGRATION_${four}`;
    if (!new RegExp(`const ${constName}\\b`).test(schema)) {
      fail(
        `规则5: 迁移 ${f} 未在 ${schemaPath} 注册 —— 缺 \`const ${constName}: &str = include_str!(...)\`。新迁移忘登记会静默不执行（编译过但新表/列不存在）。`,
      );
    }
    if (!new RegExp(`\\(\\s*${n}\\s*,\\s*${constName}\\b`).test(schema)) {
      fail(
        `规则5: 迁移 ${f} 未在 ${schemaPath} 的 MIGRATIONS 数组里登记 —— 缺 \`(${n}, ${constName})\`。新迁移忘登记会静默不执行（编译过但新表/列不存在）。`,
      );
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────
// 规则6: README 不得教用户运行已删除的 spell。
// Root cause it guards: spells were deleted (critic-loop / fullstack-feature /
// auto-dispatch) but the READMEs kept Quick-Start / walkthrough steps telling
// users to run them — a fresh user's very first example then fails. Assert that
// every `spells/<name>.md` path the READMEs reference actually exists on disk.
// ─────────────────────────────────────────────────────────────────────────
{
  const { readdir } = await import("node:fs/promises");
  let existing = new Set();
  try {
    existing = new Set(
      (await readdir(path.join(root, "spells")))
        .filter((f) => f.endsWith(".md"))
        .map((f) => f.slice(0, -3)),
    );
  } catch (e) {
    fail(`规则6: 读不到 spells 目录：${e.message}`);
  }
  for (const rel of ["README.md", "README.zh-CN.md"]) {
    let text = "";
    try {
      text = await readText(rel);
    } catch {
      continue; // 缺某语言 README 不在本规则职责内
    }
    // Skip backlog/roadmap rows (`| P1 | ... |`): those legitimately list
    // FUTURE spell files that don't exist yet (same as `cli-plugins/*.toml`
    // entries) — they aren't teaching anyone to run a removed spell.
    for (const line of text.split("\n")) {
      if (/^\s*\|\s*P[0-9]\s*\|/.test(line)) continue;
      for (const m of line.matchAll(/spells\/([a-z0-9][a-z0-9-]*)\.md/g)) {
        if (!existing.has(m[1])) {
          fail(
            `规则6: ${rel} 引用了 spells/${m[1]}.md，但该 spell 不存在（已删除的 spell 仍在文档里教用户运行 → 新用户照做必失败）。`,
          );
        }
      }
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────
// 规则7: 每个被代码读取的 FLOCKMUX_* 变量必须在 docs/configuration.md 有条目。
// Root cause it guards: a new `std::env::var("FLOCKMUX_NEW_THING")` ships but
// nobody documents it, so users can't discover the knob. Scan every .rs under
// crates/ for `FLOCKMUX_*` and assert the config doc mentions each.
// ─────────────────────────────────────────────────────────────────────────
{
  const { readdir, readFile: rf } = await import("node:fs/promises");
  async function collectRs(dir, acc) {
    let entries;
    try {
      entries = await readdir(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      if (e.name === "target" || e.name === "node_modules") continue;
      const full = path.join(dir, e.name);
      if (e.isDirectory()) await collectRs(full, acc);
      else if (e.name.endsWith(".rs")) acc.push(await rf(full, "utf8"));
    }
  }
  const sources = [];
  await collectRs(path.join(root, "crates"), sources);
  const used = new Set();
  for (const text of sources) {
    for (const m of text.matchAll(/FLOCKMUX_[A-Z_]+/g)) used.add(m[0]);
  }
  let doc = "";
  try {
    doc = await readText("docs/configuration.md");
  } catch (e) {
    fail(`规则7: 读不到 docs/configuration.md：${e.message}`);
  }
  for (const v of [...used].sort()) {
    if (!doc.includes(v)) {
      fail(
        `规则7: 环境变量 ${v} 被代码读取但 docs/configuration.md 没有条目（新加 env 忘记文档化 → 用户无从发现）。`,
      );
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────
// 规则8: i18n —— 代码用到的每个 t("key") 必须在 en + zh 都有；en/zh 结构对齐；
// 不得有「中文值与英文逐字相同」的疑似漏翻（英文回退）。
//
// Root cause it guards: 前端 i18n 是手维护的 en.json/zh.json，有三类编译器与
// vitest 都抓不到、只有真实用户能看到的「界面撒谎」：
//   (a) 代码 t("x.y") 引用了两个 locale 都没有的 key → 用户看到原始 key 串；
//   (b) 加了 en 没加 zh（或反之）→ 切语言后那条目回退、用户看到另一种语言；
//   (c) zh 值直接照抄英文（漏翻）→ 中文用户看到英文。
// i18next 复数后缀(_one/_other/…)与动态拼接 key(t(`a.${x}`) / t("a.b."))需特殊
// 处理，否则误报。本仓库 i18n 大债曾靠反复手工批次修而无回归防线，这条把它焊死。
// ─────────────────────────────────────────────────────────────────────────
{
  const { readdir, readFile: rf } = await import("node:fs/promises");
  const PLURAL = ["_zero", "_one", "_two", "_few", "_many", "_other", "_plural"];
  // 合法「中英同形」白名单：品牌名/URL/纯技术标识等不该翻译的整条 key。
  // 单词级技术名词(MCP/PTY/CLI/stdio…)不会触发回退规则——回退规则只盯「含空格
  // 分隔的多词英文短语」，单 token 天然豁免，无需在此登记。
  const FALLBACK_ALLOW = new Set([
    "settings.about.repoUrl", // 仓库 URL，不翻译
  ]);

  function flatten(obj, prefix, acc) {
    for (const [k, v] of Object.entries(obj)) {
      const key = prefix ? `${prefix}.${k}` : k;
      if (v && typeof v === "object" && !Array.isArray(v)) flatten(v, key, acc);
      else acc[key] = v;
    }
    return acc;
  }
  function baseKey(k) {
    for (const suf of PLURAL) if (k.endsWith(suf)) return k.slice(0, -suf.length);
    return k;
  }

  let en, zh;
  try {
    en = JSON.parse(await readText("web/src/i18n/locales/en.json"));
    zh = JSON.parse(await readText("web/src/i18n/locales/zh.json"));
  } catch (e) {
    fail(`规则8: 读不到/解析失败 en.json 或 zh.json：${e.message}`);
  }
  if (en && zh) {
    const fen = flatten(en, "", {});
    const fzh = flatten(zh, "", {});
    // base-key 索引：strip 复数后缀后的 key 集合，用于「存在性」与「对齐」判断，
    // 这样 zh 只给 _other（中文唯一复数类目）也算覆盖了该 base key。
    const basesEn = new Set(Object.keys(fen).map(baseKey));
    const basesZh = new Set(Object.keys(fzh).map(baseKey));
    const hasEn = (key) => fen[key] !== undefined || basesEn.has(key);
    const hasZh = (key) => fzh[key] !== undefined || basesZh.has(key);

    // (b) 结构对齐：每个 base key 必须 en/zh 都在。
    for (const b of basesEn) {
      if (!basesZh.has(b)) {
        fail(`规则8(b): key \`${b}\` 在 en.json 有、zh.json 缺 —— 切到中文会回退显示英文。`);
      }
    }
    for (const b of basesZh) {
      if (!basesEn.has(b)) {
        fail(`规则8(b): key \`${b}\` 在 zh.json 有、en.json 缺 —— 切到英文会回退显示中文。`);
      }
    }

    // (c) 疑似英文回退：zh 值与 en 逐字相同，且像「多词英文短语」（两个空格分隔
    // 的拉丁词）。单词技术名词/纯插值/无空格 URL 不触发，避免误报。
    const englishPhrase = /[A-Za-z]{2,}\s+[A-Za-z]{2,}/;
    for (const [k, v] of Object.entries(fen)) {
      if (typeof v !== "string") continue;
      if (FALLBACK_ALLOW.has(k)) continue;
      if (fzh[k] === v && englishPhrase.test(v)) {
        fail(
          `规则8(c): zh.json 的 \`${k}\` 与 en 逐字相同（"${v.slice(0, 60)}"）—— 疑似漏翻、中文用户看到英文。确属不译(品牌/URL)请加进 FALLBACK_ALLOW。`,
        );
      }
    }

    // (a) 代码里 t("literal") / i18n.t("literal") 用到的 key 必须 en+zh 都有。
    // 只看双引号字符串字面量；t(`a.${x}`) 动态 key 与以 "." 结尾的拼接前缀跳过。
    const usedFiles = [];
    async function walkSrc(dir) {
      let entries;
      try {
        entries = await readdir(dir, { withFileTypes: true });
      } catch {
        return;
      }
      for (const e of entries) {
        if (e.name === "node_modules" || e.name === "dist" || e.name === "src-tauri") continue;
        const full = path.join(dir, e.name);
        if (e.isDirectory()) await walkSrc(full);
        else if (/\.(tsx?|ts)$/.test(e.name)) usedFiles.push(await rf(full, "utf8"));
      }
    }
    await walkSrc(path.join(root, "web/src"));
    const used = new Set();
    for (const txt of usedFiles) {
      for (const m of txt.matchAll(/\bt\(\s*"([A-Za-z0-9_.]+)"/g)) {
        const key = m[1];
        if (key.endsWith(".")) continue; // 动态拼接前缀 t("a.b." + x)
        used.add(key);
      }
    }
    for (const key of [...used].sort()) {
      if (!hasEn(key)) {
        fail(`规则8(a): 代码 t("${key}") 在 en.json 找不到（含复数后缀也没有）—— 英文用户看到原始 key 串。`);
      }
      if (!hasZh(key)) {
        fail(`规则8(a): 代码 t("${key}") 在 zh.json 找不到（含复数后缀也没有）—— 中文用户看到原始 key 串。`);
      }
    }
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
