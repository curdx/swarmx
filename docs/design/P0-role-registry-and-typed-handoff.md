# P0 设计：角色注册表 + typed handoff key

> 落 F1「角色/任务感知配给」的 P0。依据：`docs/research/role-task-aware-allocation-2026-06.md`（29 仓库调研）。
> 设计日期：2026-06-02。状态：**草案，待拍板 §6 的 3 个决策点**。

P0 两件事，互为底座：
- **P0-A**：干掉 LLM 自选的 `depends_on`/`handoff_signal` 字符串 → 服务端从结构化字段**铸造** canonical 黑板 key + spawn 时校验整张依赖图 + WakeCoordinator **fail-CLOSED**。
- **P0-B**：自由文本 `role_label` → **可校验、可扩展的角色注册表**（扩展现有 `roles.rs`），角色携带 `produces/consumes`、`default_cli`、`default_model_tier`、工具 allowlist。

---

## 0. 现状锚点（代码实证，不是猜）

| 事实 | 出处 |
|---|---|
| `swarm_spawn_worker` 入参：`cli`(enum claude/codex)、`role_label`(自由串)、`system_prompt`、`handoff_signal`(自由串)、`depends_on`(自由串数组)、`model`(自由串) | `swarmx-mcp/src/tools.rs:140-176,569-630` |
| worker 存储 `NewWorker`：`agent_id/parent_agent_id/role_label/system_prompt/handoff_signal/depends_on_json/spawned_at` | `swarmx-storage/src/models.rs:152-166` |
| **BlackboardChanged 的 `path` = 生产者原样传入的 `rel_path`，服务端不重写** | `swarmx-swarm/src/swarm.rs:219-275` |
| wake 匹配 = `keys.iter().any(\|k\| k == key)` **精确字符串相等**，writer 排除自身 | `swarmx-server/src/wake.rs:192-208` |
| 磁盘命名空间 `<workspace_id>/<thread.slug>/` 写在 rel_path 里（删方向按此前缀） | `routes/rest.rs:2104,2112` |
| `.error`/`.failed` 后缀 fan-out 到 base key 的订阅者（已有） | `wake.rs:105-119` |
| 孤儿 handoff 诊断（写了某 handoff 但 0 个 depends_on 命中）**目前只 `warn!`，不阻断** | `wake.rs:231-260,518-527` |
| **已有** `RoleManifest`/`RoleRegistry`：`id/name/description/default_cli/artifact_paths/handoff_signal/depends_on/system_prompt_template`，`roles/*.md` 加载，`role_ref` 合并已实现+测试**但休眠**（只 `orchestrator` 在用） | `swarmx-server/src/roles.rs:45-152` |
| cycle 检测已存在（acyclic 校验） | `wake.rs:~888`；spells 也有 |

**核心病灶**：生产者写的 key 与消费者 `depends_on` 列的 key，是两处独立的 LLM 自选字符串，靠**字节相等**对齐，中间还要 LLM 手搓 `<workspace_id>/<thread.slug>/` 前缀。少前缀 / 尾斜杠 / typo / 大小写 → 永远不唤醒，且生产者已成功退出（`.error` fallback 也不触发）。这是 docs F3 已记录的 bug 类。

---

## 1. P0-B：角色注册表（扩展 `roles.rs`）

### 1.1 设计原则
- **扩展不新建**：复用现有 `RoleManifest`/`RoleRegistry`/`load_dir`/`role_ref` 合并，加字段、加内建角色、加项目级 override、接到 `swarm_spawn_worker`。
- **角色作者声明，绝不让 LLM 运行时发明**（调研模式 #1）。
- **`when_to_use`(面向 router) 与 `system_prompt`(面向 worker) 分开**——匹配只读薄描述符。
- **可扩展但闭集**：内建默认 + 项目 `.swarmx/roles/*.toml` 按 slug 覆盖；坏条目 soft-fail（log + skip），未知 slug 在 spawn 时**硬报错**。

### 1.2 扩展后的 `RoleManifest`（TOML front-matter）

