# 全库深审报告 — 2026-07-08

**基线提交:** `5f7d4e2`
**审查方式:** 按功能域/模块分片(15 片)并行审查 → 每条发现派独立 agent 对抗性验证(尝试证伪)→ 存活项落地修复。
**审查维度:** 正确性(correctness)、安全(security)、必要性(necessity)、冗余(redundancy)、性能(performance)、可维护性(maintainability)。

本轮不是纯罗列问题的审计,而是**审 → 验 → 修**一体:证伪掉的误报不进本报告;存活的发现全部当轮修完并跑绿测试。下面按「已修复」与「审查后判定无需改」两类给出结论,每条附**为什么这样改是对的**,以便同行复审。

---

## 一、已修复(11 处,均已过 `cargo test` / `harness-check` / 前端 `tsc`)

### 1. `swarm_send_message` 对「已 kill 的收件人」不再误判为可达 — correctness

- **文件:** `crates/swarmx-mcp/src/tools.rs:341`(`recipient_is_known`)
- **问题:** 黑洞警告(消息发给不存在的 agent 会静默无人读)此前判据是「agent_id 出现在 roster 里」。但被 kill 的 worker 仍以历史行(`killed_at != null`)留在 roster 中,它**永远读不到**这条消息。旧逻辑把它当「已知」,恰好在最常见的场景(发给一个已经退出的 worker)吞掉了警告——发信方会无限等一个不可能到来的回复。
- **修法:** 匹配条件加 `killed_at` 为 null,只把**存活** agent 视为有效目标。
- **为什么对:** 消息仍会持久化落库(不丢),只是额外补一句黑洞警告让 LLM 知道该重发给正确的活 agent。传输失败时 `recipient_is_known` 返回 `Err`,调用方默认「假设已知」不加警告——不因 roster 抖动阻断正常发送。测试从「killed 无警告」翻正为 `send_to_live_recipient_has_no_warning` + `send_to_killed_recipient_warns` 两条。

### 2. `handoff_done` / `error_present` 把「已删除的交付物」误判为仍在 — correctness(本轮最实质的 bug)

- **文件:** `crates/swarmx-storage/src/store.rs:947`(`list_workers` 的派生列)
- **问题:** `blackboard_ops` 是**追加写**账本,一个 key 被 put 再 delete,写入行会永远留着。旧的 `EXISTS (SELECT 1 … WHERE b.path = signal)` 只要历史上写过就为真,于是用户在黑板面板删掉一个交付物后,对应 worker 的 `handoff_done` 仍报 true、DAG 节点仍绿、`handoff_missing` 被抑制——状态与事实相反。
- **修法:** 取每个 path 的 `MAX(id)`(最新一条 op)并要求 `op != 'delete'`。
- **为什么对:** 这与 `blackboard_paths_present`(`store.rs:1991`)、`list_blackboard_paths` 及就绪门用的是**同一套**「这个 key 是否存在」定义——全库一个定义,不会各处漂移。已有复合索引 `idx_blackboard_path_id (path, id)` 支撑该分组查询,无额外扫描代价。`paths_present_excludes_deleted_tombstones` 测试覆盖此语义。

### 3. 黑板 key 含 `?`/`#`/`%` 时 URL 被 reqwest 误解析 — correctness

- **文件:** `crates/swarmx-mcp/src/tools.rs:694`(新增 `blackboard_url`,`read_blackboard`/`write_blackboard` 改用它)
- **问题:** 旧代码 `format!("{}/api/blackboard/{}", server_url, path)` 直接把 path 拼进 URL。若 key 含 `?` 或 `#`,reqwest 会把后半段当 query/fragment,发到服务端的 path 就被截断。
- **修法:** 用 `reqwest::Url` 的 `path_segments_mut()` 对每个 path 段做百分号编码后再拼。
- **为什么对:** 分段编码是 URL 拼接的地道做法(而非手工 `format!`),`?`/`#`/`%` 都会被正确转义,服务端拿到完整 key。

### 4. `is_sensitive` 在无 `$HOME` 时凭据目录判定失效 — security

