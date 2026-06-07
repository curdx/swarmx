# hermes-agent 深度借鉴报告（2026-06）

> NousResearch / hermes-agent — 多模型 Agent 引擎 + CLI + 多平台网关 (Python)
>
> 由 10 个专读 agent 穷举产出 · 函数级条目 **611** · 框架思想 **126** · 页面元素 **17** · 借鉴点 **104**
> 
> 本文为机器穷举 + 结构化整理，未做删减；引用格式 `文件:符号`。

**目录**：0 定位与架构哲学 · 1 框架思想/设计模式 · 2 函数级地图（穷举）· 3 页面元素穷举 · 4 flockmux 借鉴小结

## 0. 定位与架构哲学

NousResearch 的多模型 Agent 引擎 + CLI + 多平台网关（纯 Python，v0.16）。一句话：把一个**能调用工具、能自动压缩上下文、能跨 20+ 模型供应商、能被 20+ 聊天平台触达、能以 ACP/MCP 双向集成的长程 agent**做成可独立部署的 runtime。与 flockmux 最大的同构点——都要在「不假设原生 SDK、要管多供应商/多通道、要长程跑」的约束下做编排；最大差异——它在进程内用 Python 直接调 API，flockmux 用 PTY 驱动真 CLI。它的传输抽象层、上下文压缩引擎、registry 扩展点、kanban-swarm 任务分解，是 flockmux 编排内核最值得逐函数对照的对象。

## 1. 框架思想 / 设计模式

### Agent 主循环/上下文/工具执行/prompt 工程

- **ContextEngine 插件化接口 — 策略模式** — `agent/context_engine.py`
  - 压缩策略解耦为可替换插件（compressor/lcm/第三方），引擎还能暴露自定义工具给 agent 调用（get_tool_schemas），实现「上下文管理即工具」的扩展点。config.yaml 一行切换 engine。引擎失效时主循环不崩溃，降级到 fallback。
- **4-阶段分层压缩 pipeline** — `agent/context_compressor.py:compress`
  - ①廉价剪枝（无 LLM）→②边界计算（token 预算非固定 count）→③LLM 摘要（主/辅/fallback 三级）→④完整性修复（孤立 tool pair）。先做无代价的事，LLM 调用最后。每阶段独立、可测。
- **anti-thrashing 双重防护 — 效率追踪 + preflight/real 双信号** — `agent/context_compressor.py:should_compress / should_defer_preflight_to_real_usage`
  - 粗估会高估（schema 模板 token），真实 prompt_tokens 来自 API 响应，压缩后一次成功的真实值可作为下次 preflight 的基准，避免重复压缩。连续失效压缩计数防止无限压缩循环。
- **确定性 fallback 优先于无信息占位符** — `agent/context_compressor.py:_build_static_fallback_summary`
  - LLM 摘要失败时不是简单写 'N messages dropped'，而是本地解析对话结构提取 user_asks/tool_actions/paths/errors 生成结构化 fallback，保证即使无辅助模型 agent 仍能理解上下文骨架。
- **SUMMARY_PREFIX 指令精心设计 — 防止 summary 被当 active task** — `agent/context_compressor.py:SUMMARY_PREFIX`
  - 明确告知 'treat as background reference NOT active instructions'、'latest user message WINS'、'reverse signals must immediately end in-flight work'、'MEMORY.md is ALWAYS authoritative'。这是多轮 production 调试的结果，防止 AI 'resume from summary' 导致任务重复/幽灵任务。
- **工具护栏三维检测 — exact/same-tool/idempotent-no-progress** — `agent/tool_guardrails.py:ToolCallGuardrailController`
  - exact（同名同参）→精确死循环检测；same_tool（同名不同参）→策略性失败检测；idempotent_no_progress（只读工具返回相同结果）→信息获取停滞检测。三维独立计数，对应不同的 recovery hint。warn/block/halt 三级递进，默认只 warn 不阻止。
- **错误分类 pipeline 优先级链 — 从 provider-specific 到 unknown** — `agent/error_classifier.py:classify_api_error`
  - 优先级越高的规则越具体（Anthropic thinking sig、xAI subscription、content policy），越往后越通用。每个分类携带 retryable/should_compress/should_rotate/should_fallback 四个 action hint，主循环只需读 hint 不需重新判断错误类型。将几十个 provider 的错误 pattern 集中到一处维护。
- **系统提示三层分离 — 稳定/上下文/可变** — `agent/system_prompt.py:build_system_prompt_parts`
  - stable 层（identity/tools）变化最少 → upstream cache 命中率最高；volatile 层（memory/timestamp）每 turn 可变但放最后 → 不影响前缀 cache；context 层（AGENTS.md/CLAUDE.md）按需重建。分层是 Anthropic cache 命中率的关键。
- **流式状态机 scrubber — 跨 delta 边界安全** — `agent/think_scrubber.py:StreamingThinkScrubber`
  - per-delta regex 无法处理跨 delta 的 tag 边界（delta1='<think>' delta2='...'），必须有状态。hold-back 机制：末尾与 tag 前缀匹配的字节暂留，下一 delta 到来后解析。end-of-stream flush 处理未关闭块（leaking worse than truncation）。
- **消息完整性修复 — 编码健壮性层** — `agent/message_sanitization.py`
  - 不信任任何模型/用户输入的编码正确性：lone surrogate（剪贴板）→ U+FFFD；JSON tool args malformed（本地 LLM）→ 4-pass repair → {} 兜底；非 ASCII（LANG=C 容器）→ drop；所有修复都有日志，fallback 到 {} 比崩溃 session 优。
- **并发工具 dispatch 的保守安全规则** — `agent/tool_dispatch_helpers.py`
  - 不是所有工具都能并发：先检 NEVER_PARALLEL（clarify）→ 全批降顺序；再检 PARALLEL_SAFE（read/search）→ 可并发；PATH_SCOPED 用路径重叠检测；destructive terminal 命令强制顺序。宁可保守也不要并发副作用。
- **Prompt Caching breakpoint 策略 — system_and_3** — `agent/prompt_caching.py:apply_anthropic_cache_control`
  - 4 个 breakpoint 覆盖最多 cache 收益的位置：系统提示（大且稳定）+ 最近 3 条消息（每轮最可能被 cache miss 的边界）。Anthropic 原生格式 cache_control 在 message 顶层，OpenAI-compat 格式注入 content parts 最后一块。
- **Jittered backoff 防 thundering herd** — `agent/retry_utils.py:jittered_backoff`
  - 多 session 并发重试同一 provider 会在同一时刻 burst，加 monotonic counter + time_ns 种子的随机 jitter 分散重试。seed 设计防止同 clock tick 内并发 session 得相同随机值。

### 多模型传输抽象/适配器/计费限速

- **Transport ABC + 自注册模式** — `agent/transports/base.py + 各 transport 模块末尾 register_transport()`
  - 每个 transport 模块 import 时自动注册到全局 registry，get_transport() 懒发现触发 import。好处：零配置添加新 transport，不需要修改注册中心；测试中单独 import 一个 transport 不会漏注册其他(miss-then-discover)。代价：import 顺序有副作用
- **OpenAI 格式为内部 canonical，向供应商转换而非反过来** — `所有 transport.convert_messages()、ChatCompletionsTransport 近无操作实现`
  - 内部 messages 保持 OpenAI 格式，送出时才按目标供应商转换。Anthropic 把 system 拆出来、Codex 转 input items、Bedrock 转 converse format——均是单向 fanout。这让 Hermes 的 memory/skill/curator 层只需理解一种格式，供应商特殊性被 transport 层完全吸收
- **provider_data 作为 escape hatch 的 typed sidecar** — `NormalizedResponse.provider_data / ToolCall.provider_data`
  - 共享类型只放真正跨供应商的字段。协议私有状态(Codex call_id/response_item_id、Gemini thought_signature、Anthropic reasoning_details)放 provider_data dict 而非污染共享接口。protocol-aware 代码通过 property 读取，通用代码不感知。这与 flockmux 当前 'api_mode 字符串区分 + 枚举分支' 的做法相比更 OOP
- **finish_reason 到 OpenAI 标准集的强制映射** — `每个 Transport.map_finish_reason()，_STOP_REASON_MAP`
  - stop/tool_calls/length/content_filter 四种归一化 finish reason。各供应商有不同词汇(end_turn/tool_use/max_tokens/refusal/completed/incomplete...)全部映射。下游代码不需要 if api_mode == 'anthropic' 分支，直接读 finish_reason
- **Codex app-server: 同步 JSON-RPC over stdio 而非 async** — `agent/transports/codex_app_server.py:CodexAppServerClient`
  - 主循环同步，reader thread 在后台解析 stdout，blocking queue+timeout 桥接。理由：AIAgent.run_conversation 是同步的，async 会引入 surprising interrupt semantics。这和 flockmux 的 PTY 思路相通：不做原生 API 集成，而是通过进程间通信驱动黑盒 CLI
- **三队列分发模型(pending/notifications/server_requests)** — `CodexAppServerClient._dispatch`
  - JSON-RPC 消息按有无 id+有无 method 分为三类：reply(pending future)、server request(需要应答)、notification(单向)。三队列隔离让 caller 按需 drain，不混淆。approval bridge 在 server_requests 队列上工作，event projector 在 notifications 队列上工作
- **事件投影器(Event Projector)把协议事件翻译为内部消息结构** — `agent/transports/codex_event_projector.py:CodexEventProjector`
  - Codex 发 item/* 事件，Projector 在 item/completed 时物化为 OpenAI-shaped messages(assistant tool_call + tool result pairs)。有状态：_pending_reasoning 缓存 reasoning items 贴到下一条 assistant。这样 curator/memory/skill 层看到的消息结构与 chat_completions 路径完全一致，无需感知底层是 app-server 还是直接 API
- **Context length 解析的 10 级优先级降级链** — `agent/model_metadata.py:get_model_context_length`
  - 从最权威(用户显式配置)到最保守(256K 猜测)逐级 fallback。关键设计：provider 感知(同一个 claude 模型在 Anthropic=1M，Bedrock=200K，Copilot=128K)；stale cache 主动失效(Codex OAuth>=400K/Kimi<=32K/MiniMax-M3<=204K 等 known bad values)；Nous bypass cache 走实时 portal API。flockmux 完全缺少这层，只硬编码了一个 tier 名
- **跨 session 熔断文件(Cross-session Circuit Breaker File)** — `agent/nous_rate_guard.py`
  - 429 时写共享文件记录 reset_at，所有进程/session 在发请求前先检查文件。解决同一用户多开 CLI/cron/gateway 时 429 amplification(一次限速触发 3×3=9 次重试各消耗 RPH)。atomic_replace 保证文件一致性。is_genuine 区分真账号限速 vs 上游瞬时容量不足，防止后者误触断路器
- **Codex app-server session 退休机制(Session Retirement)** — `TurnResult.should_retire，CodexAppServerSession.run_turn`
  - turn 结果携带 should_retire=True 信号：超时/post-tool watchdog/OAuth失效/subprocess crash 都标记退休。caller 在下次 turn 时重建 session。把子进程生命周期管理的复杂性从 caller 剥离——caller 只看 should_retire flag 就够了
- **Usage canonical 归一化 + 定价三层查询** — `agent/usage_pricing.py:normalize_usage + estimate_usage_cost`
  - normalize_usage 把 Anthropic/Codex/ChatCompletion 三种 usage 形状归一为 CanonicalUsage；estimate_usage_cost 三层查：subscription_included shortcut→实时 /models pricing→官方文档快照。成本字段用 Decimal 避免浮点误差。status 字段区分 actual/estimated/included/unknown 供 UI 展示置信度
- **ProviderProfile 把 provider 特化逻辑从 build_kwargs 剥离** — `ChatCompletionsTransport._build_kwargs_from_profile vs 旧 flag-based path`
  - 旧路径：is_openrouter/is_nous/is_kimi 等 boolean flag 在 build_kwargs 内判断。新路径：profile.build_api_kwargs_extras/build_extra_body 把 provider quirk 封装到 profile 对象，build_kwargs 变成模板方法。注册的 provider 走 profile path，未注册的走 legacy fallback。渐进迁移：不需要一次重构所有 provider

### 记忆/技能注入/凭证池/curator

- **MemoryProvider ABC + MemoryManager Orchestrator** — `agent/memory_provider.py + agent/memory_manager.py`
  - Provider 实现抽象接口，Manager 做 fan-out + 错误隔离 + 路由。强制 1 external provider 限制是刻意牺牲灵活性换取 tool schema 可控性——多 provider 共享 tool 名空间必然冲突，且模型工具列表膨胀会降低 tool calling 精度
- **Streaming fence scrubber 状态机** — `agent/memory_manager.py:StreamingContextScrubber`
  - 流式输出 chunk 边界上 regex 无法处理跨块 tag，必须用显式状态机 + partial-match 缓冲。这个模式任何需要对 streaming output 做 tag 过滤的系统都需要，且比朴素 buffer-all-then-regex 对延迟友好
- **Prefetch/Sync 分离 + 后台预取** — `agent/memory_provider.py:prefetch / queue_prefetch`
  - prefetch() 在 turn 开始时同步读取缓存结果，queue_prefetch() 在 turn 结束后异步发起下一轮预取。把 '触发' 与 '读取' 解耦，把网络延迟移到 idle 时间，不增加 turn latency
- **Session lifecycle hooks 精细化** — `agent/memory_provider.py:on_session_switch / on_pre_compress / on_delegation`
  - 不只是 session start/end，还有 branch/resume/compress/delegation 等中间态。精细 hook 让 provider 能维护正确的 per-session 状态，否则 context compression 或 fork 后写入会落到错误的 session 记录里
- **Curator 惰性调度（inactivity-triggered，非 daemon cron）** — `agent/curator.py:maybe_run_curator / should_run_now`
  - 不需要独立守护进程或 OS cron。等 agent idle 超过 min_idle_hours 且距上次运行超过 interval_hours 才触发，天然适应 CLI/交互式场景（进程活跃时不运行）
- **LLM curator 结果三路信号优先级合并** — `agent/curator.py:_reconcile_classification`
  - absorbed_into 声明 > YAML block > tool-call heuristic。模型在删除时声明意图是最权威信号；YAML block 是次优但可能有幻觉（umbrella 不存在）；heuristic 是保底。三层设计比单一信号更鲁棒
- **Skill 生命周期状态机（active→stale→archived）** — `agent/curator.py:apply_automatic_transitions`
  - 纯函数自动转换，不依赖 LLM。stale 是软警告（可逆，用了就回 active），archived 是强归档（不自动恢复）。pinned=true 绕过所有转换——给用户一个安全阀
- **Skill 包（bundle）—— 多技能组合一次注入** — `agent/skill_bundles.py`
  - bundle 优先于 skill（同名时 bundle 赢）。一个 slash 命令加载多个 skill body，用于 '工作模式' 切换场景。mtime-keyed 缓存既捕捉文件编辑又捕捉删除（同时监视 dir mtime）
- **SKILL.md 模板变量 + inline shell 展开** — `agent/skill_preprocessing.py`
  - ${HERMES_SKILL_DIR} 让 skill 自引用脚本路径；!`cmd` 让 skill 在注入时执行命令（如 git log --oneline -5）把上下文动态化。这是把 skill 从静态文档升级为动态上下文的关键机制
- **Skill frontmatter platform/environment 双重过滤** — `agent/skill_utils.py:skill_matches_platform / skill_matches_environment`
  - platform 是硬兼容门（macOS-only skill 在 Linux 不展示）；environment 是软相关门（kanban skill 非 kanban 环境下隐藏但仍可显式 load）。两个维度正交，分别控制 '能用' 和 '推荐'
- **Credential Pool 多策略轮换 + exhaustion cooldown** — `agent/credential_pool.py:CredentialPool`
  - 用状态机而非简单 list 管理凭证：ok/exhausted（TTL 恢复）/dead（永不恢复，须显式 re-auth）。区分 transient failure（429/402 → exhausted + TTL）和 permanent failure（token_revoked → dead）。这个区别避免 dead 凭证每小时 retry 一次
- **Single-use refresh token 竞争条件处理** — `agent/credential_pool.py:_refresh_entry + _sync_*_from_*`
  - OAuth refresh token 单次使用，多进程/多 profile 共享同一 auth.json 会产生竞争。标准做法：refresh 前先 sync（adopt 外部已刷新的 token），refresh 后写回。用 token 值相等判断是否需要 sync（比时间戳更可靠）
- **Borrowed vs Owned 凭证分类 + 磁盘写入边界净化** — `agent/credential_persistence.py`
  - env 变量/GitHub CLI token 等 '借用' 凭证不应明文写盘（user 可能不知道 hermes 持久化了这些）。写盘前 sanitize 掉 token 值，只留 fingerprint。fingerprint 用于判断 env 变量和 pool 条目是否是同一值而不需要明文
- **Credential source 移除注册表（Registry + RemovalStep）** — `agent/credential_sources.py`
  - 每个 source 有自己的 remove_fn，统一的 find_removal_step 路由。消灭 auth_remove_command 里的 if/elif 链，新增 source 只加一条 register。suppress 机制防止 load_pool 重新 seed 已删的 source
- **Bitwarden 两层缓存（in-process + 磁盘）** — `agent/secret_sources/bitwarden.py`
  - 进程内 dict 缓解同一进程的热重载（gateway）；磁盘 JSON cache（0600 权限，原子 rename）缓解跨进程调用（cron/fork）。bws CLI 调用约 380ms，跨进程缓存对 CLI 场景有感知收益
- **上下文触发式 Onboarding（非阻塞问卷）** — `agent/onboarding.py`
  - 首次遇到行为分叉时展示 hint，而非 install-time wizard。config.yaml.onboarding.seen.<flag> 持久化已展示状态。比安装向导更适合 CLI 工具：用户在实际遇到问题时看到说明，而非在安装时就被轰炸

### LSP集成 / 多模态能力注册表 / 安全护栏

- **Provider+Registry 对称插件模式（6 种能力统一形状）** — `browser/image_gen/video_gen/web_search/tts/transcription 各一对 provider+registry 文件`
  - 每种多模态能力独立一套 ABC + 注册表，形状完全一致（name/is_available/get_setup_schema + register/list/get_active），新增能力只需加一对文件，不改框架核心。工具层只依赖 get_active_*() 函数，完全解耦后端实现
- **配置优先 → 可用性回退 → 遗留偏好 三段式 Provider 解析** — `browser_registry._resolve(), web_search_registry._resolve(), image_gen_registry.get_active_provider()`
  - explicit config（忽略 is_available 强绑定，让 dispatcher 给精确错误消息）→ single-eligible shortcut（只有一个时自动选）→ legacy preference walk（保持升级前行为兼容）。这三段优先级让「不设置」用户走历史默认，「设置了但凭证没配」给清晰报错，「迁移用户」不感知变化
- **Built-ins-always-win 内置优先注册守卫** — `tts_registry, transcription_registry — _BUILTIN_NAMES frozenset + 注册时拦截 + dispatch 时二次检查`
  - 内置实现在注册层和调用层双重禁止被插件覆盖。两处 frozenset 必须同步（有回归测试 TestBuiltinSync 保证），不通过 import 共享避免循环依赖
- **LSP delta 基线 + 行号重映射 过滤噪声诊断** — `LSPService.snapshot_baseline() / get_diagnostics_sync() + range_shift.build_line_shift()`
  - 写操作前快照诊断作为 baseline，写后与 post-edit 诊断做集合差分，只向 agent 呈现新增错误。编辑引起行偏移时用 difflib.SequenceMatcher 重映射 baseline 行号，防止「移动但未变」的诊断虚报为新引入。来自 Claude Code 的设计借鉴
- **异步 LSP 层 + 同步工具层 通过 BackgroundLoop 桥接** — `LSPService._BackgroundLoop + LSPService.get_diagnostics_sync()`
  - LSP 必须 async（read JSON-RPC stream），工具层是同步的。用一个 daemon asyncio loop 线程 + concurrent.futures.Future 传递结果，调用方 blocking wait。比 run_until_complete 更稳：loop 长活，可处理并发请求
- **spawning Future 合并并发请求（Promise-like coalescing）** — `LSPService._get_or_spawn() 的 self._spawning dict`
  - 第一个请求创建 asyncio.Future 放入 spawning dict；后续并发请求 await 同一个 Future，不重复 spawn 进程。spawn 完成后从 spawning 移出，后续请求走 _clients 直接复用。防止同 (server_id, root) 多进程竞争
- **Broken-set 熔断：失败后永不重试** — `LSPService._broken set + enabled_for() 短路 + _mark_broken_for_file()`
  - 语言服务器 spawn 失败（binary 缺失/timeout）后，(server_id, workspace_root) 进 broken-set。后续每次文件操作 enabled_for() 即刻返回 False，零成本跳过，不重试不超时。反模式：每次编辑都付 8s timeout 成本
- **git worktree 检测作为 LSP 功能门控** — `agent/lsp/workspace.py:resolve_workspace_for_file() → enabled_for()`
  - 只在 git worktree 内运行 LSP，防止在 home 目录、telegram 聊天 cwd 等场景启动不必要的 daemon 进程。优先用 cwd 的 git root，回退文件自身位置——支持打开 git repo 外的文件
- **安全护栏分层：write-deny（hard）+ read-block（defense-in-depth）+ 软警告（cross-profile/sandbox-mirror）** — `agent/file_safety.py`
  - write-deny 是硬拦截（SSH key/hermes credentials），read-block 是 defense-in-depth（工具层返回错误，但 terminal 仍可绕过），cross-profile/sandbox-mirror 是软警告（模型需显式 cross_profile=True 绕过）。明确分层让用户理解哪些是安全边界哪些是混淆缓解
- **脱敏子串预检 + 正则扫描 性能分层** — `agent/redact.py:_has_known_prefix_substring() + _PREFIX_SUBSTRINGS`
  - 从 prefix regex 自动提取字面前缀（_extract_literal_prefix），用 O(1) 字符串查找做预检，不含已知前缀则跳过昂贵正则。把 13 模式的平均扫描时间从 5.6us 降到 1.8us（每次 log record 都调用）。性能和完整性都不妥协
- **整文档同步（whole-document sync）代替增量 diff** — `LSPClient.open_file() textDocument/didChange`
  - 即使 server 声明支持 incremental sync，仍发送全文替换的 contentChanges。免去维护 range 书签的复杂度，所有主流 LS 都能处理。来自 OpenCode client.ts 的同款 trick
- **seed_first_push 静默首推 避免初始化推送提前解锁** — `LSPClient._handle_publish_diagnostics + LSPClient.__init__.seed_first_push`
  - TS language server 在 initialize 后立即推送一次诊断（全量），这发生在用户的 didChange 之前。seed_first_push=True 让首条推送只存储不触发 event，防止等待器以为「已收到新诊断」提前返回空结果
- **结构化事件日志 + once-per-key 去重** — `agent/lsp/eventlog.py`
  - 大量文件操作会重复触发 LSP，稳态事件（clean/disabled/no-root）降为 DEBUG 只记录首次；诊断/错误/超时事件记录为 INFO/WARNING。per-key set 去重防止同 (server_id, path) 日志刷屏。有专用 logger 名 hermes.lint.lsp 支持 grep 文档化
- **本地 bin 目录隔离 LSP 自动安装** — `agent/lsp/install.py:hermes_lsp_bin_dir() + INSTALL_RECIPES`
  - 所有自动安装的 LSP server 统一到 ~/.hermes/lsp/bin/，不污染系统 npm/go/pip 全局空间。symlink + fallback copy 处理 Windows 不支持 symlink 的情况。per-package threading.Lock 防并发重复安装

### ACP(Agent Client Protocol) 适配器

- **stdio JSON-RPC 传输 + stderr 日志隔离** — `entry.py:_setup_logging, session.py:_acp_stderr_print, session.py:SessionManager._make_agent`
  - ACP 协议要求 stdout 只走 JSON-RPC 帧，任何 print/logging 都要强制路由到 stderr。这与 flockmux 的 PTY 驱动形成对比：flockmux 用 PTY 截获全部 stdout，ACP 依靠进程本身严格区分 fd。hermes 选择 ACP 是因为它支持原生 app-server 模式，省去 PTY 开销。
- **async 事件循环 + ThreadPoolExecutor 分离（同步 AIAgent 在 worker 线程）** — `server.py:HermesACPAgent.prompt, events.py:_send_update`
  - AIAgent.run_conversation 是同步阻塞的，但 ACP server 要求 async。hermes 用 ThreadPoolExecutor(max_workers=4) + loop.run_in_executor 解耦。worker 线程回调需要 run_coroutine_threadsafe 把 update 投入 event loop。flockmux 的 PTY 驱动本质上也是同步 I/O 在 tokio spawn_blocking 里，思路一致。
- **ContextVar 隔离 per-session 状态（TLS 替代方案）** — `server.py:_run_agent, edit_approval.py:_EDIT_APPROVAL_REQUESTER`
  - 多个 ACP session 共享同一个 ThreadPoolExecutor，不能用 os.environ 全局状态。contextvars.copy_context() 包装 _run_agent 确保 HERMES_SESSION_KEY/edit_approval_requester 等只在本 session 线程可见。flockmux 目前 per-agent 状态通过 SQLite session_id 隔离，若引入更多线程级回调可借鉴此模式。
- **Session 持久化 + 透明内存还原（两层存储）** — `session.py:SessionManager, SessionManager._restore, SessionManager._persist`
  - SessionManager 同时维护 in-memory dict 和 SessionDB。get_session() miss 时自动从 DB 还原 agent 实例和历史，对调用方透明。DB 里存的是 replace_messages 原子写（防 mid-rewrite 损坏）。flockmux 的 agent 状态已在 SQLite，但 '进程重启后恢复 PTY 会话' 这层逻辑尚不完整，可参考此 restore 路径。
- **History 回放协议（spec-mandated sync replay in load/resume lifetime）** — `server.py:HermesACPAgent._replay_session_history, load_session, resume_session`
  - ACP spec 要求客户端在 load_session RPC 响应之前收到完整历史重放（Zed 在 await RPC 前就注册好 session_update 路由）。必须 await replay 后再 return LoadSessionResponse，不能用 call_soon 延迟。flockmux 如要实现 session resume，必须遵守同样的顺序约束。
- **工具调用 FIFO 队列跟踪同名并发 ID** — `events.py:make_tool_progress_cb, make_step_cb`
  - AIAgent 同一轮可能调用同名工具多次（如多个 read_file）。hermes 用 Dict[str, Deque[str]] 按工具名维护 FIFO ID 队列：started 时 append，completed 时 popleft，保证配对正确。flockmux 的实时进度已读 JSONL，但如果未来要向前端推送工具开始/结束配对事件，可参考此设计。
- **模式(mode) → 策略(policy) 双向映射，用模式暴露编辑权限控制** — `server.py:HermesACPAgent._MODE_TO_EDIT_APPROVAL_POLICY, set_session_mode, set_config_option`
  - Zed 的 ACP session 有 mode 和 config_options 两个槽位。hermes 选择把编辑审批策略(ask/workspace_session/session)映射为 ACP mode，而不是 config_option，原因是 Zed 在 mode picker 位置渲染 mode，比 config_options 更显眼且与 Claude/Codex 风格对齐。flockmux 设置页的 per-agent 权限控制可以参考此 mode 概念。
- **EditProposal 预计算 + ContextVar 门卫（pre-execution diff approval）** — `edit_approval.py:maybe_require_edit_approval, build_edit_proposal, make_acp_edit_approval_requester`
  - 编辑审批在工具执行前拦截：build_edit_proposal 调 fuzzy_find_and_replace 预计算实际 new_text，生成精确 diff 给用户审阅。requester 通过 ContextVar 绑定，非 ACP session（CLI/gateway）不受影响。should_auto_approve_edit 提供三档策略(ask/workspace/session)，敏感路径黑名单始终阻断。这是真正 pre-execution 的 diff-first 审批。
- **ACP 多内容块格式化：per-tool 专用 formatter + generic 结构化 fallback** — `tools.py:_build_polished_completion_content, _format_generic_structured_result, _format_structured_value`
  - hermes 对 20+ 工具各写了专用 formatter（_format_todo_result 等），核心工具有精确展示，未知工具走 generic(_format_structured_value 递归渲染 JSON)。这比直接返回 raw JSON 或 raw text 都要好。flockmux 当前工具结果只是文本流，可以为 blackboard/tool 结果引入类似 formatter 层。
- **todo 结果投影为 ACP native plan_update（protocol-native 任务面板）** — `events.py:_build_plan_update_from_todo_result`
  - Hermes 代理已有 todo 工具维护任务状态，ACP 有 plan sessionUpdate 供编辑器渲染任务面板。adapter 在每次 todo 工具完成后额外发一次 AgentPlanUpdate，把内部 todo 数据映射为 ACP entries(cancelled → completed + 前缀标注)。flockmux 的台账页完全可以用同样思路：任务状态变化时向前端推 WS 事件，复用黑板的 typed payload 结构。
- **提示词队列(queued_prompts) + /steer 中断恢复机制** — `server.py:HermesACPAgent.prompt, _cmd_steer`
  - session 运行中收到新 prompt 时入队而非丢弃或拒绝，返回 'Queued' 确认。cancel 时保存 interrupted_prompt_text；下次 /steer idle 时把 interrupted prompt 与 steer 指导合并重发，避免用户丢失工作。flockmux 的 queued_prompts 概念值得引入：orchestrator 发送 prompt 时若 worker 正忙，应入队而非直接报错。
- **Auth 方法声明：运行时 provider 检测 + 终端 fallback 双保险** — `auth.py:build_auth_methods, detect_provider`
  - ACP registry 要求 initialize 必须声明至少一个可用 auth method。hermes 始终追加 TerminalAuthMethod 作为 fallback（新机器首次配置），已有 credentials 时前置 AuthMethodAgent。detect_provider 识别 Callable api_key（Azure Entra ID bearer token）为合法凭证——普通字符串检测会误判。

### 多平台消息网关 + 流式分发

- **平台适配器 ABC + 工厂注册表** — `gateway/platforms/base.py:BasePlatformAdapter + gateway/platform_registry.py:PlatformRegistry`
  - BasePlatformAdapter 定义行为接口（connect/send/edit_message/delete_message），PlatformRegistry 持有 PlatformEntry 工厂 dataclass，将 '能否运行（check_fn）' + '是否配置（validate_config）' + '如何创建（adapter_factory）' + '如何独立发送（standalone_sender_fn）' 解耦成独立字段。新平台只需注册一条 PlatformEntry，gateway 核心无需修改。standalone_sender_fn 允许 cron 进程在无 live gateway 时跨进程发送。
- **结构化 typed event 词汇表（表示层/传输层分离）** — `gateway/stream_events.py + gateway/stream_dispatch.py`
  - agent worker 发出 frozen dataclass events（MessageChunk/ToolCallChunk 等），不携带任何平台知识；GatewayEventDispatcher 持有 adapter + sink，把 event 路由给 adapter 的 render_message_event/format_tool_event。adapter 决定是否渲染（None = eat），sink 决定如何投递。解决历史问题：tool-progress bubble 与 streaming draft 同时发送时竞争同一 chat 的 message_id，格式化逻辑误在 agent 侧而非 gateway 侧。
- **同步 queue 桥接异步投递（thread/async boundary）** — `gateway/stream_consumer.py:GatewayStreamConsumer`
  - agent 在 run_in_executor 线程池中同步执行；stream delta 通过 thread-safe queue.Queue 传递（on_delta 仅 put，无 asyncio 依赖）；asyncio task 从 queue 消费、rate-limit、progressive edit。_DONE/_NEW_SEGMENT/_COMMENTARY sentinel 实现带外信令（segment break/finish/commentary），不需要锁。adaptive edit interval backoff 在 flood-control 环境下自动降频。
- **Session 并发隔离：asyncio guard 先于 task spawn + stale-lock 自愈** — `gateway/platforms/base.py:handle_message + _start_session_processing + _heal_stale_session_lock`
  - _active_sessions guard 在 spawn task 前同步置位（grammY sequentialize/aiogram EventIsolation 模式），彻底关闭 '第二条消息在 task 未启动前通过检查' 的竞争窗口。_heal_stale_session_lock 在入站时检测 owner task 已退出的僵死 guard 并自愈，防止 session 永久被锁。两条防线分别处理 '启动时' 和 '崩溃后' 两种 stale lock 场景。
- **ContextVar 替代 os.environ 实现 task-local session 上下文** — `gateway/session_context.py`
  - os.environ 是进程全局的，并发 asyncio task 处理不同消息时会相互覆盖 HERMES_SESSION_PLATFORM 等变量。ContextVar 值是 task-local 的，每个 asyncio task 和它 spawn 的 executor 线程各自独立。_UNSET sentinel 区分 '从未在此 context 设置（fallback os.environ，CLI/cron 兼容）' 与 '显式清除（返回空串，不 fallback）'。
- **四级优先级展示配置解析 + 平台 tier 分层默认** — `gateway/display_config.py:resolve_display_setting`
  - 不同平台能力差异巨大（Telegram 支持 edit，SMS 不支持）。per-platform用户覆盖 > 全局用户设置 > 平台 tier 内置默认 > 全局内置默认的四层解析，让每个平台有合理的开箱即用体验（SMS 默认关闭 streaming/tool_progress），同时允许用户精细覆盖。
- **双重安全路径文件投递（allowlist + denylist + recency window）** — `gateway/platforms/base.py:validate_media_delivery_path`
  - 默认模式（单用户/私有 gateway）：仅拒绝 credential/system 路径（denylist），允许其余所有文件，对称于入站（平台发来什么都接受）。严格模式（公开 gateway）：必须在 cache 目录 OR allowlist OR 600s mtime 窗口内。recency window 的洞见：agent 产出物几乎在秒级交付，而系统文件（/etc/passwd、~/.ssh/id_rsa）mtime 以天计——两者在时间维度上天然分离。
- **跨频道 skill/prompt 绑定（channel-level routing）** — `gateway/platforms/base.py:resolve_channel_skills / resolve_channel_prompt + MessageEvent.auto_skill`
  - channel_skill_bindings 和 channel_prompts 在 adapter 入站阶段就填入 MessageEvent，让下游 agent 无需查表。parent_id fallback 允许 Discord forum thread 继承父频道的 skill，减少配置重复。auto_skill 为 list 支持多 skill 叠加。
- **平台 hook 外挂（文件系统发现 + 动态加载）** — `gateway/hooks.py:HookRegistry`
  - hook 是 ~/.hermes/hooks/*/HOOK.yaml + handler.py 的目录结构，gateway 启动时发现+动态 importlib 加载，handler 注册到 event 路由表。emit/emit_collect 双接口：emit 只触发副作用，emit_collect 收集返回值用于 policy 决策（allow/deny/rewrite）。command:* 通配符让 '拦截任意 slash command' 的 hook 只需一个注册。
- **配对码审批流（OWASP compliant 动态白名单）** — `gateway/pairing.py:PairingStore`
  - 代替静态 user_id 白名单，陌生用户收到一次性配对码，bot owner 在 CLI approve。安全设计对照 OWASP + NIST SP 800-63-4：8 字符 base32 无歧义字母 + secrets.choose + 1h TTL + 速率限制（10min/user）+ 失败锁定（5次/1h）+ 文件 chmod 0600 + 原子写。码只存 hash（sha256 + salt）。
- **Session mirror（跨进程跨平台消息同步到 transcript）** — `gateway/mirror.py:mirror_to_session`
  - CLI/cron 发出的消息通过 mirror_to_session 追写到目标 gateway session 的 JSONL+SQLite，让接收端 agent 下次回复时有发出消息的上下文。多候选 session 时，存在多个不同 user 的 session 则拒绝 mirror（避免污染他人），单 session 则直接 mirror。完全解耦：不需要 live gateway，只需要 session 文件在磁盘。