```toml
# roles/frontend.toml  （或保留 .md + +++ front-matter，见 §6 决策3）
id            = "frontend"              # ← 现有：slug，spawn 时校验
name          = "Frontend Engineer"     # ← 现有：UI 显示
description   = "..."                    # ← 现有：人读文档
default_cli   = "claude"                 # ← 现有：默认 CLI
system_prompt_template = """..."""       # ← 现有：worker prompt 模板

# ---- P0 新增 ----
when_to_use   = "UI 组件、样式、前端交互、可视化；不碰后端/shell"  # 薄路由提示(面向 orchestrator 选角色)
default_model_tier = "sonnet"            # 默认模型层(opus|sonnet|haiku)，可被 spawn 覆盖；P1 能力卡接管前的占位
produces      = ["done", "spec"]         # 本角色产出的 typed 输出种类(output-kind)。默认 ["done"]
consumes      = [                        # 本角色依赖的上游(按 角色+种类，非裸串)
  { from_role = "designer", kind = "spec" },
]
artifact_paths = ["apps/frontend/**"]    # ← 现有：软路径所有权(P1-B 升级为硬门控)

# ---- P1 前向保留(P0 不读，先占位防 schema 漂移) ----
tool_allowlist = []                      # 工具/MCP allowlist(P1-B 硬门控)
modality       = "ui"                    # ui|backend|docs|shell(P1-A 规则信号；声明非推断)
risk           = "normal"                # normal|high(P1-D 强制加 verifier)
```

新增字段全部 `#[serde(default)]`，对现有 `orchestrator` 角色与 `role_ref` 合并**向后兼容**（缺字段走默认）。`produces`/`consumes` 是 P0-A 的依赖图数据源。

### 1.3 内建角色 + 项目 override
- **内建默认**：编译进二进制（`include_str!` 或 `RoleRegistry::builtin()`），保证 fresh checkout 可用。先发一小撮 vetted 角色（调研模式 #1/反模式 #6：小而精，防 ruflo 式 98 角色漂移）：`orchestrator`(已有)、`frontend`、`backend`、`reviewer`、`test-runner`、`docs-writer`、`researcher`、`fixer`。
- **项目 override**：`<workspace_cwd>/.swarmx/roles/*.toml`，按 `id` 覆盖内建（同 RooCode `.roomodes` / claude-code-router 3 层 override）。`load_dir` 已 soft-fail（`roles.rs:98-128`），扩成「内建 → 项目」两层 merge。
- **drift 测试**：生成 JSON-schema + 单测校验内建角色与 schema 一致（仿 RooCode）。

### 1.4 `swarm_spawn_worker` 工具变更

`role_label`(自由串) → `role`(slug，校验)：

```jsonc
{
  "cli":  { ... },                          // 保留：可选 override（缺省取 role.default_cli）
  "role": {                                  // ★ 改：自由串 → 注册表 slug
    "type": "string",
    "description": "Role slug from the registry (frontend, backend, reviewer, ...). Carries default_cli/model + produces/consumes wiring. Unknown slug errors with valid options. Use swarm_list_roles to see the catalog."
  },
  "system_prompt": { ... },                  // 保留：覆盖/补充 role.system_prompt_template
  "model": { ... },                          // 保留：可选 tier override（缺省取 role.default_model_tier）
  "produces": {                              // ★ P0-A：见 §2（可选；缺省取 role.produces）
    "type": "array", "items": {"type":"string"}
  },
  "consumes": {                              // ★ P0-A：替代裸 depends_on，见 §2
    "type": "array",
    "items": { "type": "object", "properties": {
      "from_role": {"type":"string"}, "kind": {"type":"string"}
    }, "required": ["from_role","kind"] }
  }
  // ✗ 删 role_label / handoff_signal / depends_on（裸串）— 见 §6 决策2 的迁移姿态
}
```

- `required: ["role", "system_prompt"]`（`cli` 变可选，缺省取角色默认——这就是「配给」的第一步：选了角色即带 cli/model 默认）。
- 未知 `role` → `Err("unknown role 'fronend' — did you mean 'frontend'? valid: [backend, docs-writer, ...]")`（Levenshtein「did you mean」）。
- 新增只读工具 `swarm_list_roles` → 返回 `[{id, when_to_use, default_cli, default_model_tier, produces}]`，让 orchestrator 选角色有依据（面向 router 的薄描述符）。
- `role_label`(DB 列) 保留但由 `role` 派生（`role_label = role` 或 `role.name`），UI 不变。