- **文件:** `crates/swarmx-server/src/routes/files.rs:52`(`home`)、`:194`(`is_sensitive`)
- **问题:** `home()` 旧实现在 `HOME` 缺失时回落成 `/`,于是 `home.join(".ssh")` 变成 `/.ssh`——所有锚定 `$HOME` 的凭据目录/文件检查全部指向错误路径,形同虚设。这正命中项目头号事故源:**装包后 sidecar 的 CWD=/、可能无环境变量**(尤其 Windows,`HOME` 常未设)。
- **修法:** ① `home()` 改返回 `Option`,并把 `USERPROFILE`(Windows)纳入、过滤掉 `/` 和空值;② 锚定检查用 `if let Some(home)` 守卫;③ **新增 fail-closed 兜底**:即便 `$HOME` 解析不出,也按裸文件名在任意目录匹配 `.claude.json`/`.git-credentials` 等——这些名字在任何目录都不是合法的「浏览我的代码」目标。
- **为什么对:** 安全检查失败时应 fail-closed(拒读)而非 fail-open(放行)。测试加了 `is_sensitive(Path::new("/.claude.json"))` 与 `/tmp/whatever/.claude.json` 两条断言,证明缺/错 `HOME` 不再能把凭据变成可读预言机。`list_dir` 的默认目录回落也相应改为 `roots → $HOME → /` 的显式链。

### 5. goal 与 thread 跨 workspace 不一致 — correctness

- **文件:** `crates/swarmx-server/src/routes/goals.rs:173`(`create_goal`)
- **问题:** goal 的 `workspace_id` 与 `thread_id` 两个外键各自独立校验都能过,于是可以造出「属于 workspace A、却指向 B 里的 thread」的 goal,让按 thread 过滤的列表出现不一致。
- **修法:** 取到 thread 后校验 `t.workspace_id == workspace_id`,不一致返回 400。
- **为什么对:** 这是数据库外键无法表达的**跨表业务不变量**,必须在应用层守。返回 400 而非 500,因为这是调用方传参错误。

### 6. `augmented_path` 遇到含分隔符的目录会丢掉全部桌面 PATH — correctness

- **文件:** `crates/swarmx-server/src/runtime_path.rs:32`(`augmented_path`)
- **问题:** `join_paths` 只要任一目录含平台分隔符(unix 的 `:`)就整体失败,旧代码于是回落成裸父 PATH——把这个模块**存在的全部意义**(补上 Finder 启动时被剥掉的桌面 PATH 目录)全丢了,恰好在最需要它的场景(从 Finder 拉起 .app)。
- **修法:** join 失败时,过滤掉含分隔符的目录再 join 一次,让有用的目录存活;仍失败才回落裸 PATH。
- **为什么对:** 含分隔符的目录路径本就无法进 PATH,跳过它们是唯一合理选择;其余正常目录不该被一个坏目录连坐。

### 7. `split_front_matter` 在 spells / roles 两处重复 — redundancy + correctness

- **文件:** `crates/swarmx-server/src/spells.rs:434`(改 `pub(crate)`)、`crates/swarmx-server/src/roles.rs`(删本地副本,改调 spells 的)
- **问题:** roles.rs 自带一份 `split_front_matter` 副本。之前修过一个 F21 bug(diff 行 `+++ b/path` 出现在 TOML 值里会被误当作闭合 fence),但只修在 spells 那份;roles 的副本仍是旧逻辑,随时可能悄悄回归。
- **修法:** roles 复用 spells 的实现,删掉本地副本。
- **为什么对:** 两者用的是**同一套** `+++` front-matter 约定,一个实现一处修复,不给回归留副本。原注释担心「未来 roles 想支持 YAML 会耦合」——但那是假想需求,真到那天再拆不迟(YAGNI);当下的重复带来的是**已发生过的** bug 回归风险,权衡明确。

### 8. `wake.rs` 本地 `now_ms_local` 重复造轮子 — redundancy

- **文件:** `crates/swarmx-server/src/wake.rs:74`
- **问题:** wake.rs 自带一个 `now_ms_local`,注释说是为了「不跨 import」而镜像 `rest::now_ms`。
- **修法:** 删掉,直接用统一的 `now_ms()`。
- **为什么对:** 时间戳获取全库应一个来源。跨 import 不是回避复用的理由。

### 9. FK cascade 注释与事实不符 — maintainability(注释即文档,错误注释会误导后续改动)

- **文件:** `crates/swarmx-storage/src/connection.rs:23`
- **问题:** 旧注释断言「没有任何代码路径**物理**删除 FK 父行,所以 CASCADE 是休眠的」。但 `thought_traces`(迁移 0021)**确实**用了 `ON DELETE CASCADE`/`SET NULL`,且 `prune_expired` 会物理删除已投递已读的消息,级联删掉其 thought_traces——这是**活的**级联,不是休眠的。
- **修法:** 重写注释,如实列出所有物理删除路径(`prune_expired`、`delete_workspace_root`、`delete_blackboard_prefix`)与各自的级联行为。
- **为什么对:** 这是审查一条「凭旧注释以为没问题」的典型——注释是给后续改动者的契约,错误注释比没注释更危险。此处代码无 bug,但文档必须纠正。