- **LRU Agent 缓存 + config signature 变更自动 evict** — `gateway/run.py:GatewayRunner._agent_config_signature / _evict_cached_agent / _enforce_agent_cache_cap`
  - 每个 session 的 AIAgent 实例缓存复用（持有 LLM client/tool schema/memory provider），128 cap + 1h idle TTL。_agent_config_signature 哈希 model/skill/prompt 等关键 config，变更时强制 evict 重建，防止 stale agent 用旧配置继续工作。generation 计数器让 post_delivery_callback 不会错配给新 run。

### CLI kanban / swarm 多 agent 任务分解 / goals

- **CAS (Compare-And-Swap) 原子 claim，无重试自旋** — `kanban_db.py:claim_task, release_stale_claims`
  - SQLite WAL mode + BEGIN IMMEDIATE 保证写事务串行；losers 观察 rowcount==0 直接放弃而非重试，避免 thundering herd。这是分布式 claim 在本地 SQLite 上最简洁的实现：不需要 Redis/etcd，利用 SQLite 的单写者特性
- **事件溯源 (Event Sourcing) 作为诊断信号通道** — `kanban_db.py:_append_event, kanban_diagnostics.py:_active_hallucination_events`
  - 所有状态变更同步写 task_events；diagnostic rules 是纯 read-only 规则，过滤事件序列推断 active 问题。好处：diagnostic 逻辑随时可更改而不需 migrate；auto-clear 靠事件时序而非轮询
- **Fail-Open 判断 + 有界退避** — `goals.py:judge_goal, GoalManager.evaluate_after_turn`
  - judge API failure → continue（不阻断进度），parse failure → 计数 → N次连续失败才 auto-pause。API 抖动不应等同于 model 能力不足，两者阈值不同。有界 turn budget 是最终 backstop
- **三级 handoff 通道：result / run.summary / run.metadata** — `kanban_db.py:build_worker_context, complete_task`
  - task.result 是简短字符串（向后兼容）；run.summary 是结构化文字；run.metadata 是机器可读 dict（changed_files/tests_run 等）。worker 消费时优先 run.summary+metadata，老数据 fallback task.result。三层递进兼容历史数据同时支持丰富结构化 handoff
- **任务图 DAG 内嵌 kanban 的 task_links，dispatcher 与依赖共享一个内核** — `kanban_swarm.py:create_swarm, kanban_db.py:recompute_ready, decompose_triage_task`
  - 没有第二个调度系统——swarm/decompose 都是 task_links 图写法，完全复用 recompute_ready + dispatch_once。结构简单但功能完整：并行 worker、串行 verifier/synthesizer、orchestrator 等待全图，均用同一 parent→child 表达
- **LLM 作为任务路由/分解 oracle（auxiliary client 模式）** — `kanban_decompose.py:decompose_task, kanban_specify.py:specify_task, goals.py:judge_goal`
  - aux client 与主 model 解耦，可以指向不同 provider/model；所有 LLM call 都是 one-shot+lenient parse（fallback 不 crash），避免代价高的重试；temperature=0.3 在推理型任务(decompose/specify)，temperature=0 在判断型任务(judge)
- **Workspace 隔离 + 类型化（scratch/worktree/dir）** — `kanban_db.py:create_task, decompose_triage_task, build_worker_context`
  - workspace_kind 把 'ephemeral scratch' 与 '共享 worktree/dir' 明确区分；子任务继承父任务 workspace（同类型才继承 path）；dispatcher spawn 时 resolve_workspace 决定实际路径。把 workspace 类型化后 cleanup 和 git worktree 创建可对不同类型使用不同策略
- **Sticky Block vs. Circuit-Breaker Block 事件区分** — `kanban_db.py:_has_sticky_block, recompute_ready, _record_task_failure`
  - 同为 blocked 状态，两类来源需不同恢复路径：工人主动 block（需人类干预 → unblock_task）vs 断路器 gave_up（operator 可 reassign 触发自动重试）。通过最近 blocked/unblocked 事件极性判断，无需额外列，backward compatible
- **Diagnostic Rule 注册表 + Action kind 驱动 UI** — `kanban_diagnostics.py:_RULES, DiagnosticAction, compute_task_diagnostics`
  - 规则注册表让新规则=新函数+追加到列表；action.kind 驱动 dashboard render buttons，CLI render hints，解耦诊断逻辑与展现层。每个 rule 独立可测，可以 swallow rule 异常不影响整体
- **幂等 Swarm 拓扑（blackboard topology 恢复）** — `kanban_swarm.py:create_swarm`
  - create_swarm 先查 root task 的 blackboard 是否已有 topology；若有且完整则直接返回已存在的 ids 不重复创建。idempotency_key 在 create_task 层防止重复任务；两层幂等保证 webhook 重试/gateway 重启安全
- **Board 多项目隔离 + default 向后兼容路径** — `kanban_db.py:kanban_db_path, boards_root, scoped_current_board`
  - default board 保持 <root>/kanban.db 老路径（不需 migrate），非 default board 用新目录结构。ContextVar scoped override 保证多 board 并发操作线程安全，dispatcher spawn 时注入 env pin 确保 worker 路径不因解析链差异漂移
- **Ralph-style goal loop 驱动 kanban worker** — `goals.py:run_kanban_goal_loop, GoalManager.evaluate_after_turn`
  - worker 不是单次 prompt→response，而是 judge 驱动的多轮 session：继续/结束由 auxiliary judge 决定而非 worker 自判。worker 失去控制权（只管执行），judge 拥有 done/continue 决策权。turn budget 是强 backstop 防无限跑，finalize nudge 对'完成但未调 API'给一次优雅退出机会

### Web dashboard 鉴权 / PTY 桥 / proxy / TUI

- **双 token WS 鉴权分层：单次 browser ticket vs 进程级 internal credential** — `ws_tickets.py + web_server.py:_ws_auth_reason`
  - Browser 不能在 WS upgrade 发 Authorization，用 ?ticket= 解决；server 自己 spawn 的子进程需要 reconnect，30s ticket 不够，改用 per-process multi-use credential；两种凭证共享同一验证入口但语义完全不同，威胁模型清晰分离
- **多 provider 链式验证 + fail-open-gracefully** — `middleware.py:gated_auth_middleware + _attempt_refresh`
  - 多 provider 允许同时激活（Nous + 自托管 OIDC 共存）；provider 返回 None 表示'不认识这个 token'，抛 ProviderError 表示'IDP 不可达但无法否认'；两种失败语义不同，链式处理才能区分
- **透明 AT/RT 分离 + 无感刷新** — `cookies.py + middleware.py`
  - AT Max-Age 绑定 token 实际过期时间，浏览器会主动删除；AT 消失时 middleware 自动用 RT 刷新，对用户透明；AT/RT cookie 生命周期解耦(15min vs 30天)，不用轮询 expiry
- **cookie 前缀选择策略 (__Host- / __Secure- / bare)** — `cookies.py:_resolved_name`
  - __Host- 最安全但 Path 必须为 /，与反代 prefix 不兼容；根据 HTTPS + prefix 组合动态选择前缀，保持最强可用约束；reader 用 fallback 遍历三种变体确保找到
- **open-redirect 多层防御** — `middleware.py:_safe_next_target + routes.py:_validate_post_login_target`
  - next= 在 gate→/auth/login→PKCE cookie→callback 全链路携带，每层独立校验(gate 出口、/auth/login 入口、callback 入口)；PKCE cookie 承担跨 IDP 跳转的 next= 传输，IDP 返回的 URL 上的 next= 参数被明确忽略
- **PTY 双向异步分离：reader task + writer loop** — `web_server.py:pty_ws`
  - PTY master 读阻塞不影响 WS 写，用 asyncio executor 跑 bridge.read(timeout)；writer 在 event loop 同步跑，收到 WS 消息立即写；两路异步共享 bridge 对象但 PTY master fd 是内核 sync point，不需要额外锁
- **PTY 进程注入 env 而非 CLI args 传配置** — `web_server.py:_resolve_chat_argv + _build_gateway_ws_url`
  - HERMES_TUI_GATEWAY_URL / HERMES_TUI_SIDECAR_URL 等通过 env 注入；argv 不变；设计解耦：TUI binary 不需要感知 dashboard 的端口配置，只读 env；internal credential 也只走 env，XSS 无法从 HTML 中读取
- **DNS rebinding 防护：HTTP middleware + WS 手动检查双保险** — `web_server.py:host_header_middleware + _ws_host_origin_reason`
  - FastAPI HTTP middleware 不运行在 WS 路由上，WS 升级必须自己复现 Host/Origin 验证；两层独立实现，防止 FastAPI 路由机制变化导致保护丢失
- **auth gate 条件激活：loopback 免鉴权** — `web_server.py:should_require_auth`
  - 本机访问不需 OAuth 避免 DX 摩擦；公网绑定自动激活鉴权；--insecure flag 提供 escape hatch 但 RFC1918 LAN 仍算 public（同 LAN 攻击者是威胁模型）；loopback 用 SESSION_TOKEN 常量时间比较作 WS 鉴权
- **proxy 适配器模式 + 白名单路径 + 单次 retry** — `proxy/server.py + proxy/adapters/base.py`
  - Proxy server 完全不知道 provider 细节；adapter 只负责凭证解析和路径白名单；401/429 时调 get_retry_credential 而非无限重试；hop-by-hop headers 明确 strip list
- **service manager Protocol 抽象 + capability check** — `service_manager.py`
  - 不同 OS/init 系统共享接口但 capability 不同；runtime registration 只有 s6 支持，调用方先 supports_runtime_registration() 再调；Protocol runtime_checkable 允许 isinstance 检查
- **s6 service 目录原子写：tmp→rename + pre-seed supervise skeleton** — `service_manager.py:S6ServiceManager.register_profile_gateway`
  - 写 tmp dir 再 rename 防 s6-svscan 在半初始化目录触发；pre-seed supervise/ 目录为 hermes 用户所有，规避 s6-supervise 以 root 创建后 hermes 用户 EACCES 的已知问题
- **curses 菜单 callback-driven 抽象** — `curses_ui.py:_run_curses_menu`
  - 三个菜单(单选/多选/搜索)以前逐条复制事件循环；抽取公共 loop，per-menu 差异通过 draw_header/draw_row/on_action callback 注入；_KEEP 哨兵替代 bool 让 reducer 语义更清晰
- **审计日志 fail-safe：写失败 Warning 不中断鉴权** — `dashboard_auth/audit.py:audit_log`
  - 审计日志是观测手段，不应成为鉴权关键路径；磁盘满/权限问题只 log Warning；_REDACTED_FIELDS 集中管理，加新敏感字段在一处

### 顶层引擎(cli/run_agent/state/压缩/批跑/mcp_serve/cron)

- **Forwarder-Module 分拆模式** — `run_agent.py AIAgent 类，agent/ 子目录各模块`
  - AIAgent 是纯 forwarder 壳：__init__ 委托 agent_init，run_conversation 委托 conversation_loop，codex 委托 codex_runtime。大文件通过 forwarder 让外部 import 保持稳定，内部拆包到职能模块。flockmux 的 agents.rs 700+行已有类似压力，可参照
- **声明式 Schema 迁移(Beets/sqlite-utils 模式)** — `hermes_state.py SessionDB._reconcile_columns`
  - SCHEMA_SQL 是唯一的 schema 真相源。每次启动用 in-memory SQLite 解析 DDL 得到期望列集，对比 live 表 PRAGMA table_info，自动 ALTER TABLE ADD COLUMN。不需要维护版本号迁移链，新增列只要改 SCHEMA_SQL 即可。flockmux 当前 migrations.rs 是 version-gated chain，可借鉴这种声明式 diff-and-patch 思路
- **WAL 写竞争：BEGIN IMMEDIATE + 随机 jitter retry** — `hermes_state.py SessionDB._execute_write`
  - SQLite 内置 busy handler 是确定性退避，多进程并发时形成护卫队(convoy)效应。改为短 timeout(1s)+应用层随机 jitter(20-150ms，最多 15 次)，每 50 次写做一次 WAL TRUNCATE checkpoint，有效打散 convoy。flockmux 同样是 SQLite WAL 多进程写，可直接借鉴此策略
- **压缩锁：原子 DELETE(expired)+INSERT OR IGNORE 实现悲观并发控制** — `hermes_state.py compression_locks 表 + try_acquire_compression_lock`
  - 两个压缩路径同时决定压缩同一 session 会产生孤儿 child session。用 SQLite 原子事务做进程级分布锁：先删过期行，再 INSERT OR IGNORE，最后 SELECT 验证 holder 匹配。crashed holder 通过 expires_at 自动过期回收。flockmux blackboard handoff key 可参照此机制防竞态
- **Cron Wake-Gate 模式：脚本最后行 JSON {wakeAgent: false} 跳过 LLM** — `cron/scheduler.py _parse_wake_gate + _build_job_prompt`
  - 定时任务并不每次都需要调用 LLM。让数据采集脚本用 JSON 约定告诉 scheduler 本次没有新内容，直接跳过 agent run + token 消耗。结合 no_agent=True 模式(完全绕过 LLM，bash 直接输出)可实现三层：纯 bash→有条件 LLM→总是 LLM。flockmux 的 cron 可设计相同的 wake-gate 协议
- **Cron 交付：live adapter 优先，HTTP standalone 降级** — `cron/scheduler.py _deliver_result`
  - 结果投递先尝试从 gateway adapters 字典取活跃 adapter(支持 E2EE 平台如 Matrix 加密投递)，失败后用独立 asyncio.run() HTTP 请求。两条路径分工：在线时减 HTTP 往返，gateway 不在线时保证投递。flockmux 的通知投递可借鉴这种 adapter_or_fallback 模式
- **并发 Cron：sequential pool(单线程) + parallel pool(有界多线程) 分层** — `cron/scheduler.py _get_sequential_pool/_get_parallel_pool + tick()`
  - workdir/profile job 会修改 os.environ 全局状态，必须串行。普通 job 可并发。tick() 把两类 job 分流到两个持久化线程池：sequential=单线程 FIFO，parallel=有界并发。两个池都是 fire-and-forget，ticker 线程不阻塞。in-flight 去重防止同一 job 跨 tick 堆叠。flockmux 的角色感知派发与此类似
- **mtime 双重检查的廉价轮询** — `mcp_serve.py EventBridge._poll_once`
  - 200ms 高频轮询 SQLite 若每次真正查库代价显著。先 stat() sessions.json 和 state.db 的 mtime，两者都未变化则直接 return。mtime 检查约 1μs，真正有变化才执行 SQL 查询。flockmux 的 transcript.rs tail 可参考此 mtime-gate 减少无谓 read
- **轨迹压缩：保护头尾+贪婪中段压缩+LLM 摘要替换** — `trajectory_compressor.py TrajectoryCompressor.compress_trajectory`
  - 训练轨迹 token 超限时，保护开头(system/first human/gpt/tool)和结尾(last N turns)不动，只压缩中间段。从中间段起按 token 贪婪积累直到 savings >= needed，被压缩段用 LLM 生成的摘要替换为单条 human 消息。这样模型看到的头尾信息完整，中段被语义化压缩。flockmux 的对话历史管理目前无此机制，context 过长时 agent 只能靠 claude 自身 context window
- **Toolset 分布采样用于训练数据多样性** — `toolset_distributions.py DISTRIBUTIONS + sample_toolsets_from_distribution`
  - 批量产训练数据时不固定工具集，而是用概率分布采样每个 toolset 是否启用。不同分布(image_gen/research/science/development)偏向不同工具组合，生成的轨迹更多样。这和 flockmux 角色注册表的 toolset per-role 配置思想类似，但用于离线数据生成
- **MCP Server 自暴露：把自己变成工具** — `mcp_serve.py create_mcp_server`
  - hermes 把自己的消息历史和跨平台会话 expose 成 MCP tools(conversations_list/messages_read/events_wait 等)，让 Claude Code/Codex 等 MCP client 直接读取 hermes 的内部状态、发消息、管理 approval。这是 agent 互联互通的关键模式：orchestrator 通过 MCP 协议观察子 agent 状态。flockmux 可以做同样的事：把 flockmux blackboard/agent 状态暴露成 MCP tools

### 桌面壳 (apps/desktop) + 共享层 (apps/shared)

- **Backend 描述符驱动 spawn（candidate chain + smoke probe）** — `electron/main.cjs resolveHermesBackend → ensureRuntime → startHermes`
  - 把「找什么 binary」和「怎么 spawn」完全分离：resolveHermesBackend 只返回描述符对象（kind/command/args/env/bootstrap），ensureRuntime 做最终校验，实际 spawn 另一函数负责。每级候选配 smoke-test（--version / import 探针），不通过就降级而非抛错，保证 fallback 链不被坏候选断路
- **Bootstrap 事件流协议（JSON lines → typed events → ring buffer snapshot）** — `bootstrap-runner.cjs → broadcastBootstrapEvent → bootstrapState`
  - install 脚本通过 JSON lines 吐出结构化进度（manifest/stage/log/complete/failed），runner 解析后广播给 renderer，同时维护 server-side snapshot（含 500 行 log ring）。renderer 刷新后可查快照恢复状态，不依赖 renderer 自身的内存。这是「长进程 + GUI 解耦」的标准模式
- **Nanostores per-id 派生 atom 缓存（memoized computed）** — `apps/desktop/src/store/panes.ts $paneOpen/$paneState/$paneWidthOverride`
  - 用 Map 缓存 computed atom 引用，保证同一 id 的 useStore 订阅引用不变，防止每次渲染都创建新 computed 触发全量重订阅。这是 nanostores 在多实例场景的最佳实践
- **增量消息仓库同步（Incremental External Store）** — `apps/desktop/src/lib/incremental-external-store-runtime.ts`
  - override @assistant-ui/react 的全量 setMessages，改为增量 diff（addOrUpdate + deleteAbsent）。流式输出长消息时 O(1) 追加而非 O(N) 全替，避免 DOM 树重建和滚动位置跳动。这是高吞吐 streaming UI 的关键优化
- **CSS 变量作为布局协议（PaneShell emit → AppShell consume）** — `apps/desktop/src/components/pane-shell + apps/desktop/src/app/shell/app-shell.tsx`
  - PaneShell 把各 pane 宽度 emit 为 CSS 变量 --pane-{id}-width，AppShell 和 TitlebarControls 用 calc() 消费。跨越 React 组件树边界传递尺寸信息，不需要 context 或 store，纯 CSS 同步。这是「组件间布局协调」的零开销方案
- **多 profile 后端池（LRU + keepalive-fresh 保护）** — `electron/main.cjs backendPool + POOL_MAX_BACKENDS + POOL_IDLE_MS + POOL_KEEPALIVE_FRESH_MS`
  - primary 后端独立管理，pool 只存 extra profiles。LRU eviction 但 keepalive-fresh 窗口内（90s）的后端豁免驱逐，防止正在使用的 profile 被杀。idle reaper 定时清理，cap 防止同时跑过多后端消耗内存
- **连接配置双层鉴权（token vs oauth ticket）** — `electron/connection-config.cjs + main.cjs gateway 连接路径`
  - Legacy token 模式简单追加 ?token=；OAuth 模式 REST 用 HttpOnly cookie，WS 用单次 ticket（POST /api/auth/ws-ticket）。两者接口统一为 buildGatewayWsUrl/WithTicket，调用方不区分。resolveTestWsUrl 探测时 oauth mint 失败必须 throw 而非 skip，防止 HTTP ok 但 WS 失败的假阳性
- **自更新原子操作（Windows venv shim 文件锁释放 + macOS detached bash swap）** — `electron/main.cjs applyUpdates + releaseBackendLockForUpdate + applyUpdatesPosixInApp`
  - 两平台更新策略不同：Windows 必须先完全终止所有后端（含 taskkill /T /F 树杀）等待 shim 解锁再启动外部 updater；macOS 可在进程内跑 hermes update + rebuild，然后写 detached bash 脚本等父进程退出再做 ditto bundle swap。原子 rename 保证 swap 失败不留烂 bundle
- **状态栏/标题栏 slot 注入（SetStatusbarItemGroup/SetTitlebarToolGroup 回调）** — `apps/desktop/src/app/shell/statusbar-controls.tsx + titlebar-controls.tsx`
  - 页面/pane 通过 props 收到 setter 函数，可以 mount 时注册、unmount 时清空自己的 items。比全局 store 更干净：items 生命周期与组件绑定，不用手动清理全局状态
- **Cron 表达式反向解析到预设（scheduleOptionForExpr）** — `apps/desktop/src/app/cron/index.tsx`
  - 通过解析 5-field cron 的 day/hour/minute token 规律匹配到 daily/weekdays/weekly/monthly/hourly 预设，custom 兜底。编辑时预设选择器和原始 cron 输入双向同步（预设→表达式，表达式→最近预设）。减少用户直接接触 cron 语法的认知负担

## 2. 函数级地图（穷举）

### Agent 主循环/上下文/工具执行/prompt 工程  ·  40 项

- `agent/context_engine.py:ContextEngine` — 上下文引擎抽象基类 (ABC)，定义 should_compress/compress/update_from_response/on_session_start/on_session_end/on_session_reset/get_tool_schemas/handle_tool_call/update_model 全套接口；maintain 6 个 token 追踪字段供外层读取
  - 💡 引擎可以自己暴露工具给 agent 调用（get_tool_schemas + handle_tool_call），LCM 引擎就用这个扩展点注入 lcm_grep/lcm_describe；threshold_percent/protect_first_n/protect_last_n 作为类属性可被子类覆盖
- `agent/context_compressor.py:ContextCompressor` — 默认上下文引擎，4 阶段压缩：① 工具输出剪枝（无 LLM 调用）② 头部保护 ③ token 预算尾部保护 ④ 结构化 LLM 摘要；支持迭代更新摘要、焦点主题引导压缩、失败回退
  - 💡 anti-thrashing 机制：连续两次压缩节省 < 10% 则跳过；abort_on_summary_failure 控制失败时是中止还是插入确定性 fallback 摘要；SUMMARY_PREFIX 携带精心设计的指令防止 AI 把 summary 当 active task 执行
- `agent/context_compressor.py:ContextCompressor._prune_old_tool_results` — 无 LLM 调用的廉价预处理：按 token budget 确定尾部边界，对尾部外工具结果生成语义单行摘要（'[terminal] ran npm test -> exit 0, 47 lines'），去重完全相同的工具输出，截断过长 tool_call arguments
  - 💡 JSON 结构截断而非字符串切片，防止残缺 JSON 被下游提供商 400 拒绝
- `agent/context_compressor.py:_summarize_tool_result` — 针对 20+ 种工具名称生成语义 1-liner 摘要（terminal/read_file/write_file/search_files/patch/browser_*/web_search/execute_code/memory/todo/delegate_task 等），比通用占位符携带更多语义
- `agent/context_compressor.py:ContextCompressor._generate_summary` — 调用辅助模型（auxiliary_client）生成结构化 14 段摘要（Active Task/Goal/Constraints/Completed Actions/Active State/In Progress/Blocked/Key Decisions/Resolved Questions/Pending User Asks/Relevant Files/Remaining Work/Critical Context）；支持迭代更新 vs 首次生成两条路径；支持 focus_topic 引导
  - 💡 冷却期机制（transient 30s，unknown 60s，no provider 600s）；摘要模型失败时自动 fallback 到主模型重试一次；输出经 redact_sensitive_text 过滤敏感信息
- `agent/context_compressor.py:ContextCompressor._build_static_fallback_summary` — LLM 摘要生成失败时本地确定性构建 fallback 摘要：收集 user_asks/assistant_actions/tool_actions/relevant_files/blockers/last_dropped_turns，生成和结构化摘要相同格式的内容，携带 REDACTED 处理
- `agent/context_compressor.py:ContextCompressor._sanitize_tool_pairs` — 压缩后修复孤立的 tool_call/tool_result 对：删除无对应 assistant call 的 tool result；为无对应 result 的 assistant tool_call 插入 stub result，防止 API 拒绝
- `agent/context_compressor.py:ContextCompressor._find_tail_cut_by_tokens` — 按 token 预算从尾部倒走确定压缩边界；内置 1.5x soft ceiling 防止在超大单条消息内部切割；调用 _align_boundary_backward 避开 tool_call/result 组；调用 _ensure_last_user_message_in_tail 防止最近用户消息落入压缩区
- `agent/context_compressor.py:ContextCompressor._ensure_last_user_message_in_tail` — 修复 bug #10896：align_boundary_backward 拉动边界导致最新用户消息进入压缩区，active task 从 context 中消失；将 cut_idx 拉回到最新用户消息
- `agent/context_compressor.py:_strip_historical_media` — 在所有压缩后消息中，找到最新携带图片的用户消息作为锚点，将锚点之前的所有图片 parts 替换为占位符，防止旧截图 base64 每轮都重新上传
- `agent/context_compressor.py:ContextCompressor.should_compress_preflight` — 区分 preflight 粗估（estimate_messages_tokens_rough）和 real usage（来自 API 响应的真实 prompt_tokens）；should_defer_preflight_to_real_usage 在压缩后粗估增长不多时用真实值避免重复压缩
- `agent/iteration_budget.py:IterationBudget` — 线程安全的 per-agent 迭代计数器，consume()/refund()/remaining 三个操作；parent 默认 90 次，subagent 默认 50 次；execute_code 通过 refund() 归还预算
- `agent/tool_guardrails.py:ToolCallGuardrailController` — per-turn 工具调用护栏：追踪完全相同调用的失败次数（exact_failure）、同工具不同 args 的累计失败（same_tool_failure）、幂等工具返回相同结果的次数（no_progress）；produce warn/block/halt 三级决策
- `agent/tool_guardrails.py:ToolCallGuardrailConfig` — 护栏配置数据类，支持从 config.yaml 的 tool_loop_guardrails 节读取；warn/hard_stop 两套阈值分别配置，hard_stop 默认关闭（仅 warn）
- `agent/tool_guardrails.py:ToolCallSignature` — 工具调用稳定 ID：tool_name + canonical sorted JSON args 的 SHA256 hash；用于 exact failure 计数和 no_progress 去重
- `agent/tool_guardrails.py:canonical_tool_args` — 对工具参数做 sorted key JSON 序列化，确保参数顺序不影响相同性判断
- `agent/tool_guardrails.py:classify_tool_failure` — 工具失败检测：terminal 查 exit_code，file mutation 类查 bytes_written/success；通用检测 JSON 中 error/failed 字段和 Error 前缀
- `agent/tool_guardrails.py:append_toolguard_guidance` — 将护栏警告/halt 信息追加到工具结果末尾，让 LLM 在 tool_result 中直接看到循环警告
- `agent/tool_guardrails.py:toolguard_synthetic_result` — 为被 block 的工具调用生成合成 tool result（role=tool 消息），携带 guardrail metadata
- `agent/think_scrubber.py:StreamingThinkScrubber` — 流式 reasoning block 清洗状态机：per-delta feed + flush 接口；处理跨 delta 的 partial tag 边界；支持 think/thinking/reasoning/thought/REASONING_SCRATCHPAD 5 种 tag；区分 closed pair（任意位置抑制）和 unterminated open（只在块边界抑制）
- `agent/think_scrubber.py:StreamingThinkScrubber.feed` — 核心 hot-path：判断当前是否在 block 内，识别 closed pair、boundary-gated open tag、partial-tag hold-back；返回当前 delta 可见部分
  - 💡 partial-tag hold-back：delta 末尾与任意 tag 前缀匹配时留存，下一 delta 解析；防止 delta1='<think>' 被错误截断
- `agent/error_classifier.py:classify_api_error` — 8 阶段 API 错误分类 pipeline：provider-specific → HTTP status → error_code → message patterns → SSL transient → server disconnect+large session → transport type → unknown；返回 ClassifiedError 含 reason/retryable/should_compress/should_rotate_credential/should_fallback
- `agent/error_classifier.py:FailoverReason` — 错误原因枚举：auth/auth_permanent/billing/rate_limit/overloaded/server_error/timeout/context_overflow/payload_too_large/image_too_large/model_not_found/provider_policy_blocked/content_policy_blocked/format_error/invalid_encrypted_content/multimodal_tool_content_unsupported/thinking_signature/long_context_tier/oauth_long_context_beta_forbidden/llama_cpp_grammar_pattern/unknown
- `agent/error_classifier.py:_classify_by_status/_classify_400/_classify_402` — HTTP status 到 ClassifiedError 的精细映射；402 区分 billing vs transient rate limit（有 'try again' 等信号则为 rate_limit）；400 区分 context_overflow/image_too_large/multimodal_tool_content_unsupported/format_error；500+大 session → context_overflow
- `agent/retry_utils.py:jittered_backoff` — 带 jitter 的指数退避：base * 2^(attempt-1) + uniform(0, ratio*delay)；用单调计数器 + time_ns 作种子确保并发会话退避时间分散，防止 thundering herd
- `agent/message_sanitization.py:_repair_tool_call_arguments` — 4-pass 修复 malformed tool_call arguments JSON：strict=False parse + re-serialise → 去尾逗号/补括号 → 删余括号迭代 → escape 控制字符；全部失败则返回 {}
  - 💡 防止 GLM/llama.cpp 类模型产生残缺 JSON 卡死整个会话
- `agent/message_sanitization.py:_sanitize_messages_surrogates` — 遍历消息列表中所有字符串字段（content/name/tool_calls/reasoning_content/reasoning_details 等额外字段）替换 lone surrogate，防止 OpenAI SDK json.dumps 崩溃
- `agent/message_sanitization.py:_strip_images_from_messages` — 从所有消息中移除 image_url/image/input_image parts；tool 角色消息内容完全是图片时用文本占位而非删除消息（否则 tool_call_id 悬空 400）
- `agent/message_sanitization.py:_escape_invalid_chars_in_json_strings` — 字符级扫描 JSON 原始文本，对字符串值内的控制字符（0x00-0x1F）做 \uXXXX 转义，保留已有转义不双重处理
- `agent/prompt_caching.py:apply_anthropic_cache_control` — system_and_3 缓存策略：系统消息 + 最后 3 条非系统消息各加 cache_control breakpoint（5m 或 1h TTL）；native Anthropic 对 tool 消息直接加 cache_control，OpenAI-compat 格式对 content parts 加
- `agent/context_references.py:parse_context_references / ContextReference` — 解析 @file:path @folder:path @url:url @diff @staged 引用语法，展开为系统提示补丁注入；附带敏感路径保护（.ssh/.aws/.kube 等 blocked）
- `agent/conversation_loop.py:run_conversation` — agent 主循环（~3900 行）：per-turn reset → surrogate 清洗 → 建系统提示 → model call with streaming → tool dispatch → retry/fallback/compression → 后置 hook；包含 context compression preflight 和 post-API real usage 触发
  - 💡 _tool_guardrails.reset_for_turn() 在每 turn 开始调用确保护栏计数器不跨 turn 累积；set_runtime_main 把当前模型通知给 auxiliary_client 做 per-turn 生效
- `agent/conversation_compression.py:compress_context` — 实际压缩调用链：调用 context_engine.compress → 分裂 SQLite session（生成新 session_id）→ 通知 plugin context engines → 重建系统提示（保持 cache warm）→ 返回压缩后消息和新系统提示
- `agent/conversation_compression.py:check_compression_model_feasibility` — session 启动时检查辅助压缩模型的 context window 是否能装下主模型压缩阈值对应的内容，否则降级阈值或警告
- `agent/trajectory.py:save_trajectory` — 以 ShareGPT 格式 append 对话到 JSONL 文件；completed=True 存 trajectory_samples.jsonl，False 存 failed_trajectories.jsonl；用于数据飞轮
- `agent/tool_dispatch_helpers.py:_should_parallelize_tool_batch` — 多工具批次并发性评估：NEVER_PARALLEL_TOOLS（clarify）强制顺序；PARALLEL_SAFE_TOOLS（读/搜索类）可并发；PATH_SCOPED_TOOLS 检查路径是否重叠决定并发；destructive terminal 命令强制顺序
- `agent/tool_dispatch_helpers.py:_is_destructive_command` — 正则判断 terminal 命令是否含 rm/cp/mv/sed -i/dd/git reset 等破坏性操作或 > 重定向，用于阻止并发执行
- `agent/tool_result_classification.py:file_mutation_result_landed` — 判断 write_file/patch 结果是否表示写入成功（bytes_written present / success=true），用于护栏 failure 检测，避免把成功写入误判为失败
- `agent/system_prompt.py:build_system_prompt_parts` — 系统提示三层组装：stable（identity/tools/skills/env hints）+ context（AGENTS.md/.cursorrules 等发现的上下文文件）+ volatile（memory/USER.md/timestamp）；三层分开保持 upstream cache 稳定
- `agent/prompt_builder.py:_scan_context_content` — 上下文文件注入前扫描 prompt injection / promptware / C2 patterns（用 tools/threat_patterns.py）；检测到则替换为 BLOCKED 占位符

### 多模型传输抽象/适配器/计费限速  ·  65 项

- `agent/transports/base.py:ProviderTransport` — 抽象基类：定义统一多供应商传输接口。4个必实现抽象方法：convert_messages/convert_tools/build_kwargs/normalize_response；3个可选hook：validate_response/extract_cache_stats/map_finish_reason。职责边界明确：不拥有 client 构建、streaming、credential refresh、retry——只管格式转换和响应归一化
  - 💡 职责边界设计极克制：6行注释明确列出 NOT 属于此层的8项关注点，防止职责蔓延
- `agent/transports/types.py:NormalizedResponse` — 跨供应商响应归一化数据类。共享字段(content/tool_calls/finish_reason/reasoning/usage)对所有下游可靠；provider_data 存放协议私有状态(Anthropic reasoning_details/Codex reasoning_items/Gemini thought_signature)。兼容层 property 让旧 shim 代码无需改动
  - 💡 provider_data 是开闭原则的关键：新增协议无需改共享类型，只需往 dict 里加字段
- `agent/transports/types.py:ToolCall` — 归一化工具调用。id/name/arguments 是通用字段；provider_data 存 Codex 的 call_id+response_item_id、Gemini 的 thought_signature(extra_content)。向后兼容 property：tc.function.name/tc.function.arguments 让旧调用点零改动
  - 💡 call_id、response_item_id、extra_content 都通过 property 从 provider_data 动态读取，同一个类兼容 3种协议
- `agent/transports/types.py:Usage` — 通用 token 计量数据类。prompt/completion/total/cached tokens 4个字段，所有供应商统一汇报
- `agent/transports/types.py:build_tool_call` — 工厂函数：自动序列化 arguments(dict→json string)，将 provider_fields 收集进 provider_data
- `agent/transports/types.py:map_finish_reason` — 通用 finish reason 映射辅助：供应商 stop reason 映射到归一化集合(stop/tool_calls/length/content_filter)，None 安全 fallback 到 stop
- `agent/transports/__init__.py:register_transport` — 传输注册：api_mode 字符串→传输类，支持动态注册
- `agent/transports/__init__.py:get_transport` — 传输工厂：按 api_mode 获取实例，懒发现(lazy discover)——miss 时自动触发 _discover_transports，测试中按需 import 单个 transport 也能正常 miss-then-discover
- `agent/transports/__init__.py:_discover_transports` — 自动注册所有传输模块：try-import 四个 transport 模块，import 成功触发模块末尾的 register_transport 调用
- `agent/transports/anthropic.py:AnthropicTransport` — Anthropic Messages API 传输实现。convert_messages 委托 anthropic_adapter.convert_messages_to_anthropic；normalize_response 解析 content blocks(text/thinking/tool_use)；validate_response 允许空 content 当 stop_reason==end_turn；extract_cache_stats 读 cache_read_input_tokens/cache_creation_input_tokens
- `agent/transports/chat_completions.py:ChatCompletionsTransport` — OpenAI Chat Completions 传输实现，服务 16+ 兼容供应商(OpenRouter/Nous/NVIDIA/Qwen/Ollama/DeepSeek/xAI/Kimi 等)。convert_messages 深度清洗 Codex 私有字段/Gemini extra_content/_前缀内部标记/tool_name；build_kwargs 含 provider_profile 单路径和 legacy flag 双路径；normalize_response 保留 Gemini thought_signature 进 provider_data
- `agent/transports/chat_completions.py:ChatCompletionsTransport.convert_messages` — 消息清洗：strip codex_reasoning_items/codex_message_items/tool_name/`_`-前缀内部标记；按目标模型是否为 Gemini family 决定是否保留 extra_content(thought_signature)。延迟 deepcopy：先扫描需要 sanitize 才复制，避免每次全量 deepcopy
  - 💡 extra_content 只留给 Gemini target 的保守逻辑：非 Gemini strict providers (Fireworks/Mistral) 收到此字段 HTTP 400；这种模型感知的 field stripping 是多供应商兼容的核心痛点