---

## 2. P0-A：typed handoff key + 图校验 + fail-CLOSED wake

### 2.1 canonical key 铸造（单一真相源）

服务端在 spawn 时**铸造** key，LLM 永不手搓前缀。铸造函数：

```
fn mint_handoff_key(thread_scope: &str, role_slug: &str, kind: &str) -> String
// = format!("{thread_scope}/{role_slug}.{kind}")
// thread_scope = "<workspace_id>/<thread_slug>"（与现有磁盘命名空间一致，见 §6 决策1）
// 例: "ws_ab12/dark-mode/frontend.done"
```

- `role_slug` 来自注册表（已校验）；`kind` 来自 `produces`（默认 `done`）。两端都从这**同一个铸造值**派生 → 结构上不可能漂移。
- 与现有约定兼容：`<role>.done` / `<role>.error`，`base_key_aliases` 的 `.error`/`.failed` fan-out 不变（`wake.rs:110`）。`.error` 仍是 `<thread_scope>/<role>.error`。

### 2.2 producer 侧：把 key 注入 prompt（P0），后续上工具（P1）

- **P0（低风险）**：spawn 时服务端把铸造的 key 注入 worker 的 system_prompt 尾部：
  > `完成后，向黑板写入这个 key（原样复制，勿改）：<minted_key>。失败时写 <minted_key 去 .done 加 .error>。`
  LLM 只是复制服务端给的串，不再自创。
- **P1（更稳，去字符串）**：加 MCP 工具 `swarm_signal(kind, status)`，服务端按调用者 agent_id → role → 铸造 key 映射，LLM 连复制都不用。P0 先不做，预留。

### 2.3 consumer 侧：`consumes` 解析成铸造 key

- `consumes: [{from_role, kind}]` → 服务端解析：在**同 thread** 内找声明/在线的 `from_role` 生产者，取其铸造 key，写进该 worker 的 `depends_on`（DB `depends_on_json` + `register_wake_subs`）。
- orchestrator 引用的是「角色+种类」（人读、稳定），机器侧落到铸造 key（typed）。等价调研模式 #12（human-readable ref → canonical id at plan time）。

### 2.4 spawn 时图校验（fail-LOUD，新增）

`POST /api/worker` 接到 `consumes` 后，**spawn 前**校验：

1. **未知依赖**：每个 `{from_role, kind}` 必须能解析到「某个已声明角色的 `produces` 含该 kind」。否则 `Err("worker 'x' depends on designer.spec, but no role 'designer' is declared / no live producer will write '.spec' — valid producers: [...]")`。
2. **无生产者**：该 thread 内当前无、且本次 spawn 批次也不会拉起对应生产者 → 拒绝（不是静默永等）。
3. **环检测**：复用现有 acyclic 校验（`wake.rs:~888`），成环 → 列出环路径报错。
4. **near-miss 建议**：from_role 拼错 → Levenshtein「did you mean」。

> 注：orchestrator 常一次规划多个 worker（DAG）。校验需支持「本批次声明但尚未 spawn 完」的前向引用——按「本次提交的角色集 ∪ 该 thread 在线 producer」做解析域（仿 langgraph-supervisor build 时 `agent_names - handoff_destinations == {}` 集合差断言）。

### 2.5 WakeCoordinator fail-CLOSED（升级现有诊断）

现状 `orphaned_handoff_diagnosis` 只 `warn!`（`wake.rs:231-260`）。升级：

- worker 落地后，若其某个 `depends_on` key **没有任何在线/声明的生产者**（spawn 校验已挡掉大部分，但生产者中途死亡会再现）→ 不再让它无限挂在永不出现的 key 上：
  - 标记该 worker 为新状态 **`blocked`（surfaced）**，写 `<thread_scope>/<role>.blocked` 黑板事件，**唤醒 orchestrator** 让它决策（重拉生产者 / 改图 / 放弃）。