### 10. `mcp_admin` API key 安全注释过度承诺 — maintainability(诚实的降级)

- **文件:** `crates/swarmx-server/src/routes/mcp_admin.rs:130`
- **问题:** 旧注释暗示用 `-e KEY=val` 传 key 就完全不进 argv、`ps` 读不到。实际上 `mcp add` 子进程存活的亚秒窗口内,`-e` 的值仍出现在其 argv。
- **修法:** 注释如实说明这个窗口暴露,但同时论证它**不额外提权**:能 `ps` 到该进程的同机同 UID 进程,同样能直接读已落盘的同一把 key(`~/.claude.json` 等),且本服务是 loopback 单用户桌面场景。
- **为什么对:** 安全注释宁可诚实降级也不虚假拔高。这里的结论是「窗口存在但无额外风险」,有据可查,而非「绝对安全」的空头承诺。代码本身(用 env 而非拼 argv 持久化)是对的,不需改。

### 11. 删除死代码 `search_blackboard` — necessity

- **文件:** `crates/swarmx-storage/src/store.rs`(删方法)、`crates/swarmx-storage/tests/store_test.rs`(删对应测试)
- **问题:** `Store::search_blackboard`(FTS5 全文搜黑板)全库无任何调用点——前端不调、路由不接、其它 crate 不用。
- **修法:** 删除方法及其单测。
- **为什么对:** 已多处求证(`git grep search_blackboard` 全库仅剩本报告与工作流脚本引用,生产代码零引用)确认是死代码。黑板的 FTS 表本身仍在(消息搜索 `message_search_fts5` 在用),删的只是这个无人调用的入口。**注意:** 删功能属于「断言没被用到」的高危结论,已按项目铁律走完多轮实证(grep 调用点 + 看前端 endpoints + 看测试)才下手。

---

## 二、审查后判定「无需改」的项(必要性 / 冗余维度的克制结论)

对抗性验证阶段证伪掉一批「浮于表面」的发现。以下几类是**看着可疑、实则合理**,明确记录以免下轮重复报同样的误报:

- **`spells/` 只 ship `init.md`、其余多 agent 机制(role_ref/allow_cycles/shared_workspace)保留但当前不走** — 这是 CLAUDE.md 明确记录的 Magentic-One 设计选择(拓扑由 orchestrator 运行时即兴派,不预声明),**不是** half-baked。保留 + 单测,留作未来。判定:保留。
- **四个 CLI 适配(claude/codex/opencode/reasonix)间的相似代码** — 各引擎差异实质(opencode 全屏 TUI + `/tui` HTTP、reasonix `serve` HTTP-SSE、claude/codex PTY),表面相似的样板抽 shared 收益有限、反而耦合各自演进。判定:不强抽。
- **`store.rs` 3600 行 / `rest.rs` 3937 行 / `workspaces.rs` 3783 行超大文件** — 可维护性确有压力,但拆分是**大动、需专门一轮**的重构,与本轮「审查+修 bug」不同频;且拆分本身有回归风险。判定:记录为技术债,不在本轮动。
- **`.app` 打包资源缺失** — 已由 `include_str!` 编译进二进制(spells/roles/cli-plugins builtin),CWD=/、无环境变量也能跑。判定:非问题(历史事故已修)。

---

## 三、验证结论

| 门禁 | 结果 |
|---|---|
| `cargo build --workspace` | ✅ 通过 |
| `cargo test -p swarmx-mcp` | ✅ 8 passed |
| `cargo test -p swarmx-storage` | ✅ 40 passed |
| `cargo test -p swarmx-server` | ✅ 46 + 3 + 290 passed |
| `node scripts/harness-check.mjs` | ✅ 通过 |
| `web` `npm run build`(tsc 门禁) | ✅ 通过 |

浏览器端到端回归见本报告落地后的实测记录(建空间 → 黑板增删 → 消息发送 → 竞赛/融合视图),按项目第二原则用 chrome-devtools 真实走一遍。

---

## 附:审查方式说明

本轮的可信度来自**对抗性验证**:每条初查发现都派一个独立 agent 专门尝试**证伪**它(默认是误报,读代码确证为真才判 CONFIRMED)。这压掉了「看着像 bug、实则有防护」的一大类假阳性。落地的 11 处都是过了这一关、且我(架构师视角)复核过修法地道性的存活项。证伪掉的、以及判定「无需改」的,均如实记录在第二节,不藏。