- `agent/transports/chat_completions.py:ChatCompletionsTransport._build_kwargs_from_profile` — 基于 ProviderProfile 构建 API kwargs 的单路径：profile.prepare_messages/build_api_kwargs_extras/build_extra_body 三步调用，覆盖 temperature/max_tokens/reasoning/extra_body 组装；native Gemini base_url 时只保留 thinking_config 其余 extra_body 全丢
- `agent/transports/chat_completions.py:_build_gemini_thinking_config` — Hermes 统一 reasoning_config 到 Gemini thinkingConfig 的映射：Gemini 2.5 只发 includeThoughts，Gemini 3 Flash 三档(low/medium/high)，Gemini 3 Pro 两档(low/high)；非 Gemini 模型返回 None 避免 HTTP 400
- `agent/transports/chat_completions.py:_model_consumes_thought_signature` — 判断目标模型是否为 Gemini family（需要 replay extra_content），用于决定是否 strip thought_signature
- `agent/transports/codex.py:ResponsesApiTransport` — OpenAI Responses API(Codex) 传输实现。_last_issuer_kind 记录最近一次 issuer(xAI/GitHub/codex.com/openai)；convert_messages 调用 _chat_messages_to_responses_input 且按 issuer 过滤 foreign-issuer reasoning blocks；normalize_response 提取 codex_reasoning_items/codex_message_items；preflight_kwargs 做调用前验证
- `agent/transports/codex.py:ResponsesApiTransport._resolve_issuer_kind` — 按 params 判断 Responses endpoint 类型(xai/github/codex-backend/openai)，用于 reasoning block 跨轮回放隔离——防止不同 provider 的 encrypted reasoning 混用导致 API 拒绝
- `agent/transports/bedrock.py:BedrockTransport` — AWS Bedrock Converse API 传输实现。sentinel keys(__bedrock_converse__/__bedrock_region__)插入 kwargs 供 dispatch site 识别并转 boto3 调用；normalize_response 兼容 raw boto3 dict 和已归一化 SimpleNamespace 两种形状
- `agent/transports/codex_app_server.py:CodexAppServerClient` — JSON-RPC 2.0 over stdio 客户端：spawn `codex app-server`子进程，one reader thread解析 stdout dispatch 到 pending future/notifications/server_requests 三条队列；用 blocking queue+timeout 而非 asyncio，与同步主循环兼容；stderr 环形缓冲(最近500行)用于诊断
- `agent/transports/codex_app_server.py:CodexAppServerClient.initialize` — initialize+initialized 二步握手；兼容不同 codex 版本 thread_id 序列化差异(id/sessionId/threadId 多路降级)
- `agent/transports/codex_app_server.py:CodexAppServerClient.request` — 同步 JSON-RPC 请求：mint id，put 到 pending map，send，blocking get with timeout；timeout 时主动移除 pending，raise TimeoutError
- `agent/transports/codex_app_server.py:CodexAppServerClient._dispatch` — 消息路由：有id+result/error→pending future；有id+method→server_requests 队列(approval)；无id→notifications 队列(streaming events)
- `agent/transports/codex_app_server.py:parse_codex_version` — 解析 `codex --version` 输出为 (major, minor, patch) tuple，用于版本门控
- `agent/transports/codex_app_server.py:check_codex_binary` — 校验 codex CLI 是否安装且满足最低版本，setup wizard 和 runtime startup 入口用
- `agent/transports/codex_app_server_session.py:CodexAppServerSession` — 单 Codex thread per Hermes session。run_turn 主循环：poll notifications/server_requests/interrupt/post-tool 静默超时 watchdog/subprocess 存活检测；TurnResult.should_retire 通知 caller 重建 session；approval bridge(_handle_server_request)映射 Hermes 策略到 codex JSON-RPC 响应
- `agent/transports/codex_app_server_session.py:CodexAppServerSession.run_turn` — 驱动一个用户→助手→工具轮次：ensure_started→turn/start→事件循环(notification+server_request交替drain)→turn/completed；post_tool_quiet_timeout watchdog：工具结束后静默N秒触发退休；<turn_aborted>标记检测：不等 turn/completed 直接终结
  - 💡 post-tool watchdog 是镜像 openclaw beta.8 的修复：codex 卡死后不会自动退出，必须有外部检测
- `agent/transports/codex_app_server_session.py:_classify_oauth_failure` — 字符串匹配判断 codex OAuth token 失效，返回 codex login 操作指引；保守匹配——不确定就原样报错，不重定向
- `agent/transports/codex_app_server_session.py:_coerce_turn_input_text` — 将 OpenAI 富内容(list of parts)压缩为 text：保留 text parts，image 用 [image attached] 占位，对 app-server turn/start text-only 接口兼容
- `agent/transports/codex_app_server_session.py:CodexAppServerSession._track_pending_file_change` — 维护 item_id→change_summary 缓存：item/started 时提取变更摘要，item/completed 时移除；供 apply_patch approval bridge 展示实际变更内容(approval params 本身不携带 changeset)
- `agent/transports/codex_event_projector.py:CodexEventProjector` — 有状态投影器：将 codex item/completed 事件序列翻译为 OpenAI-shaped messages(assistant/tool pairs)。状态：_pending_reasoning 缓存 reasoning items，贴到下一条 assistant 消息。只在 item/completed 物化消息，streaming delta 忽略
- `agent/transports/codex_event_projector.py:CodexEventProjector.project` — 路由单个 notification：agentMessage/commandExecution/fileChange/mcpToolCall/dynamicToolCall/userMessage/reasoning 各有具体投影，unknown item 投影为 opaque assistant note 保证无遗漏
- `agent/transports/codex_event_projector.py:_deterministic_call_id` — 为 tool_call 生成稳定 id：有 item_id 时用 codex_{type}_{id}，否则 sha256 内容哈希——跨 session replay 时 prefix cache 保持有效
- `agent/transports/hermes_tools_mcp_server.py:_build_server` — 将 Hermes tool 表面的一个子集(web_search/browser/vision/kanban 等)通过 FastMCP stdio 协议暴露给 codex app-server 子进程；按 EXPOSED_TOOLS 白名单过滤，_AGENT_LOOP_TOOLS(delegate_task/memory 等需要 AIAgent 上下文的工具) 不暴露
- `agent/transports/hermes_tools_mcp_server.py:main` — MCP server 入口：设置 HERMES_QUIET/HERMES_REDACT_SECRETS 环境变量，FastMCP stdio transport 运行
- `agent/model_metadata.py:get_model_context_length` — 10 步优先级 context length 解析：config_override→cache→Bedrock static→endpoint /models→local server→Anthropic API→provider-aware(Copilot/Nous/Codex OAuth/GMI/Ollama/models.dev)→OpenRouter→hardcoded defaults→local last resort→256K fallback
  - 💡 逐层 bypass 和 cache invalidation 设计极细：Codex OAuth entry>=400K 认为是旧API值要失效；Kimi <=32K 认为是 OpenRouter 少报要失效；Nous 只持久化 portal 来源不持久化 OR fallback——每层都有理由
- `agent/model_metadata.py:fetch_model_metadata` — 从 OpenRouter API 拉取全量模型 metadata(含 context_length/pricing/max_completion_tokens)，内存缓存 1小时，_add_model_aliases 同时建 bare model 和 provider/model 两个索引
- `agent/model_metadata.py:fetch_endpoint_model_metadata` — 从自定义 endpoint 的 /models 拉取 metadata，LM Studio 走 /api/v1/models 特殊分支，llama.cpp 额外查 /v1/props 获取运行时实际 context；内存缓存 5min per base_url
- `agent/model_metadata.py:_resolve_codex_oauth_context_length` — Codex OAuth context 解析：优先 live probe chatgpt.com/backend-api/codex/models，fallback 到硬编码表(最长 key 优先匹配)；Codex OAuth 限制比直接 API 小(gpt-5.5 直接API=1.05M，Codex=272K)
- `agent/model_metadata.py:detect_local_server_type` — 通过 probe 探测本地 server 类型(ollama/lm-studio/vllm/llamacpp)：按特定 endpoint 指纹区分，结果供 context_length 解析分支使用
- `agent/model_metadata.py:_infer_provider_from_url` — 从 base_url hostname 推断 provider 名，_URL_TO_PROVIDER 映射表 + auto-extend from ProviderProfile 注册表
- `agent/model_metadata.py:parse_context_limit_from_error` — 从 API 错误消息文本里 regex 提取 context limit 数字，用于错误恢复时降档
- `agent/model_metadata.py:estimate_tokens_rough` — 4 chars/token 粗估；ceiling division 避免短文本估为 0
- `agent/model_metadata.py:estimate_request_tokens_rough` — 全请求 token 粗估：system_prompt + messages(images 按每张 1500 token flat cost) + tools schema；50+ 工具时 schema 本身 20-30K token
- `agent/usage_pricing.py:CanonicalUsage` — 跨供应商统一 token 计量数据类：input/output/cache_read/cache_write/reasoning/request_count；computed properties：prompt_tokens(=input+cache_read+cache_write)、total_tokens
- `agent/usage_pricing.py:PricingEntry` — 定价条目：input/output/cache_read/cache_write/request 每百万 token 成本(Decimal)；source(来源类型)/source_url/pricing_version/fetched_at 可观测性字段
- `agent/usage_pricing.py:CostResult` — 费用估算结果：amount_usd/status(actual/estimated/included/unknown)/source/label/notes；status 区分是否可信
- `agent/usage_pricing.py:resolve_billing_route` — 从 model_name+provider+base_url 解析计费路由(BillingRoute)：识别 subscription_included(Codex OAuth)/official_models_api(OpenRouter)/official_docs_snapshot(Anthropic/OpenAI)/unknown
- `agent/usage_pricing.py:normalize_usage` — 跨 API shape 的 usage 归一化：Anthropic(input_tokens+cache_read_input_tokens)、Codex Responses(input_tokens包含cache需减去 details)、Chat Completions(prompt_tokens+prompt_tokens_details+Anthropic fallback fields)三路分支
- `agent/usage_pricing.py:estimate_usage_cost` — 计算一次 API 调用的估算成本：resolve_billing_route→get_pricing_entry→按 input/output/cache_read/cache_write/request 计量费用；subscription_included 路由直接返回 included 状态
- `agent/usage_pricing.py:get_pricing_entry` — 三层定价查询：subscription→OpenRouter 实时 API→endpoint /models→官方文档快照表
- `agent/usage_pricing.py:_OFFICIAL_DOCS_PRICING` — 官方文档定价快照：(provider, model) → PricingEntry，覆盖 Anthropic/OpenAI/DeepSeek/Google/Bedrock/MiniMax 主流模型；Decimal 精度，避免浮点计费误差
- `agent/rate_limit_tracker.py:RateLimitState` — 完整 rate limit 状态：requests/tokens × per-minute/per-hour 四桶，parse 自 x-ratelimit-* 响应头
- `agent/rate_limit_tracker.py:RateLimitBucket` — 单限速桶：limit/remaining/reset_seconds；computed properties：used/usage_pct/remaining_seconds_now(带时间流逝修正)
- `agent/rate_limit_tracker.py:parse_rate_limit_headers` — 解析 x-ratelimit-* HTTP headers 到 RateLimitState，case-insensitive；无限速头返回 None
- `agent/rate_limit_tracker.py:format_rate_limit_display` — 格式化 rate limit 状态为终端/聊天可读文本，含 ASCII 进度条和超 80% 警告
- `agent/nous_rate_guard.py:record_nous_rate_limit` — 记录 Nous Portal 429 状态到共享文件(atomic write)；reset 时间解析优先级：x-ratelimit-reset-requests-1h > x-ratelimit-reset-requests > retry-after；fallback 300s cooldown
- `agent/nous_rate_guard.py:nous_rate_limit_remaining` — 检查 Nous Portal 是否在限速冷却中，过期自动清理文件
- `agent/nous_rate_guard.py:is_genuine_nous_rate_limit` — 区分真实账号限速(remaining==0 且 reset>=60s)与上游供应商瞬时容量不足(不应触发全局熔断)；用于防止 DeepSeek 429 触发熔断后阻塞同 Nous 下 Kimi/MiMo 等其他模型
- `agent/account_usage.py:fetch_account_usage` — 按 provider 分派：openai-codex→_fetch_codex_account_usage；anthropic→_fetch_anthropic_account_usage；openrouter→_fetch_openrouter_account_usage
- `agent/account_usage.py:_fetch_codex_account_usage` — 从 chatgpt.com/backend-api/wham/usage 拉取 Codex OAuth 账号用量：Session/Weekly 两个时间窗口 used_percent，credits 余额
- `agent/account_usage.py:_fetch_anthropic_account_usage` — 从 Anthropic /api/oauth/usage 拉取 OAuth 账号用量：five_hour/seven_day 等窗口 utilization；仅 OAuth token 可用，普通 API key 返回 unavailable_reason
- `agent/account_usage.py:_fetch_openrouter_account_usage` — 从 OpenRouter /credits + /key 拉取余额和 key 配额使用情况：daily/weekly/monthly breakdown
- `agent/account_usage.py:AccountUsageSnapshot` — 账号用量快照数据类：provider/source/fetched_at/plan/windows(list of AccountUsageWindow)/details/unavailable_reason
- `agent/models_dev.py:ModelInfo` — models.dev 单模型完整 metadata 数据类：id/name/family/provider_id；capabilities(reasoning/tool_call/attachment/temperature/structured_output/open_weights)；modalities；limits(context_window/max_output/max_input)；cost(input/output/cache_read/cache_write per million USD)；knowledge_cutoff/release_date
- `agent/codex_runtime.py:run_codex_app_server_turn` — Codex app-server runtime 入口：懒建 CodexAppServerSession(per AIAgent)；调 session.run_turn；TurnResult.should_retire 触发退休重建；异常时强制关闭 session 确保下次重建

### 记忆/技能注入/凭证池/curator  ·  76 项

- `agent/memory_manager.py:sanitize_context` — 用正则去掉 <memory-context> fence 标签和 [System note:...] 块，防止 provider 注入的标签泄漏到 UI
  - 💡 同时消除三类污染：fence 块整体、standalone system-note 行、孤立 fence 标签
- `agent/memory_manager.py:StreamingContextScrubber` — 有状态状态机，流式 scrub 跨 chunk 的 <memory-context> span，防止流式输出时标签在 chunk 边界上被分割导致泄漏
  - 💡 _find_boundary_open_tag 只匹配行首 open tag，避免正文中偶发 <memory-context> 字符串误触发
- `agent/memory_manager.py:StreamingContextScrubber.feed` — 每次接受一个 streaming chunk，返回可见部分；内部用 _buf 缓冲可能是标签前缀的尾巴
- `agent/memory_manager.py:StreamingContextScrubber.flush` — stream 结束时 flush 缓冲；若仍在 span 内则丢弃（防止泄漏），否则原样输出
- `agent/memory_manager.py:build_memory_context_block` — 把 prefetch 拿回的上下文包裹进 <memory-context> fence，附加 [System note:...] 声明'这是记忆不是用户输入'
- `agent/memory_manager.py:MemoryManager` — 管理 built-in + 最多一个 external provider；汇聚 system prompt 块、prefetch、sync、tool routing、生命周期 hook
  - 💡 强制 one-external-provider 限制，防止工具名冲突和 schema 膨胀
- `agent/memory_manager.py:MemoryManager.add_provider` — 注册 provider 并构建 tool_name→provider 路由表；第二个 external provider 被拒绝并发出 warning
- `agent/memory_manager.py:MemoryManager.prefetch_all` — 对所有 provider 调用 prefetch()，合并结果；单个 provider 失败不阻塞其他
- `agent/memory_manager.py:MemoryManager.sync_all` — turn 完成后向所有 provider 写入；用 inspect.signature 检测 provider 是否接受 messages 参数，兼容旧接口
- `agent/memory_manager.py:MemoryManager.on_session_switch` — session_id 轮换（/resume /branch /reset /new，含 context compression）时通知所有 provider；rewound=True 仅在 /undo 时传入
- `agent/memory_manager.py:MemoryManager.on_pre_compress` — context compression 前回调，收集 provider 提取的摘要注入压缩 prompt，防止关键记忆被压缩丢失
- `agent/memory_manager.py:MemoryManager.on_memory_write` — built-in memory tool 写入时通知 external provider，让第三方 backend 镜像写操作；用 inspect.signature 检测 metadata 参数兼容性
- `agent/memory_manager.py:MemoryManager.on_delegation` — 父 agent 中，子 agent 完成时回调，把 task+result 传给 memory provider 作为观测数据
- `agent/memory_manager.py:MemoryManager.initialize_all` — 启动所有 provider，自动注入 hermes_home，避免 provider 自己 import get_hermes_home
- `agent/memory_provider.py:MemoryProvider` — 抽象基类，定义 memory provider 接口：initialize / prefetch / queue_prefetch / sync_turn / get_tool_schemas / handle_tool_call / shutdown + 可选 hook
  - 💡 queue_prefetch 分离 '触发预取' 与 '读取结果'，允许 provider 在 turn 间隔异步拉数据
- `agent/memory_provider.py:MemoryProvider.get_config_schema` — 返回 provider 需要的配置字段列表，供 hermes memory setup wizard 引导用户配置；字段支持 secret/required/choices/env_var/url
- `agent/curator.py:load_state / save_state` — 读写 .curator_state JSON 文件；save_state 用 tmp+fsync+rename 原子写，防止写到一半崩溃
- `agent/curator.py:should_run_now` — 检查 enabled/paused/interval 决定是否触发 curator；首次运行时 seed last_run_at 为当前时间并推迟一个完整周期，不在 update 后立即运行
- `agent/curator.py:apply_automatic_transitions` — 纯函数（不调用 LLM）：按 last_activity_at 时间戳将 skill 状态机推进（active→stale→archived）；pinned skill 不受影响
- `agent/curator.py:run_curator_review` — 完整的 curator 运行流程：pre-mutation snapshot → apply_automatic_transitions → spawn AIAgent fork 做 LLM pass → 写 REPORT.md/run.json → 更新 cron 引用
  - 💡 默认 daemon thread 异步执行，干运行(dry_run)不改变 last_run_at，不修改 skill 库
