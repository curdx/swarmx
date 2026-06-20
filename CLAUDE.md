# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# flockmux — 项目工作约定

flockmux 是一个浏览器仪表盘:在 PTY 下拉起真实的 `claude` / `codex` / `opencode` / `reasonix` CLI,
把它们组成一个 swarm,让它们通过共享收件箱 + 黑板互相消息、协作完成一个任务。
后端是 Rust(axum,loopback `127.0.0.1:7777`),前端是 Vite + React + xterm.js,
桌面端用 Tauri 把后端作为 sidecar 打包。

## 头号原则:永远站在「装完包的真实用户」角度验证,而不是开发机

这个项目最容易犯、也最伤用户的一类错误是:**在仓库根目录跑得好好的,打成安装包发出去就是坏的。**
原因是开发时服务从仓库根目录启动,`spells/`、`roles/`、`cli-plugins/` 这些运行时资源就在手边;
而真实用户装的是 Tauri 打的包,服务作为 sidecar 运行时:

- 当前工作目录(CWD)是 `/`,不是仓库根目录;
- 没有任何 `FLOCKMUX_*` 环境变量;
- `env!("CARGO_MANIFEST_DIR")` 指向的是**构建机器**上的路径,在用户电脑上根本不存在。

所以任何「用相对路径 / `CARGO_MANIFEST_DIR` 去找的文件或目录」在用户机上都找不到。
**历史事故(现已修复,保留作教训):** 安装版点「新建空间」报 “后端未加载 `init` spell” —— 因为
`spells/`/`roles/`/`cli-plugins/` 根本没被打进 `.app`。**当前修法不是「拷目录进包」,而是用 `include_str!`
把 `init.md` / 8 个 role / 4 个 `*.toml` 全部编译进二进制**(见下方「运行时资源」),CWD=`/`、无环境变量也能跑。
教训不变:运行时要读的东西,必须先确认它在装包后真的存在。

### 因此,任何改动落地前必须自问:

1. **这段逻辑依赖的文件/目录,在用户安装版里真的存在吗?** 不确定就去翻打包配置,别假设。
2. **新用户的完整路径是:下载 → 安装 → 打开 → 立刻能用,全程零命令。** 任何要求用户手动跑命令的方案都是 bug,不是 feature。
3. 运行时需要读的资源,要么编译期 `include_*!`/`sqlx::migrate!` 嵌进二进制,要么作为 Tauri `bundle.resources` 打进包并在启动 sidecar 时用环境变量把绝对路径传进去。两者必居其一。

## 常用命令

后端 / workspace(仓库根目录):

```bash
cargo build --workspace                 # 全量构建(缺 shim 二进制服务会启动失败,先跑这个)
cargo test  --workspace                 # 跑全部 Rust 测试(CI 硬门禁)
cargo test  -p flockmux-swarm           # 单个 crate
cargo test  -p flockmux-mcp tools_list  # 单个测试(按名字过滤)
cargo clippy --workspace --all-targets  # lint(CI 里是 informational)
cargo fmt --all                         # 格式化
cargo run -p flockmux-server            # 起后端(必须从仓库根目录,才能就近找到 spells/roles/cli-plugins)
```

前端(`web/` 目录):

```bash
npm_config_cache=/tmp/.npm-flockmux npm install   # 装依赖(/tmp 缓存绕沙箱 EACCES)
npm run dev        # Vite 开发服务器(:5173)
npm run build      # tsc -b && vite build —— tsc 是 CI 的类型检查硬门禁
npm test           # vitest run(单测)
npm run test:e2e   # playwright
npm run lint       # eslint
```

桌面端 / 打包(`web/` 目录):

```bash
npm run sidecar:release   # 编译 release 后端二进制 + 拷成 Tauri sidecar
npm run tauri:build       # 出真实安装包(.app/.dmg/...)
npm run tauri:dev         # Tauri 开发(注意:debug 下 Tauri 不自动拉后端,见下方「开发环境」)
```