- 生产者死亡的**传递级联**（P1-D 完整版的前置）：复用 M6c 的 `<role>.error` fallback（`wake.rs:53-96`），P0 先保证「直接依赖方收到 `.error` 而非静默等待」（已部分有）；图级传递留 P1-D。

### 2.6 存储变更

`NewWorker`（`models.rs:152`）：
- 加 `role_slug: String`（替代/伴随 `role_label`）。
- 加 `produces_json: String`、`consumes_json: String`（typed 引用，留档/回放/DAG）。
- `handoff_signal` → 由铸造派生填入（保留列，值变成 minted key，不再是 LLM 串）。
- `depends_on_json` → 由 `consumes` 解析后的 minted keys 填入（保留列）。
- DB migration：`ALTER TABLE workers ADD COLUMN role_slug / produces_json / consumes_json`（SQLite ADD COLUMN，nullable + default，兼容老行）。

---

## 3. 改动面清单（按 crate）

| crate / 文件 | 改动 |
|---|---|
| `swarmx-server/src/roles.rs` | `RoleManifest` 加 `when_to_use/default_model_tier/produces/consumes/tool_allowlist/modality/risk`(均 default)；`RoleRegistry` 加 `builtin()` + 内建→项目两层 merge；加 `mint_handoff_key()` |
| `roles/*.toml`（新内建集） | frontend/backend/reviewer/test-runner/docs-writer/researcher/fixer + 现有 orchestrator |
| `swarmx-mcp/src/tools.rs` | `swarm_spawn_worker` schema：`role_label→role`(slug)、删裸 `handoff_signal/depends_on`、加 `produces/consumes`；新增 `swarm_list_roles` |
| `swarmx-server/src/routes/rest.rs`（`/api/worker`） | 解析 `role`→注册表校验；铸造 key；解析 `consumes`→depends_on；**spawn 前图校验**(未知/无生产者/环/did-you-mean)；prompt 注入铸造 key |
| `swarmx-server/src/wake.rs` | `orphaned_*` 从 warn → fail-CLOSED(`blocked` 状态 + 唤醒 orchestrator)；铸造 key 与 `register_exit_key`/`register_wake_subs` 对齐 |
| `swarmx-storage/src/models.rs` + migration | `NewWorker` 加 `role_slug/produces_json/consumes_json`；新 migration ADD COLUMN |
| 前端 DAG | 边由 `consumes`(typed) 画，不再解析裸串；`blocked` 状态新样式 |

---

## 4. 验收（回归网，依 E2E 策略 memory）

- **store 层确定性单测**：`mint_handoff_key` 幂等；图校验对「未知依赖/无生产者/成环/did-you-mean」各一桩；内建角色 schema drift 测试。
- **wake 单测**：扩 `select_targets`/`orphaned_handoff_diagnosis` — 铸造 key 命中、缺前缀的旧式裸串**拒绝**而非静默、生产者死亡 → 依赖方 `blocked` 而非挂起。
- **WS-broadcast smoke(.mjs)**：spawn 一条 `designer→frontend` 链，校验 `consumes` 自动解析 + 唤醒；故意拼错 `from_role` → spawn 被拒（不再静默永等）。
- **浏览器走查**：DAG 边正确、`blocked` 节点高亮、未知角色报错带 valid options。

---

## 5. 为什么这是 P0（调研依据）

- typed handoff key 是**全仓库最强共识**（调研模式 #3）：openai-agents-python `handoff()` 返对象+`input_type` 校验、langgraph build 时集合差断言、conductor 注册即失败、MetaGPT `cause_by` 从类算出、agno 服务端铸造 ID。swarmx 当前的裸串正是反模式 #1，且已中招(F3)。
- 角色注册表是解决 #1/#3 的**底座**，且 swarmx **已有 80% 实现**（休眠的 `roles.rs`），P0 主要是激活+接线，effort 真实为 M。
- 二者互锁：注册表的 `produces/consumes` 喂给 key 铸造与图校验；没有注册表，typed key 没有「角色」可锚。

---