- `agent/curator.py:_run_llm_review` — spawn AIAgent fork 执行 curator prompt，redirect stdout/stderr 到 /dev/null 防止污染终端；collect tool_calls 用于后续分类
- `agent/curator.py:_classify_removed_skills` — 分析 tool calls 判断被删 skill 是 consolidated（吸收进 umbrella）还是 pruned（因陈旧归档）；file_path 用 component 匹配，内容字段用 word-boundary regex
- `agent/curator.py:_parse_structured_summary` — 从 LLM final response 解析 ```yaml 块中的 consolidations/prunings 列表，容错处理 missing/malformed
- `agent/curator.py:_extract_absorbed_into_declarations` — 从 tool calls 提取 skill_manage(action=delete, absorbed_into=...) 的模型声明，作为最权威的分类信号
- `agent/curator.py:_reconcile_classification` — 三路信号优先级合并：model-declared absorbed_into > model YAML block > tool-call heuristic；模型幻构不存在的 umbrella 时降级到 heuristic
- `agent/curator.py:_build_rename_summary` — 生成用户可见的 '你的 skill 去哪了' 摘要（最多展示 10 条）；consolidated 显示 old→new，pruned 显示 pruned(stale)
- `agent/curator.py:_write_run_report` — 在 logs/curator/{timestamp}/ 写 run.json（机器可读完整数据）+ REPORT.md（人可读）+ cron_rewrites.json
- `agent/curator.py:maybe_run_curator` — 会话启动钩子：best-effort 检查 idle 时间 + 间隔 gate，决定是否触发 curator；永不 raise
- `agent/insights.py:InsightsEngine` — 查询 sessions/messages SQLite 表，生成 token 消耗/成本/工具使用/skill 使用/活跃时段 报告
- `agent/insights.py:InsightsEngine.generate` — 主入口：按天数窗口拉取所有维度数据并汇总
- `agent/insights.py:InsightsEngine._get_tool_usage` — 双源合并工具调用计数：1）messages.tool_name 列（gateway 设置）2）assistant messages 的 tool_calls JSON；取两者最大值避免重复计数
- `agent/insights.py:InsightsEngine._get_skill_usage` — 从 assistant tool_calls 提取 skill_view/skill_manage 调用，按 skill name 聚合 view_count/manage_count/last_used_at
- `agent/insights.py:InsightsEngine._compute_activity_patterns` — 分析 by_day/by_hour 分布，计算 busiest_day/busiest_hour/active_days/max_streak(连续天数)
- `agent/insights.py:InsightsEngine.format_terminal` — 将报告渲染为对齐的 box-drawing 终端 UI，含 bar chart（_bar_chart 用 █ 填充）
- `agent/skill_bundles.py:scan_bundles / get_skill_bundles` — 扫描 skill-bundles/*.yaml，构建 /slug→bundle_info 映射；用 mtime 检查缓存新鲜度（dir mtime 覆盖删除，file mtime 覆盖编辑）
- `agent/skill_bundles.py:build_bundle_invocation_message` — 加载 bundle 引用的所有 skill，拼接成单条 user message 注入；缺失 skill 仅警告不阻断；调用 bump_use 追踪使用
- `agent/skill_bundles.py:save_bundle / delete_bundle` — bundle CRUD：写 yaml 并 scan_bundles() 刷缓存
- `agent/skill_commands.py:scan_skill_commands` — 递归扫描 ~/.hermes/skills/ 及 external_dirs，读取 SKILL.md frontmatter，构建 /cmd-name→skill_info 映射；过滤 platform/environment/disabled
- `agent/skill_commands.py:_load_skill_payload` — 按 name 或绝对路径加载 skill；先做 lexical 路径归一化再 resolve symlink，防止 symlink skill 被误拒
- `agent/skill_commands.py:_build_skill_message` — 格式化 skill 注入 message：template var 替换 → inline shell 展开 → 附加 skill dir → 注入 config vars → 列出 supporting files（references/templates/scripts/assets）
- `agent/skill_commands.py:_inject_skill_config` — 从 frontmatter metadata.hermes.config 读取 skill 声明的 config 变量，解析当前值注入 [Skill config:...] 块
- `agent/skill_commands.py:build_skill_invocation_message` — slash 命令触发时调用：load payload → bump_use → build message；加 activation_note 告知模型'用户显式激活了这个 skill'
- `agent/skill_commands.py:build_preloaded_skills_prompt` — CLI -s 预加载多个 skill，返回 (prompt_text, loaded_names, missing)
- `agent/skill_commands.py:reload_skills` — 热重载 skill 目录，返回 added/removed/unchanged diff；不失效 system prompt cache（保留 prefix cache）
- `agent/skill_preprocessing.py:substitute_template_vars` — 替换 ${HERMES_SKILL_DIR} / ${HERMES_SESSION_ID} token；未解析的 token 原样保留供作者 debug
- `agent/skill_preprocessing.py:expand_inline_shell` — 将 SKILL.md 中 !`cmd` 展开为 shell 执行结果；以 skill dir 为 cwd；输出截断到 4000 chars
- `agent/skill_preprocessing.py:run_inline_shell` — 单条 bash -c 执行，超时/FileNotFound/RuntimeError 均返回 [inline-shell error:...] 标记而非 raise
- `agent/skill_utils.py:parse_frontmatter` — 从 SKILL.md 解析 YAML frontmatter；优先 CSafeLoader，降级到 key:value 简单分割（宽容错误 YAML）
- `agent/skill_utils.py:skill_matches_platform / skill_matches_environment` — frontmatter 中 platforms:/environments: 字段过滤；platform 支持 Termux/Android 兼容；environment 支持 kanban/docker/s6 探测
- `agent/skill_utils.py:get_external_skills_dirs` — 读 skills.external_dirs 配置，返回验证后的外部 skill 目录列表；用 mtime-keyed 进程内缓存避免重复 YAML 解析
- `agent/skill_utils.py:extract_skill_config_vars / resolve_skill_config_values` — 从 frontmatter 提取 skills.config.* 变量声明，从 config.yaml dotpath 解析当前值；展开 ~ 路径
- `agent/skill_utils.py:iter_skill_index_files` — os.walk 遍历 skill 目录，排除 VCS/.venv/cache 目录，返回排序后的 SKILL.md 路径
- `agent/credential_pool.py:PooledCredential` — dataclass 表示单个凭证条目：provider/auth_type/status/error 信息/request_count/extra；runtime_api_key 属性按 provider 路由（Nous 需要 NAS invoke JWT）
- `agent/credential_pool.py:PooledCredential.from_dict / to_dict` — 从 JSON 反序列化/序列化；to_dict 调用 sanitize_borrowed_credential_payload 防止 env key 等临时凭证写到 auth.json
- `agent/credential_pool.py:CredentialPool` — 线程安全（_lock）的多账号凭证池：支持 fill_first/round_robin/random/least_used 四种选取策略；维护 exhaustion cooldown 状态
- `agent/credential_pool.py:CredentialPool.select` — 按策略选取可用凭证，自动清过期的 exhaustion 状态，自动刷新 OAuth token
- `agent/credential_pool.py:CredentialPool.mark_exhausted_and_rotate` — 标记当前凭证为 exhausted/dead，轮换到下一个；dead 状态用于 terminal auth failure（token_invalidated/revoked），不参与 TTL 恢复
- `agent/credential_pool.py:CredentialPool.acquire_lease / release_lease` — 并发软租约：跟踪 active_leases 计数，prefer 租约数最少的凭证，超过 max_concurrent 软上限时仍选最少的而非阻塞
- `agent/credential_pool.py:CredentialPool._refresh_entry` — per-provider OAuth token 刷新（anthropic/openai-codex/xai-oauth/nous）；含竞争条件处理：发现 auth.json 已被别的进程刷新则 adopt 新 token
- `agent/credential_pool.py:CredentialPool._sync_*_entry_from_*` — 四个同步方法：从 ~/.claude/.credentials.json / auth.json 检测外部进程刷新的 token 并 adopt，防止 single-use refresh token 冲突
- `agent/credential_pool.py:CredentialPool._sync_device_code_entry_to_auth_store` — pool-level 刷新后把新 token 写回 auth.json singleton；set_active=False 防止静默切换 active_provider
- `agent/credential_pool.py:load_pool` — 主入口：从 auth.json 读取 → _seed_from_singletons → _seed_from_env → _prune_stale_seeded → 按策略 normalize 优先级 → 有变化时写回
- `agent/credential_pool.py:_seed_from_singletons` — 从各 provider 的 singleton 凭证文件（claude/.credentials.json/auth.json/qwen-cli/copilot gh cli）注入 pool
- `agent/credential_pool.py:_seed_from_env` — 从 ~/.hermes/.env 优先（再 os.environ）读取 API key 注入 pool；支持 suppression gate 防止 hermes auth remove 后自动复活
- `agent/credential_pool.py:_exhausted_ttl / _parse_absolute_timestamp / _extract_retry_delay_seconds` — 解析 provider 返回的 reset_at（支持 epoch/ISO/ms）和 error message（quotaResetDelay/retry after N sec/Resets in Xhr Ymin）计算 cooldown 时长
- `agent/credential_pool.py:get_pool_strategy` — 从 config.yaml credential_pool_strategies.<provider> 读取选取策略
- `agent/credential_pool.py:get_custom_provider_pool_key` — 按 base_url 或 name 匹配 custom_providers 配置，返回 'custom:<name>' pool key；优先 name 匹配解决多 provider 共享 base_url 的歧义
- `agent/credential_sources.py:RemovalResult / RemovalStep` — 凭证移除合同：RemovalStep 注册 provider+source+remove_fn；find_removal_step 路由；统一 cleaned/hints/suppress 三步契约
- `agent/credential_sources.py:_register_all_sources` — 注册所有凭证来源的移除步骤（gh_cli/env/claude_code/hermes_pkce/nous/openai-codex/xai-oauth/qwen/minimax/custom）；顺序有意：provider-specific 在 wildcard 前
- `agent/credential_persistence.py:is_borrowed_credential_source` — 判断凭证是否 '借用' 的（env/claude_code/gh_cli 等）—— 仅 Hermes 自有的 OAuth token 才持久化到 auth.json
- `agent/credential_persistence.py:sanitize_borrowed_credential_payload` — 磁盘写入边界：borrowed 凭证写盘前剥去 access_token/api_key 等秘密字段，只保留 fingerprint+元数据
- `agent/credential_persistence.py:_credential_secret_fingerprint` — 计算凭证值的 sha256[:16] fingerprint，用于判断 env 变量是否与 pool 中存储的是同一值
- `agent/secret_sources/bitwarden.py:fetch_bitwarden_secrets` — 调用 bws CLI 拉取 BSM secrets；两层缓存（in-process dict + 磁盘 bws_cache.json，mode 0600 原子写）
- `agent/secret_sources/bitwarden.py:apply_bitwarden_secrets` — load_hermes_dotenv 钩子：从 BSM 拉取并 set os.environ；防止 bootstrap token 被 BSM 中的同名变量覆盖
- `agent/secret_sources/bitwarden.py:install_bws` — 懒加载安装 pinned bws 二进制：下载 zip + checksum 文件 → SHA-256 验证 → 解压 → chmod +x → 原子 rename 到 hermes_home/bin/bws
- `agent/onboarding.py:busy_input_hint_* / tool_progress_hint_*` — 上下文触发式一次性 onboarding hint，首次遇到行为分叉时展示（如首次消息 while agent busy），通过 config.yaml onboarding.seen.<flag> 追踪是否已展示

### LSP集成 / 多模态能力注册表 / 安全护栏  ·  90 项

- `agent/lsp/__init__.py:get_service` — 进程全局 LSPService 单例访问器，懒加载 + 双重检查锁，首次创建后注册 atexit 清理钩子
  - 💡 atexit 而非信号钩子：SIGKILL 下子进程由内核回收，只需清理正常退出路径
- `agent/lsp/__init__.py:shutdown_service` — 幂等拆除 LSP 服务，安全多次调用
- `agent/lsp/servers.py:SpawnSpec` — ServerDef.resolve() 的返回值 dataclass：command/workspace_root/cwd/env/initialization_options/seed_diagnostics_on_first_push
- `agent/lsp/servers.py:ServerDef` — 一个语言服务器的完整描述符：extensions 元组 + resolve_root 回调 + build_spawn 回调，matches() 方法做扩展名匹配
  - 💡 resolve_root 和 build_spawn 都是可调用字段而非方法，支持 lambda 注册（见 gleam）
- `agent/lsp/servers.py:ServerContext` — 传给 build_spawn 的上下文：workspace_root + install_strategy + binary_overrides + env_overrides + init_overrides
- `agent/lsp/servers.py:find_server_for_file` — 线性扫 SERVERS 注册表找第一个 matches() 的服务器
- `agent/lsp/servers.py:language_id_for` — 把文件路径映射到 LSP languageId 字符串（用于 textDocument/didOpen）
- `agent/lsp/servers.py:_root_or_workspace` — nearest_root + exclude 两路合并：找到 exclude 优先返回 None（禁用服务器），找不到 marker 则退到 workspace 根
- `agent/lsp/servers.py:_spawn_pyright` — Pyright 语言服务器 SpawnSpec 构建器：自动探测 venv interpreter 路径注入 initializationOptions
- `agent/lsp/servers.py:_detect_python` — 从 VIRTUAL_ENV env、.venv/venv 目录依次探 python 可执行文件
- `agent/lsp/servers.py:_resolve_override` — 读取用户在 servers_cfg 中配置的二进制路径覆盖
- `agent/lsp/servers.py:_spawn_typescript/_spawn_gopls/_spawn_rust_analyzer/…` — 各语言服务器的 SpawnSpec 构建器（共 23 个），统一模式：resolve_override → which → try_install
- `agent/lsp/manager.py:_BackgroundLoop` — 在 daemon 线程中跑一个 asyncio 事件循环，提供 run(coro, timeout) 让同步调用者阻塞等待异步结果
- `agent/lsp/manager.py:LSPService` — 进程级 LSP 服务：管理 (server_id, workspace_root) → LSPClient 映射、broken-set（失败后跳过）、delta_baseline（写前快照）
- `agent/lsp/manager.py:LSPService.create_from_config` — 从 hermes_cli.config 读取 lsp.* 配置构造服务实例
- `agent/lsp/manager.py:LSPService.enabled_for` — 三重门控：全局开关 + 扩展名匹配 + broken-set 短路
- `agent/lsp/manager.py:LSPService.snapshot_baseline` — 写前调用：快照当前诊断作为 delta 基线，失败则进 broken-set
- `agent/lsp/manager.py:LSPService.get_diagnostics_sync` — 同步获取诊断，支持 delta 过滤（与基线做差）和 line_shift 行号重映射
  - 💡 delta 过滤后还会 roll forward baseline（下次 delta 相对最新状态）
- `agent/lsp/manager.py:LSPService._mark_broken_for_file` — 超时/异常后把 (server_id, workspace_root) 进 broken-set，并 kill 残留进程，首次 WARNING 后续 DEBUG
- `agent/lsp/manager.py:LSPService._get_or_spawn` — 核心异步调度：查缓存 → 等 spawning future → 新建 LSPClient.start()，并发请求合并到同一 spawning future
  - 💡 spawning dict 防止并发为同一 key 重复 spawn，类似 Promise 合并
- `agent/lsp/manager.py:LSPService.get_status` — 返回服务快照供 hermes lsp status CLI 展示
- `agent/lsp/manager.py:_diag_key` — 诊断内容相等性 key（severity+code+source+message+range），跨编辑 delta 去重用
- `agent/lsp/client.py:LSPClient` — 单个语言服务器进程的异步客户端：spawn + initialize 握手 + textDocument 同步 + publishDiagnostics 接收 + shutdown
- `agent/lsp/client.py:LSPClient.start` — spawn 进程 + initialize 握手状态机，失败置 error 状态
- `agent/lsp/client.py:LSPClient._spawn` — asyncio.create_subprocess_exec 启动 LS，创建 stderr drain task 防止 pipe buffer 满
- `agent/lsp/client.py:LSPClient._initialize` — 发送 initialize request（含 capabilities 声明）→ initialized 通知 → 可选 didChangeConfiguration
  - 💡 整文档 sync：声明 incremental 能力但发整文档替换，节省 range 书签管理（OpenCode 同样策略）
- `agent/lsp/client.py:LSPClient._reader_loop` — 从 stdout 持续读 Content-Length 帧，分发到 _dispatch_response/_dispatch_request/_dispatch_notification
- `agent/lsp/client.py:LSPClient.open_file` — 首次 didOpen（含 CREATED 事件）/ 后续 didChange（含 CHANGED 事件）+ version 递增
- `agent/lsp/client.py:LSPClient.wait_for_diagnostics` — 并发 pull（textDocument/diagnostic）+ push 等待，预算 5s/10s，best-effort 超时静默返回
- `agent/lsp/client.py:LSPClient._wait_for_fresh_push` — 单调计数器 + asyncio.Event 等待 publishDiagnostics，含 150ms debounce（TS 常连发两次）
- `agent/lsp/client.py:LSPClient._send_request_with_retry` — ContentModified(-32801) 指数退避重试最多 3 次（0.5s/1s/2s），匹配 Claude Code 策略
- `agent/lsp/client.py:LSPClient.diagnostics_for` — 合并 push + pull 两路诊断并按 _diagnostic_key 去重
- `agent/lsp/client.py:LSPClient.shutdown` — 优雅关闭：shutdown request + exit 通知 + SIGTERM + 等 1s + SIGKILL，幂等
- `agent/lsp/client.py:LSPClient._handle_workspace_configuration` — 响应服务器反查配置请求，从 init_options 中按点分路径提取
- `agent/lsp/client.py:LSPClient._handle_publish_diagnostics` — 存储 push 诊断，seed_first_push 时首条静默不触发 event（避免 TS 初始化推送提前解锁等待）
- `agent/lsp/client.py:file_uri / uri_to_path` — file:// URI 与绝对路径互转，处理 Windows 驱动器字母
- `agent/lsp/client.py:_dedupe / _diagnostic_key` — 诊断去重 key：severity+code+source+message+range，跨 push/pull 两路合并
- `agent/lsp/protocol.py:encode_message / read_message` — Content-Length 帧编解码；read_message 防御：8KiB header 上限 + 64MiB body 上限
- `agent/lsp/protocol.py:classify_message` — 按 id/method/result/error 字段组合识别 request/response/notification/invalid
- `agent/lsp/protocol.py:make_request/make_notification/make_response/make_error_response` — JSON-RPC 2.0 信封构造器
- `agent/lsp/workspace.py:find_git_worktree` — 从起始路径向上找 .git（file 或 dir），64层上限防循环，带 cache
- `agent/lsp/workspace.py:nearest_root` — 向上找 marker 文件，exclude 优先（Deno/TypeScript 互斥），ceiling 参数限制搜索范围
- `agent/lsp/workspace.py:resolve_workspace_for_file` — 两路锚定：优先用 cwd 的 git worktree，回退用文件自身位置，返回 (root, gated_in)
- `agent/lsp/workspace.py:is_inside_workspace` — 路径包含关系检测，用 commonpath 处理大小写不敏感文件系统
- `agent/lsp/install.py:try_install` — 按包名找/安装 LSP 服务器二进制，per-package 锁防并发重复安装，结果缓存
- `agent/lsp/install.py:_install_npm / _install_go / _install_pip` — 三路安装器，统一安装到 ~/.hermes/lsp/bin/ 避免污染系统工具链
- `agent/lsp/install.py:hermes_lsp_bin_dir` — 返回 $HERMES_HOME/lsp/bin/ 隔离目录
- `agent/lsp/install.py:detect_status` — 无副作用探测包状态：installed/missing/manual-only
- `agent/lsp/range_shift.py:build_line_shift` — 用 difflib.SequenceMatcher 从 pre/post 文本构建行号重映射函数，deleted 区域返回 None
- `agent/lsp/range_shift.py:shift_diagnostic_range` — 把单条诊断的 start/end 行通过 shift 函数重映射，start 落 deleted 则整条 drop
- `agent/lsp/range_shift.py:shift_baseline` — 批量重映射基线诊断列表，drop deleted 的条目
- `agent/lsp/reporter.py:format_diagnostic` — 单条诊断格式化为 'ERROR [line:col] message [code] (source)' 一行
- `agent/lsp/reporter.py:report_for_file` — 构造 <diagnostics file=...> XML 块，按 severity 过滤，上限 20 条
- `agent/lsp/reporter.py:truncate` — 4000 字符硬截断，加 '…[truncated]' 标记
- `agent/lsp/eventlog.py:log_active/log_diagnostics/log_timeout/log_server_error/log_spawn_failed` — 结构化 LSP 事件日志，带 once-per-key 去重（避免大量相同文件刷警告）
- `agent/lsp/eventlog.py:_announce_once` — 线程安全 once-per-key 发射逻辑，基于 set + lock
- `agent/browser_provider.py:BrowserProvider` — 云浏览器后端 ABC：name/is_available()/create_session()/close_session()/emergency_cleanup()，兼容旧版 is_configured()/provider_name() 别名
- `agent/browser_registry.py:register_provider` — 注册云浏览器 provider，同名覆盖 + debug 日志，类型验证
- `agent/browser_registry.py:_resolve` — 按 configured → legacy preference(browser-use→browserbase) 三段式解析活跃 provider，is_available() 异常不杀解析
- `agent/image_gen_provider.py:ImageGenProvider` — 图像生成后端 ABC：name/is_available/list_models/get_setup_schema/generate()，统一 success_response/error_response 封装
- `agent/image_gen_provider.py:save_b64_image / save_url_image` — base64 解码/URL 下载图像到 $HERMES_HOME/cache/images/，有 25MB 大小限制，防 content-type 伪装
- `agent/image_gen_provider.py:success_response / error_response` — 统一图像生成响应结构，success/image/model/prompt/aspect_ratio/provider 字段
- `agent/image_gen_registry.py:get_active_provider` — 三段回退：explicit config → 单注册 provider → legacy FAL 优先
- `agent/image_routing.py:decide_image_input_mode` — 决定本轮图像输入模式（native/text）：explicit config → aux vision override → models.dev 能力查询
- `agent/image_routing.py:extract_image_refs` — 从自由文本扫描本地路径和 http(s) 图像 URL，跳过代码块内引用
- `agent/image_routing.py:build_native_content_parts` — 构造 OpenAI-style content parts 列表（text + image_url）并追加 hint 行让模型能引用路径
- `agent/image_routing.py:_sniff_mime_from_bytes` — 从 magic bytes 嗅探 MIME（PNG/JPEG/GIF/WEBP/BMP/HEIC）防 Content-Type 欺骗
- `agent/image_routing.py:_supports_vision_override` — 从 config.yaml 按 model.supports_vision → providers.<p>.models.<m>.supports_vision 解析用户声明的视觉能力
- `agent/web_search_provider.py:WebSearchProvider` — 网络搜索/提取后端 ABC：name/is_available/supports_search/supports_extract/search/extract，单类可同时实现两种能力
- `agent/web_search_registry.py:get_active_search_provider / get_active_extract_provider` — 按 search_backend/extract_backend/backend 三层配置 + capability filter 解析活跃 provider
- `agent/web_search_registry.py:_resolve` — 5 阶段解析：explicit config → single-eligible → legacy preference walk（firecrawl→parallel→tavily→exa→searxng→brave-free→ddgs）
- `agent/tts_provider.py:TTSProvider` — TTS 后端 ABC：name/is_available/list_voices/list_models/synthesize()/stream()，voice_compatible 标记
- `agent/tts_provider.py:TTSProvider.stream` — 可选流式合成，不实现则 NotImplementedError，dispatcher 降级到 synthesize
- `agent/tts_registry.py:register_provider` — 注册 TTS provider，拒绝内置名（edge/openai/elevenlabs/…）防覆盖，built-ins-always-win 双重强制
- `agent/transcription_provider.py:TranscriptionProvider` — STT 后端 ABC：name/is_available/list_models/transcribe()，response 字典 {success,transcript,provider,error}
- `agent/transcription_registry.py:register_provider` — 注册 STT provider，拒绝内置名（local/groq/openai/mistral/xai/local_command）
- `agent/video_gen_provider.py:VideoGenProvider` — 视频生成后端 ABC：支持 text-to-video 和 image-to-video 统一 generate()，capabilities() 声明模态/分辨率/时长
- `agent/video_gen_provider.py:save_b64_video / save_bytes_video` — 视频字节写到 $HERMES_HOME/cache/videos/
- `agent/video_gen_registry.py:get_active_provider` — 两段回退：explicit config → 单注册 provider（无 legacy preference，video 是新能力）
- `agent/file_safety.py:is_write_denied` — 写操作拦截：SSH key/shell rc/hermes .env/auth.json/mcp-tokens 等敏感路径 + 可选 HERMES_WRITE_SAFE_ROOT 沙箱根
- `agent/file_safety.py:get_read_block_error` — 读操作拦截：hermes 内部 cache/auth.json/.anthropic_oauth.json/mcp-tokens/.env 等凭证文件，返回 defense-in-depth 说明文案
- `agent/file_safety.py:classify_cross_profile_target / get_cross_profile_warning` — 跨 profile 写操作软拦截：检测目标是否属于非活跃 profile 的 skills/plugins/cron/memories
- `agent/file_safety.py:classify_sandbox_mirror_target / get_sandbox_mirror_warning` — 沙箱镜像写操作拦截：检测 .../sandboxes/<backend>/<task>/home/.hermes/... 路径形状
- `agent/file_safety.py:classify_container_mirror_target / get_container_mirror_warning` — 容器内镜像写操作拦截：由调用方提供 mirror_prefix（Docker bind mount 路径）
- `agent/file_safety.py:build_write_denied_paths / build_write_denied_prefixes` — 构建精确路径 set + 目录前缀 list，覆盖 SSH/AWS/GnuPG/kubeconfig/GitHub CLI/gcloud 等凭证目录
- `agent/redact.py:redact_sensitive_text` — 多层正则脱敏：已知前缀(sk-/ghp_/AIza/…) + ENV赋值 + JSON字段 + Authorization头 + Telegram token + 私钥块 + DB连接串 + JWT + 手机号
  - 💡 子串预检跳过不含关键前缀的文本，把 13 模式扫描从 5.6us 降到 1.8us
- `agent/redact.py:mask_secret` — 显示时脱敏：保留 head/tail 字符，短于 floor 则全 *** 替换
- `agent/redact.py:RedactingFormatter` — logging.Formatter 子类，对所有 log message 自动调用 redact_sensitive_text
- `agent/redact.py:_extract_literal_prefix / _has_known_prefix_substring` — 从 regex pattern 提取字面前缀子串，用于 O(n) 快速预检
- `agent/redact.py:_redact_query_string / _redact_url_query_params / _redact_url_userinfo / _redact_http_request_target_query_params / _redact_form_body` — URL 查询参数/userinfo/HTTP access log request-target/form-body 各场景脱敏

### ACP(Agent Client Protocol) 适配器  ·  92 项

- `acp_adapter/entry.py:_BenignProbeMethodFilter` — logging.Filter 子类，过滤掉 ACP liveness probe(ping/health)触发的 RequestError -32601 噪声日志，其余背景任务错误照常上报
  - 💡 懒导入 acp.exceptions.RequestError 保证在 agent-client-protocol 未安装时模块仍可导入
- `acp_adapter/entry.py:_setup_logging` — 把全部 logging 路由到 stderr，保持 stdout 作为 ACP stdio JSON-RPC 专用通道
- `acp_adapter/entry.py:_load_env` — 从 HERMES_HOME/.env 加载环境变量，不存在时静默跳过
- `acp_adapter/entry.py:_parse_args` — 解析 hermes-acp CLI 参数：--version/--check/--setup/--setup-browser/--yes
- `acp_adapter/entry.py:_run_check` — 导入 acp 和 HermesACPAgent 验证依赖完整性后退出
- `acp_adapter/entry.py:_run_setup` — 交互式 Hermes provider/model 配置，结束后询问是否安装 browser tools
- `acp_adapter/entry.py:_run_setup_browser` — 通过 dep_ensure 安装 agent-browser + Playwright Chromium，幂等，返回 0/1
- `acp_adapter/entry.py:main` — 入口：加载 env、配置 logging、跑 MCP tool discovery、创建 HermesACPAgent 并调 acp.run_agent 进入 stdio 事件循环
- `acp_adapter/auth.py:detect_provider` — 解析当前 Hermes 运行时 provider；识别 Callable api_key（Azure Entra ID）为合法凭证
- `acp_adapter/auth.py:has_provider` — 返回是否已配置可用 provider
- `acp_adapter/auth.py:build_auth_methods` — 构建 ACP initialize 响应中的 auth_methods：已有 provider 时注册 AuthMethodAgent，始终追加 TerminalAuthMethod（hermes-setup）供首次配置
- `acp_adapter/server.py:HermesACPAgent` — acp.Agent 主类，包含全部 ACP protocol 方法实现；包装 AIAgent 为 ACP agent server
- `acp_adapter/server.py:HermesACPAgent.on_connect` — 保存 acp.Client 连接句柄，用于后续主动推送 session_update
- `acp_adapter/server.py:HermesACPAgent._session_modes` — 返回 SessionModeState：Default/Accept Edits/Don't Ask 三档编辑审批模式，映射到 edit_approval_policy
- `acp_adapter/server.py:HermesACPAgent._edit_approval_policy_for_state` — 从 session state mode 解析出 edit_approval_policy 和 cwd，供 auto-approve 判断
- `acp_adapter/server.py:HermesACPAgent._encode_model_choice` — 把 provider+model 编码为 'provider:model' 字符串，让 ACP client 携带 provider 上下文
- `acp_adapter/server.py:HermesACPAgent._build_model_state` — 返回 SessionModelState：从 hermes_cli.models 枚举当前 provider 的 curated 模型列表，含当前 model 标注
- `acp_adapter/server.py:HermesACPAgent._resolve_model_selection` — 'provider:model' 输入解析：先 parse_model_input，再 detect_provider_for_model 自动识别 provider
- `acp_adapter/server.py:HermesACPAgent._build_usage_update` — 用 estimate_request_tokens_rough 估算系统提示+历史+工具 schema 的 token 用量，生成 ACP UsageUpdate 供 Zed 圆形 context 指示器
- `acp_adapter/server.py:HermesACPAgent._send_usage_update` — 异步推送 context usage 到 ACP client
- `acp_adapter/server.py:HermesACPAgent._send_session_info_update` — 推送会话 title 更新（auto-title 完成后调用），含 updated_at=now
- `acp_adapter/server.py:HermesACPAgent._schedule_usage_update` — 用 loop.call_soon + create_task 在当前 event loop tick 末异步调度 usage 刷新
- `acp_adapter/server.py:HermesACPAgent._register_session_mcp_servers` — 把 ACP client 提供的 McpServerStdio/Http/Sse 注册进 hermes MCP registry，刷新 agent 工具面；自动扩展 enabled_toolsets 加入 mcp-<name>
- `acp_adapter/server.py:HermesACPAgent.initialize` — ACP handshake：返回 agent 能力声明（load_session/image/fork/list/resume），附 auth_methods
- `acp_adapter/server.py:HermesACPAgent.authenticate` — 校验 method_id 与已配置 provider 匹配，terminal-setup method 需实际 provider 存在才返回 AuthenticateResponse
- `acp_adapter/server.py:HermesACPAgent._flatten_history_text` — 把 OpenAI 兼容的 text-or-parts 历史字段规范化为单字符串
- `acp_adapter/server.py:HermesACPAgent._history_message_text` — 从持久化 message 提取 displayable text，调 _flatten_history_text
- `acp_adapter/server.py:HermesACPAgent._history_reasoning_text` — 提取 reasoning_content 或 reasoning 字段（覆盖 DeepSeek/Moonshot 和 codex 两种 transport）
- `acp_adapter/server.py:HermesACPAgent._history_message_update` — 把 user/assistant 历史行转为 ACP UserMessageChunk/AgentMessageChunk
- `acp_adapter/server.py:HermesACPAgent._history_thought_update` — 把思考内容转为 ACP AgentThoughtChunk
- `acp_adapter/server.py:HermesACPAgent._history_tool_call_name_args` — 从 OpenAI-style tool_call dict 提取 function name 和 arguments（兼容 JSON 字符串 args）
- `acp_adapter/server.py:HermesACPAgent._history_tool_call_id` — 从 tool_call dict 取 stable id（兼容 id/call_id/tool_call_id 三个键名）
- `acp_adapter/server.py:HermesACPAgent._replay_session_history` — 加载/恢复会话时把历史消息流式推送给 ACP client（user/agent chunk + thought chunk + tool start/complete），满足 spec 要求在 load_session 响应生命期内完成重放
- `acp_adapter/server.py:HermesACPAgent.new_session` — 创建新 ACP session，注册 MCP servers，调度 commands/usage 更新，返回 session_id+model+modes
- `acp_adapter/server.py:HermesACPAgent.load_session` — 恢复已有 session，更新 cwd，同步完成历史回放，再返回响应
- `acp_adapter/server.py:HermesACPAgent.resume_session` — resume 语义同 load，找不到时静默 fallback 到 create_session
- `acp_adapter/server.py:HermesACPAgent.cancel` — 设置 cancel_event，保存当前 prompt_text 为 interrupted，调 agent.interrupt()
- `acp_adapter/server.py:HermesACPAgent.fork_session` — 深拷贝 history 到新 session，复用当前 model
- `acp_adapter/server.py:HermesACPAgent.list_sessions` — 按 cwd 过滤 + cursor 分页返回 SessionInfo 列表，page_size=50
- `acp_adapter/server.py:HermesACPAgent.prompt` — ACP 核心方法：解析 content blocks → slash command 或 AIAgent.run_conversation；用 contextvars.copy_context + ThreadPoolExecutor 隔离并发 session；设置 TLS approval_cb/edit_approval/HERMES_INTERACTIVE/HERMES_SESSION_ID；完成后 drain queued_prompts；生成 ACP Usage 含 thought_tokens/cached_read_tokens
- `acp_adapter/server.py:HermesACPAgent._handle_slash_command` — 分发 /help /model /tools /context /reset /compact /steer /queue /version，未知 command 返回 None 落回 LLM
- `acp_adapter/server.py:HermesACPAgent._cmd_compact` — 手动触发 agent._compress_context，临时置 _session_db=None 避免 SQLite session 分裂副作用，报告压缩前后 msg 数和 token 估算
- `acp_adapter/server.py:HermesACPAgent._cmd_steer` — agent 运行中时调 agent.steer() 注入引导；idle 时若有 interrupted_prompt 则拼接作为新 prompt；无 prior 则入队
- `acp_adapter/server.py:HermesACPAgent.set_session_model` — ACP protocol 级模型切换：resolve provider+model，重建 agent 实例，持久化
- `acp_adapter/server.py:HermesACPAgent.set_session_mode` — 持久化 Zed mode_id（编辑审批策略）
- `acp_adapter/server.py:HermesACPAgent.set_config_option` — 接受任意 ACP config option；edit_approval_policy 有 typed 处理，其余存入 config_options 字典
- `acp_adapter/server.py:_resource_link_to_parts` — 把 ACP ResourceContentBlock 读取并转为 OpenAI content parts：图片→image_url data URL，文本→格式化文本；处理 WSL Windows 路径转换；大文件截断/binary 跳过
- `acp_adapter/server.py:_embedded_resource_to_parts` — 把 ACP EmbeddedResourceContentBlock(TextResourceContents/BlobResourceContents) 转 OpenAI parts
- `acp_adapter/server.py:_content_blocks_to_openai_user_content` — 把 ACP 多模态 prompt blocks（Text/Image/Resource/Embedded）聚合为 OpenAI user content，纯文本输入退化为字符串保持兼容性
- `acp_adapter/server.py:_path_from_file_uri` — URI/path → Path，处理 WSL Windows drive 形式（file:///C:/... 或 C:\... → /mnt/c/...）
- `acp_adapter/session.py:SessionState` — per-session dataclass：agent实例/cwd/model/history/cancel_event/is_running/queued_prompts/runtime_lock/当前&中断 prompt text
- `acp_adapter/session.py:SessionManager` — 线程安全 session 管理器：in-memory _sessions dict + SessionDB 持久化；透明从 DB restore
- `acp_adapter/session.py:SessionManager.create_session` — 生成 UUID session，_make_agent，注册 task cwd，持久化到 DB
- `acp_adapter/session.py:SessionManager.get_session` — 优先取内存，miss 时调 _restore 从 DB 还原（透明恢复）
- `acp_adapter/session.py:SessionManager.fork_session` — deepcopy history，新建 agent，新建 session，persist
- `acp_adapter/session.py:SessionManager.list_sessions` — 合并内存+DB sessions，按 cwd 过滤，按 updated_at 倒序排列，返回 lightweight info dicts
- `acp_adapter/session.py:SessionManager._persist` — 原子性 replace_messages 写 DB，create_session 若不存在，否则 update_session_meta；session_meta JSON 含 cwd/provider/base_url/api_mode
- `acp_adapter/session.py:SessionManager._restore` — 从 DB get_session + get_messages_as_conversation，重建 AIAgent，注册 task cwd，放入内存
- `acp_adapter/session.py:SessionManager._make_agent` — 真正创建 AIAgent：load_config → resolve_runtime_provider → AIAgent(**kwargs)；platform='acp'；路由 stdout→stderr；支持 agent_factory 注入（测试）
- `acp_adapter/session.py:_expand_acp_enabled_toolsets` — 合并基础 toolset 列表与 mcp-<server_name> 动态 toolset，去重保序
- `acp_adapter/session.py:_translate_acp_cwd` — Windows drive path → WSL /mnt/<drive>/... 路径转换，非 WSL 透传
- `acp_adapter/session.py:_normalize_cwd_for_compare` — cwd 归一化：expanduser + WSL 转换 + normpath，供 cwd 过滤比较
- `acp_adapter/tools.py:get_tool_kind` — 工具名 → ACP ToolKind（read/edit/execute/search/fetch/think/other）映射
- `acp_adapter/tools.py:build_tool_title` — 按工具名定制人类可读 title（含参数摘要），如 'terminal: <cmd>' / 'patch (replace): <path>'
- `acp_adapter/tools.py:build_tool_start` — 创建 ToolCallStart 事件；为各工具定制 start content（patch/write_file 在 auto-approve 时已展示 diff；terminal 展示命令；todo 展示预览列表等）
- `acp_adapter/tools.py:build_tool_complete` — 创建 ToolCallProgress(completed/failed)；调 _build_tool_complete_content 生成结构化内容；failed 判断用 _tool_result_failed
- `acp_adapter/tools.py:_tool_result_failed` — 保守地判断工具是否失败：特定 error prefix、success/ok=false、非零 exit_code、polished tool 有 error 无 content
- `acp_adapter/tools.py:_build_polished_completion_content` — 分发各 polished 工具的专用 formatter，fallback generic structured formatter
- `acp_adapter/tools.py:_parse_unified_diff_content` — 解析 unified diff 文本为 ACP tool_diff_content blocks（多文件）
- `acp_adapter/tools.py:_format_todo_result` — todo JSON → Markdown 任务列表 + 进度汇总（completed/in_progress/pending/cancelled）
- `acp_adapter/tools.py:_format_generic_structured_result` — 通用结构化 JSON 结果 → 紧凑 Markdown bullets，有 priority_keys 优先展示顺序
- `acp_adapter/tools.py:_format_structured_value` — 递归渲染嵌套 JSON 为 Markdown bullets，max_depth/max_items 防爆炸
- `acp_adapter/tools.py:_fenced_text` — 动态确定 fence 长度（反引号计数+1），防止内容中的反引号破坏 code block
- `acp_adapter/tools.py:extract_locations` — 从工具参数提取文件路径+行号为 ToolCallLocation 列表
- `acp_adapter/events.py:_build_plan_update_from_todo_result` — 把 todo 工具结果转为 ACP AgentPlanUpdate，供 Zed plan 面板展示；cancelled 状态标注前缀后映射为 completed（ACP 无 cancelled 状态）
- `acp_adapter/events.py:make_tool_progress_cb` — 创建 AIAgent.tool_progress_callback：捕获 tool.started 事件，生成 ToolCallStart，用 FIFO deque 跟踪同名工具并发 ID，auto-approve 路径下预先计算 edit diff
- `acp_adapter/events.py:make_step_cb` — 创建 AIAgent.step_callback：从 prev_tools 列表 FIFO 弹出 tool_call_id，发 ToolCallProgress(completed)；todo 工具额外发 plan_update
- `acp_adapter/events.py:make_thinking_cb` — 创建 reasoning_callback：推送 AgentThoughtChunk
- `acp_adapter/events.py:make_message_cb` — 创建 stream_delta_callback：推送 AgentMessageChunk
- `acp_adapter/events.py:_send_update` — 从 worker 线程 fire-and-forget ACP update：safe_schedule_threadsafe + future.result(timeout=5)
- `acp_adapter/permissions.py:make_approval_callback` — 返回 Hermes-compatible approval 回调，桥接到 ACP request_permission；build_permission_options 映射 allow_once/session/always/deny；超时 60s 默认 deny
- `acp_adapter/permissions.py:_build_permission_options` — 构造 ACP PermissionOption 列表；allow_session 用 kind=allow_always（ACP 无 session 级），deny_always 探测 SDK 版本支持
- `acp_adapter/permissions.py:_build_permission_tool_call` — 生成带 perm-check-N ID 的 ToolCallUpdate payload（status=pending），附加在 request_permission 中
- `acp_adapter/permissions.py:_map_outcome_to_hermes` — ACP AllowedOutcome.option_id → Hermes approval 字符串（once/session/always/deny）
- `acp_adapter/edit_approval.py:EditProposal` — 冻结 dataclass：tool_name/path/old_text/new_text/arguments，表示单文件编辑提案
- `acp_adapter/edit_approval.py:set_edit_approval_requester` — 在 ContextVar 绑定 ACP edit approval requester，返回 Token
- `acp_adapter/edit_approval.py:reset_edit_approval_requester` — 用 Token 恢复上一个 requester 绑定
- `acp_adapter/edit_approval.py:build_edit_proposal` — 为 write_file/patch(replace mode) 构建 EditProposal；patch 调 fuzzy_find_and_replace 预计算 new_text
- `acp_adapter/edit_approval.py:should_auto_approve_edit` — 按策略(ask/workspace_session/session)和路径敏感度决定是否自动审批；敏感路径(.git/.ssh/id_rsa/.env)始终拒绝
- `acp_adapter/edit_approval.py:maybe_require_edit_approval` — 工具派发前门卫：若有 requester 且 build_edit_proposal 成功，调用 requester；deny 则返回 JSON 错误字符串拦截工具执行
- `acp_adapter/edit_approval.py:build_acp_edit_tool_call` — 为 request_permission 构建 ToolCallUpdate payload(kind=edit, status=pending)，含 tool_diff_content
- `acp_adapter/edit_approval.py:make_acp_edit_approval_requester` — 返回同步 requester：先 should_auto_approve_edit 快速路径，否则通过 ACP request_permission 向用户展示 diff + 审批

### 多平台消息网关 + 流式分发  ·  40 项

- `gateway/platforms/base.py:BasePlatformAdapter` — 所有平台适配器的 ABC 抽象基类。持有 message_handler/topic_recovery/active_sessions/pending_messages/session_tasks 等核心状态；定义 connect/disconnect/send/edit_message/delete_message/send_draft 等纯虚方法；内置 handle_message 完整分发状态机（debounce/guard/bypass/background-task）；管理 fatal_error 状态机与 runtime_status 持久化
  - 💡 supports_draft_streaming() + send_draft() 可选重写，让 Telegram 等平台原生流式预览；REQUIRES_EDIT_FINALIZE 类属性控制 DingTalk AI Card 等需要显式 finalize 的平台；enforces_own_access_policy 属性跳过 env-allowlist 双重鉴权
- `gateway/platforms/base.py:handle_message` — 核心入站分发：coerce_plaintext_command → topic_recovery → session_key → stale-lock 自愈 → 如有活动 session 则 bypass 命令/clarify/busy_handler/debounce/pending_queue；否则 _start_session_processing → asyncio.create_task(_process_message_background)
  - 💡 guard 先于 task spawn 置位（grammY sequentialize 模式），彻底关闭并发 duplicate-task 竞争窗口
- `gateway/platforms/base.py:_send_with_retry` — 带退避重试的 send 封装；区分 connecterror/connectionreset 等可重试错误与 timeout 不可重试错误；SendResult.retryable=True 可由适配器显式标记
  - 💡 不重试 timeout（非幂等 send 可能已到达服务器），只重试连接级故障
- `gateway/platforms/base.py:MessageEvent` — 平台无关的标准化入站消息模型：text/message_type/source/media_urls/media_types/reply_to_message_id/auto_skill/channel_prompt/channel_context/internal/timestamp
  - 💡 internal=True 允许后台进程通知绕过用户鉴权；auto_skill 字段让频道绑定 skill 在事件入站时就确定，不依赖 gateway runner 的额外查表
- `gateway/platforms/base.py:SendResult` — 发送结果 dataclass：success/message_id/error/raw_response/retryable/continuation_message_ids
  - 💡 continuation_message_ids 支持超长内容拆分为多条消息后记录全部 id，后续 edit 总能定位最新可见消息
- `gateway/platforms/base.py:EphemeralReply` — str 子类包装器：slash-command 返回此类型时，gateway 在 ttl_seconds 后自动删除该消息
  - 💡 str 子类保持透明性（现有 in/startswith/== 测试不变）；平台不实现 delete_message 时静默忽略 TTL
- `gateway/platforms/base.py:merge_pending_message_event` — burst 合并：PHOTO+PHOTO → 合并 media_urls；mixed media+text → append；TEXT+TEXT (merge_text=True) → 文本 join；防止多条快速消息打开多轮 agent
  - 💡 仅作为 pending_messages dict 的写入入口，调用方无需关心合并策略
- `gateway/platforms/base.py:validate_media_delivery_path` — 校验模型输出的本地文件路径是否允许作为附件发送。默认模式：任何非 credential/system 路径；严格模式：仅 cache 目录或 HERMES_MEDIA_ALLOW_DIRS 或 600s 内新产生的文件
  - 💡 denylist 覆盖 /etc /proc ~/.ssh ~/.aws ~/.kube ~/.docker ~/.hermes/.env 等；recency window 用 mtime 区分 agent 刚产生的产物与老系统文件，精准拦截 prompt-injection 路径外泄
- `gateway/platforms/base.py:cache_media_bytes / cache_image_from_url / cache_audio_from_url` — 统一媒体缓存入口：按 ext/MIME 分类路由到 image/video/audio/document cache；URL 版本带指数退避重试（429/5xx）和 SSRF 防护（redirect guard）
  - 💡 _ssrf_redirect_guard 在每个 redirect 上重新校验目标 URL，阻止 302 跳转到内网绕过预检
- `gateway/platforms/base.py:resolve_proxy_url / proxy_kwargs_for_aiohttp` — 代理 URL 解析：PLATFORM_ENV_VAR → HTTPS_PROXY/HTTP_PROXY/ALL_PROXY → macOS scutil --proxy；SOCKS 优先用 aiohttp-socks connector（rdns=True 强制远端 DNS）
  - 💡 _detect_macos_system_proxy 自动读取系统代理，无需用户手动配置
- `gateway/platforms/base.py:_keep_typing` — 后台异步循环持续发送 typing indicator，每 N 秒一次；_typing_paused set 控制暂停（approval 等待期间不发）
  - 💡 typing_paused 而非停止 loop，保留 loop 状态方便 resume
- `gateway/platforms/base.py:register_post_delivery_callback / pop_post_delivery_callback` — one-shot 投递后回调注册，支持 generation 感知（(generation, callback) 元组），防止过期 run 的回调清除新 run 注册的回调
  - 💡 generation 计数器 _begin/_invalidate_session_run_generation 解决同 session 重入时回调错位问题
- `gateway/platforms/base.py:_text_debounce_store / _queue_text_debounce / _flush_text_debounce` — busy session 期间文本消息防抖：默认等待 0.35s，最多 1.0s，合并连续输入为单条 pending；超时后 flush 为 pending_message
  - 💡 debounce 仅对 TEXT 类型消息生效，PHOTO/VOICE 直接 queue 不 debounce
- `gateway/platforms/base.py:resolve_channel_prompt / resolve_channel_skills` — 从 config.extra.channel_prompts / channel_skill_bindings 按 channel_id → parent_id 优先顺序解析每频道 system prompt 和预加载 skill
  - 💡 channel_skill_bindings 支持 list 格式多 skill；parent_id fallback 让子线程继承父频道配置
- `gateway/stream_events.py:StreamEvent (MessageChunk/MessageStop/Commentary/ToolCallChunk/ToolCallFinished/LongToolHint/GatewayNotice)` — 结构化流式事件词汇表：纯数据 frozen dataclass，无行为无平台知识；描述 what happened，不规定 how to deliver
  - 💡 设计约束：不持久化到对话历史，仅表示层 stream；Union 显式穷举而非 marker base class，exhaustive match 缺 case 是类型错误而非静默 fallthrough
- `gateway/stream_dispatch.py:GatewayEventDispatcher` — typed event → adapter 渲染 → delivery sink 的薄路由层。MessageChunk/Stop/Commentary → adapter.render_message_event(sink)；ToolCallChunk → adapter.format_tool_event → enqueue_tool_line；new 模式去重（同 tool 连续调用仅报一次）
  - 💡 dispatch() 吞掉所有异常（presentation 不能 break agent loop）；同步调用，无 asyncio
- `gateway/stream_consumer.py:GatewayStreamConsumer` — 同步 agent worker thread → 异步平台投递的桥接：on_delta(text) thread-safe 写 queue.Queue；run() asyncio task 从 queue 消费、rate-limit、progressive edit 或 native draft；finish() 写 _DONE sentinel
  - 💡 支持 draft/edit/off 三种 transport；_filter_and_accumulate 状态机过滤 <think>/<reasoning> 等 block；_should_send_fresh_final 处理长时间流式响应用新消息替换旧预览（时间戳准确）；_MAX_FLOOD_STRIKES=3 触发永久禁 edit
- `gateway/stream_consumer.py:GatewayStreamConsumer.on_delta / on_segment_break / on_commentary / finish` — stream consumer 四个 thread-safe 写入口：text delta/工具边界分段/_NEW_SEGMENT sentinel/(_COMMENTARY,text)/流完成 _DONE
  - 💡 _COMMENTARY 是完整文本（不是 delta），在 queue 里用 tuple 与 str delta 区分
- `gateway/stream_consumer.py:GatewayStreamConsumer.run` — 主消费 loop：按 edit_interval 节流 edit；_resolve_draft_streaming 决定 draft/edit 模式；overflow 时 _send_new_chunk 追加新消息；finish 时 _try_fresh_final / _try_strip_cursor
  - 💡 adaptive backoff：flood error 累计后 _current_edit_interval *= 1.5；flood_strikes >= MAX_FLOOD_STRIKES 后永久切 buffer_only 模式
- `gateway/hooks.py:HookRegistry` — 文件系统钩子发现加载注册机：扫描 ~/.hermes/hooks/*/HOOK.yaml + handler.py；支持精确事件匹配和 command:* 通配符；emit()/emit_collect() 同时支持 sync/async handler
  - 💡 加载时立即 sys.modules[module_name] = module 解决 Pydantic 前向引用问题；错误 catch 后 pop sys.modules 防止缓存坏模块
- `gateway/pairing.py:PairingStore` — DM 配对码审批流：8 字符 base32 随机码（无歧义字母）1h TTL；每平台最多 3 pending；每用户 10min rate limit；5 次失败 lockout 1h；文件 chmod 0600 + temp-rename 原子写入
  - 💡 配对码 hash 存储（secrets.choice + hashlib sha256 + salt），原文不落盘；_user_id_aliases 处理 WhatsApp phone/JID 多格式匹配
- `gateway/channel_directory.py:build_channel_directory` — 启动时构建跨平台频道目录 JSON：Discord/Slack 直接 API 枚举；其余平台从 sessions.json 历史推断；含 plugin 注册平台；5min 定期刷新；写 ~/.hermes/channel_directory.json
  - 💡 _SKIP_SESSION_DISCOVERY 集合跳过 local/api_server/webhook 等非消息平台
- `gateway/delivery.py:DeliveryTarget / route_delivery` — cron 输出投递路由：origin(回源) / platform:chat_id(显式) / platform(home channel) / local(文件)；截断 4000 chars；silence narration 检测过滤
  - 💡 _SILENCE_NARRATION 正则过滤模型输出的 *(silent)* 等无声叙述标记，防止 cron 对话投递噪声消息
- `gateway/mirror.py:mirror_to_session` — 跨平台投递镜像：把 CLI/cron 发出的消息追写到目标 session transcript（JSONL+SQLite），让接收端 agent 知道有消息被发出
  - 💡 多候选 session 时若无精确 user_id 匹配则拒绝（返回 None），防止污染他人 session
- `gateway/platform_registry.py:PlatformRegistry / PlatformEntry` — 平台适配器注册中心：PlatformEntry dataclass 持有 adapter_factory/check_fn/validate_config/is_connected/setup_fn/apply_yaml_config_fn/standalone_sender_fn/env_enablement_fn 等全部扩展点；registry.create_adapter() 依次调用 check→validate→factory
  - 💡 standalone_sender_fn 允许 cron 跨进程发送无需 live gateway；apply_yaml_config_fn 让 plugin 拥有自己的 YAML config 翻译，不污染 core config.py；cron_deliver_env_var 声明 home channel 支持
- `gateway/session.py:SessionSource / build_session_key` — 入站消息源描述：platform/chat_id/chat_type/user_id/thread_id/guild_id/parent_chat_id 等；build_session_key 决定 group/thread session 隔离粒度（per-chat 或 per-user）
  - 💡 user_id_alt/chat_id_alt 支持 Signal UUID / Feishu union_id 等二级稳定 ID；guild_id 提供 Discord guild / Slack workspace 范围隔离
- `gateway/session_context.py:set_session_vars / get_session_env / clear_session_vars` — 用 contextvars.ContextVar 替代 os.environ 存储 session 上下文，实现并发 task 隔离；get_session_env 向后兼容 os.getenv 调用
  - 💡 _UNSET sentinel 区分 '从未设置（fallback os.environ）' 与 '显式清除（返回空串不 fallback）'
- `gateway/display_config.py:resolve_display_setting` — 四级优先级解析展示设置：per-platform用户覆盖 > 全局用户设置 > 平台内置默认 > 全局内置默认；平台分 4 tier（high/medium/low/minimal）
  - 💡 Tier 低平台（SMS/Email）默认 streaming=False/tool_progress=off/interim_messages=False；高频道（Telegram DM）默认全开
- `gateway/run.py:GatewayRunner` — Gateway 生命周期 orchestrator：管理所有 platform adapter connect/disconnect；创建 LRU agent cache（128 cap + 1h idle TTL）；路由 message_handler；驱动 _run_agent；管理 stream consumer + tool progress + cron ticker；处理 restart/drain/exit
  - 💡 _agent_config_signature 检测 config 变更，变更时自动 evict cached agent；_is_intentional_model_switch 防止误判 model 切换为 config 变更
- `gateway/run.py:_run_agent` — 单轮 agent 执行：构建 history/media/context → run_in_executor（线程池）→ stream consumer task + tool progress loop（send_progress_messages）→ 投递最终响应；处理 streaming/non-streaming/media 三条路径
  - 💡 progress_callback 通过 GatewayEventDispatcher 路由 typed events；stream_delta_cb 写入 consumer queue（thread-safe）；_step_callback_sync 每 tool-calling loop 检查 interrupt
- `gateway/run.py:start_gateway` — 顶层入口：加载 config → 构建 GatewayRunner → signal handler（SIGTERM/SIGINT/SIGUSR1 restart）→ platform connect → cron ticker → 等待 shutdown
  - 💡 SIGUSR1 触发热重启（drain + EX_TEMPFAIL 退出让 systemd 重拉）；--replace 模式先杀旧 pid 再启动
- `gateway/run.py:_build_gateway_agent_history` — 为 agent 构建对话历史：从 session JSONL 回放；处理 observed-group-context 注入（Telegram group 观察到的消息）；处理 auto_continue 新鲜度窗口
  - 💡 _AGENT_CACHE_MAX_SIZE + _AGENT_CACHE_IDLE_TTL 约束 LRU agent cache 不无限增长
- `gateway/run.py:_coerce_gateway_timestamp` — 容错解析 agent 返回的 timestamp：int/float/ISO string/datetime 对象，bool 特殊排除（bool is subclass of int）
- `gateway/status.py:write_runtime_status / acquire_scoped_lock / release_scoped_lock` — 平台运行状态持久化（gateway_state.json）；scoped 锁防止同 token 多 gateway 并发（file lock on gateway.lock）；跨平台支持 Windows msvcrt + POSIX fcntl
  - 💡 acquire_scoped_lock 返回 (acquired, existing_metadata)，允许调用方展示当前锁持有者 PID
- `gateway/platforms/api_server.py:APIServerAdapter / ResponseStore` — OpenAI-compatible REST API 适配器（aiohttp web server）：/v1/chat/completions SSE 流式 + 同步；ResponseStore SQLite 持久化响应（LRU 淘汰+文件权限 0600）；CORS/幂等性/rate-limit middleware
  - 💡 _derive_chat_session_id 用 model+system_prompt+messages hash 自动派生 session id，无需显式管理 session；idempotency cache 防重复提交
- `gateway/platforms/telegram.py:TelegramAdapter` — Telegram Bot API 适配器：长轮询 + PTB；处理 DM topic 模式（forum/supergroup topics + DM topic 两种）；MarkdownV2 格式化（表格/代码块/emoji）；send_draft（Bot API 9.5 sendMessageDraft）；edit overflow split（超长响应自动分段）
  - 💡 _should_retry_without_dm_topic_reply_anchor：DM topic reply anchor 失败时自动 fallback 到 direct_messages_topic_id 元数据，避免消息丢失
- `gateway/platforms/slack.py:SlackAdapter` — Slack Socket Mode 适配器；SocketMode + watchdog loop（连接断开自动重连）；slash command 处理；Block Kit 解析为 plain text；thread context cache；per-channel session 隔离
  - 💡 _socket_watchdog_loop：每 60s 主动探测连接状态，dead socket 静默不报 PTB 的 polling conflict 类错误
- `gateway/platforms/feishu.py:FeishuAdapter / normalize_feishu_message` — 飞书适配器：解析复杂 post/rich_text/interactive card/merge_forward 消息为统一 FeishuNormalizedMessage；支持 mention 解析；发送 markdown post / 卡片
  - 💡 _normalize_merge_forward_message 把合并转发消息展开为多条嵌套文本，不丢失上下文
- `gateway/platforms/wecom.py:WecomAdapter` — 企业微信适配器：webhook 接收 + AES 解密（wecom_crypto）；dm_policy/group_policy 自管访问控制；enforces_own_access_policy=True 跳过全局 env 鉴权
- `gateway/platforms/whatsapp.py:WhatsAppAdapter` — WhatsApp 适配器（通过本地 bridge 进程）：启动 bridge → WebSocket → 接收/发送消息；管理 bridge PID file；require_mention 过滤群组非 @ 消息
  - 💡 _kill_stale_bridge_by_pidfile：启动时清理遗留 bridge 进程，防止多 bridge 冲突

### CLI kanban / swarm 多 agent 任务分解 / goals  ·  44 项

- `kanban_db.py:Task` — 任务行的内存视图 dataclass，字段覆盖 id/title/body/assignee/status/priority/workspace_kind/claim_lock/consecutive_failures/goal_mode/goal_max_turns/session_id 等，含 from_row() 防御性兼容旧列名(spawn_failures→consecutive_failures)
  - 💡 goal_mode/goal_max_turns 把 Ralph-style goal loop 嵌入任务行——单字段切换 worker 从 one-shot 变持续循环；session_id 让 per-session board 无需 tenant+时间启发式
- `kanban_db.py:Run` — task_runs 行的内存视图，每次 claim 生一行，记录 claim_lock/worker_pid/last_heartbeat_at/outcome/summary/metadata/error；支持同一任务多次重试
  - 💡 summary+metadata 是结构化 handoff 通道，下游 worker 经 build_worker_context 读取；outcome 语义集合覆盖 completed/blocked/crashed/timed_out/spawn_failed/gave_up/reclaimed
- `kanban_db.py:create_task` — 原子创建任务+写 task_links+记 created 事件；支持 parents(→status:todo) / triage / idempotency_key / skills / goal_mode / workspace_kind(scratch/worktree/dir) / board-default-workdir 继承
  - 💡 skills 列表与 toolset 名反混淆——在 create_task 里主动校验，拒绝 toolset 名混入 skills，防止 agent 常见分类错误
- `kanban_db.py:claim_task` — CAS 原子 ready→running；写 task_runs 行；记录 worker_pid；执行时若父任务未 done 则原地退回 todo（second-level enforce）
  - 💡 双重守卫：先检查未完成父依赖（防止 racy writer 错误 promote），再做 claim_lock IS NULL CAS；losers 观察 rowcount==0 直接放弃，无重试自旋
- `kanban_db.py:heartbeat_claim` — 在 running 状态下延长 claim TTL，同步更新 task_runs
  - 💡 15 min 默认 TTL + heartbeat 延续机制解耦 'worker alive' 与 'LLM 慢速响应'；不调用 heartbeat 的 logic-loop worker 最终被 DEFAULT_CLAIM_HEARTBEAT_MAX_STALE_SECONDS 强制回收
- `kanban_db.py:release_stale_claims` — 回收 TTL 过期 running 任务；若 worker PID 在本机仍存活则改为延展 claim（发 claim_extended 事件）而非强制回收；heartbeat stale 超 1h 则仍强制回收
  - 💡 PID liveness 延展避免'慢模型单次 LLM call 超 15 min 被误杀'的 spawn-reclaim 死循环
- `kanban_db.py:recompute_ready` — 批量把所有父任务均 done/archived 的 todo/blocked 任务提升为 ready；区分 sticky block（worker 显式调 kanban_block）与 circuit-breaker block（消极回收），sticky 不自动恢复
  - 💡 _has_sticky_block 通过查最近 blocked/unblocked 事件区分两类 blocked，这是最干净的非状态机事件驱动判定方式
- `kanban_db.py:complete_task` — running\|ready→done；验证 created_cards（防幻觉）；记录 summary/metadata；prose scan 检测 summary 里引用不存在的 t_xxx id
  - 💡 created_cards 验证三条信任路径：created_by==assignee / created_by==task_id / 属于 child_links；任意 phantom 触发 completion_blocked_hallucination 事件并抛 HallucinatedCardsError，不修改 task 状态
- `kanban_db.py:block_task / unblock_task` — worker 主动 block（发 blocked 事件）或 operator 解除（发 unblocked 事件）；block 打 sticky 标记阻止 recompute_ready 自动恢复
  - 💡 sticky block 是人机协作接口——worker 遇到需要人类决策的情况显式 block，比超时/crash 语义清晰
- `kanban_db.py:decompose_triage_task` — 原子 fan-out：在单个 write_txn 内创建所有子任务+链接+把根节点从 triage→todo；预校验 Kahn 拓扑排序防止兄弟循环；子任务继承根节点 workspace
  - 💡 根节点被链接在每个子节点下方（root 等全子任务完成再 promote 到 ready）——orchestrator 等全图完成后可二次判断是否追加任务
- `kanban_db.py:specify_triage_task` — triage→todo 升级 + 更新 title/body/assignee；若无父依赖立即调 recompute_ready 直通 ready，不等下次 tick
  - 💡 在同一 write_txn 内不嵌套 nested BEGIN IMMEDIATE，所以 recompute_ready 在 txn 提交后才调用，避免 SQLite 嵌套事务异常
- `kanban_db.py:build_worker_context` — 为 worker 构建完整任务上下文字符串：title+body+附件路径+历史 attempt(summary/error/metadata)+父任务结果+同一 assignee 近期角色历史+评论线程；全部字段有 cap
  - 💡 _CTX_MAX_* 常量独立可调，防止单字段（如 1MB summary）把整个 prompt 炸掉；role history（同 assignee 最近 5 次完成）无需用户配置 SOUL.md 就给 worker 隐式连续性
- `kanban_db.py:dispatch_once` — 一个 dispatcher tick：回收 stale/crashed/timed_out worker → recompute_ready → 按 priority+created_at 扫 ready 任务 → claim+spawn；支持 max_spawn(并发上限)/ max_in_progress / per_profile 上限 / default_assignee 自动路由
  - 💡 max_spawn 是全局并发上限（而非每 tick 预算），与 max_in_progress 区别在于：后者让慢 worker 自然消化积压再补充；两者可组合
- `kanban_db.py:detect_crashed_workers` — 检测 running 任务的 worker_pid 已不在本机 OS 存活（_pid_alive）→ 视情况 reclaim/auto-block；区分 rate_limit 退出码（75=EX_TEMPFAIL）不计入 failure_counter
  - 💡 KANBAN_RATE_LIMIT_EXIT_CODE=75 让 rate-limited worker 重排队不消耗 retry 预算；30s crash grace 避免 fork→/proc 可见性窗口误杀
- `kanban_db.py:_record_task_failure` — 递增 consecutive_failures 计数；达到 effective_limit（per-task max_retries > 全局 failure_limit > DEFAULT）时触发电路断路器：status→blocked，发 gave_up 事件
  - 💡 gave_up 与 blocked 是不同事件：前者是自动断路，后者是 worker 主动标记；recompute_ready 通过 _has_sticky_block 区分两者，不会自动恢复 gave_up 任务
- `kanban_db.py:_record_task_failure / reassign_task` — reassign_task 将 consecutive_failures 重置为 0——重新分配本身视为 operator 干预，给新 profile 一个干净开始
  - 💡 failure counter 作用域是 task+profile 组合（同 profile 连续失败），换 profile 清零符合直觉
- `kanban_db.py:scoped_current_board / get_current_board` — board 解析链：ContextVar scope → HERMES_KANBAN_BOARD env → current 文件 → default；scoped_current_board 是线程/协程安全的临时覆盖
  - 💡 dispatcher 在 spawn worker 时将 HERMES_KANBAN_DB+HERMES_KANBAN_WORKSPACES_ROOT+HERMES_KANBAN_BOARD 全部注入 env，确保 worker 子进程与 dispatcher 看同一块棋盘
- `kanban_db.py:write_txn` — BEGIN IMMEDIATE 写事务 context manager；异常时 rollback（捕获 SQLite 已自动 rollback 的情况）；commit 后检查 torn-extend（page_count vs 文件大小）
  - 💡 post-commit 文件完整性校验在实践中是主动防腐蚀：torn-extend 立即报错而非等到下次读时神秘 crash
- `kanban_db.py:_cross_process_init_lock` — flock/msvcrt 文件锁保证多进程首次连接时 schema 初始化/WAL 激活/migrate 串行执行
  - 💡 解决了'dispatcher burst 时多 worker 并发首次打开同一 DB'的竞态——Python 进程内的 threading.RLock 不够
- `kanban_db.py:_verify_created_cards / HallucinatedCardsError` — 校验 worker 在 complete_task 时声明的 created_cards 真实存在且由该 worker 创建；phantom id 触发 blocking 事件不完成任务
  - 💡 三条信任路径+事件记录使幻觉阻断可审计；completion_blocked_hallucination 事件是 diagnostic 规则的信号源
- `kanban_db.py:_scan_prose_for_phantom_ids` — 正则扫描 completion summary 里 t_<hex> 格式引用，返回不存在于 DB 的 id 列表；advisory 不阻断
  - 💡 advisory pass 记录 suspected_hallucinated_references 事件，dashboard 可展示警告而不是阻断 worker
- `kanban_db.py:enforce_max_runtime` — 检测超 max_runtime_seconds 的 running 任务，SIGTERM→SIGKILL grace 终止 worker_pid，任务退回 ready 并记 timed_out 事件
  - 💡 与 claim TTL 机制分层：TTL 防幽灵 lock，max_runtime 防无限跑任务
- `kanban_db.py:reap_worker_zombies` — 在 dispatch_once 开头调用 waitpid(-1, WNOHANG) 收尸以防僵尸进程积累
  - 💡 dispatcher 进程是所有 worker 的父进程，不主动收尸会在高频 spawn 场景造成僵尸积累
- `kanban_decompose.py:decompose_task` — 读一个 triage 任务，调 aux LLM(kanban_decomposer slot) 返回 JSON 任务图；若 fanout=false 退化为 specify；校验 assignee 对应真实 profile；调 kb.decompose_triage_task 原子落地
  - 💡 fanout 字段让 decomposer 可以退化为 specifier——同一 LLM prompt 覆盖两个用例；invalid assignee rewrite 到 default_assignee，子任务永远有归属
- `kanban_decompose.py:_build_roster / _format_roster` — 从 profiles_mod.list_profiles() 构建 roster 列表供 LLM 选择 assignee；没有 description 的 profile 标注 ⚠ undescribed
  - 💡 roster 格式化直接影响 LLM 路由质量——描述越丰富，LLM 匹配越准；⚠ 标注是对用户的隐式 incentive
- `kanban_decompose.py:DecomposeOutcome` — 结构化 decompose 结果：ok/reason/fanout/child_ids/new_title；调用方可区分失败原因而不是 try-except
  - 💡 never raises on expected failure modes——caller 的 --all sweep 可以对单个失败 task continue，不需要异常处理
- `kanban_specify.py:specify_task` — 读单个 triage 任务 → aux LLM(triage_specifier slot) → 解析 {title, body} JSON → specify_triage_task；JSON 解析失败 fallback 为 plain text body
  - 💡 fallback 策略（整个 response 作为 body）保证即使 weak judge model 不输出 JSON 也不会把任务 strand 在 triage
- `kanban_swarm.py:create_swarm` — 在现有 kanban 数据库里创建 4 层拓扑：planning root(立即 done) → parallel workers(ready) → verifier(todo 等 workers) → synthesizer(todo 等 verifier)；幂等：重复调用从 root blackboard 读回 topology
  - 💡 不引入第二个调度器——把 swarm 拓扑表达为普通 task_links 图，完全复用 dispatcher/recompute_ready 机制；blackboard 用结构化 JSON comments 而非独立存储
- `kanban_swarm.py:post_blackboard_update / latest_blackboard` — 向 root 任务追加 JSONL 格式 comment 作为 blackboard 更新；latest_blackboard 合并所有 comment，后写覆盖前写同一 key
  - 💡 blackboard = task_comments 上的 last-write-wins merge；免去独立 KV 存储，利用已有审计通道
- `kanban_swarm.py:SwarmWorkerSpec / SwarmCreated` — SwarmWorkerSpec 是 worker 卡的参数集(profile/title/body/skills/priority/max_runtime)；SwarmCreated 是 create_swarm 返回的 id 包，含 as_dict() 供 blackboard 序列化
  - 💡 frozen dataclass + as_dict 保证 blackboard topology record 可序列化，幂等恢复时只需 json.loads
- `goals.py:GoalState` — session goal 的可序列化状态：goal text/status/turns_used/max_turns/last_verdict/subgoals；to_json/from_json 实现 SessionDB 持久化
  - 💡 subgoals 是 GoalState 的追加需求列表，judge prompt 和 continuation prompt 都会注入 subgoals_block，支持用户中途追加子目标
- `goals.py:GoalManager` — session 维度 goal 状态机：set/clear/pause/resume/add_subgoal/remove_subgoal/evaluate_after_turn/next_continuation_prompt；每次 turn 后调 evaluate_after_turn 决定是否续跑
  - 💡 evaluate_after_turn 返回 decision dict(should_continue/continuation_prompt)解耦 loop 驱动与状态机；judge failure fail-open（不阻断进度）+ consecutive parse failure auto-pause（防弱 judge model burn token）
- `goals.py:judge_goal` — 调 auxiliary model 判断 goal 是否达成；支持 subgoals；解析 {done, reason} JSON；fail-open返回 continue
  - 💡 parse_failed 与 API error 区分：前者累计计入 consecutive_parse_failures（弱 model 问题），后者不计（网络抖动）；_parse_judge_response 两步 JSON 解析 + markdown fence strip
- `goals.py:run_kanban_goal_loop` — 驱动 kanban worker 的 Ralph-style goal loop：检查 task_status → judge 上一轮 response → done: 发 finalize nudge / continue: 发 continuation prompt → budget 耗尽调 block_fn
  - 💡 finalize nudge 机制：judge 认为完成但 worker 未调 kanban_complete，给一次显式 nudge 再 block（而非直接 block 或静默退出）
- `kanban_diagnostics.py:compute_task_diagnostics` — 对单个 task+events+runs 运行所有 rule，返回 severity 排序的 Diagnostic 列表；rule 异常被 swallow（dashboard 不能 500）
  - 💡 rule 注册表(_RULES 列表)让新规则仅需追加，order 决定同 severity 时渲染顺序
- `kanban_diagnostics.py:_rule_hallucinated_cards` — 检测 completion_blocked_hallucination 事件（无 completed/edited 后续则仍 active）；返回带 phantom_ids 和可操作 actions(comment/reclaim/reassign) 的 Diagnostic
  - 💡 _active_hallucination_events 的清零逻辑：遇到 completed/edited 事件则 active 列表清空，支持 auto-clear
- `kanban_diagnostics.py:_rule_repeated_failures` — 连续失败计数达 threshold 时报 error/critical；分类 spawn_failed/timed_out/crashed 给出具体 cli_hint(hermes -p X doctor / hermes kanban log X)
  - 💡 severity 随失败次数升级（>2x threshold → critical）；threshold 从 kanban.failure_limit 派生保持与 circuit breaker 一致
- `kanban_diagnostics.py:_rule_stranded_in_ready` — ready 超时未被 claim：30min 起报 warning，2x → error，6x → critical；只对有 assignee 的任务报（unassigned 有独立 diagnostic）
  - 💡 identity-agnostic（不分 hermes profile / external worker / Claude Code lane），统一覆盖 typo assignee / 已删 profile / 外部 worker 下线三类原因
- `kanban_diagnostics.py:_rule_block_unblock_cycling` — 统计滑动窗口内 block→unblock 的 cycle 次数；stuck_in_blocked 对快速循环失效，本规则补充覆盖
  - 💡 chronological walk 用 id order 而非 created_at（多事件同秒时 id 才是真正到达顺序）
- `kanban_diagnostics.py:triage_aux_status / _rule_triage_aux_unavailable` — 检查 triage 任务是否有可用 auxiliary model（decomposer/specifier）；provider=auto 时回落主 model 可见性；报 cli_hint action 告知如何配置
  - 💡 fail-silent 原则：config 不包含 auxiliary/kanban/model 字段时（如测试场景）返回 None 不报警，避免低层调用者被噪音淹没
- `kanban_diagnostics.py:DiagnosticAction / Diagnostic` — DiagnosticAction 按 kind(reclaim/reassign/unblock/cli_hint/open_docs/comment) 分类；suggested=True 标记首选恢复路径；Diagnostic 含 count/first_seen_at/last_seen_at 供时序展示
  - 💡 action.kind 驱动 dashboard 渲染 buttons + CLI 渲染 hints，同一数据结构双端复用；suggested flag 让 UI 高亮而无需前端硬编码规则
- `session_recap.py:build_recap` — 纯本地从 messages 列表计算 session 摘要：turn 统计+近期窗口+tool 用量 Counter+file 触碰记录+最后一条 user prompt/assistant reply
  - 💡 zero LLM call——recap 是即时免费操作；工具名→文件路径映射 (_FILE_EDIT_TOOLS) 支持多种工具方言；_coerce_text 兼容 str/list 内容块
- `kanban_db.py:kanban_home / board_dir / kanban_db_path / workspaces_root` — 多级 path 解析链：HERMES_KANBAN_* env → default board → 非 default board 独立目录；default board 向后兼容保持 <root>/kanban.db 路径
  - 💡 dispatcher 主动把 3 个 path env 注入 worker 子进程 env，防止 worker 与 dispatcher 路径解析分歧
- `kanban_db.py:_guard_existing_db_is_healthy / _backup_corrupt_db` — 首次连接时 PRAGMA integrity_check；corrupt 时 content-addressed sha256 备份（同一字节多次备份不放大磁盘）→ 抛 KanbanDbCorruptError，不静默覆盖
  - 💡 sha256 指纹备份名使并发 dispatcher + worker 重复触发同一 corrupt 只产生一个备份文件

### Web dashboard 鉴权 / PTY 桥 / proxy / TUI  ·  48 项

- `dashboard_auth/middleware.py:gated_auth_middleware` — FastAPI HTTP middleware，auth_required=True 时拦截所有请求；先过公开路由白名单，再读 session cookie 尝试每个 provider.verify_session，失败则走 _attempt_refresh 透明轮换 AT；HTML 路由 302→/login，API 路由 401 JSON 带 login_url
  - 💡 多 provider 顺序尝试，某个 provider 抛 ProviderError(IDP 不可达)不中断链，记录后继续下一个；区分 'transient IDP 不可达' vs 'token 真无效' 返回 503 vs 401
- `dashboard_auth/middleware.py:_unauth_response` — 区分 API 路径(返回含 login_url 的 401 JSON)与 HTML 路径(302 重定向 /login?next=...)；login_url 带 X-Forwarded-Prefix 感知
  - 💡 JSON envelope 中 error 字段区分 unauthenticated vs session_expired，让 SPA fetch-wrapper 无需解析细节即可全页跳转
- `dashboard_auth/middleware.py:_safe_next_target` — 构建 URL-encoded next= 参数；拒绝绝对 URL、双斜线开头、/login /auth/ /api/* 路径防 open-redirect 和 JSON 乱视
  - 💡 API 路径明确排除（成功后重定向到 /api/xxx 会显示裸 JSON）
- `dashboard_auth/middleware.py:_attempt_refresh` — 用 refresh_token 轮换 session；RefreshExpiredError → 返回 None 触发强制重登；ProviderError(网络临时故障) → 同样返回 None 走更安全的 re-login 而非 500
  - 💡 AT 过期后浏览器只发 RT cookie(AT Max-Age 到期浏览器自动删除)，这是 refresh 路径的 common case 非 edge case
- `dashboard_auth/ws_tickets.py:mint_ticket` — 为 WS 升级生成 32 字节随机 token，TTL=30s，存内存 dict；同时 GC 已过期 ticket
  - 💡 单次使用(consume 即 pop)；30s 设计刚好够 SPA 拿到 ticket 后立刻发起 WS 握手
- `dashboard_auth/ws_tickets.py:consume_ticket` — 验证并消费 ticket；单次语义：pop 即删；token 截断 8 字符后写日志防止 secret 泄漏
  - 💡 先 pop 再检查 expires_at，防时间窗口内重放(已 pop 找不到)
- `dashboard_auth/ws_tickets.py:internal_ws_credential` — 返回进程级永久 WS 凭证，懒初始化，只生成一次；给服务器自己 spawn 的子进程（PTY child）使用
  - 💡 永不过期、多次使用、从不注入 HTML/SPA；只通过子进程环境变量传递，XSS 无法读到
- `dashboard_auth/ws_tickets.py:consume_internal_credential` — constant-time 比较 (secrets.compare_digest) 验证 internal credential；不消费，允许多次 reconnect
  - 💡 区别于 consume_ticket 的一次性语义；返回固定 identity info dict 与 ticket path 保持接口一致
- `dashboard_auth/cookies.py:_resolved_name` — 根据 HTTPS + prefix 组合选择 cookie 前缀：HTTP→bare，HTTPS+prefix=/foo→__Secure-，HTTPS+prefix=''→__Host-
  - 💡 __Host- 要求 Path=/，与反代 prefix=/hermes 不兼容，需退化到 __Secure-
- `dashboard_auth/cookies.py:clear_session_cookies` — 对三种 cookie 前缀变体(bare/__Secure-/__Host-)全部发 Max-Age=0 删除，应对 setter 与 clearer 请求 shape 不一致的情况
  - 💡 不知道当初用哪种前缀 set 的，所以三种都清
- `dashboard_auth/cookies.py:set_session_cookies` — 设置 AT cookie(Max-Age=token TTL)和 RT cookie(Max-Age=30天)，RT 为空时跳过(provider 无 refresh token 的 degraded 模式)
  - 💡 AT 生命周期绑定 token 实际 TTL；RT 30天上限是 cookie 侧宽松上限，real authority 是 portal 24h
- `dashboard_auth/base.py:DashboardAuthProvider` — OAuth provider 插件抽象基类：start_login/complete_login/verify_session/refresh_session/revoke_session；密码登录可选扩展 supports_password + complete_password_login
  - 💡 failure 语义严格分层：None(unknown token)/ProviderError(网络故障)/RefreshExpiredError(RT dead)/InvalidCodeError(OAuth code 无效)
- `dashboard_auth/base.py:assert_protocol_compliance` — 在测试中调用，检查 provider 实现是否满足协议(name/display_name 非空 + 所有方法可调用 + 无未实现 abstract method)
  - 💡 强制每个 provider 单元测试调用此函数——harness-check 等价的协议机械校验
- `dashboard_auth/registry.py:register_provider / list_providers / get_provider` — 线程安全的全局 provider 注册表；register_provider 先调 assert_protocol_compliance；list_providers 返回注册顺序副本
  - 💡 插件通过 plugin context hook 调 register_provider；middleware 迭代 list_providers 首次验证成功即返回
- `dashboard_auth/routes.py:auth_callback` — OAuth callback：从 PKCE cookie 解析 provider/state/verifier/next；state mismatch 返回 400；complete_login 后写 session cookies，清 PKCE cookie，302 到 next 或 /
  - 💡 next= 从 cookie 读（IDP 不回传），callback URL 上的 next= query 参数被明确忽略（攻击者可控）
- `dashboard_auth/routes.py:auth_password_login` — 密码登录 POST；per-IP 滑动窗口限速(10次/60s)；失败不区分'用户不存在'vs'密码错误'；成功返回 JSON {ok, next} + set cookies
  - 💡 用 fetch POST(非表单 submit)，规避 302 被 fetch 透明跟随进入跨域 OAuth 流的问题
- `dashboard_auth/routes.py:api_auth_ws_ticket` — POST /api/auth/ws-ticket，已认证用户 mint 单次 WS ticket，返回 {ticket, ttl_seconds}
  - 💡 SPA 每条 WS 连接前单独 mint 一次 ticket，不复用
- `dashboard_auth/routes.py:_password_rate_limited` — per-IP 滑动窗口限速器；使用 time.monotonic 避免时钟回拨；未知 IP 共享 _unknown_ bucket(fail-safe 倾向限速)
- `dashboard_auth/login_page.py:render_login_html` — 服务端渲染登录页：无 React/JS 依赖(除密码 provider 时内联最小 script)；列出所有注册 provider 的 OAuth 按钮或密码表单；next_path 传递到每个按钮 href
  - 💡 next 先 URL-encode 再 HTML-escape(双层保护)；OAuth-only 页面完全无 script，密码页内联极小 fetch 脚本
- `dashboard_auth/prefix.py:normalise_prefix` — 清洗 X-Forwarded-Prefix：去 trailing slash，拒绝 .. 和注入字符，限制 64 chars
  - 💡 防 header injection 进 cookie Path 和 OAuth redirect_uri
- `dashboard_auth/prefix.py:resolve_public_url` — 三层 fallback：env HERMES_DASHBOARD_PUBLIC_URL → config.yaml dashboard.public_url → 空(让请求重建)；两层校验，一层坏不影响另一层
- `dashboard_auth/audit.py:audit_log` — JSON 追加写安全审计日志(~/.hermes/logs/dashboard-auth.log)；自动 redact token/code/cookie 等敏感字段；写失败 Warning 不 raise(不能因为审计日志坏掉阻断鉴权)
  - 💡 _REDACTED_FIELDS frozenset 枚举所有敏感字段名，kwargs 过滤后写 JSONL
- `pty_bridge.py:PtyBridge` — 封装 ptyprocess.PtyProcess 的 PTY 主控端；spawn/read/write/resize/close；POSIX-only 带优雅 Windows fallback(ImportError + PtyUnavailableError)
  - 💡 read 用 select() 非阻塞轮询 + timeout；write 用 memoryview 循环写防短写；resize 用 TIOCSWINSZ + 坐标 clamp(解决 WSL2 返回 columns=131072 的问题)
- `pty_bridge.py:PtyBridge.close` — SIGHUP→0.5s→SIGTERM→0.5s→SIGKILL 三段升级终止；调 proc.close(force=True) 防僵尸
  - 💡 每段 0.5s grace 等待 isalive()，不是硬 sleep
- `pty_bridge.py:_clamp_dimension` — 将 PTY 尺寸 clamp 到 [1, MAX_COLS/ROWS]；非法值(overflow, TypeError)退化到 1 而不是抛 struct.error
- `web_server.py:pty_ws` — WebSocket /api/pty：依次检查 auth→host/origin→peer IP；accept 后 spawn PtyBridge；asyncio.create_task 读 PTY→WS；主循环收 WS→写 PTY；识别 resize escape sequence 转 bridge.resize()
  - 💡 两路并发(reader task + writer loop)；resize 用正则匹配消费，不传给 PTY
- `web_server.py:_ws_auth_reason` — 区分三种 auth 模式：loopback(?token=SESSION_TOKEN 常量时间比较)，insecure(同)，gated(?ticket= 单次消费 or ?internal= 多次验证)；返回 (reason, cred_type)
  - 💡 gated 模式明确拒绝 ?token= 路径（SPA 不再携带 token）；internal 走 multi-use 路径供 PTY child reconnect
- `web_server.py:_build_gateway_ws_url` — 构建 PTY child 连接到 /api/ws 的 URL；loopback 用 token，gated 用 internal_ws_credential；inject 到子进程 env HERMES_TUI_GATEWAY_URL
  - 💡 child 读这个 URL 一次并在每次 reconnect 复用，所以不能用 30s 过期的 ticket
- `web_server.py:_ws_host_origin_reason` — WS 升级时验证 Host header 和 Origin header 防 DNS rebinding；Electron/file:// origin 豁免(非 web 客户端)
  - 💡 FastAPI HTTP middleware 不跑在 WS 路由上，所以 WS 需要自己的 Host 校验
- `web_server.py:should_require_auth` — 决策 OAuth gate 是否激活：host==loopback→no；non-loopback AND --insecure→no；non-loopback AND NOT --insecure→yes
  - 💡 RFC1918 被视为 PUBLIC，LAN 同机器也需要 auth gate
- `web_server.py:host_header_middleware` — HTTP middleware 校验 Host header 匹配绑定 interface；DNS rebinding 防护；0.0.0.0 bind 豁免(operator 已 opt-in)
  - 💡 GHSA-ppp5-vxwm-4cf7 明确提及为设计依据
- `proxy/server.py:create_app / handle_proxy` — aiohttp Web app：/v1/{tail} 转发给 upstream；先 filter hop-by-hop headers，替换 Authorization；stream 响应回客户端；401/429 时调 adapter.get_retry_credential() 重试一次
  - 💡 路径白名单(adapter.allowed_paths)；retry 仅一次；stream 用 iter_any() 保持 SSE
- `proxy/adapters/base.py:UpstreamAdapter` — proxy upstream 抽象接口：name/display_name/allowed_paths/is_authenticated/get_credential/get_retry_credential；UpstreamCredential dataclass 含 bearer+base_url+token_type+expires_at
- `proxy/adapters/nous_portal.py:NousPortalAdapter` — Nous Portal 具体 adapter：读 ~/.hermes/auth.json，调 resolve_nous_runtime_credentials 获取 inference JWT；force_refresh 触发重新获取；401 upstream 时 get_retry_credential 强制刷新
  - 💡 quarantine 逻辑：terminal 刷新失败时将 oauth state 隔离，防无效 token 无限重试
- `service_manager.py:ServiceManager (Protocol)` — init 系统抽象协议：start/stop/restart/is_running + 仅 s6 的 register_profile_gateway/unregister_profile_gateway/list_profile_gateways；runtime_checkable
  - 💡 supports_runtime_registration() 让调用方 capability check，host backend 直接 NotImplementedError
- `service_manager.py:S6ServiceManager` — s6-overlay 容器内 per-profile gateway 动态注册：写 service directory(run script + log/run + supervise skeleton)→ s6-svscanctl -a 触发自动 pickup；unregister 先 stop+wait，再 svscanctl -an reap，再 rmtree
  - 💡 _seed_supervise_skeleton 提前创建 hermes-owned supervise/ 目录，绕过 s6-supervise root 创建的 EACCES 问题
- `service_manager.py:S6ServiceManager._render_run_script` — 生成 s6 service run 脚本：with-contenv 读运行时 env → HOME 重置 → venv activate → s6-setuidgid 降权 → exec hermes gateway run；default profile 不带 -p 参数
  - 💡 HERMES_HOME 从运行时 env 读而非 Python 字符串替换，支持 -e HERMES_HOME=/data/hermes 动态挂载
- `service_manager.py:_s6_running` — 检测 s6-svscan 是否为 PID1：读 /proc/1/comm(world-readable)+ 检 /run/s6/basedir；不用 /proc/1/exe(非 root 无法读 symlink)
  - 💡 两个信号都要满足，防止单一信号误判
- `dashboard_register.py:cmd_dashboard_register` — 自动化 OAuth client 注册：resolve Nous AT → POST portal /api/oauth/self-hosted-client → 写 ~/.hermes/.env HERMES_DASHBOARD_OAUTH_CLIENT_ID；幂等；managed 环境快速失败
- `dashboard_register.py:_register_self_hosted_client` — 标准库 urllib(无依赖) POST portal API；结构化 HTTP error 处理(401/403 带具体提示)；超时 15s
- `curses_ui.py:_run_curses_menu` — 共享 curses 单/多选事件循环，callback 驱动：draw_header/draw_row/on_action/draw_footer；搜索通过 / 触发，ESC 清除 query；scroll 自动跟 cursor
  - 💡 非 TTY stdin 直接返回 cancel_value；curses 不可用时调 fallback()；on_action 返回 _KEEP 哨兵表示继续循环
- `curses_ui.py:read_menu_key / _decode_menu_key` — 解码 getch() 为 NAV_UP/DOWN/SELECT/TOGGLE/CANCEL/NONE；ESC 后 60ms 等 continuation bytes 判断是否箭头键序列而非裸 ESC
  - 💡 修复 keypad(True) 下部分终端仍然发 raw CSI 序列导致 ESC 被当 cancel 的 bug
- `curses_ui.py:_fuzzy_score / _token_score` — 模糊匹配评分：连续匹配+5，word boundary/first-char+3，prefix bonus+8，exact+20；忠实移植自 TypeScript fuzzy.ts 保持 CLI/Web/TUI 三端排序一致
  - 💡 AND 语义（所有 token 都要匹配），任一 token 不匹配返回 None
- `curses_ui.py:flush_stdin` — curses.wrapper 返回后用 termios.tcflush 清 stdin buffer；防止 escape 序列残留污染后续 input() 调用
  - 💡 curses.endwin() 不会清 OS 输入缓冲，这个 flush 是必须的
- `pt_input_extras.py:install_shift_enter_alias / install_ctrl_enter_alias` — 注入 CSI-u / xterm modifyOtherKeys 键序列到 prompt_toolkit ANSI_SEQUENCES，映射到 (Escape, ControlM) 实现 Shift+Enter/Ctrl+Enter 换行
  - 💡 部分序列 stock prompt_toolkit 已有但映射到错误 key，需要 overwrite；macOS Terminal 原生不发区分序列，无法在应用层修复
- `pt_input_extras.py:install_ignored_terminal_sequences` — 将 focus in/out 序列(ESC[I/O) 注册为 Keys.Ignore，防止 Ghostty/iTerm2 focus report 污染输入 buffer
  - 💡 用 setdefault 不强覆盖，让用户或下游注册优先
- `stdio.py:configure_windows_stdio` — Windows 上强制 UTF-8 stdio：SetConsoleCP(65001)，TextIOWrapper.reconfigure，设 PYTHONIOENCODING=utf-8，设 PYTHONUTF8=1；无 TERM 时设默认 EDITOR=notepad；PATH 补 Hermes managed tool dirs
  - 💡 无操作在非 Windows 平台，全局幂等
- `web_server.py:pub_ws / gateway_ws / events_ws` — 三条 WS 端点：/api/pub(PTY child→dashboard 事件 sidecar)，/api/ws(JSON-RPC gateway)，/api/events(dashboard→browser sidebar)；统一 _ws_auth_ok 鉴权

### 顶层引擎(cli/run_agent/state/压缩/批跑/mcp_serve/cron)  ·  81 项

- `run_agent.py:AIAgent.__init__` — Agent 主类构造器，接受 50+ 参数(model/provider/toolsets/callbacks/session_db/credential_pool/reasoning_config 等)，实际委托给 agent.agent_init.init_agent 执行
  - 💡 构造器是纯 forwarder，把全部参数转发给 agent_init 模块——大类拆包成多个独立模块，避免单文件超 5k 行
- `run_agent.py:AIAgent.run_conversation` — 主对话入口：接受 user_message/system_message/history/task_id/stream_callback，委托 agent.conversation_loop.run_conversation 执行，返回 {final_response, completed, failed, error, ...}
  - 💡 同样是 forwarder 模式；对话循环真正逻辑在 agent/conversation_loop.py
- `run_agent.py:AIAgent.chat` — 简化接口，调用 run_conversation 后只返回 final_response 字符串
- `run_agent.py:AIAgent._execute_tool_calls` — 分发 assistant 消息中的 tool_calls，支持 sequential 和 concurrent 两种执行模式
- `run_agent.py:AIAgent._execute_tool_calls_concurrent` — 并发执行多个 tool call（ThreadPoolExecutor），适用于无依赖的工具批
- `run_agent.py:AIAgent._execute_tool_calls_sequential` — 串行执行 tool calls，工具结果逐个注入 messages
- `run_agent.py:AIAgent.interrupt` — 设置中断标志，下一次循环迭代前检测，支持 message 附言
- `run_agent.py:AIAgent.steer` — 向正在运行的对话注入一条引导消息（非中断），下轮工具结果后插入
- `run_agent.py:AIAgent.get_activity_summary` — 返回 agent 活跃状态字典：seconds_since_activity/last_activity_desc/current_tool/api_call_count/max_iterations，供 cron inactivity 超时检测用
- `run_agent.py:AIAgent._touch_activity` — 每次 API 调用/流式 delta/工具执行时更新 last_activity_at 和描述，供超时检测读取
- `run_agent.py:AIAgent.close` — 释放 agent 持有的所有资源：openai client、browser、terminal 沙箱、httpx 连接池
- `run_agent.py:AIAgent._save_trajectory` — 把完成的对话转换为 from/value 格式并追加到 JSONL 轨迹文件，用于训练数据采集
- `run_agent.py:AIAgent._convert_to_trajectory_format` — 把内部 messages 列表转换成 {from, value} 轨迹格式
- `run_agent.py:AIAgent._persist_session` — 把当前 messages 批量写入 SessionDB，含工具调用计数和 token 统计
- `run_agent.py:AIAgent._flush_messages_to_session_db` — 增量刷新未 observed 的消息到 state.db，每条消息逐行 append_message
- `run_agent.py:AIAgent._build_system_prompt` — 组装最终 system prompt：soul identity + context files + memory + tool schema + cron hint 等多段拼接
- `run_agent.py:AIAgent.switch_model` — 在对话中途热切换 model/provider/api_key/base_url，重新创建 openai client
- `run_agent.py:AIAgent._run_codex_app_server_turn` — codex app-server 模式的单轮执行，委托 agent.codex_runtime
- `run_agent.py:AIAgent.reset_session_state` — 清空对话历史和 session_id，用于开启新会话而不重建 agent 对象
- `run_agent.py:_launch_cwd_for_session` — 判断当前 session source（cli/gateway/cron）决定是否记录 launch cwd 到 state.db
- `hermes_state.py:SessionDB.__init__` — 初始化 SQLite 连接(WAL+NFS fallback)，建表，运行 _reconcile_columns，启用 FTS5(降级处理)
- `hermes_state.py:SessionDB._execute_write` — BEGIN IMMEDIATE + 随机 jitter retry(20-150ms，最多 15 次)写事务封装，每 50 次写做一次 WAL TRUNCATE checkpoint
- `hermes_state.py:SessionDB._reconcile_columns` — 解析内存中 SQLite 对比 SCHEMA_SQL 和 live 表，自动 ALTER TABLE ADD COLUMN，实现无版本号的声明式 schema 迁移
- `hermes_state.py:SessionDB.apply_wal_with_fallback` — 尝试设置 WAL 模式，NFS/SMB 不兼容时 fallback 到 DELETE 模式，每 db_label 只 log 一次 warning
- `hermes_state.py:SessionDB.try_acquire_compression_lock` — 对 session_id 原子性 DELETE(expired)+INSERT OR IGNORE 加压缩锁，防止两个压缩路径竞态产生孤儿 child session
- `hermes_state.py:SessionDB.release_compression_lock` — 只有 holder 匹配时才删除压缩锁，防止 late-return 覆盖新持有者
- `hermes_state.py:SessionDB.list_sessions_rich` — 带 preview+last_active 的分页 session 列表查询，支持 compression 链递归 CTE 投影 tip、FTS search、archived 过滤
- `hermes_state.py:SessionDB.get_compression_tip` — 沿 parent_session_id + end_reason=compression 边走链找最新 continuation session id，最多走 100 步
- `hermes_state.py:SessionDB.append_message` — 向 messages 表插入一条消息记录，自动更新 session.message_count/tool_call_count，支持多模态内容 JSON 编码
- `hermes_state.py:SessionDB.update_token_counts` — 支持 absolute(网关 累计覆盖) 和增量(CLI per-call delta) 两种模式更新 token/cost 统计
- `hermes_state.py:SessionDB.finalize_orphaned_compression_sessions` — 扫描并关闭 7 天内 api_call_count=0 的孤儿压缩 continuation sessions，修复 orphaned_compression bug
- `hermes_state.py:SessionDB._encode_content/_decode_content` — 用 NUL 前缀哨兵(\x00json:) 把多模态 list/dict content 序列化为可绑定 SQLite 的字符串，读取时 decode 还原
- `hermes_state.py:SessionDB.sanitize_title` — 清理 session title：去 ASCII/Unicode 控制字符、折叠空白、截断到 100 字符
- `hermes_state.py:SessionDB.resolve_session_by_title` — 按 title 找 session，优先最新编号变体(title #N)，实现压缩链的用户可见名延续
- `trajectory_compressor.py:CompressionConfig` — 轨迹压缩配置 dataclass：tokenizer/压缩目标 token 数/保护段设置/摘要 LLM 配置/并发限制，支持从 YAML 加载
- `trajectory_compressor.py:TrajectoryMetrics` — 单条轨迹压缩指标：原始/压缩 token 数、轮数变化、压缩区间、API 调用次数、是否仍超限
- `trajectory_compressor.py:AggregateMetrics` — 批量压缩聚合指标，含 token/turn/ratio 分布统计，输出 JSON 报告
- `trajectory_compressor.py:TrajectoryCompressor.__init__` — 初始化 HuggingFace tokenizer + 摘要 LLM 客户端(OpenRouter/自定义 endpoint 按 URL 自动识别 provider)
- `trajectory_compressor.py:TrajectoryCompressor._find_protected_indices` — 找到需保护的头部轮(first system/human/gpt/tool)和尾部轮(last N turns)，返回可压缩中间区间
- `trajectory_compressor.py:TrajectoryCompressor.compress_trajectory` — 同步压缩单条轨迹：计 token→找保护区→贪婪积累到 savings_target→用 LLM 生成摘要替换被压缩段
- `trajectory_compressor.py:TrajectoryCompressor.compress_trajectory_async` — 同上，异步版本，使用 async_call_llm 生成摘要
- `trajectory_compressor.py:TrajectoryCompressor._generate_summary/_generate_summary_async` — 调用 LLM 生成 [CONTEXT SUMMARY]: 前缀的压缩摘要，带 jitter retry 和 fallback 静态文本
- `trajectory_compressor.py:TrajectoryCompressor.process_directory/_process_directory_async` — 并行处理整个目录 JSONL：asyncio.Semaphore 控并发(默认50)，per-entry 超时(300s)，rich progress bar 显示
- `trajectory_compressor.py:main` — CLI 入口：支持单文件/目录/采样百分比/dry_run 模式，用 fire 解析参数
- `cron/scheduler.py:tick` — 定时 tick 主函数：fcntl 文件锁防止并发、获取 due_jobs、先 advance_next_run 再分 sequential/parallel 两个线程池异步提交
- `cron/scheduler.py:run_job` — 执行单个 cron job：应用 per-job profile context 后调用 _run_job_impl
- `cron/scheduler.py:_run_job_impl` — cron job 核心执行：no_agent 短路(纯 bash)→ wake_gate 检测 → 组 prompt → 构建 AIAgent → inactivity 超时监控(轮询 get_activity_summary) → 结果投递
- `cron/scheduler.py:_build_job_prompt` — 组装 cron prompt：脚本输出注入、context_from(依赖其他 job 最新输出 8k 截断)、skills 加载(含 bundle 解析)、injection 扫描
- `cron/scheduler.py:_scan_assembled_cron_prompt` — 组装后 prompt 注入扫描：无 skills 走严格模式，有 skills 走宽松模式(invisible unicode sanitize 而非 block)，防 #3968 漏洞
- `cron/scheduler.py:_deliver_result` — cron 结果投递：优先走 live gateway adapter(支持 E2EE 如 Matrix)，降级走 standalone HTTP，支持 MEDIA: 媒体文件作为附件发送
- `cron/scheduler.py:_run_job_script` — 在 HERMES_HOME/scripts/ 沙箱内执行 .sh(bash) 或 .py 脚本，path traversal 防护，输出敏感信息 redact
- `cron/scheduler.py:_parse_wake_gate` — 解析脚本最后一行是否为 {wakeAgent: false} JSON，为 false 则跳过整个 LLM 调用
- `cron/scheduler.py:_job_profile_context` — ctx manager：per-job 临时切换 HERMES_HOME(profile)，快照/还原 os.environ，保证 profile jobs 串行不互相污染
- `cron/scheduler.py:_resolve_delivery_targets` — 解析 job.deliver 字段为具体投递目标列表，支持 origin/all/platform:chat_id 格式，all 展开为所有配置了 home channel 的平台
- `cron/scheduler.py:_submit_with_guard` — job_id 级别在途去重：同一 job 上次 tick 的 run 未完成时跳过本次提交，防止 job 堆叠
- `cron/scheduler.py:_get_parallel_pool/_get_sequential_pool` — 持久化 ThreadPoolExecutor：parallel pool 跨 tick 复用，sequential pool 单线程保证 workdir/profile job 串行
- `cron/jobs.py:get_due_jobs` — 读取 jobs.json，过滤 enabled + next_run_at <= now 的 job 列表，处理 oneshot grace window
- `cron/jobs.py:mark_job_run` — 写回 job 的 last_run_at/last_status/error，并用 croniter 重新计算下次运行时间
- `cron/jobs.py:advance_next_run` — 在 jobs 执行前先更新 next_run_at(防止 tick 文件锁外的重入)，mark_job_run 完成后覆盖
- `cron/jobs.py:save_job_output` — 把 job 输出 markdown 写到 OUTPUT_DIR/{job_id}/{timestamp}.md，路径安全校验防止 job_id path traversal
- `cron/jobs.py:_normalize_job_record` — 读时归一化 cron job dict：skill/skills 字段对齐、name/schedule_display 回填、state 推断
- `mcp_serve.py:create_mcp_server` — 用 FastMCP 注册 10 个 tool：conversations_list/conversation_get/messages_read/attachments_fetch/events_poll/events_wait/messages_send/permissions_list_open/permissions_respond/channels_list
- `mcp_serve.py:EventBridge` — 后台轮询 SessionDB(200ms)，维护有序事件队列(up to 1000 条)，支持 poll_events(cursor 分页) 和 wait_for_event(long-poll 阻塞)两种消费模式
- `mcp_serve.py:EventBridge._poll_once` — mtime 双重检查(sessions.json + state.db)，文件未变化时整个 poll_once 为 no-op，避免 200ms 轮询造成 I/O 压力
- `mcp_serve.py:EventBridge.respond_to_approval` — 解决一个 pending approval，发出 approval_resolved 事件，best-effort(无 gateway IPC)
- `mcp_serve.py:run_mcp_server` — 启动 EventBridge + FastMCP stdio 服务，KeyboardInterrupt 时优雅停止 bridge
- `mcp_serve.py:_extract_message_content` — 从 message dict 提取纯文本内容，兼容 list-of-parts 多模态格式
- `mcp_serve.py:_extract_attachments` — 从消息提取非文本附件：image_url/image block + MEDIA: tag regex 匹配
- `toolsets.py:get_toolset` — 按名称获取 toolset 定义 dict(description/tools/includes)，支持动态 plugin toolsets
- `toolsets.py:resolve_toolset` — 递归展开 toolset.includes 引用，去重后返回最终 tool name 列表，带环路检测
- `toolsets.py:resolve_multiple_toolsets` — 批量 resolve 多个 toolset 名，合并去重
- `toolsets.py:get_all_toolsets` — 返回内置 + plugin toolsets 的完整字典
- `toolsets.py:create_custom_toolset` — 运行时动态注册新 toolset
- `toolset_distributions.py:DISTRIBUTIONS` — 定义多种采样分布(default/image_gen/research/science/development/safe)：toolset_name → 被选概率(%)
- `toolset_distributions.py:sample_toolsets_from_distribution` — 按概率随机采样每个 toolset，返回当次批跑实际启用的 toolset 列表(用于训练数据多样性)
- `model_tools.py:get_tool_definitions` — 触发 tool 模块自注册 discover_builtin_tools()，按 enabled/disabled_toolsets 过滤，返回 OpenAI schema list
- `model_tools.py:_run_async` — 从同步上下文运行 async tool handler：无 running loop 走持久化 event loop，有 running loop 开子线程隔离，worker thread 用 thread-local loop
- `batch_runner.py:BatchRunner` — 并行批量跑 agent：multiprocessing Pool + 分批 checkpoint，支持 --resume 断点续跑，每批结果写 JSONL
- `batch_runner.py:_process_single_prompt` — worker 进程内处理单条 prompt：构建 AIAgent → run_conversation → 提取 tool_stats/reasoning_stats → 格式化轨迹
- `batch_runner.py:_normalize_tool_stats` — 把工具统计归一化为固定 schema(ALL_POSSIBLE_TOOLS 全集填 0)，保证 HuggingFace datasets 加载无 schema 不一致
- `mini_swe_runner.py:main` — SWE benchmark 单任务/批量跑：支持 local/docker/modal 执行环境，输出 Hermes from/value 格式轨迹

### 桌面壳 (apps/desktop) + 共享层 (apps/shared)  ·  35 项

- `apps/desktop/electron/main.cjs:loadInstallStamp` — 读取打包时写入的 install-stamp.json（含 commit/branch/builtAt），给 bootstrap runner 提供精确的 git ref，找不到时返回 null 而非 throw
  - 💡 先查 resources/ 再查 build/，双候选降级；schemaVersion 字段校验防止旧 JSON 被误用
- `apps/desktop/electron/main.cjs:resolveHermesHome` — 确定 HERMES_HOME 路径：Windows 优先 LOCALAPPDATA/hermes，若已有旧 ~/.hermes 则透明迁移；测试沙箱下放到 userData 子目录
  - 💡 Windows 遗留路径检测避免用户丢失已有配置，是跨平台安装路径统一的关键逻辑
- `apps/desktop/electron/main.cjs:resolveHermesBackend` — 6 级候选链：env override → dev checkout → bootstrap-complete marker → PATH hermes(带 --version 探针) → system python(带 import 探针) → bootstrap-needed sentinel
  - 💡 每级都有 smoke-test 防止烂候选进入 spawn；返回 sentinel 而非 throw，让 GUI 驱动 bootstrap 流程而非死路
- `apps/desktop/electron/main.cjs:ensureRuntime` — 对 backend 描述符做最后 runtime 校验并收尾：bootstrap-needed 时驱动 runBootstrap 并在成功后递归重解；bootstrap=true 时验证 venv python 路径并写入 backend.command
  - 💡 bootstrap 完成后递归调 resolveHermesBackend 而非硬编码新路径，保持单一解析路径
- `apps/desktop/electron/main.cjs:registerMediaProtocol` — 注册 hermes-media:// 自定义协议，仅允许音视频扩展名，委托 Electron net 做 Range 请求实现可拖动播放，绕过 16MB base64 限制
  - 💡 STREAMABLE_MEDIA_EXTS 白名单 + net.fetch bypassCustomProtocolHandlers 是防止任意文件读取的双重门控
- `apps/desktop/electron/main.cjs:applyUpdates` — 统一更新入口：Windows 走 hermes-setup.exe --update 并先 releaseBackendLockForUpdate 解锁 venv shim；macOS/Linux 走 applyUpdatesPosixInApp 内置流程（git pull + hermes desktop --build-only + bash swap 脚本）
  - 💡 Windows venv shim 文件锁是核心约束，releaseBackendLockForUpdate 用 taskkill /T /F + 轮询 O_RDWR 探针确保锁真正释放
- `apps/desktop/electron/main.cjs:applyUpdatesPosixInApp` — macOS/Linux 原地更新：运行 hermes update + hermes desktop --build-only，然后写入 shell swap 脚本（等父进程退出后 ditto 复制 .app bundle + xattr quarantine 解除 + open 重启）
  - 💡 detached bash 脚本等待父 PID 消失再操作 bundle，避免 Electron 自我替换竞态
- `apps/desktop/electron/main.cjs:releaseBackendLockForUpdate` — Windows 专用：收集所有后端 PID（主+pool），SIGTERM + taskkill /T /F 树杀，轮询 venv shim O_RDWR 直到解锁或 15s 超时
  - 💡 仅 Windows 有强制文件锁；POSIX 直接返回 unlocked:true，无 SIGKILL 改动
- `apps/desktop/electron/main.cjs:resolveHealedBranch` — 自愈分支：用 ls-remote --exit-code 探测 origin 上分支是否存在，若已删（exit 2）自动回退 main 并持久化配置
  - 💡 只在 exit 2（ref absent）时切换，网络错误不触发；防止 bb/gui 等临时分支合并后客户端卡在已消失的分支
- `apps/desktop/electron/main.cjs:findSystemPython (Windows)` — 三级探测：PEP 514 注册表 → 标准安装目录（Program Files/LocalAppData）→ py.exe 带版本参数；限定 3.11-3.13，排除 MS Store stub 和 3.14
  - 💡 不回退到 PATH 裸 python.exe，防止 MS Store Store-popup 和 3.14 Rust 编译失败
- `apps/desktop/electron/main.cjs:writeFileAtomic` — 原子写文件：先写 .tmp 再 rename，防止 crash/断电时 JSON 配置文件部分写入损坏
  - 💡 极简一行实现，应用于所有配置文件写入
- `apps/desktop/electron/main.cjs:broadcastBootstrapEvent` — bootstrap 事件聚合器：维护 bootstrapState（manifest/stages/log ring-500/error），同时广播到 renderer webContents
  - 💡 renderer 重载后可用 hermes:bootstrap:get IPC 查询快照恢复状态，避免刷新后丢失进度
- `apps/desktop/electron/main.cjs:updateBootProgress` — 合并 boot 进度 patch，progress 默认不允许下降（allowDecrease option），广播到 renderer
  - 💡 防止进度条倒退的 monotonicProgress 保证
- `apps/desktop/electron/main.cjs:createPythonBackend / createActiveBackend` — 构造 backend 描述符（kind/command/args/env/root/bootstrap）；createActiveBackend 优先 venv python 但降级到 system python（venv 不存在时），bootstrap=true
  - 💡 描述符驱动 spawn，所有路径收敛到同一 ensureRuntime 处理，无重复 spawn 逻辑
- `apps/desktop/electron/main.cjs:backendPool (multi-profile)` — 按 profile 维护额外后端池（最多 POOL_MAX_BACKENDS=3），LRU eviction + idle reaper(POOL_IDLE_MS=10min)，活跃判断用 POOL_KEEPALIVE_FRESH_MS=90s
  - 💡 primary 后端不进 pool；pool 专为非 primary profile 的懒启动设计，避免一个 profile 会话杀掉正在运行的另一个
- `apps/desktop/electron/connection-config.cjs:resolveTestWsUrl` — 按 authMode 决定 WS URL：token 模式直接拼 ?token=；oauth 模式调 mintTicket（injection） 拿 ?ticket=，mint 失败时 THROW 而非 skip
  - 💡 oath mint 失败必须 throw 而非返回 null，否则 HTTP 探针通过但 WS 认证失败导致假阳性 reachable
- `apps/desktop/electron/connection-config.cjs:cookiesHaveLiveSession` — 判断 cookie jar 是否有可用 session：AT cookie（~15min）OR RT cookie（24h），AT 过期但 RT 在则仍为 live
  - 💡 AT-only 检查会每 15min 强制重登，RT 检查才对应真实会话生命周期
- `apps/desktop/electron/main.cjs:looksBinary` — 用 null byte 和 12% 控制字符比例启发式判断文件是否为二进制，用于 preview 文本路由
  - 💡 纯 buffer scan，无外部依赖，tab/LF/CR 不计入 suspicious
- `apps/desktop/src/lib/incremental-external-store-runtime.ts:IncrementalExternalStoreThreadRuntimeCore` — 覆盖 @assistant-ui/react 的 ExternalStoreThreadRuntimeCore，__internal_setAdapter 时用 syncRepositoryIncrementally 做增量 diff 而非整体替换
  - 💡 避免流式输出时每条消息触发 O(N) 全量 re-render；shallowEqual 保护 capabilities/suggestions 不必要重渲染
- `apps/desktop/src/lib/incremental-external-store-runtime.ts:syncRepositoryIncrementally` — 对 messageRepository 做增量同步：addOrUpdateMessage 新增/更新，再从 repository.export() 中删除不在 incoming 中的消息，最后 resetHead
  - 💡 O(N+M) 而非 O(N*M)，保证长会话中滚动位置不被重置
- `apps/desktop/src/store/panes.ts:$paneWidthOverride / $paneOpen / $paneState` — 用 nanostores computed + Map 缓存 per-pane 派生 atom，保证 useStore 订阅引用稳定
  - 💡 widthOverride 仅存内存（注释标注 phase 2 再加 persistWidth）；open 状态持久化到 localStorage
- `apps/desktop/src/app/right-sidebar/files/use-project-tree.ts:useProjectTree` — 懒加载目录树：全局 atom $projectTree 存储 TreeNode 树，loadChildren 按需 fetch 并 patchNode 就地更新子树；collapseAll 递增 nonce 强制 react-arborist 重挂而不清缓存
  - 💡 inflight Set 防止同一目录并发请求；requestId 竞态保护：切 cwd 后旧请求结果被忽略
- `apps/desktop/src/app/right-sidebar/terminal/use-terminal-session.ts:useTerminalSession` — 基于 xterm.js(WebGL) 完整 PTY 终端：ResizeObserver rAF 合并 fit，drag-drop 文件路径自动 shell-quote 注入，selection 浮动按钮「Add to chat」，stripInitialPromptGap 过滤启动时空白
  - 💡 WebGL renderer 在 SGR 着色上比 DOM renderer 更准确；scheduleResize 用 rAF 合并防止 WebGL mid-rebuild crash
- `apps/desktop/src/app/cron/index.tsx:CronView` — Cron 任务管理页：列出所有 job（搜索/排序），支持 pause/resume/trigger/create/edit/delete；scheduleOptionForExpr 把 cron 表达式反向匹配到预设选项；scheduleSummary 生成人类可读描述
  - 💡 delivery channel 支持 local/telegram/discord/slack/email，体现 hermes cron 内置通知路由
- `apps/desktop/src/app/settings/model-settings.tsx:ModelSettings` — 模型设置页：主模型 provider+model 二级下拉；8 个辅助任务（vision/compression/mcp/title_generation...）各自独立 override，支持「Reset all to main」和逐条「Set to main」/「Change」
  - 💡 辅助任务 scope=auxiliary+task 的 API 设计允许不同子任务用不同模型，context compression 和 vision 可以用便宜模型节省成本
- `apps/desktop/src/app/profiles/index.tsx:ProfilesView + SoulEditor` — Profile 管理页：左侧列表+右侧详情，SOUL.md 内嵌编辑器（Textarea + 脏标记 + 保存），copySetup 生成安装命令到剪贴板，clone_from_default 选项
  - 💡 SOUL.md 作为 persona/system prompt 的人类可读文件暴露在 UI 中直接编辑，是「给 AI 写 soul」的 UX 模式
- `apps/desktop/src/store/activity.ts:buildRailTasks` — 聚合三类活动为统一 RailTask 数组：working session tasks + preview server restart + desktop action tasks，按 updatedAt 排序
  - 💡 completed 任务有 5min TTL prune + HISTORY_LIMIT=8 防止无限增长
- `apps/desktop/src/app/shell/statusbar-controls.tsx:StatusbarControls` — 底部状态栏：左右两区域，支持 text/link/action/menu 四种 item variant；menu variant 用 DropdownMenu，overflow-x-clip 防止滚动条
  - 💡 SetStatusbarItemGroup 回调接口允许子页面/pane 注册自己的状态栏条目，实现页面级 slot 注入
- `apps/desktop/src/app/shell/app-shell.tsx:AppShell` — 顶层 shell：CSS 变量计算 titlebar/sidebar/file-browser 的宽度布局，支持 macOS traffic-light 偏移和 Windows native overlay 宽度；-webkit-app-region:drag 精确到 pixel 的无边框拖动区域
  - 💡 titlebarToolsWidth 数学公式随 pane tool 数量动态计算，防止工具按钮落入系统窗口按钮下方
- `apps/desktop/src/components/pane-shell/pane-shell.tsx:PaneShell` — 可拖拽多面板布局容器：left/right pane + main 区域；ResizeObserver + pointer capture 实现拖拽调宽；emits CSS --pane-{id}-width 变量供子树消费
  - 💡 disabled prop 让路由层临时隐藏 pane 不写 store，与持久化 open 状态解耦
- `apps/shared/src/json-rpc-gateway.ts:JsonRpcGatewayClient` — 全功能 JSON-RPC over WebSocket 客户端：连接状态机（idle/connecting/open/closed/error），connectTimeout(15s) 防僵尸 connecting，pending Map 追踪 request→promise，on/onAny/onEvent 事件订阅
  - 💡 connectTimeout 期满时主动 close socket 并清空 pending，防止 sleep/wake 后 UI 永远卡在 connecting 状态
- `apps/desktop/electron/bootstrap-runner.cjs:runBootstrap` — 驱动 install.ps1/install.sh 逐 stage 执行，解析 JSON lines 输出为 manifest/stage/log/complete/failed 事件流，支持 AbortSignal 取消
  - 💡 install 脚本的 JSON lines 协议把安装进度变成结构化流，renderer 可做精确 checklist 而非盲等
- `apps/desktop/electron/main.cjs:rememberLog` — 内存日志 ring（300行）+ 异步 append 到 desktop.log，64KB buffer 满时触发立即 flush，否则 120ms 定时批量写
  - 💡 日志与 hermesLog（内存）和磁盘并行，crash 时 hermesLog 可在 IPC 上发回 renderer 辅助诊断
- `apps/desktop/src/app/settings/gateway-settings.tsx:GatewaySettings` — gateway 连接配置页：local/remote 模式切换；remote 支持 token（静态）和 oauth（HttpOnly cookie + ws-ticket）两种鉴权；包含 connection probe（HTTP + WS 全链路测试）
  - 💡 oauth 模式的 ws-ticket 机制解耦了 REST session cookie 和 WS 认证，支持 HttpOnly 安全策略
- `apps/desktop/src/app/chat/right-rail/preview-pane.tsx:PreviewPane` — 右侧预览面板：Electron webview 渲染本地或远程 URL；watch 文件变化自动 reload；onRestartServer 触发 preview server 重启（45s timeout）；loadError 状态机区分 MIME 错误/连接拒绝/通用失败
  - 💡 webview 元素扩展了 openDevTools/reload/getURL 等方法，通过 ref 直接调用

## 3. 页面元素穷举

### CLI kanban / swarm 多 agent 任务分解 / goals

#### kanban board CLI (hermes kanban ...)
- **元素**：状态图标 ◻▶●⏱⊘✓— 前缀任务行；--json 开关切换机读输出；--board 全局 flag；boards 子命令(list/create/rm/switch/show/rename/set-default-workdir)；任务子命令(create/list/show/complete/block/unblock/reclaim/reassign/archive/link/unlink/events/comments/comment/log/diagnostics/boards/swarm/decompose/specify/describe/attach/daemon)
- **交互**：hermes kanban boards switch <slug> 写 <root>/kanban/current；hermes kanban create --workspace worktree:<path> 指定隔离工作区；hermes kanban daemon 启动独立 dispatcher（已被 gateway 内嵌取代）
- **UX 细节**：--json 输出与人类可读输出共存；dispatcher 存在性检测(_check_dispatcher_presence) 在 create 后给出 'gateway is running / no gateway running' 提示，引导用户执行 hermes gateway start

#### swarm CLI (hermes kanban swarm)
- **元素**：--goal / --worker profile:title[:skill,skill] (可多次) / --verifier-assignee / --synthesizer-assignee / --priority / --dry-run
- **交互**：parse_worker_arg 解析 profile:title:skills 三段式；create_swarm 原子写入 4 层 kanban 拓扑；dry-run 输出拟创建拓扑但不写 DB
- **UX 细节**：swarm 结果打印 root_id/worker_ids/verifier_id/synthesizer_id，操作员可立即用这些 id 跟踪进度

#### goal loop CLI (/goal /subgoal slash commands)
- **元素**：/goal <text> 设置 goal；/goal status 显示 ⊙/⏸/✓ 状态行；/goal pause/resume/clear；/subgoal <text> 追加子目标；/subgoal list/rm <n>/clear
- **交互**：GoalManager.evaluate_after_turn 在每次 turn 后自动驱动；continuation prompt 由 next_continuation_prompt() 生成注入下一轮
- **UX 细节**：status_line 格式 '⊙ Goal (active, 3/20 turns, 2 subgoals): ...' 一行内含全量状态；auto-pause 时输出具体配置 yaml 片段指导修复弱 judge model

### Web dashboard 鉴权 / PTY 桥 / proxy / TUI

#### /login 服务端渲染登录页
- **元素**：品牌 wordmark(Nous Research + 方形 dot)；卡片容器(inset bevel shadow)；h1 Sign in；subtitle 提示文字；provider-list 栅格布局；OAuth provider: .provider-btn 超链接按钮(amber 背景，uppercase，hover filter:brightness，active filter:invert，focus-visible 轮廓)；密码 provider: .provider-form 表单含 username 输入/password 输入/.form-error[role=alert]错误区/submit 按钮；页脚 footer(分隔线 + Public bind · Auth required)；Sign-in unavailable 空页(无 provider 时)
- **交互**：OAuth provider 点击 → GET /auth/login?provider=N[&next=...] → IDP 跳转；密码表单 submit → fetch POST /auth/password-login(JSON) → 200 时 window.location.assign(data.next)；429 显示'Too many attempts'，401 显示'Invalid username or password'，网络错误显示 Network error；按钮 disabled 在飞行中
- **UX 细节**：OAuth 页完全无 JS(script-free)，密码页仅内联极小 fetch 脚本；无 React 无 SPA bundle 依赖，pre-auth 可渲染；字体从 /fonts/ 预鉴权路径加载；slide-up 动画(prefers-reduced-motion 豁免)；CSS 选择 ::selection 匹配品牌色

### 桌面壳 (apps/desktop) + 共享层 (apps/shared)

#### Bootstrap/Install Overlay（首次安装进度）
- **元素**：分阶段 checklist（manifest.stages），每阶段 pending/running/succeeded/skipped/failed 状态图标；log ring「Show details」展开 500 行输出；Copy output 按钮；进度百分比条；Cancel 按钮（中止 install）；Reload and retry 按钮（清除 latch failure）；「不支持平台」专属状态展示 installCommand
- **交互**：实时接收 hermes:bootstrap:event IPC 事件更新 checklist；renderer 刷新后调 hermes:bootstrap:get 快照恢复状态
- **UX 细节**：单调进度保证（不倒退）；阶段状态颜色编码；阶段 durationMs 展示帮助定位慢步骤

#### Boot Progress Screen（启动等待）
- **元素**：文字状态消息（phase + message）；进度圆环/条；错误展示区（含 recentHermesLog 最近 20 行）
- **交互**：接收 hermes:boot-progress IPC 实时更新
- **UX 细节**：BOOT_FAKE_MODE 支持 UI 演示模式（每步间隔 650ms）

#### AppShell + Titlebar
- **元素**：macOS: 左侧 traffic lights + titlebar 拖动区（精确到像素）；Windows/Linux: 右侧 native overlay buttons 预留 144px；左侧 session sidebar 折叠按钮；右侧工具集群（haptics/profiles/settings/file-browser）；pane 工具集群（由当前页面注入）
- **交互**：-webkit-app-region:drag 区域精确排除所有按钮；fullscreen 时 traffic lights 高度自适应
- **UX 细节**：--titlebar-content-inset 在 sidebar 关闭时自动扩展拖动区域避免内容被 traffic lights 遮挡

#### Statusbar（底部）
- **元素**：左右两区：text/link/action/menu 四种 item；menu 项支持 submenu icon + 路由 navigate；overflow-x-clip 不显示横向滚动条
- **交互**：子页面通过 SetStatusbarItemGroup 动态注入 items；menu variant 用 Radix DropdownMenu side=top 向上弹出
- **UX 细节**：item.detail 字段显示辅助信息（如当前 cwd），不同于 item.label 的主信息

#### Cron 管理页
- **元素**：顶部搜索框 + refresh 按钮；活跃/总数统计标签；「New cron」按钮；任务行（标题+状态 pill+delivery badge，cron 表达式+last/next 时间，last_error 展示）；右侧 ContextMenu（pause/resume/trigger/edit/delete）；CronEditorDialog（name 可选/prompt 必填/频率预设下拉/delivery 下拉/custom cron 输入/人类可读预览）；DeleteConfirmDialog
- **交互**：行点击→编辑；频率预设切换时同步 cron 表达式；custom 模式显示原始 input；trigger 立即运行
- **UX 细节**：状态 pill 四色编码（good/warn/muted/bad）；自然语言时间显示「Every day at 9:00 AM」；delivery 非 local 时才显示 badge

#### Model Settings 页
- **元素**：主模型：provider 下拉 + model 下拉 + Apply 按钮；8 辅助任务列表（vision/web_extract/compression/skills_hub/approval/mcp/title_generation/curator）：每行显示当前 provider·model 或 auto·use main；「Set to main」快捷按钮/「Change」展开行内 provider+model 二级下拉；「Reset all to main」全量重置
- **交互**：Apply 主模型时回调 onMainModelChanged 通知 live UI；辅助任务行内编辑不 modal
- **UX 细节**：hint 字段（Image analysis/Context compaction...）帮助用户理解任务用途；isAuto 显示「auto · use main model」而非空

#### Gateway Settings 页
- **元素**：Local/Remote 模式切换卡片；Remote 模式：URL 输入框/token 静态输入+preview（显示后 6 位）/oauth Sign In 按钮（打开 IDP）+Connected 状态/Test Connection 按钮（HTTP+WS 全链路探测，显示延迟）；per-profile remote override 列表
- **交互**：连接测试分阶段显示 HTTP reachable → WS open；oauth 使用 Electron session cookies 判断 live session
- **UX 细节**：token preview 只显示后 6 位保护安全；RT cookie 存在时即视为有效避免每 15min 重登

#### Profiles 管理页
- **元素**：左列：profile 列表（name/skill_count/.env badge/default 标记）+「New Profile」按钮；右列：详情（model·provider/技能数）+SOUL.md 内嵌编辑器（Textarea + 脏标记 + Save）+Rename/Copy Setup Command/Delete 操作按钮；CreateProfileDialog（name 验证 regex + clone_from_default checkbox）；RenameProfileDialog
- **交互**：SOUL.md dirty 时 Save 按钮 active；Copy Setup Command 生成 hermes profile setup 命令并写剪贴板；clone_from_default 继承默认 profile 配置
- **UX 细节**：profile path 以 font-mono 全路径展示（title 属性悬停）；default badge 小标且不可删除/重命名

#### Terminal Tab（右侧边栏）
- **元素**：xterm.js WebGL 终端（font SF Mono/Menlo/JetBrains Mono，11px，scrollback 1000）；顶部 shellName 标签（bash/zsh/pwsh 等）；全屏/恢复按钮（screen-full/screen-normal）；选中文字时浮动「Add to chat」按钮（含快捷键提示）；启动中 spiral-search loading 动画
- **交互**：文件从 file browser drag-drop 到终端自动 shell-quote 路径注入；Cmd+Shift+X（或自定义）快捷键发送选区到 chat；ResizeObserver rAF 合并 fit 防止 WebGL mid-rebuild crash
- **UX 细节**：stripInitialPromptGap 过滤启动时的空白行防止 xterm 初始空白；WebGL renderer SGR 颜色比 DOM 准确

#### File Browser（右侧边栏/左侧 pane）
- **元素**：react-arborist 虚拟目录树（懒加载，placeholder Loading... 节点）；expand/collapse 三角；Refresh/Collapse all 按钮；drag source（拖入 terminal 自动 quote 路径）；EACCES 等错误显示在节点旁
- **交互**：首次展开时 loadChildren fetch，inflight Set 防并发；cwd 变化自动 refreshRoot；collapseNonce 强制 react-arborist remount 但保留 data 缓存
- **UX 细节**：openState 独立于 data（collapse 不清数据），下次展开秒开

#### Preview Pane（chat 右侧 right-rail）
- **元素**：Electron <webview> 渲染本地或远程 URL；顶部 URL 地址栏 + reload/hard-reload/DevTools 按钮；底部可拖拽 console 面板（日志过滤，高度可调）；loadError 状态（Server not found/Preview app boot fail/generic）+ Restart server 按钮（45s timeout）；LocalFilePreview 用 shiki 语法高亮文本文件
- **交互**：文件变化 watch（120ms debounce）自动 reload；webview reloadIgnoringCache 强制刷新；onRestartServer 触发 hermes preview server 重启
- **UX 细节**：isModuleMimeError 识别 Vite/ESM MIME 类型错误给出专属提示；console panel 近底自动滚动

#### Composer（聊天输入框）
- **元素**：富文本 contenteditable editor（inline @-ref / /slash 补全）；附件预览列表；语音录音按钮（use-mic-recorder/use-voice-recorder）；@-completion popover（文件/agent/文档引用）；/slash popover（built-in + profile skills）；queue panel（排队待发送消息）；URL dialog（粘贴 URL 时触发）；help hint（键盘快捷键提示）
- **交互**：@ 触发引用补全；/ 触发 skill 命令；shift+enter 换行/enter 发送；拖放图片/文件；voice conversation 模式
- **UX 细节**：use-live-completion-adapter 实现实时 @ 补全；inline-refs 把 mention 渲染为 chip 样式而非原始文本

#### Command Palette
- **元素**：全局 cmdk 搜索框；分组命令（session 操作/设置跳转/profile 切换/皮肤切换...）；快捷键 badge
- **交互**：Cmd+K 唤起；Escape 关闭；Enter 执行命令
- **UX 细节**：页面路由命令直接 navigate；外部命令回调方式解耦

## 4. flockmux 借鉴小结（本 repo）

| 优先级 | 借鉴点 | 价值 | 工作量 | 落到 flockmux 哪里 |
|---|---|---|---|---|
| P0 | 上下文压缩策略：结构化 14 段摘要 prompt + SUMMARY_PREFIX 指令设计 | flockmux worker 跑长任务时 PTY session 会话历史无限增长（JSONL tail），用这套结构化摘要方法可以触发 /compact 或到达阈值自动压缩，保留 Active Task/Completed Actions/Blocked 等关键字段而不丢失任务上下文 | 中：Rust 调辅助模型（已有 call_llm 等价的 /api/chat 路由），参照 SUMMARY_PREFIX 文本和 14 段 template 复制到 flockmux 的 orchestrator prompt，触发时机放在 worker 会话 JSONL token 估算超 threshold 时 | backend/src/orchestrator.rs 或新增 context_compressor.rs；写 /api/compress 端点供前端 /compress 命令调用；SUMMARY_PREFIX 直接翻译为 Rust String constant |
| P0 | 工具调用护栏：exact_failure / same_tool_failure / idempotent_no_progress 三维检测 | flockmux worker 最常见的 hang 是工具死循环（bash 命令重试/search 无进展），护栏可在 PTY 注入侧检测 tool_result 序列，warn 时 inject 指导文字，block 时注入 stop 信号或通知 orchestrator 自愈重派 | 中：Rust 实现 ToolCallGuardrailController 等价结构体，读 transcript.rs 中广播的 AgentActivity（已有工具级活动数据），在 wake.rs 或新增 guardrail.rs 做判断；PTY inject 注入警告文字已有基础（wake_coordinator.rs inject） | backend/src/guardrail.rs（新建）；transcript.rs 提供工具名+结果事件；wake.rs 或 agent_runtime.rs 在 after_tool_call 点调用；inject 路径已有 PTY inject 基础设施 |
| P0 | normalize_usage 跨 API shape 的 token 计量归一化 | flockmux 已有实时进度(读 JSONL)但缺 Usage/Cost 可观测(MEMORY 里标记的 P0 空白)。hermes 的 normalize_usage 三路分支处理 Anthropic/Codex Responses/Chat Completions 三种 usage 字段差异非常实用。flockmux 可以直接移植逻辑，把每次 worker 完成时的 token 统计归一化存库 | S(2-3小时，纯计算逻辑，无外部依赖，Rust 实现比 Python 更简单) | transcript.rs 或新建 usage.rs。DB 新增 agent_usage 表存 per-turn 统计，前端 Usage 面板消费 |
| P0 | PricingEntry 官方文档定价快照 + estimate_usage_cost | MEMORY 里 P0 空白：Usage/Cost 可观测。hermes 的静态定价表(Anthropic/OpenAI/DeepSeek/Google/Bedrock/MiniMax 主流模型 $/M token)可直接移植为 Rust const 表，estimate_usage_cost 逻辑也极简单。让 flockmux 每次 spawn worker 后能展示估算成本，帮用户控制开销 | M(含 UI：后端1天定价表+计算，前端0.5天展示)。可拆分：先只做后端日志，再做前端展示 | spawn 完成回调→写 usage 表→WS 推送→前端 Usage 面板。定价表用 include_str!("pricing.toml") 嵌入二进制，不依赖网络 |
| P0 | InsightsEngine：token/cost/tool 使用量统计报告 | flockmux MEMORY 中明确标注 'Usage/Cost 可观测' 是已知空白。hermes 的 InsightsEngine 提供了完整的 SQL 查询模板：session 表 + messages 表双源合并工具调用计数、per-model cost 估算、活跃时段热图 | 中（SQL 查询可直接参照，flockmux 已有 SQLite，需加 billing_provider/cost 字段到 sessions 表 + 前端展示面板） | 后端新增 /api/insights 端点 + GET /api/sessions 聚合查询；前端在设置页或单独「用量」页展示；成本估算参考 hermes usage_pricing 的 CanonicalUsage 结构 |
| P0 | 安全护栏分层（write-deny/read-block/软警告）移植到 flockmux 文件操作工具 | flockmux worker 在 PTY 里跑 claude/codex，已有 --dangerously-skip-permissions，但没有对 .env/.ssh/hermes 自身配置的写保护。借鉴 file_safety.is_write_denied() + is_write_denied() 的精确路径 set + 目录前缀 list 做 MCP file tool 的写拦截 | small — Rust 可直接翻译该逻辑，不涉及异步 | src/tools/file_guard.rs（新文件）；接入 MCP tool_executor 在写文件前调用；.env/.ssh/workspace auth 文件进 hard deny，flockmux DB/config 进 soft warn |
| P0 | todo 结果投影为 WS plan 事件（台账升级为实时任务面板） | flockmux 台账页是 MEMORY 里说的 P0 升级点。照 _build_plan_update_from_todo_result 思路：agent 每次更新 todo/任务状态时，额外推一条 WS PlanUpdate 事件，前端任务面板实时刷新而不依赖轮询 /api/blackboard。 | 中（transcript.rs 的 JSONL tail 已有 AgentActivity 广播，在 AgentActivity 里识别 todo tool 结果 + 解析 JSON + 生成 PlanUpdate WS 事件） | transcript.rs AgentActivity 广播 → 新增 PlanUpdate WS event type；前端 blackboard 页订阅 plan_update 消息，替换现有手动刷新 |
| P0 | Usage/Cost 可观测：token 用量 estimate + context 压力 ACP UsageUpdate | flockmux 已知空白：Usage/Cost 可观测。hermes 的 _build_usage_update 用 estimate_request_tokens_rough(history + system_prompt + tools) 实现轻量估算，不需要调 API，可以本地算。给前端 context 指示器/Token 计费页提供数据。 | 中（Rust 侧估算 token 可用 tiktoken-rs 或简单 chars/4 估算；读 session history from DB；推 WS UsageUpdate 事件） | 新建 usage_estimator.rs：读 SessionHistory + SystemPrompt + enabled MCP tools schemas，估算 input tokens；transcript.rs 在 turn 结束后推 WS UsageUpdate；前端设置页/蜂群页展示 context 压力环形图 |
| P0 | typed stream event 词汇表（MessageChunk/ToolCallChunk/GatewayNotice 等 frozen dataclass）+ GatewayEventDispatcher | flockmux 当前 transcript.rs tail 广播 AgentActivity 是纯字符串，工具调用和 agent 文本混在一起。引入 typed events 后，前端可以精确渲染工具进度 vs 文本流 vs 网关通知，adapter（未来 Slack/Telegram 插件）可以 eat 掉不支持的事件类型而不影响历史 | 中（定义 Rust enum StreamEvent，替换 AgentActivity broadcast，前端按 type switch 渲染） | transcript.rs + WS broadcast 层：AgentActivity 升级为 StreamEvent enum；前端 swarm 成员栏活动行按 ToolCallChunk/MessageChunk 分别显示工具名+参数预览 vs 文本流 |
| P0 | Session guard 先于 task spawn + stale-lock 自愈（sequentialize 模式） | flockmux 目前 worker spawn 有并发窗口：同一 session 连续两条消息可能都通过 active_check。参照 hermes _start_session_processing 在 spawn 前同步置 guard；_heal_stale_session_lock 在入站时检测僵死 guard | 小（在 spawn 路径加同步 guard set；定期或入站时检测 owner task 是否仍 alive） | agent.rs / orchestrator.rs spawn 路径：active_sessions HashSet<session_key> 在 tokio::spawn 前置位；actor 退出时 finally 清除；heal 在下一条消息入站时触发 |
| P0 | Dispatcher circuit breaker：consecutive_failures 计数 + per-task max_retries 覆盖 + 自动 block（gave_up 事件） | flockmux 目前 .error fallback 只做一次自愈重派，没有 N 次失败后停止的断路机制。hermes 的 consecutive_failures + effective_limit(per-task > global) 可以防止 broken task 无限 respawn 消耗 token | 2-3 天：在 agent DB 加 consecutive_failures 列；spawn 成功时清零；failed 时递增+比较 limit；超限写 blocked 状态 | backend/agents.rs + spawn 路径：在现有 spawn_worker / record_agent_event 里加 failure counting；给 orchestrator 露出 max_retries 字段 |
| P0 | WS 鉴权：单次 browser ticket (POST /api/auth/ws-ticket) + 进程级 internal credential | flockmux 当前 WS 没有鉴权，一旦支持 --host 公网绑定就是安全漏洞；browser ticket 解决浏览器无法在 WS upgrade 设 Authorization 的根本问题；internal credential 解决服务器自己 spawn 的 WS client(如 JSONL watcher) reconnect 问题 | 后端约 200 行 Rust(in-memory HashMap + secrets::token_urlsafe + 30s TTL GC)；前端每条 WS 连接前 POST 一次 ws-ticket，约 50 行 JS；改现有 /api/ws /api/events 鉴权入口 | 在 routes/ws.rs 加 /api/auth/ws-ticket 端点；WsTicketStore 作 AppState 字段；现有 WS 升级路径检查 ?ticket= 参数；server 自己 spawn 的 WS client(transcript tail) 改走 ?internal= |
| P0 | DNS rebinding 防护：Host header middleware + WS 升级手动 Host/Origin 检查 | flockmux 也有 web dashboard，loopback 没有鉴权，DNS rebinding 攻击会让攻击者网站访问 127.0.0.1:PORT 的 API；Host header 校验是 zero-dependency 防护 | axum layer 加 Host header 校验约 60 行 Rust；WS upgrade 路径加 Origin 校验约 40 行 | axum middleware 层(tower Layer)；在 start_server 时将 bound_host 存 AppState，middleware 读取做比较；loopback 绑定接受 localhost/127.0.0.1/::1 别名 |
| P0 | 把 flockmux 自身暴露成 MCP server（blackboard/agent 状态可读，消息可发） | 让外部 Claude Code/Codex 实例直接通过 MCP 读 blackboard、查 agent 状态、触发 wake，实现 orchestrator 和外部工具的零 HTTP 互通。hermes mcp_serve 已有完整蓝本 | 中(1-2周)：基于已有 WS 广播基础，用 axum + rmcp/mcp-rs 起 stdio MCP server，暴露 list_agents/read_blackboard/write_blackboard/list_messages/send_message 等 | 新建 src/mcp_server.rs，使用 rmcp crate 注册 tool handler，路由到现有 blackboard/agent API；配置示例写入 docs/mcp-setup.md |
| P0 | SQLite 写竞争：BEGIN IMMEDIATE + 随机 jitter retry 替换 SQLite timeout 等待 | flockmux 多 agent 并发写 state.db 时当前依赖 SQLite 内置 busy handler，高并发下 convoy 效应导致 TUI 冻结。hermes 的 jitter retry 策略可打散 convoy，加上每 50 次写做 WAL TRUNCATE checkpoint 控制 WAL 文件增长 | 小(1天)：在 src/db.rs 的 execute_write 封装中加 jitter retry loop + periodic checkpoint | src/db.rs 的 DB 写入封装，rusqlite 的 busy_handler 改为应用层 tokio::time::sleep + rand jitter，加 PRAGMA wal_checkpoint(TRUNCATE) 调用 |
| P0 | Usage/Cost 可观测：session 级 token/cost 统计写 SQLite，API 暴露聚合查询 | hermes SessionDB 的 update_token_counts 记录 input/output/cache_read/cache_write/reasoning token 和 estimated/actual cost_usd，list_sessions_rich 聚合显示。flockmux 已有 MEMORY.md 记录这是 P0 空白，直接参照 hermes schema 字段 | 中(2-3天)：在 recordings/sessions 表加 token_input/token_output/cost_usd 字段，transcript.rs 解析 JSONL usage 字段写入，UI 显示 | src/db/schema.rs 加字段 + src/transcript.rs 解析 claude usage event + src/routes/sessions.rs GET /api/sessions 返回聚合 + Web UI 成员栏显示 cost |
| P1 | 错误分类 pipeline：8 阶段 provider-agnostic 错误→action 映射（FailoverReason + retryable/should_compress/should_rotate/should_fallback） | flockmux 目前 worker 出错时靠 orchestrator 自愈（.error fallback），但错误类型未区分：rate_limit 应退避重试，context_overflow 应触发压缩，billing 应换凭证，content_policy 不应重试。结构化分类可让 orchestrator 做更智能的自愈决策 | 中：把 error_classifier.py 中的 pattern list 翻译为 Rust const 数组，classify_api_error 翻译为纯函数，ClassifiedError 翻译为 Rust struct；在 worker 收到 PTY 错误输出时调用分类，结果写入 agent error_meta，orchestrator 读取决定 recovery action | backend/src/error_classifier.rs（新建纯函数）；agent_runtime.rs 或 wake.rs 在 .error 事件触发时调用；orchestrator prompt 可读取 error_meta 作为自愈上下文 |
| P1 | 工具输出智能单行摘要（_summarize_tool_result）替代通用占位符 | flockmux 黑板(台账) 和 worker 历史中工具结果都以原始 JSON 存储，体积大且不利于 orchestrator 理解。用语义摘要 '[terminal] ran npm test -> exit 0, 47 lines' 替代原文，既节省 token 又保留语义 | 小：在 transcript.rs 的 AgentActivity 广播点对 tool_result 调用摘要函数；Rust 实现 summarize_tool_result 覆盖 terminal/read_file/write_file/search_files/patch 主要工具，其余 generic fallback | backend/src/transcript.rs 的 tool 事件处理；或在 blackboard.rs 写入时摘要化；前端 drawer 活动 tab 展示摘要行 |
| P1 | anti-thrashing 机制：连续 2 次压缩节省 <10% 则停止，并建议 /new 或 /compress <focus> | 防止 flockmux 无限触发压缩（每次只剪掉 1-2 条消息），设计简单：维护 ineffective_count 计数器，两次无效后暂停并通知用户 | 小：在 context_compressor.rs 内加 ineffective_compression_count 字段，compress 后对比估算 token 差值，<10% 递增，>=10% 清零；超 2 次在前端状态面板展示提示 | backend/src/context_compressor.rs；前端 swarm 页面 worker 状态卡展示 |
| P1 | 孤立 tool_call/tool_result 对修复（_sanitize_tool_pairs） | flockmux 压缩或裁剪消息历史时可能产生孤立的 tool result（没有对应 assistant tool_call），claude API 会报错；插入 stub result 或删除孤立 result 可防止 API 400 | 小：Rust 实现 sanitize_tool_pairs，在消息历史被修改（压缩/截断）后调用；逻辑简单：遍历两遍，第一遍收集 surviving call_ids，第二遍删孤立 results/插 stub | backend/src/context_compressor.rs 的 compress 后处理 |
| P1 | ProviderTransport ABC + 自注册 registry 模式 | flockmux 目前 worker 驱动写死 claude/codex 两种 CLI，通过 api_mode 字符串 + if/else 分支处理差异。引入 Transport 抽象层后：每种 CLI(claude/codex) 及未来可能的 gemini-cli/copilot-cli 各实现一个 Trait，convert_messages/build_spawn_args/normalize_output 三步统一接口，新增 CLI 只需新文件 + 注册，不改主路径 | M(1-2周，先定义 trait 和两个实现，不需要迁移计费层) | models_config.rs + spawn 汇聚点。Rust trait 对应 Python ABC，registry 用 HashMap<&str, Box<dyn CliTransport>>。PTY 驱动保持不变，transport 只负责 spawn args 组装和 JSONL 输出解析 |
| P1 | NormalizedResponse + provider_data escape hatch | flockmux 的 AgentActivity/transcript 数据结构需要兼容 claude 和 codex 两种 JSONL 格式。参照 NormalizedResponse 设计：共享字段(content/tool_calls/finish_reason)对通用代码可靠，provider_data 存放各 CLI 私有字段(codex 的 call_id/response_item_id，claude 的 reasoning_details)。消除当前 struct 里大量 Option<ClaudeSpecific>/Option<CodexSpecific> 字段 | S(半天，只改数据结构定义和几个 parser 函数，不影响 WakeCoordinator 等业务逻辑) | transcript.rs AgentActivity struct。Rust 可用 serde_json::Value 作 provider_data，或定义 enum ProviderSpecific { Claude(ClaudeData), Codex(CodexData) } |
| P1 | TurnResult.should_retire Session 退休机制 | flockmux 当前 worker hang 不退出是已知盲区(MEMORY 中 M6c error fallback 设计)。hermes 的 should_retire 信号设计很干净：turn 结果携带标记，下次 spawn 时 caller 直接重建 session。flockmux 可在 worker 超时/post-tool quiet/OAuth fail 时发送 should_retire=true 给 orchestrator，触发 respawn 而不是等到 stop hook | S(概念清晰，Rust 实现：TurnResult 加 bool 字段，WakeCoordinator 或 spawn.rs 消费) | worker 状态机：done 状态细分 done_retire，orchestrator 收到后用 .error fallback 已有路径自愈重派 |
| P1 | hermes_tools_mcp_server: 把 Hermes 工具通过 MCP stdio 暴露给 codex | flockmux 已有 per-agent MCP config(--strict-mcp-config)。hermes 的方案把 Hermes 自己的工具通过 FastMCP stdio 暴露给 codex app-server 子进程，让 codex 可以调用 web_search/browser/kanban 等。flockmux 可以用同样思路：把 flockmux 的 blackboard 读写/方向切换等工具通过 MCP 暴露给 worker CLI，不依赖 PTY inject | M(需要写 MCP server，实现 blackboard_read/write/wake tool，配置 per-agent MCP config 注入) | 新建 mcp_server.rs 或 Python MCP server 进程，per-agent MCP config 里加 flockmux-tools entry。blackboard 操作通过 HTTP 回调 flockmux server，不共享内存 |
| P1 | 流式输出 fence scrubber 状态机 | flockmux 将 memory/system-note 类内容注入 agent 上下文后，若 PTY 读取的输出被 WS 广播到前端，fence 标签可能泄漏到聊天气泡。StreamingContextScrubber 的跨 chunk 状态机可直接移植，在 transcript.rs tail 广播前过滤 | 小（约 100 行 Rust 状态机，已有 Python 参考实现） | transcript.rs 的 AgentActivity 广播前加 scrubber 层；或在 ws broadcast 的 text delta 处理路径加 filter |
| P1 | CredentialPool 多账号轮换 + exhausted/dead 状态机 | flockmux 目前 per-CLI 模型配置是单账号。多账号 API key 池 + 自动轮换（round-robin/least_used）+ 429 cooldown 是生产环境必需。dead 状态区分 permanent failure 防止每小时无效 retry | 中（Rust struct CredentialPool，JSON 持久化到 auth.json 同路径；策略 enum 可 TOML 配置） | 在 models_config.rs 的 provider 解析层之上加 CredentialPool；spawn worker 时 select() 获取 access_token，worker 报 429/401 时调 mark_exhausted_and_rotate() 并重试 |
| P1 | Borrowed vs Owned 凭证磁盘净化（sanitize_borrowed_credential_payload） | flockmux 目前直接存 API key 明文。环境变量来源的 key 不应写回磁盘。引入 borrowed 分类：env/cli-injected key 不持久化，只存 fingerprint；hermes 自有 OAuth token 可持久化 | 小（在写 auth.json 的边界函数加 sanitize 逻辑） | credential_persistence.rs：is_borrowed_source() + sanitize_payload()，在 CredentialPool._persist() 调用前过滤 |
| P1 | Provider+Registry 对称插件模式用于 flockmux worker 能力扩展 | flockmux 现在只有 claude/codex 两种 CLI，未来要加 GPT-codex/Gemini CLI/本地 ollama 等。用 hermes 的 provider+registry 模式：每个 CLI worker 实现 CliProvider ABC（spawn_command/check_available/default_flags），CliRegistry 做 get_active_cli(role)。新 CLI 加一个文件，零改框架 | medium — 需要重构现有 spawn 逻辑，但 models_config.rs 里已有 per-CLI 抽象的思路 | 落在 src/spawn/ 模块；CliProvider trait（Rust trait 代替 Python ABC）；CliRegistry 对应 spawn_manager；与现有 resolve_model_tier() 合并 |
| P1 | redact_sensitive_text + RedactingFormatter 用于 flockmux 日志和 WS 广播 | flockmux 的 AgentActivity WS 广播、transcript tail、eventlog 都可能包含 API key（worker 在回复里粘贴密钥）。借鉴 hermes 的子串预检 + 正则分层脱敏，在 WS 广播前过滤，防止 key 出现在浏览器 console 和日志文件 | small — Rust regex crate 实现同样逻辑，_PREFIX_PATTERNS 直接复用 | src/redact.rs（新文件）；接入 transcript.rs 的广播路径 + logging formatter；_REDACT_ENABLED 对应 config 里的 security.redact_secrets |
| P1 | spawning Future 合并并发请求 防止重复 spawn | flockmux 的 WakeCoordinator 可能在同一时刻唤醒多个等待同一 blackboard key 的 worker。借鉴 _spawning dict 模式：对同一 (role, workspace) 的并发 spawn 请求合并到同一个 oneshot channel，第一个真正 spawn，后续等结果 | small — Rust 用 tokio::sync::OnceCell 或 DashMap<key, Sender> 实现 | src/spawn/manager.rs 的 spawn_worker()；已有 spawning_lock，可直接升级为 coalescing 版本 |
| P1 | 内置优先注册守卫（Built-ins-always-win）用于 flockmux 角色注册表 | flockmux 角色注册表（role registry）里有 orchestrator/worker/reviewer/test-runner 等内置角色。若用户可以通过配置注册同名角色覆盖内置行为会破坏编排逻辑。借鉴 _BUILTIN_NAMES frozenset + 注册拦截，保证内置角色不可被用户覆盖 | tiny | src/role_registry.rs：BUILTIN_ROLE_SLUGS const set；register_role() 在注册时校验 |
| P1 | git worktree 检测作为功能门控用于 flockmux directions | flockmux 的 multi-thread directions 功能要求 workspace 是 git repo 才能创建 worktree。借鉴 find_git_worktree() 逻辑（.git file 也识别）+ workspace_cache 加速，在 direction 创建前快速判断是否支持 worktree 隔离 | tiny — 已有类似逻辑，规范化即可 | src/workspace.rs；gate direction creation API |
| P1 | session/load 历史回放协议：resume 时向前端同步推送 user/agent/tool chunk | flockmux 当前重连后前端看不到历史 tool 活动。照 ACP spec：恢复会话时把历史消息逐条推 WS，在响应返回之前完成，前端能展示完整对话链路。 | 中（需在 load/resume session 接口里按 MessageRecord 格式推 WS tool_start/tool_complete/message 事件） | src/routes/sessions.rs + WS broadcast：恢复 session 时遍历 storage::MessageRecord，转换成现有 AgentActivity/ToolCall WS 事件格式推给前端；session.py:_replay_session_history 是直接参考实现 |
| P1 | pre-execution diff approval：编辑类工具在真正执行前向用户展示 diff + 三档策略(ask/workspace/session) | flockmux 目前对 worker 文件操作无审批机制。引入 EditProposal + should_auto_approve_edit 三档策略后，可在 flockmux 设置页暴露 per-workspace 或 per-session 编辑权限，敏感路径黑名单自动保护 .env/.ssh。 | 大（需要 flockmux PTY 层能在工具调用后、执行前注入拦截——当前 PTY 是透传的，需要 hook 或 MCP tool wrapping） | permissions 模块（待建）：在 per-CLI MCP config 里注入一个 edit-gate MCP server，flockmux 做 server 端审批逻辑；或者在 WakeCoordinator 层面拦截写黑板操作 |
| P1 | 工具调用结果 per-tool formatter（替代原始 JSON 字符串透传） | flockmux 聊天页工具结果目前直接展示 raw text。引入 per-tool formatter（read_file 加行号头、terminal 加 exit code、search_files 去重截断、todo 变 Markdown 任务列表），UI 可读性大幅提升。 | 中（前端 JS 层按 tool_name 分发 formatter，不需要改 Rust 后端） | chat.js / ChatMarkdown：对 AgentActivity.tool_result 按 tool_name dispatch formatter，复用 flockmux 已有的 ChatMarkdown 渲染器（已支持 code block/image） |
| P1 | queued_prompts + interrupted_prompt_text 机制（session 忙时入队而非报错） | flockmux orchestrator 给 busy worker 发 prompt 时当前可能报错或忽略。引入 queued_prompts：push 到 session 队列，返回确认，worker 完成当前 turn 后自动 drain 队列，支持 /steer 中断恢复语义。 | 中（SessionState 加 queued_prompts deque，prompt handler 加 is_running 判断；drain 循环在 turn 结束后执行） | session.rs + WakeCoordinator：worker 正忙时把 prompt 推入 per-session queue（存 DB），turn 完成时 WakeCoordinator 触发 drain；对应 flockmux 已有的 mailbox 概念可以扩展 |
| P1 | /compact 手动压缩：在 PTY session 里注入压缩指令，避免 context 超限卡死 | flockmux 目前无 context 压缩触发机制。hermes _cmd_compact 的思路可移植：检测 token 估算超过 threshold 后，向 PTY inject '/compact' 或类似 claude 内置压缩命令，或 spawn 一个专用 summarizer worker 替换历史。 | 中（PTY inject 方式依赖 claude CLI 的内置 /compact；或 flockmux 自己读历史、调 API 压缩后 replace_messages） | transcript.rs 监测 token 估算阈值 → 触发 PTY inject '\x15/compact\r' 或 spawn compact-worker；前端蜂群页/活动行展示压缩状态 |
| P1 | 同步写入/异步消费 queue.Queue 流式桥（on_delta thread-safe → asyncio consumer rate-limit edit） | flockmux PTY 驱动 worker 已经是异步边界：PTY 读在 tokio task，但流式更新前端靠 WS broadcast 没有 rate-limit 和 buffer。引入 hermes 式的 edit-interval buffer（50-200ms 合并），减少 WS 帧数，避免前端抖动 | 中（Rust channel mpsc 替代 queue.Queue，tokio task 做 consumer；前端 debounce 可在 JS 侧做） | transcript.rs tail → broadcast channel；前端聊天气泡改为 patch（edit）模式而非每 token append；WS 协议加 MessageEdit 事件类型 |
| P1 | 四级展示配置解析（per-CLI tier 默认 + 用户覆盖 + 全局默认） | flockmux 已有 per-CLI model 配给，但工具进度/流式/interim_messages 等展示设置没有 tier 分层。对标 hermes：claude/codex worker 可按 tier（high/low）给出合理开箱体验（claude 默认显 tool_progress，codex 默认简洁） | 小（在 models_config.rs 或 display_config.rs 加 tier + per-CLI 覆盖；前端设置页加对应项） | models_config.rs + 设置页：per-CLI display_tier 字段 + resolve_display_setting Rust 函数；蜂群成员栏根据 tier 决定展示哪些进度信息 |
| P1 | PlatformEntry 工厂注册表（adapter_factory + check_fn + standalone_sender_fn 三件套） | flockmux 多 CLI 抽象现在 cli_config.rs if/else 分支。引入 CliEntry registry（类似 PlatformEntry），未来加 Discord/Slack 推送通道时无需修改核心代码；standalone_sender_fn 对应 flockmux 无 live gateway 时 cron/通知投递 | 中（定义 CliEntry trait + CliRegistry；现有 claude/codex adapter 注册进去；消除 run.rs 里的 if/else 分支） | 新建 cli_registry.rs；spawn.rs create_adapter() 走 registry 而非 match；为未来 notif-push 等通道预留 standalone_sender |
| P1 | 媒体投递路径安全校验（denylist + recency window + allowlist） | flockmux 聊天图片预览已有 /api/file 端点+白名单+magic 嗅探，但 agent 产出物（报告 PDF、截图等）发给用户时没有路径安全校验，存在 prompt-injection 外泄风险 | 小（在 /api/file 或新 deliver 端点加 validate_media_delivery_path 逻辑：denylist 覆盖 /etc/.ssh/.aws，recency 600s） | routes/file.rs 或新 routes/deliver.rs：Rust 实现 denylist + mtime recency check；HERMES_MEDIA_DELIVERY_STRICT 对应 env 开关 |
| P1 | 跨进程 session mirror（CLI/cron 发出消息追写到目标 session transcript） | flockmux 黑板变更通过 WakeCoordinator 广播，但 orchestrator 主动发出的消息（如 cron 通知）没有写入 worker session 的上下文历史，导致 worker 下次唤醒时缺少 '我给用户发了 X' 的记忆 | 小（mirror_to_session Rust 版：查 session_id by platform+chat_id，append assistant 消息到 messages 表） | storage.rs / messages 表：mirror_delivery() 追写；黑板写入后 WakeCoordinator 唤醒时 worker 已有完整上下文 |
| P1 | Task graph DAG：typed handoff key 升级为 parent→child 依赖图，recompute_ready 驱动 todo→ready 自动提升 | flockmux 目前 typed handoff key 是单向键值，无法表达多 worker 并行+串行聚合的图结构（swarm/decompose 模式）。引入 task_links 表即可支持 plan→parallel_workers→verifier→synthesizer 四层拓扑，复用现有 dispatcher tick | 4-6 天：task_links 表 + recompute_ready 逻辑 + WakeCoordinator 改为监听 parent done 而非单一 blackboard key | orchestrator/typed handoff：现有 BlackboardChanged + WakeCoordinator 可扩展为 'parent done → wake all children'；blackboard key 体系可继续保持，但在其上叠加 DAG 语义 |
| P1 | Diagnostic rule 引擎：stateless read-only rules 对 agent/task 状态生成 Diagnostic(kind/severity/title/detail/actions) | flockmux QA 2026-06 找到的 F3 绿点撒谎/F6 死 agent 头像等问题，需要人工判断。hermes 的 diagnostic rules 把这类检测机械化：repeated_failures/stranded_in_ready/stuck_in_blocked/block_unblock_cycling 对应 flockmux 的常见失效场景 | 3-4 天：定义 DiagnosticRule trait + 注册表；在 /api/agents/:id 和 board API 附加 diagnostics 字段；前端渲染 warning/error badge | backend/routes/agents.rs：在 agent 详情 API 追加 diagnostics 计算；前端蜂群成员栏/台账页增加 diagnostic badge（参照 hermes dashboard patterns） |
| P1 | Stranded-in-ready diagnostic：超时无人 claim 的任务自动升级 warning→error→critical | flockmux 台账里的 worker 如果因 assignee typo 或 profile 不存在而一直在 ready 状态，目前无主动提醒。hermes 的 age-based stranded 检测 identity-agnostic，30min→warning / 2h→error / 6h→critical | 1 天：在 diagnostic 规则里加 stranded_in_ready；检查 agents.status=pending 且 created_at 超阈值；前端台账补 badge | backend/routes/swarm.rs + 前端台账：worker 列表 API 附加 age_seconds 和 stranded 标志；前端台账 status 列增加超时警告样式 |
| P1 | build_worker_context 模式：为 worker 构建结构化上下文字符串（任务+历史尝试+父任务 handoff+角色历史+评论） | flockmux worker 目前通过 mailbox inject 接收上下文，但格式不标准化；hermes 的 build_worker_context 有清晰的 section 结构和字段 cap，可作为 flockmux 'typed handoff key 内容' 的格式规范 | 1-2 天：参照 hermes 的 markdown section 格式定义 flockmux handoff context 模板；在 WakeCoordinator 注入 worker 时附加 parent_results + prior_attempts | orchestrator 的 worker spawn system prompt 构建逻辑：把现有 blackboard read 结构化为 hermes 风格的 ## Parent task results / ## Prior attempts 章节 |
| P1 | Claim heartbeat + stale claim 回收：TTL + PID liveness 双检，heartbeat stale 作兜底 | flockmux 目前 worker 超时靠 5s auto-kill grace（结构上截不断）。hermes 的三层保障：PID alive 则延展 claim TTL / 1h heartbeat stale 强制回收 / max_runtime 强制 SIGTERM 可以更精细控制 worker 生命周期 | 3 天：给 running agent 加 claim_expires + last_heartbeat_at；Rust worker 每 N min 写 heartbeat；dispatcher tick 检查 stale | backend/agents.rs + PTY runner：worker PTY 读到 token 时顺带更新 last_heartbeat_at（类似 hermes _touch_activity 模式，复用 JSONL tail 的已有 ticker） |
| P1 | 鉴权门控条件激活 should_require_auth：loopback→免鉴权，公网绑定→OAuth gate | 本机开发免摩擦，公网安全；RFC1918 视为公网（LAN 同机器是威胁模型），--insecure flag 作 escape hatch | 逻辑约 20 行，配合 Host 校验和 WS ticket 一起上 | flockmux 已有 --host 参数，在 start_server 计算 auth_required 存 AppState；目前 API 无鉴权，上 gate 后还需要给 /api/status 等探针加公开白名单 |
| P1 | 公开路径 allowlist 单一来源(PUBLIC_API_PATHS frozenset)供多个 middleware 共享 | 防止不同 middleware 独立维护 allowlist 导致漂移(hermes 真实踩坑：两个 middleware 列表不同步导致 portal liveness probe 401) | 极低：提取常量文件，改两处引用 | src/routes/public_paths.rs 定义 PUBLIC_PATHS 常量；session token middleware 和 OAuth gate middleware 都引用它 |
| P1 | 审计日志：JSON per-line，敏感字段 redact frozenset，写失败 warn 不 panic | flockmux 目前无安全审计日志；鉴权事件(login/logout/ticket mint/reject)可观测对调试 'WS 一直断开' 类问题极有价值 | 约 100 行 Rust(serde_json + tokio::fs::OpenOptions append)；事件 enum + redact 字段 HashSet | src/auth_audit.rs；在 middleware 和 ws 升级路径调用；写日志用 spawn_blocking 避免阻塞 async 路径 |
| P1 | PTY bridge：SIGHUP→SIGTERM→SIGKILL 三段升级终止 + 尺寸 clamp 防 WSL2/崩溃维度 | flockmux 已有 PTY 驱动，但 close 路径和 resize 路径是否有相同的 robust 处理？WSL2 columns=131072 这类 bug 在公网部署时更常见 | 审查/加固 flockmux 现有 pty 代码；实现 clamped TIOCSWINSZ；三段 kill 约 30 行 | src/pty.rs 或对应模块；resize 消息路径加 MIN/MAX 边界检查；kill 路径加超时等待 |
| P1 | 声明式 Schema 迁移：SCHEMA_SQL 做 diff 自动 ADD COLUMN，废弃 version chain | flockmux 当前 migrations.rs 是 version-gated chain，新增列需要手写 migration 且顺序敏感。hermes 的 _reconcile_columns 用 in-memory SQLite 解析 DDL，对比 live 表自动 ALTER TABLE ADD COLUMN，新增列只改 SCHEMA_SQL | 中(2-3天)：在 src/db/schema.rs 实现 reconcile_columns()，startup 时运行 diff+patch | src/db/schema.rs，rusqlite::Connection::execute 执行 ALTER TABLE，补充 harness-check 规则确保 SCHEMA_SQL 和实际表结构一致 |
| P1 | Cron wake-gate 协议：脚本末行 JSON {wakeAgent: false} 跳过 agent 调用 | flockmux 计划做 cron，直接设计 wake-gate 协议可避免无意义的 agent/LLM 调用。无新内容的数据采集脚本直接返回 silent，节省 token 和 PTY 启动开销 | 小(半天)：cron 执行逻辑加 parse_wake_gate，解析脚本最后一行 JSON | src/cron/executor.rs 新增 parse_wake_gate 函数，no_agent 模式(纯 bash)和 wake_gate=false 都走 silent path 不 spawn PTY |
| P1 | Cron job 级别在途去重(in-flight dedup guard)防止 job 跨 tick 堆叠 | 定时任务如果上次 tick 的 run 未完成，本次 tick 看到 due 后会再次 spawn 产生多个实例。hermes 用全局 _running_job_ids set + 锁实现跨 tick 去重，相同 job_id 在途时直接跳过 | 小(半天)：在 cron tick 逻辑前查 running_set | src/cron/scheduler.rs 维护 Arc<Mutex<HashSet<String>>> running_job_ids，dispatch 前检查，job 完成后 remove |
| P1 | 压缩锁：原子 DELETE(expired)+INSERT OR IGNORE 防止 blackboard 写入竞态产生孤儿 | flockmux WakeCoordinator 触发多个依赖方时可能存在竞态：两个 orchestrator 同时看到黑板变更并派发相同 worker。hermes 的压缩锁机制(TTL + holder 校验 + expired 自动回收)可用于 blackboard handoff key 的独占写保护 | 小(1天)：在 blackboard.rs 加 acquire_write_lock/release_write_lock，基于 SQLite 事务实现 | src/blackboard.rs 加 handoff_locks 表，WakeCoordinator 在 dispatch 前 try_acquire，agent done 后 release |
| P1 | Backend 候选链 + smoke-probe 降级（resolveHermesBackend 模式） | flockmux 目前 claude/codex CLI 找不到就直接报错；hermes 的多级候选+smoke-test 模式更健壮，可以减少「找不到 CLI」类的用户报障 | 小（Rust 侧在 spawn 前加 which+version-probe 逻辑，已有 PTY 基础） | models_config.rs / spawn 入口：在 resolve CLI binary 时加候选链（env override → PATH → brew prefix → 报错），probe 用 --version 快速验证 |
| P1 | Cron 任务管理（定时 prompt dispatch + delivery channel） | flockmux 已有 MEMORY 中标注「cron 定时」为空白；hermes 的实现包含 delivery 路由（local/telegram/discord/slack/email），且 UI 完整可参考 | 大（Rust 需 cron scheduler + 任务持久化 + delivery router；前端 CronView 可参考 hermes 直接移植） | 后端新增 cron_jobs SQLite 表 + 调度器（tokio timer）+ delivery trait；前端复用 hermes CronView 的 scheduleOptionForExpr/scheduleSummary 逻辑 |
| P1 | 辅助任务模型分配（per-task model override：compression/vision/mcp...） | flockmux 的模型配给已有 per-CLI 分配（MEMORY: models_config.rs），hermes 进一步细化到 per-task（context compaction 用便宜模型），可降低 token 成本 | 中（后端 models_config.rs 加 AUX_TASKS 枚举+存储；前端设置页参考 hermes ModelSettings） | models_config.rs 扩展 scope=auxiliary+task 字段；设置页「模型」tab 新增辅助任务配置区；compression/mcp 两个 task 最高优先 |
| P1 | File Browser（react-arborist 懒加载目录树 + IPC 读目录 + 拖入终端自动 quote） | flockmux MEMORY 标注「文件浏览器」为空白；hermes 的实现含防并发（inflight Set）、跨 cwd 请求竞态保护（requestId）、collapseNonce 强制重挂不清数据等细节值得直接借鉴 | 大（Electron 有 IPC 读目录；flockmux 需后端 API /api/fs/readdir + 前端 arborist tree；拖入终端要 custom MIME 类型） | 前端右侧边栏新增文件浏览器 tab；后端 routes/ 加 /api/workspace/:id/fs/ls；集成到终端拖放（flockmux 已有 xterm 终端基础） |
| P1 | Incremental External Store Runtime（流式消息增量 diff 不全量替换） | flockmux 聊天页长会话中如果每条 WS 消息触发全量 re-render 会出现卡顿；hermes 的 incremental diff 模式保持滚动位置稳定 | 小（前端 chat store 改为 addOrUpdate + deleteAbsent 逻辑，React 侧用 useMemo 做增量） | 前端 chat 状态管理：消息 map 用 id 为 key，收到 WS delta 时 patch 对应 message 而非整体替换 messages 数组 |
| P1 | 原子文件写入（writeFileAtomic: .tmp + rename） | flockmux 后端写配置文件如果直接 write 可能在 crash 时产生损坏 JSON；hermes 的 atomic rename 是防御性写法 | 极小（Rust: tempfile crate + rename，已经标准库支持） | 后端所有配置写入路径（settings.json/connection.json/models_config.toml 等）改用 tempfile → rename |
| P2 | StreamingThinkScrubber：流式 reasoning block 清洗状态机 | flockmux PTY 驱动 claude 时若模型输出 <think> 块，当前直接透传给前端聊天窗口，reasoning 内容用户不需要看到（且占空间）；状态机 scrubber 可在 PTY 输出 pipe 处过滤 | 小：Rust 实现 StreamingThinkScrubber 等价结构体（feed/flush），在 PTY stdout 处理链中插入，过滤 <think>...</think> 区间；状态持久到每个 worker 实例 | backend/src/pty_handler.rs 或 transcript.rs 的 stdout 管道处理 |
| P2 | Prompt Caching breakpoint：system_and_3 策略 | flockmux orchestrator/worker 系统提示 + 每轮消息都可加 Anthropic cache_control breakpoint，降低多轮对话 input token 成本约 75%；当前 flockmux 未做任何 cache 标注 | 小：apply_anthropic_cache_control 逻辑约 40 行，Rust 实现直接操作消息 JSON 注入 cache_control；仅在 provider=anthropic 时生效，其他 provider 跳过 | backend/src/prompt_caching.rs（新建）；在 agent_runtime.rs build_api_messages 时调用；需检查 flockmux 用的 claude API 是否 native anthropic 格式 |
| P2 | 历史图片剥离（_strip_historical_media）防止旧截图每轮重传 | 若 flockmux 支持图片输入（已有图片预览功能），旧消息中的 base64 图片会每轮重新上传给 API，巨额 token 浪费；找到最新带图片消息作锚点，之前的全替换占位符 | 小：纯消息列表操作，Rust 实现 strip_historical_media；在 build_api_messages 或压缩后调用 | backend/src/context_compressor.rs 或 agent_runtime.rs |
| P2 | 确定性 fallback 摘要（_build_static_fallback_summary） | 当辅助摘要模型不可用时，本地解析对话结构提取关键信息生成结构化 fallback，比空占位符好得多；flockmux 在辅助模型 503 时可用此法保持 orchestrator 理解上下文 | 中：Rust 实现本地 NLP（正则抽取路径/工具名/错误文本），生成固定格式摘要；不依赖 LLM | backend/src/context_compressor.rs 的 generate_summary 失败分支 |
| P2 | Context length 多级解析链(provider-aware) | flockmux spawn 时传给 worker 的 system prompt 长度不可知，容易超 context window 导致 worker 400 fail。hermes 的 get_model_context_length 10 级解析链(含 Codex OAuth 的 272K limit vs OpenAI 直接 API 1.05M 的区分)对 flockmux 的 per-CLI 模型配给直接有用：spawn 前先估算 system_prompt+task tokens 是否 fit | L(完整实现需要网络请求+缓存+per-provider 逻辑，建议只移植 hardcoded defaults 表 + 简单 OpenRouter 查询作 P2) | models_config.rs 新增 fn get_context_window(model: &str, provider: &str) -> usize，spawn 前 preflight check。先移植 DEFAULT_CONTEXT_LENGTHS 静态表(50行 Rust const)，后续再加在线查询 |
| P2 | CodexAppServerClient: JSON-RPC over stdio 三队列分发 | flockmux 用 PTY 驱动 codex，hermes 实现了 codex app-server 的原生 JSON-RPC over stdio 协议。虽然 MEMORY 里 ACP support reality 已明确 codex app-server 是可行路径，当前 flockmux PTY 驱动解析输出复杂。三队列分发(pending future/server_requests/notifications)是精密但可复用的设计：notifications 流对应 flockmux 已有的 AgentActivity 广播 | XL(需要完全理解 codex app-server 协议，替换 PTY 驱动，风险高)。建议仅借鉴三队列架构思路，不做实际替换 | 如果未来做 codex native mode，spawn.rs 里 PtyProcess 替换为 AppServerProcess，三队列用 tokio::sync::mpsc 实现 |
| P2 | 跨 session 限速熔断文件(Cross-session Rate Limit Circuit Breaker) | flockmux 多 worker 并发 spawn 时，一个 worker 遇到 429 后其他 worker 也在打同一个供应商 API。hermes 的 nous_rate_guard 用共享文件实现跨进程熔断，atomic_replace 保证一致性，is_genuine 区分账号限速 vs 上游瞬时拥塞防止误触发。flockmux 可实现类似机制，减少 token 浪费 | M(共享文件逻辑简单，关键是 is_genuine 判断需要解析 x-ratelimit-* header，PTY 路径无法获取 HTTP header——需要从 JSONL error output 里解析) | rate_limit.rs 新文件，spawn.rs 在 worker error 回调里触发。PTY 里无 HTTP header 是障碍：可退化为只用 retry-after header 或从 error message 正则提取 reset time |
| P2 | CodexEventProjector: 把协议事件投影为 canonical messages 的无损翻译层 | flockmux 的 transcript.rs tail + 广播 AgentActivity 已经在做类似事情，但缺结构化投影(tool call pairs/reasoning 贴到 assistant 消息)。hermes 的 EventProjector 确保 curator/memory 层看到一致的消息结构。flockmux 可以参照，让聊天 UI 展示的 agent 消息不只是 raw text，而是有 tool call 展开、reasoning 折叠等结构化渲染 | M(主要是前端工作：解析 JSONL 事件序列投影到结构化消息，聊天 UI 按类型渲染) | transcript.rs 扩展 AgentActivity 结构体，前端聊天气泡按 item_type 路由渲染逻辑(已有 ChatMarkdown，需要加 ToolCallBubble/ToolResultBubble) |
| P2 | MemoryProvider ABC + MemoryManager orchestrator 模式 | flockmux 目前 CLAUDE.md / AGENTS.md 是静态注入。抽象出 ContextProvider trait（Rust），允许多 provider（builtin file、workspace memory、per-agent 动态上下文）在 prefetch/sync 接口下插拔，为未来 memory backend 预留扩展点 | 中（设计 trait + builtin impl，约 300 行） | spawn worker 时注入 context 的路径（现在直接拼 system prompt），改为调用 ContextManager.prefetch_all()；session end 时调用 sync_all() |
| P2 | Curator 惰性调度 + skill lifecycle 状态机（active/stale/archived） | flockmux 的 skill（对应 hermes skill）目前没有生命周期管理。引入 last_activity_at + state 字段，idle 触发 LLM fork 做 umbrella 合并，防止 skills 目录碎片化。特别适合 blackboard key 和角色注册表的定期清理 | 大（需 Rust skill DB schema 扩展 + curator agent spawn + REPORT.md 生成） | 在角色注册表（role registry）中加 state/last_used_at；利用现有 spawn_worker 机制 fork 一个 curator orchestrator；curator 结果写入 blackboard |
| P2 | RemovalStep 注册表（统一 credential source 移除合同） | flockmux 若支持多来源凭证（env/file/OAuth）后，每个来源的移除逻辑各不同（清 .env / suppress / 清 auth.json）。Registry+RemovalStep 模式消灭 if/elif 链，让新增来源只加一条注册 | 小（跟随 CredentialPool 一起设计时添加） | credential_sources.rs：RemovalStep trait + REGISTRY Vec；auth remove 命令路由到 find_removal_step() |
| P2 | Skill bundle（多技能组合注入） | flockmux 现有 skill 是单个注入。bundle 允许用户用一条 slash 命令激活一组 skill（如 /backend-dev 加载 code-review + testing + pr-workflow），是 '工作模式' 概念的自然实现 | 小（在现有 skill slash 命令处理前查 bundle 注册表，YAML 格式） | 在 flockmux 的命令面板和 slash 命令解析层加 bundle lookup；bundle YAML 存在 ~/.config/flockmux/skill-bundles/ |
| P2 | Skill frontmatter platform/environment 双重过滤 | flockmux 未来如果支持 skill 注入，需要区分 '这个 skill 在 macOS 才有效' 和 '这个 skill 只在有 docker 时相关'。两个正交维度的过滤比单一 enabled flag 更精细 | 小（在 skill 扫描时读 frontmatter 做过滤） | skill_registry.rs 的 scan() 路径加 platform_filter + env_filter；platform 从 std::env::consts::OS 检测，environment 从进程环境变量/文件系统探测 |
| P2 | SKILL.md inline shell 展开（!`cmd`） | 让 skill 内容在注入时动态化（如执行 git log 插入最近提交）。适合 flockmux 的 agent 上下文注入场景：orchestrator 可以在 spawn worker 时把当前 workspace git 状态注入到 system prompt | 小（在 system prompt 构建时做 regex 替换 + subprocess 执行，需做超时和截断） | spawn worker system_prompt 构建路径；或 CLAUDE.md 加载时展开，替换 ${FLOCKMUX_WORKSPACE_STATUS} 等 token |
| P2 | Bitwarden Secrets Manager 集成（bws CLI 驱动 + 两层缓存） | flockmux 目前 API key 存 .env 明文。BSM 集成允许所有敏感凭证集中托管，只有 bootstrap token 需要本地存储。subprocess 驱动（非 SDK）简化 Rust 侧依赖 | 中（Rust 实现 bws 调用 + 两层缓存 + 安装引导） | 在 env_loader 路径（server 启动时）调用 bws secret list，结果注入 std::env；磁盘缓存放 ~/.config/flockmux/cache/bws_cache.json，mode 0600 |
| P2 | 上下文触发式一次性 onboarding hint 系统 | flockmux 没有 onboarding。不需要安装向导，只需要在首次遇到行为分叉时展示一次 hint（如首次 worker fail 时提示 harness-check，首次 merge conflict 时提示 AI merge-resolver）。config.yaml onboarding.seen 跟踪 | 小（约 50 行 Rust：hint 注册表 + 读写 seen flags） | event 处理层（WS 广播前）检查 onboarding flag；前端接收特殊 type=onboarding_hint 的消息并用不同 UI 渲染 |
| P2 | 多模态能力注册表模式用于 MCP provider 选择 | flockmux MCP 管理页目前是硬编码，借鉴 web_search_registry 的 capability filter + legacy preference walk，让每个 MCP provider 声明 supports_image/supports_audio/supports_code 等能力，工具层自动路由到对应 provider，配置显式绑定 vs 自动探测分离 | medium | src/mcp_admin.rs + 新增 capability routing 层；get_setup_schema() 对应现有 MCP tools picker UI |
| P2 | LSP delta baseline 思路用于 flockmux 工具结果去噪 | flockmux 的实时进度（transcript tail）会推送 worker 的全量 tool output，包括大量「已知诊断」噪声。借鉴 snapshot_baseline + set-difference 思路：对 worker 的 file lint 结果做 delta 过滤，只向 orchestrator/UI 推送「本轮新增的错误」 | medium — 需要在 transcript.rs 里维护 per-file 诊断快照 | src/transcript.rs 的 broadcast pipeline；与现有 AgentActivity 合并，加 DiagnosticDelta 字段 |
| P2 | once-per-key 日志去重器用于 flockmux eventlog | flockmux 频繁触发的事件（worker 发工具调用、WakeCoordinator 轮询）会刷屏日志。借鉴 hermes eventlog 的 _announce_once(bucket, key) 模式：同 (agent_id, event_type) 的警告只 WARNING 首次，后续降为 DEBUG | tiny | src/eventlog.rs（新文件）或在现有 tracing 层加 filter；DashSet<(AgentId, EventKind)> 作去重 bucket |
| P2 | 跨配置文件（cross-profile）软警告模式用于 flockmux 方向隔离 | flockmux 的多方向（direction）特性中，一个方向的 agent 可能误写入另一方向的 blackboard 或 worktree 文件。借鉴 classify_cross_profile_target 的路径形状检测 + 软警告返回给 agent，要求显式确认再跨方向写 | small | src/directions/guard.rs；接入 blackboard write API；direction_slug 对应 profile name |
| P2 | ContextVar 隔离 per-session tool 回调（解决多 worker 共享线程时 TLS 泄漏） | flockmux 若未来在 tokio 层引入多 session 共享 thread pool，per-session 状态必须用 Rust 的类 Arc<Mutex> + 显式传参或类似 ContextVar 的局部变量机制隔离，否则 approval_cb、session_id 等状态会跨 session 串台。 | 小（flockmux 目前 per-agent PTY 已天然隔离，在引入 in-process 多 session 时才需要） | spawn.rs / agent_runner.rs：为每个 worker run 调用保留 Arc<SessionContext> 携带 session_id/approval state，而非读全局 |
| P2 | session fork：深拷贝 history 到新 session 支持分支探索 | flockmux 已有多方向(thread)功能，但 fork_session 语义更细：在同一 workspace 内 fork 单个 session 的历史到新 session，适合 A/B 对比同一任务的不同策略，不需要完整 git worktree 隔离。 | 小（SessionManager 已有 fork_session 参考；flockmux 侧需要 POST /api/sessions/:id/fork，deepcopy history，生成新 session_id） | routes/sessions.rs 新增 fork 接口；存储层 replace_messages 复制；前端方向列表展示 fork 来源 parent_session_id |
| P2 | ACP auth 声明模式：初始化时声明 auth_methods，含 fallback terminal 配置引导 | 若 flockmux 未来接入 Zed/Cursor 等支持 ACP 的编辑器，必须在 initialize 响应中声明至少一个 auth_method。hermes 的 build_auth_methods 双保险模式（provider + terminal fallback）是最佳实践，新机器开箱即用。 | 小（仅 ACP server 启动时的声明逻辑，不影响核心功能；flockmux 当前不走 ACP 但此为 park 功能的激活前置） | 若实现 L4 ACP：acp_adapter/auth.rs（新建），检测 flockmux 已配置的 claude/codex 凭证，返回 AuthMethodAgent；无凭证时返回 TerminalAuthMethod 引导运行 flockmux setup |
| P2 | ContextVar 等价的 per-task session context（替代全局状态） | flockmux 用 session_key 查表传递上下文，但工具调用链中若需要 platform/chat_id/thread_id 需要显式传参或查 DB。借鉴 hermes 的 task-local context，让工具函数直接读 current session context 而不需要额外参数 | 中（Rust task_local! 宏 + Arc<SessionContext>；spawn_worker 时 set；tool 函数 with_borrow 读取） | spawn.rs + tools/：SESSION_CONTEXT task_local；工具（write_file/read_file 等）读 session cwd/platform 无需传参 |
| P2 | 配对码审批流（动态白名单，代替静态 user_id 列表） | flockmux 当前无入站鉴权（本地工具面向开发者）。若未来加 Slack/Telegram 通知推送通道，需要控制谁能触发 agent。配对码方案比静态 allowlist 更友好：陌生用户自助申请，管理员 CLI approve | 中（pairing_store.rs + /api/pairing 端点；CLI approve 命令；code hash 存 SQLite） | 新建 pairing.rs；与 agents.db 共用 SQLite；设置页加 '待审批用户' 列表 + approve/deny 操作 |
| P2 | HookRegistry（文件系统 hook 发现 + emit/emit_collect 双模式） | flockmux 目前无用户级扩展点。harness-check 等脚本只能在 CI 跑，无法在 gateway 事件（agent:start/agent:end/session:start）上触发自定义逻辑（如记录 cost、发 Slack 通知、触发外部 webhook） | 中（hooks.rs：扫描 ~/.flockmux/hooks/*/hook.toml + handler.wasm 或 handler.sh；emit 在 agent lifecycle 关键点触发） | 新建 hooks.rs；agent.rs 在 start/step/end 调用 hook_registry.emit()；emit_collect 用于 policy 决策（是否允许某个工具调用） |
| P2 | 频道目录 JSON（从 session 历史推断可达频道，5min 刷新） | flockmux 黑板 + 成员栏已有 workspace 列表，但无法按 channel/chat_id 路由消息（send_message tool 需要 human-readable channel 名称解析到 ID）。channel_directory.json 提供轻量的名称→ID 缓存 | 小（channel_directory.rs：从 messages 表 distinct chat_id+chat_name 构建；/api/channels 端点；定时刷新） | 新建 routes/channels.rs；agent send_message 工具读此目录解析目标；前端设置页或黑板视图可展示 |
| P2 | Ralph-style goal loop：per-worker 持续循环，auxiliary judge 判断 done/continue，turn budget 作 backstop，finalize nudge | flockmux worker 目前是 one-shot：PTY 里 claude/codex 跑完一次即算完成。引入 goal loop 可以让 orchestrator 指定'当 judge 认为完成时才算完成'，特别适合 open-ended 任务（写报告/重构/调研） | 5-7 天：goal_mode 字段 + run_kanban_goal_loop 移植为 Rust（或 orchestrator profile 层 Python interop）；需要 auxiliary judge API call（可复用 spawn 时用的模型配给） | orchestrator 的 role 注册表：给 goal_mode=true 的 worker 在 worker done hook 里调 judge；judge 的 auxiliary client 对应 flockmux 的 per-CLI model 配给 |
| P2 | Blackboard 用结构化 JSON comments 实现 last-write-wins merge（swarm blackboard 模式） | flockmux 当前 blackboard 是 key-value KV；hermes 把 blackboard 表达为 root task 的特殊格式 comment，天然有时序+作者溯源+现有 dashboard 可见。适合 flockmux 的群聊作为 blackboard 审计通道 | 1-2 天：在 swarm root message thread 约定 [swarm:blackboard] prefix 格式；latest_blackboard 合并函数移植到 TS/Rust | 现有聊天/群聊：在群聊消息里支持 meta: {key, value} 结构化 payload；WakeCoordinator 可监听 meta.key 变更作为 trigger |
| P2 | Idempotency key on task create：webhook 重试/orchestrator 重启安全 | flockmux spawn 时没有 idempotency key，orchestrator crash 重启可能 double-spawn 同一 worker。hermes 的 idempotency_key 检查是 SELECT then INSERT 模式（可接受极低概率 race），轻量且有效 | 0.5 天：agents 表加 idempotency_key 列；spawn_worker 时传 key；SELECT 先检查同 key 非 archived 行 | backend/routes/swarm.rs spawn 路径：swarm_spawn_worker 工具增加 idempotency_key 参数；MCP tool schema 同步更新 |
| P2 | 多项目 Board 隔离：每个 board 独立 DB + workspaces + logs，ContextVar scoped override 保线程安全 | flockmux 目前一个 workspace 对应一套 SQLite，多 workspace 并发本质上是多实例。hermes 的 board 设计允许单进程管理多 board，每 tick 可以轮询所有 board，更适合 flockmux 多 workspace(directions) 场景 | 中等：需要 flockmux 的多 direction worktree 已经稳定，再叠加 per-direction DB 隔离 | 已有 multi-thread directions feature：directions 和 board 概念对应，每个 direction 一个独立 agent DB（类似 hermes board per project） |
| P2 | 密码登录 per-IP 滑动窗口限速器 | flockmux 若加密码/API key 认证，暴力猜测无任何防护；滑动窗口比固定窗口更精准 | DashMap<String, VecDeque<Instant>> + 清理逻辑约 80 行 Rust | axum extractor 或 middleware 层；按 X-Forwarded-For 或直连 IP 为 key；_unknown_ bucket 合并无法归属的请求 |
| P2 | cookie 三前缀清除：clear 时对 bare/__Secure-/__Host- 三种变体全发 Max-Age=0 | flockmux 若加 session cookie，HTTPS 部署和 HTTP 本地开发的 cookie 名不同，clear 时只删一种会有残留 cookie 死灵 | 约 20 行 Rust cookie helper | 在 routes/auth.rs 的 logout / session expire 路径加 clear_all_variants 辅助函数 |
| P2 | proxy adapter 抽象：UpstreamAdapter 接口 + 路径白名单 + 单次 retry credential | flockmux 若加模型 proxy(让 worker 通过 flockmux 访问 LLM)，这个 adapter 模式避免 proxy server 知道任何 provider 细节；401 retry 是 token 轮换场景必须的 | Rust trait UpstreamAdapter + aiohttp 等价 reqwest client + 单次 retry 逻辑；约 200 行 | src/proxy/ 模块；可先只实现 Anthropic/OpenAI adapter；allowed_paths 作为 auth 上游 |
| P2 | open-redirect 多层防御：next= 在每个处理点独立校验(拒绝 //evil、/api/*、auth 路径) | flockmux 若加登录流，next= 参数是经典 open-redirect 注入点；hermes 明确枚举拒绝场景包括 /api/* 防用户看到裸 JSON | 约 30 行校验函数 + 在所有 next= 读取点调用 | src/auth/redirect.rs validate_post_login_target()；在 /login /auth/callback 两处调用 |
| P2 | Cron 结果投递：WS adapter 优先，HTTP 降级的双路投递 | flockmux cron 结果可以通过 WS 广播给 Web UI(当前已支持)或推送到外部通知(Telegram/钉钉等)。hermes 的 live adapter 优先模式：先查 gateway adapters 字典走 WS，失败再走独立 HTTP，保证 E2EE/在线时低延迟 | 中(3天)：抽象 DeliveryAdapter trait，WS 广播作为一种 impl，外部平台 HTTP 作为另一种 impl | src/cron/delivery.rs，WsAdapter 复用现有 AgentEvent 广播，HttpAdapter 调外部 webhook 配置的 URL |
| P2 | mtime gate：EventBridge 轮询前先 stat() 文件 mtime 作为 no-op guard | flockmux transcript.rs 目前 tail JSONL 文件，如果 worker 空跑不产生 output 仍然每次触发 read。可加 mtime check：文件 mtime 未变则直接跳过，减少无谓 I/O | 极小(2小时)：在 transcript tail 循环加 metadata().modified() 缓存比较 | src/transcript.rs tail_loop，tokio::fs::metadata().modified() 对比上次 mtime，无变化时 sleep(poll_interval) 直接 continue |
| P2 | Bootstrap 事件流协议：install 脚本输出 JSON lines → 结构化 manifest/stage/log 事件 → renderer checklist | flockmux spawn worker 时目前没有初始化进度可视化；hermes 的协议可以参考用于 MCP 服务安装/工具 setup 等长任务，让用户看到每步状态而非等黑屏 | 中（需后端约定 JSON lines 格式 + 前端 checklist 组件） | 后端 routes/spawn.rs 在 spawn worker 时可以对 MCP 初始化、workspace setup 等阶段广播 typed event；前端蜂群页的活动行升级为带 stage checklist 的 drawer |
| P2 | 多 Profile 后端池（LRU + keepalive-fresh 保护 + 懒启动） | flockmux 已有 workspace 概念但 profile 多实例管理较弱；hermes pool 的 LRU + 活跃保护设计可以移植到 flockmux 的多 workspace 场景 | 中（Rust 侧 HashMap<workspace_id, BackendHandle> + idle reaper task） | server/src/agent/ 或 workspace 管理层：per-workspace 维护独立 PTY session，idle 超时后释放，新请求时懒启动 |
| P2 | xterm WebGL renderer + ResizeObserver rAF 合并 + selection→chat（终端选区注入聊天） | flockmux 已有终端；hermes 的 stripInitialPromptGap（过滤启动空白）、WebGL fallback、rAF 合并 resize（防 mid-rebuild crash）、selection Add to chat 浮动按钮 是 polish 细节 | 小（纯前端优化，xterm 已存在；WebGL addon + 几个事件处理） | 前端 terminal 组件：加 WebglAddon、ResizeObserver rAF 合并、selection 浮动按钮（发到 composer） |
| P2 | CSS 变量布局协议（PaneShell emit → 跨组件消费，无 context/store） | flockmux 侧边栏宽度等布局信息可能在多处重复计算；hermes 的 --pane-{id}-width CSS 变量方案让 titlebar、sidebar、preview 等独立组件用 calc() 就能响应宽度变化 | 小（纯 CSS 重构，无 JS 变化） | 前端 layout 层：sidebar/panel 宽度改为 emit CSS 变量，titlebar 和 pane 组件用 calc() 而非 JS 计算 |
| P2 | Profile SOUL.md 内嵌编辑器（persona/system prompt 可视化编辑） | flockmux 缺少 worker agent 系统提示的 UI 管理；hermes 的 SOUL.md 把 persona 暴露成可编辑文件，用户可直接在 GUI 里写 system prompt | 小（前端 textarea + 后端 GET/PUT /api/profiles/:name/soul 接口） | 前端设置页或角色注册表页：每个角色可编辑默认 system_prompt；后端 roles.toml 或 DB 存储；spawn 时注入 --system-prompt |
| P2 | 自更新分支自愈（resolveHealedBranch：origin 删分支后自动回 main） | flockmux 未来若有自更新功能，当跟踪分支被删时需要优雅降级而非报错 | 极小（git ls-remote --exit-code 探针 + 写持久化配置） | 未来 self-update 模块：update_branch 配置 + 每次 check 前 ls-remote probe |

