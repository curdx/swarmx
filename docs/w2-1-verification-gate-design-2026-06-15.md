# W2-1 确定性验收门 — 落地设计(基于 2025–2026 最新权威实践)

> 日期:2026-06-15。来源:Workflow 5 代理调研(ctx7 + WebSearch)。本文档为有出处的设计,实现见同名提交。

## 核心范式(有出处)
> **「评判 agent 真正产出了什么,而不是它走了什么路。」** — Anthropic《Demystifying evals for AI agents》原话:*"it's often better to grade what the agent produced, not the path it took"*,并给同构反例:订票 agent 说"已订好"≠ 数据库里真有预订。

落到 flockmux:**worker 写黑板 handoff key ≠ done,只是"声明完成";服务端必须在 worker 的 cwd 亲自跑一遍它声称的验证命令,按真实退出码决定 done / .error**。对应 Claude Code 官方 **TaskCompleted hook**:*"Exit with code 2 to prevent completion and send feedback"*(code.claude.com/docs/en/hooks)。

**关键判断(四路调研一致):对代码任务,确定性 grader(真退出码)严格优于二次 LLM 判定。** Anthropic:*"software is generally straightforward to evaluate: does the code run and do the tests pass?"*;且官方 `/goal` evaluator *"does not call tools"*——只能信 worker 自报,正是 flockmux 要堵的。

**定位:业界无完全对位先例。** Codex/LangGraph 都是 *agent 内部循环*;W2-1 把验证权从 worker **上收到服务端**,更强但需自研装配(业界只提供组件:确定性执行 + evaluator-optimizer 打回 + guardrail 硬停)。

## 三态机(OpenAI cookbook / LangGraph grade 路由)
`done / needs_fix(喂回自修) / error(终态交人)`。worker 写 key → 服务端 `verifying` → 跑命令:
- exit 0 → **done**。
- exit≠0 且 `attempts < max_verify_retries(默认2)` → **needs_fix**:删掉刚写的 key(撤销"完成"声明)+ MCP `isError:true`+text(退出码+截断日志)喂回**还活着的** worker(经 `Swarm::send_message`+PTY kick)→ 自修重写 key → 再跑门。
- 超上限 → 写 `<key>.error`(现有 .error→blocked 机制)+ 摘要交队长。

死循环护栏(必抄):Claude Code 连续阻塞 8 次强制放行 + `stop_hook_active` 提前退出;Codex 修复循环 max=3;Auto Mode 连续3/累计20 交人。flockmux 取 **2**。

## 安全执行(《How we contain Claude》戒律:*"Tool output is an attack surface even when trusted"* + *"防御落在 environment 层"* + *"别造自己的沙箱"*)

**v1 最小安全集(零外部依赖,纯 Rust,契合"装机零命令"红线):**
| 维度 | 做法 |
|---|---|
| 不拼 shell | `Command::new(prog).arg(...)` 逐参 argv,**绝不 `sh -c`**(复用 `worktree.rs::git()` 范式) |
| 严格白名单 | 只允许 `cargo {test,build,clippy,check}` / `npm/pnpm/yarn {test,build,ci,run …}` / `python -m pytest` 等枚举;拒绝含 `&& \| ; $() ` 反引号 `> <` 的串;spawn 时白名单外 **直接 400 fail-loud** |
| 硬超时 | worker 线程 + recv_timeout;test/build 放宽到分钟级(默认 600s) |
| 进程组回收 | `Command::process_group(0)` 建新组 + 超时 `libc::killpg`(SIGTERM→等→SIGKILL,抄 `pty/lib.rs:204`)+ wait 收尸 |
| 并发上限 | `tokio::sync::Semaphore`(默认 2) |
| 重建 env | 不带 `AWS_*`/token 等凭证进子进程 |
| 输出截断 | 喂回日志只取末尾 K 行 |

**v2 理想态(OS 原生沙箱,非容器/microVM——本地优先 value 在真工作树就地跑):** macOS `sandbox-exec`(Seatbelt,Apple 标 deprecated 但仍用,记技术债)/ Linux `bubblewrap`+Landlock+seccomp;默认允许写工作目录、网络默认拒。参考 Anthropic `sandbox-runtime`(srt) profile,在 Rust 里自生成调用(避免 Node 包打包负担)。⚠️ 必须真机验证 `~/.cargo`/`~/.npm` 缓存在沙箱下可读写。

## 诚实性:验证未决窗口不撒谎
新增中间态 `verifying`,改 `routes/tasks.rs::effective_status`:`handoff_done && verify_pass → done`;`handoff_done && verifying/fail-with-retries → verifying`(不点亮 done);`.error → blocked`。状态落库(workers 加 `verify_status` 列),不依赖再读黑板。先例:Claude Code 验证未决期 task 仍 `in_progress`。

## 实现步骤
1. **schema**:`swarm_spawn_worker`(tools.rs)+ `SpawnWorkerRequest`(protocol)加 `handoff_verify: [cmd]`;migration 给 workers 加 `verify_cmds_json/verify_status/verify_attempts`;spawn 时白名单校验+持久化(白名单外 400)。
2. **verify.rs**(新):白名单解析(纯函数+单测)+ 安全 timeboxed exec(argv+process_group+killpg+semaphore+env重建+截断)。
3. **拦截**:worker 写 primary handoff key 时,若该 worker 有 verify_cmds → 置 verifying + 异步起 verify。幂等靠 verify_attempts + 提前退出。
4. **裁决+喂回**:exit0→pass(done);fail+retries→删key+attempts++ +send_message喂回+wake;超限→写.error+交队长。
5. **状态机**:tasks.rs effective_status 加 verifying;list_tasks SQL 取 verify_status;VALID_STATUSES 加 verifying;单测。
6. **真机验证**:隔离后端 spawn 真 claude worker,故意编译失败+声明 `cargo build` verify → 门判 fail、喂回日志、自修二次通过转 done;超2次转 .error。验证装机版资源齐全、全程零命令。

## 事实/推断分界
事实(出处/已读代码):TaskCompleted exit2、"grade what it produced"、`/goal` evaluator 不能跑命令、确定性 grader 优先、How we contain Claude 戒律、flockmux 现状(key→done 撒谎、cwd 已知、killpg/git 现成、MCP 2024-11-05 走 isError+text)。
推断(标注):`max_verify_retries=2` 默认值;`~/.cargo`/`~/.npm` 沙箱可读写(必须真机验证);三态映射为设计建议非原话。