CI(`.github/workflows/ci.yml`)的硬门禁是:`node scripts/harness-check.mjs`(跨文件不变量静态检查)、
`cargo build/test --workspace --locked`、`web` 的 `npm run build`、以及隔离后端的 `directions-smoke.mjs`。
需要真实 CLI(claude/codex 登录在 PATH)的 swarm/resume/stress 烟测是手动的,见 `scripts/golden-cli-test.sh`。

跑一个隔离的全栈用于真 UI 验证(非默认端口 7788/5188、数据目录在 `/tmp`,不碰你长期开着的 dev 会话):

```bash
bash scripts/test-stack.sh        # build + 起,留着跑
bash scripts/test-stack.sh stop   # 拆掉
```

## 架构总览(big picture)

**分层(自底向上):**

1. **PTY 在最底层。** 每个 agent 是未经修改的真实 `claude`/`codex`/`opencode`/`reasonix` 二进制,
   跑在 `portable-pty` 下。OAuth、限流、套餐限制行为和你在终端里敲 `claude` 完全一致 —— flockmux **从不**
   读取或持久化 OAuth token,只把 `HOME` 透传给子进程,复用 `~/.claude/`、`~/.codex/` 里已有的凭证。
2. **shim 在中间。** `flockmux-shim`(`execvp` 真 CLI 的极薄包装)发 OSC ready/exit 序列,让服务端知道何时就绪/退出。
3. **MCP 在最上层。** swarm 消息以原生 MCP 工具(`swarm_send_message`、`swarm_write_blackboard`…)暴露给 LLM。
   `flockmux-mcp` 是个 stdio JSON-RPC server,同时托管 `wake-check` 子命令(被 CLI 的 Stop hook 调用)。

**唤醒机制(关键概念):** agent 在每个回合结束时由 Stop hook 触发 `wake-check` → 查未读消息 →
有则 `{decision:block,reason:...}` 让 CLI 续跑一个回合去读并回复。M6b 的 push-style wakeup 进一步在黑板
某个 key 被写时,既投递 mailbox note 又往订阅者 PTY 注入 `\x15…\r`,把已停下的 agent 当场唤醒,无轮询、无死锁。
注意各引擎差异:opencode 当队长走全屏 TUI + 官方 `/tui` HTTP 控制接口(见 `opencode_tui.rs`),
reasonix 走 `reasonix serve` 的 HTTP/SSE(非 PTY,见 `reasonix_serve.rs`)。

**Workspace crate 布局:**

| Crate | 职责 |
|---|---|
| `flockmux-protocol` | WebSocket 帧 schema、REST DTO,server 与各 client 共享 |
| `flockmux-shim` | `execvp` 真 CLI 的 OSC 包装(每 agent 一个);CI harness-check 会校验它进了 Tauri externalBin |
| `flockmux-pty` | `portable-pty` 包装 + 双线程桥接 + 单调 seq 环形缓冲 |
| `flockmux-server` | axum HTTP/WS 网关。路由(`src/routes/`)、生命周期、pre-spawn 补丁、spell 执行器、role 注册表、`WakeCoordinator`、reaper、billing、engine-probe。各引擎适配在 `src/cli/{claude,codex,opencode,reasonix}.rs` |
| `flockmux-swarm` | 每 agent 收件箱、黑板 CRUD、notify-debouncer 文件监听 |
| `flockmux-mcp` | stdio MCP server + `wake-check` 子命令 |
| `flockmux-storage` | SQLite + FTS5,迁移、agents/messages/recordings/blackboard 表 |
| `flockmux-recorder` | asciicast v2 写入器,EOF 时 finalize |
| `flockmux-cli` | 极薄入口(`flockmux up` 起 server + 开仪表盘) |

**磁盘上的运行时资源(非 crate,随包发):**

- `spells/` —— spell 注册表。**当前刻意极简:只 ship `init.md`**(建空间时拉一个 orchestrator),
  下游全由 orchestrator 用 `swarm_spawn_worker` 按任务即兴派(Magentic-One 模型,不预声明拓扑)。
  多 agent 机制(`role_ref`/`allow_cycles`/shared_workspace)仍完整保留 + 单测,留作未来。