## 6. 待拍板决策（见对话内提问）

1. **铸造 key 的 scope 前缀**：`<workspace_id>/<thread_slug>/<role>.<kind>`(全，配现有磁盘命名空间) vs `<thread_slug>/...` vs 短 `<role>.<kind>`(靠 per-direction 隔离)。
2. **迁移姿态**：additive + deprecate 裸串(向后兼容，旧 spawn 仍能跑) vs 硬切(删裸 `depends_on`/`handoff_signal`，更干净，内部工具无外部调用方)。
3. **角色文件格式**：沿用现有 `.md` + `+++` front-matter(与 spells 一致) vs 改纯 `.toml`(更轻，roles 注释里提过可能想要)。

---

## 7. 实现状态（2026-06-02 落地）

决策落定：**全前缀 key / 硬切删旧 / 沿用 .md**。后端全链路实现完毕，`cargo test --workspace` 全绿（server 144 / mcp 42 / storage 25 / swarm 9，零失败）。

| Task | 状态 | 说明 |
|---|---|---|
| 角色注册表 schema + 两层加载 + `mint_handoff_key` | ✅ | `roles.rs`：`RoleManifest` 扩 when_to_use/default_model_tier/produces/consumes(+P1 占位)；`RoleConsume`；`builtin()`(include_str! 8 角色) + `overlay()`；`main.rs` 改 builtin→repo dir 两层。14 单测绿。 |
| 内建角色文件 | ✅ | `roles/` 新增 frontend/backend/reviewer/test-runner/docs-writer/researcher/fixer(+现有 orchestrator)。 |
| 存储层 + migration 0011 | ✅ | `NewWorker` 加 role_slug/produces_json/consumes_json；migration 0011 三列 ADD COLUMN；record_worker/list_workers_by_ids 同步;2 往返单测绿。 |
| MCP 工具 | ✅ | `swarm_spawn_worker`：role(slug)+produces+consumes，删裸 handoff_signal/depends_on，cli/model 转可选；新增 `swarm_list_roles`；工具计数测试 9→10。 |
| `/api/worker` 校验+铸造 | ✅ | 角色注册表校验(未知→400+valid options+Levenshtein did-you-mean) + 项目 `.swarmx/roles/` overlay；cli/model 从 role 默认解析；produces→minted keys；consumes 图校验(未知/不产该 kind/自依赖→400)→minted depends_on；铸造 key 注入 worker prompt；新增 GET /api/roles。 |
| WakeCoordinator fail-CLOSED | ✅(部分) | 死亡 fallback 的 `.error` key 对齐为 `<minted_signal>.error`(与 worker 自愿失败一致；base_key_aliases 精确 fan 到 `.done` 消费者)；orchestrator+依赖方在生产者死亡时经 `select_targets(signal)` 已被唤醒。2 wake 单测绿。 |
| 前端 DAG | ✅(无需改) | `web/src/lib/dagEdgeDerivation.ts` 按 handoff_signal↔depends_on 匹配画边;二者现皆 server-minted 且一致 → **零改动照常工作**,minted key 可读显示。 |
| 回归测试 | ✅(自动化部分) | roles/wake/storage 确定性单测已落并绿(契合 E2E 策略 memory)。 |

**显式延到 P1-D（不在 P0）**：
- **`blocked` 状态 + 图级传递级联**：生产者「从未被 spawn」(非死亡)这一运行时缺口——orchestrator 引用了合法角色但始终不拉起对应 producer 时，依赖方仍会等。P0 的 spawn 时校验已消灭 typo/未知角色/不产该 kind 三类(F3 主因)；「从未实例化」需超时/启发式,属 P1-D cascade。
- **多 kind 的 `.error` 分别 fan-out**：P0 的 error_key = 主 handoff key + `.error`,单 kind 场景完备;多 kind 失败 fan-out 是 P1 细化。

**剩余人工验证（需活的 orchestrator LLM 会话）**：真实浏览器走查一条 `designer→frontend` consumes 链 + 故意拼错 role 看 400——需要 spawn 真 CLI(无假 seam,见 memory),作为后续在活 workspace 里做。