- `roles/` —— 每角色一个 `.md` SOP 模板(含 orchestrator)。
- `cli-plugins/` —— 每 CLI 一个 `.toml`(含 `claude`)。

**典型数据流:** 浏览器 `POST /api/agent {cli}` → `PluginRegistry.get(cli)` → `spawn::spawn_agent()`
fork shim 并 exec 真 CLI、写 per-workspace 的 MCP/hook 配置 → PTY pump 扫 OSC_READY 广播 ShimReady、
recorder 落 `.cast` → 浏览器开 `/ws/pty/<agent_id>` 双向流 + `/ws/swarm` 收 agent_state/message 事件。

## 运行时资源 / 打包清单(发版前逐项核对)

这份清单按「实测打包配置」整理(权威来源 = `web/src-tauri/tauri.conf.json` 的 `externalBin`/`bundle.resources`
+ `web/src-tauri/scripts/build-sidecar.sh`;`tauri build` 就是执行它们)。

**A. 已 `include_str!` 编译进二进制 —— 不需要打包、改了也无需动打包配置:**

- `spells/init.md` —— `spells.rs` 的 `SpellRegistry::builtin()`
- `roles/*.md`(8 个角色,含 orchestrator)—— `roles.rs` 的 `RoleRegistry::builtin()`
- `cli-plugins/*.toml`(claude/codex/opencode/reasonix)—— `plugins.rs` 的 `PluginRegistry::builtin()`

磁盘上这三个目录现在只是**可选 overlay**(`default_*_dir()` 解析:`FLOCKMUX_*_DIR` > `CARGO_MANIFEST_DIR` 相对 >
裸相对),**当前 overlay 为空**——即编译进去的 builtin 才是生效内容。所以「目录没打进 .app」不再致命。

**B. 必须随包发(不能 embed)—— 增删这类东西时务必同步改打包配置:**

- 3 个 sidecar 二进制 `flockmux-server` / `flockmux-shim` / `flockmux-mcp` —— `externalBin`,由 `build-sidecar.sh` 拷
- `cli-plugins/opencode/flockmux-wake.js` —— **当前唯一**的 `bundle.resources` 项。它是 opencode 的 wake hook,
  是 JS、要交给 opencode/node 执行,**没法 embed**。运行时定位见 `cli/opencode.rs` 的 `opencode_wake_plugin_path()`:
  `FLOCKMUX_OPENCODE_PLUGIN`(Tauri sidecar 注入打包后的绝对路径)> `CARGO_MANIFEST_DIR` 相对(仅 dev)。
  找不到则 opencode 丢 auto-wake、降级但不崩。

**新增运行时资源时的判定:能 `include_str!` 进二进制就 embed(走 A);若是脚本/必须落盘的文件(走 B),
就加进 `bundle.resources` 并在启动 sidecar 时用环境变量把绝对路径注进去 —— 两者必居其一。**

## 发版 = 必须验证安装版本身

不要只验证 `cargo run` / `tauri dev`。每次发版前,至少在本机:

1. `tauri build` 出真实安装包;
2. 确认 `.app/Contents/Resources/`(及 Windows/Linux 对应位置)里这些资源目录都在;
3. 启动安装版,确认「新建空间」能跑通(或至少 `/api/spells` 能列出 `init`),全程不碰命令行。

发版脚本与流程见 `scripts/bump-version.mjs`、`.github/workflows/release.yml`、`web/src-tauri/scripts/build-sidecar.sh`。
版本号是 workspace 级(`Cargo.toml` 的 `[workspace.package].version`);bump 用 `node scripts/bump-version.mjs <x.y.z>` → push → 打 `v*` tag。

## 开发环境

- 先 `cargo build --workspace`(缺 shim 二进制服务会启动失败)。
- 前端依赖:`web/` 下 `npm_config_cache=/tmp/.npm-flockmux npm install`(绕沙箱 EACCES)。
- 端口:后端 7777 / 前端 5173。
- debug 构建下 Tauri **不**自动拉起服务,需自己 `cargo run -p flockmux-server`(从仓库根目录,才能就近找到资源目录)。
