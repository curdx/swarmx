# 角色/任务感知配给（role/task-aware allocation）调研

> 调研日期：2026-06-02 · 方法：dynamic workflow（39 个子 agent，7 路并行 GitHub 搜索 → 去重选型 → 每仓库一个 agent 真读源码 → 综合 → 对抗式查漏 → 补读再综合）· 覆盖：**29 个仓库真读**，覆盖全部 6 种路由策略 · 全程用已认证 `gh` 拉真实 star 与源码。
>
> 目标：为 swarmx「F1 角色/任务感知配给」找设计依据——让「哪个角色 + 哪个 CLI + 哪个模型 接哪个任务」从**拍脑袋猜**变成**系统化/能力驱动**。

---

## 0. 一句话结论

> 跨 29 个高星仓库、6 种路由策略，**成熟实现无一例外收敛到「分层 hybrid」**：便宜确定性规则吃掉 80% 常规路，LLM/学习层只兜住模糊尾部，且**「匹配角色」与「选模型/CLI 层」解耦**。
>
> 对 swarmx，最高置信、最广共识、最低风险的两件事是 **P0**：
> 1. **干掉 LLM 自选的 `depends_on`/`handoff_signal` 字符串** → 改为「框架派生的 typed key + spawn 时校验整张依赖图 + WakeCoordinator fail-CLOSED」。
> 2. **把自由文本 `role_label` 换成「可校验、可扩展的角色注册表」**（TOML，serde 校验，像 RooCode 的 Zod ModeConfig），角色携带 `produces/consumes`、`default_cli`、`default_model_tier`、工具 allowlist。
>
> 然后 **P1**：先上确定性规则选 cli/model（modality 必须**声明**而非推断）→ per-(cli,model) 能力卡做**硬门控**（不只软分层）→ 双阶段「强规划/廉价执行」(Aider architect/editor) → 就绪门 + verifier 角色 + 级联失败。**P2**：可插拔配给器 + 校准成本阈值（走**离线训练**的 RouteLLM 路线，**不是**在线 bandit）+ 打 typed 结果日志攒语料。

---

## 1. 仓库清单（29 个，按 star 降序）

| 仓库 | Stars | 类目 | 路由策略 |
|---|---:|---|---|
| langflow-ai/langflow | 149,079 | workflow-engine | hybrid |
| langgenius/dify | 143,509 | workflow-engine | hybrid |
| OpenHands/OpenHands | 75,649 | cli-swarm | hybrid |
| FoundationAgents/MetaGPT | 68,478 | role-spec | hybrid |
| microsoft/autogen | 58,632 | framework | hybrid |
| ruvnet/claude-flow | 57,409 | cli-swarm | hybrid |
| crewAIInc/crewAI | 52,659 | framework | hierarchical-manager |
| BerriAI/litellm | 49,019 | model-router | hybrid |
| bmad-code-org/BMAD-METHOD | 48,469 | role-spec | hybrid |
| Aider-AI/aider ⁺ | 45,676 | role-spec | static-config（+能力卡）|
| agno-agi/agno | 40,463 | framework | hierarchical-manager |
| oh-my-claudecode (Yeachan-Heo) | 35,567 | cli-swarm | hybrid |
| musistudio/claude-code-router | 34,635 | model-router | rule-based |
| conductor-oss/conductor | 31,877 | workflow-engine | static-config |
| openai/openai-agents-python ⁺ | 26,850 | framework | hybrid |
| RooCodeInc/Roo-Code ⁺ | 24,188 | role-spec | hybrid |
| Portkey-AI/gateway | 11,938 | model-router | rule-based |
| tensorzero/tensorzero | 11,426 | model-router | learned/cost-aware |
| katanemo/plano | 6,561 | model-router | hybrid |
| open-multi-agent | 6,306 | planner-supervisor | hybrid |
| lm-sys/RouteLLM ⁺ | 4,961 | model-router | learned/cost-aware（离线）|
| VRSEN/agency-swarm | 4,429 | framework | hybrid |
| vllm-project/semantic-router | 4,253 | model-router | hybrid |
| ulab-uiuc/LLMRouter | 1,878 | model-router | hybrid |
| langchain-ai/langgraph-supervisor-py | 1,587 | planner-supervisor | hierarchical-manager |
| NVIDIA-AI-Blueprints/llm-router | 282 | model-router | hybrid |
| eclipse-lmos/lmos-router | 41 | model-router | hybrid |
| haorui-harry/agent-harness | 36 | role-spec | hybrid |
| **SeemSeam/agent-roles-spec** ◆ | 9 | role-spec | static-config |

⁺ = 对抗式 critic 追加（补强 P0 typed-handoff / P1 双阶段模型 / 离线学习路线）。◆ = 用户点名（全新、模型中立的 role.toml 规范，star 很低但贡献一个具体点子，见 §9）。

---

## 2. 配给策略分类法（taxonomy）

成熟系统几乎都是下面几种的**分层组合**。单用一种的，要么过于死板，要么过于昂贵/脆弱。

### 2.1 static-config（模型/角色/CLI 在声明时钉死，人选）
- **怎么做**：每个角色是声明对象（Python 构造器 / TOML / markdown frontmatter / JSON），携带 1 个固定模型 + 工具集；换模型 = 换一个预声明的 agent。无 per-task 选择。
- **代表**：MetaGPT(`RoleCustomConfig{role,llm}`)、agency-swarm、autogen、crewAI、agno、openai-agents-python、conductor、**Aider（关键例外：固定槽位但有真·能力卡）**、agent-roles-spec。
- **取舍**：最简单、可 diff、零路由延迟；但**无法把难任务升级到强模型、也无法把琐碎任务降级** —— 这正是 swarmx **不能照抄**的缺口。Aider 证明：static 可以接受，**前提是能力以数据声明而非靠猜**。

### 2.2 rule-based（对任务字段跑布尔/关键词/正则/打分，first-match-wins）
- **怎么做**：抽便宜同步信号（token 数、code-fence/关键词、工具类型、文件路径密度、显式 task tag）→ 有序规则/加权分 → 命中桶映射到目标。无 LLM 调用，<1ms，确定可调试。
- **代表**：claude-code-router(6 条固定优先级谓词)、Portkey gateway(`$eq/$in/$gt/$regex/$and/$or` + 默认)、litellm complexity_router(7 维加权 SIMPLE/MEDIUM/COMPLEX)、Dify IF_ELSE、oh-my-claudecode(regex SIGNALS→SCORER→RULES)。
- **取舍**：便宜、可审计、可复现，**每个成熟 router-class 仓库都建议先上这层**；但匹配器是英文关键词/正则，对改写脆弱，**不可靠提取「modality」**。最适合那些**确实可抽取**的信号（上下文大小、code/shell 出现、显式声明 tag）。

### 2.3 llm-as-router（一个 manager LLM 读 prose 能力简介，吐出收件人）
- **怎么做**：每个候选角色用 name + 自由文本「USE THIS when…」描述广告给 manager LLM；LLM 发 tool call/收件人名，框架对注册集解析。
- **代表**：MetaGPT MGX TeamLeader「Mike」、langgraph-supervisor(`transfer_to_<agent>`)、OpenHands delegate、agency-swarm(`SendMessage` enum 限定)、autogen `SelectorGroupChat`、crewAI hierarchical、agno Team route、openai-agents-python(`handoff_description`)、Roo-Code(`switch_mode`)、plano(4B Arch-Router)。
- **取舍**：零规则维护、处理开放式任务、能力就是好文案；但**质量 = 写描述的纪律**，每决策多一次模型调用，且**重新引入脆弱的自由文本收件人字符串**——除非用**闭集 enum 校验**。健壮实现（agency-swarm/autogen/agno/openai-agents-python）都校验吐出的名字、返回 `NOT_FOUND + 合法选项` 让它自纠。

### 2.4 hierarchical-manager（专职 manager 拥有派发 + 重规划）
- **怎么做**：manager 分解目标、派发子任务、每步后重决策，常维护持久 plan/ledger 与恢复循环。
- **代表**：autogen **MagenticOne**(Task Ledger + Progress Ledger + `is_in_loop`/`is_progress_being_made` 停滞检测 → 重规划)、langgraph-supervisor(星型)、crewAI hierarchical、open-multi-agent(coordinator 分解成 task DAG)、MetaGPT MGX。
- **取舍**：职责清晰（worker 专注能力、manager 拥有配给+校验+重试）；MagenticOne 的双 ledger + 停滞检测是「`.done` 永远没来」的原则性答案。但**星型每轮 round-trip 一个 LLM**——并行的 token/延迟瓶颈，**与 swarmx 并行扇出黑板模型不合**。

### 2.5 semantic-embedding（任务文本向量相似度匹配能力示例）
- **怎么做**：embed query 与每个 (agent,能力,示例) 短语 → 取最近 → 按 agent 聚合分 → 绝对阈值 + 领先 margin 门控 → 无命中则弃权。
- **代表**：litellm `auto_router`(semantic_router 库)、lmos-router(Qdrant + 阈值门 → 「无自信匹配」)、NVIDIA(CLIP→RouterNetwork MLP)、ulab LLMRouter(KNN/GraphRouter)。
- **取舍**：解决关键词漏掉的改写/同义；**阈值门给出真·弃权信号**（无角色过线→升级 LLM 仲裁）。但每请求一次 embedding 调用（延迟）、需精选示例语料、通常要外部向量库。

### 2.6 learned/cost-aware —— 两条分支（critic 重点校正）
- **离线训练（推荐路线，day-one 可用，无需在线量）**：离线在标注语料训预测器，按预测质量/难度 per-request 路由，成本旋钮离线校准（目标成本% → 分位阈值）。**代表**：**lm-sys/RouteLLM**（matrix-factorization/BERT/causal-LLM/Elo 在 Chatbot Arena 偏好数据上训练；`calibrate_threshold` 离线映射成本%→阈值；跨模型对泛化免重训）、NVIDIA llm-router、ulab LLMRouter。**这条路不需要 ~1000 次在线会话/Postgres/冷启动**，是更便宜、更低风险的学习选项。局限：单轴（只难度，无 modality/risk）、训在通用人类偏好 Elo（非任务正确性）、需标注语料。
- **在线 bandit（更高风险）**：维护 per-(任务桶, 候选) 后验（Beta/Thompson 或 Track-and-Stop），采样→观测 reward→更新。**代表**：litellm `adaptive_router`、ruflo model-router(per-complexity Beta bandit)、TensorZero(Track-and-Stop)、semantic-router `rl_driven`。**取舍**：自调优、能惩罚过度配置；但**需在线量 + 存储 + 冷启动**，且**关键缺陷：reward 是你定义的指标，没有一个仓库在「正确性」上闭环**（只奖「没报错/匹配启发式」）。

### 2.7 hybrid / 分层（**真实世界主导模式**）
- **怎么做**：拆决策——确定性/便宜层吃明显情况，fallback（LLM 或学习）吃模糊尾部；或两阶段（能力/角色匹配 → 成本/分层选择）。**匹配层与模型选择层解耦**。
- **代表**：litellm(pre-routing hook → 组内负载均衡)、plano(4B 选 route → route 内确定性 cost/latency 排序)、semantic-router(ML 信号 → 布尔决策 AST → 可插拔选择 + 置信门升级)、open-multi-agent(LLM 分明显任务 → 确定性 Scheduler 兜底)、oh-my-claudecode(角色轴 regex + 模型层轴 scorer)、**Aider(architect 规划 → editor 应用)**、NVIDIA。
- **取舍**：两全——80% 便宜确定、尾部智能，各层可换可基准。**这就是 swarmx 该建的**。代价：更多活动件、两份契约要保持一致。**每个成熟的 router-class 仓库最终都收敛到这里。**

---

## 3. 通用模式（14 条，值得借鉴）

1. **声明式角色/能力注册表即数据**（TOML/JSON/YAML/frontmatter），schema 校验，可扩展但闭集（自定义按 slug 覆盖内建）。角色**作者声明，绝不让 LLM 运行时发明**。→ RooCode Zod `ModeConfig` + `.roomodes` + JSON-schema 漂移测试；BMAD `module.yaml`；OpenHands `AgentDefinition`；agent-roles-spec `role.toml`。
2. **能力锚定在具体 affordance**（实际工具/MCP/CLI 集 + 工具组 allowlist），不只 prose 简介。只读 explorer **拿不到**编辑工具。→ OpenHands `tools` allowlist；RooCode 工具组 + `fileRegex`（`isToolAllowedForMode` 强制）；Aider `edit_format`。
3. **Typed/对象/服务端铸造的 handoff 引用，替代 LLM 打字的字符串**，build/spawn 时校验 + 运行时 fail-CLOSED。**全仓库最强共识**。→ openai-agents-python `handoff()` 返回 Agent **对象** + `input_type` Pydantic 严格 schema 先校验再 `on_handoff`；MetaGPT `cause_by` 从 Action 类算出；langgraph-supervisor build 时集合差断言；conductor `TaskReferenceNameUniqueConstraint`（注册即失败）；agno 服务端铸造 ID + `NOT_FOUND` 返回合法选项。
4. **两阶段解耦**：「哪个能力/角色合适」（语义/LLM/规则）与「给定实时成本/负载选哪个实例/层」（确定性）分开。→ litellm pre-routing hook vs 负载均衡；plano route-by-description vs route 内排序；Aider `main_model` vs `editor_model`。
5. **成本/延迟/质量作为一等声明元数据**，驱动分层 + 置信门升级（先跑便宜，仅在低置信/失败时升级）。→ litellm `quality_tier` + `min_quality_tier`；NVIDIA per-model 成本+阈值；semantic-router 置信 looper(0.72, small_to_large)；RouteLLM 校准成本%阈值。
6. **解耦 router 模型与 worker 模型**——便宜模型（或纯规则）决配给，强模型执行。→ autogen 独立 manager `model_client`；plano 专职 4B；BMAD/Aider「commit msg/总结/review 用便宜/不同模型」。
7. **硬门控（capability GATING）与软偏好（score）分离**。资格过滤先于打分。→ litellm `tier:free|paid` + `min_quality_tier` 过滤；RooCode 工具组/fileRegex 硬阻断；agent-harness `banned_high_risk_skills`。
8. **闭集 enum 收件人 + 自纠 NOT_FOUND**：选择者从校验集挑，错选返回合法选项让它重试而非静默误路。→ agency-swarm 收件人 enum；autogen 参与集校验 + 反馈重试；agno `NOT_FOUND` + 全目录。
9. **订阅即契约 / 按声明类型 pull 的 pub-sub** —— **最贴 swarmx 黑板拓扑**。消费者声明它 watch 的输出**种类**，key 从注册种类算出，绝不手打。→ MetaGPT `_watch([ActionClass])` over message pool；conductor worker 轮询它实现的精确 task-type。
10. **fallback 链按稳定 group/role 名键控**，前置校验，覆盖失败 + 上下文窗口溢出。→ litellm `fallbacks` + `context_window_fallbacks`（init 校验）；plano 指标排序 `RoutingResult.models`；claude-code-router 每场景 fallback。
11. **可解释路由 trace**：每决策吐 `reasons[]` + confidence + 触发信号，坏选可审计。→ OMC `explainRouting`；semantic-router `EvaluateDecisionsWithTrace`；agent-harness `routing_trace`/`route_regret`。
12. **人读依赖引用在 plan 时解析成 canonical ID**（对作者可读、对机器 typed），未解析/歧义/成环时**大声失败**。→ open-multi-agent `dependsOn`-by-title→UUID；crewAI `Task.context = list[Task]` 对象引用；Aider `editor_edit_format = 'editor-' + base`（生产/消费不能各自漂移）。
13. **状态枚举状态机 + claim token 作 wake 驱动**，替代不透明 per-task key。固定状态词表让协调器挑第一个匹配状态的任务、无匹配时 HALT-with-options。→ BMAD `sprint-status.yaml`(backlog→ready→in-progress→review→done)；OMC typed task store + claim-token 乐观并发 + 合法转移表。
14. **共享权威决策/架构产物**，每个并行 worker 加载为持久上下文，防跨并发 worker 的 API/命名/状态冲突。→ BMAD `architecture.md`；agent-harness 共享 typed `GraphState`。

---

## 4. 反模式（11 条，重点 1/3/4/5/8）

1. **脆弱自由文本 handoff/收件人/key 契约，静默或迟到失败**。**swarmx 已中招**（`depends_on`/`handoff_signal` typo/前缀/大小写漂移破坏 WakeCoordinator，见 docs F3）。处处可见：plano route 名不符 warn-and-drop；NVIDIA `MODEL_ROUTER_TO_TARGET` 重复 key bug + 子串模糊重映；claude-code-router 拼错模型名静默落默认；LLMRouter `_parse_model_name` 模糊猜 + 落第一个模型。**模糊归一化是症状不是解药。**
2. **能力只建模为 prose**（角色句/描述），无机读 tag → 路由质量 = 写描述纪律，无法按 modality/risk/上下文大小匹配。LLM-router 仓库通病。
3. **单一 LLM router 作唯一机制**，无确定性规则层、无校验、无学习 fallback → 每决策烧一次模型调用，且可能误路（只有 retry-to-previous 兜底，会把坏路由伪装成「进展」）。litellm/NVIDIA/plano 都明确建议**先上规则**。
4. **星型拓扑** —— 每个 worker 每轮 round-trip 中央 manager LLM → 并行的 token+延迟瓶颈，**结构上不合 swarmx 并行扇出黑板**。langgraph-supervisor 维护者自己都在转向。
5. **模型钉在 ROLE 上、无 per-task 分层** —— 无法升级难任务/降级琐碎任务。**这正是 swarmx 必须补的维度。**
6. **角色/能力定义重复、无单一真相源 → 漂移**。ruflo 在 markdown frontmatter + route.ts + router.js 三处声明能力且互相矛盾。
7. **照抄仓库内部 magic number 当权威**。ruflo `costMultiplier`(haiku .04/sonnet .2/opus 1.0)、litellm「~1000 会话」、「100-500ms embedding」都是**仓库自断言常数/凑整，非核实定价/独立基准**。方向（opus≫sonnet≫haiku；惩罚过度配置）对，**精确乘数必须从真实 Anthropic 定价重新推导**。
8. **隐式完成信号（"发布输出然后祈祷"）静默卡死依赖方**。swarmx 已学到（Stop-hook/WakeCoordinator）。MetaGPT 经典完成 = 「我发布了 Action 输出」（漏发布卡死 watcher）。
9. **英文-only、易重叠、改写脆弱的关键词/正则/子串匹配器，包装成「智能路由」**。多个仓库 README 自认（litellm「偏英文 best-effort」、OMC「关键词集设计上就重叠」、ruflo「不是学习模型、是启发式表」却营销「89% 准确」）。
10. **把「modality(ui/backend/docs/shell)」当成可干净抽取的信号**。几乎每个 rule-router 实际键控 token 数/关键词/正则/工具类型，**没有一个仓库可靠从自由文本推断 modality**。设计假设能推断 modality 的 router 缺乏依据（见 §7）。
11. **学习路由优化「没报错」而非「正确」**。无仓库闭合正确性环：bandit 奖你定义的指标；RouteLLM 用通用人类偏好 Elo；LLMRouter 用离线基准标签。

---

## 5. swarmx 现状诊断（三大问题）

| # | 问题 | 现状 |
|---|---|---|
| 1 | **配给靠猜** | orchestrator 调 `swarm_spawn_worker` 时手动选 cli(claude/codex)、model、role_label —— 启发式拍脑袋，非能力/成本驱动，无 per-task 分层 |
| 2 | **脆弱字符串契约** | `depends_on`/`handoff_signal` 是 LLM 自选字符串，typo/前缀/大小写漂移 → WakeCoordinator 静默不唤醒（docs F3 已记） |
| 3 | **无任务→能力匹配** | 没有把任务性质（modality/risk/cost/上下文大小）匹配到 agent 声明能力的机制 |

---

## 6. 落地建议（按优先级，含 effort 与借鉴来源）

### P0 — 最高置信、最广共识、最低风险

**P0-A（effort M）· 干掉 LLM 自选 handoff key，改框架派生 + spawn 时校验整图**
- worker 声明它产出的 typed **输出种类**；后端铸造 canonical 黑板 key（如 `normalize(role)+'.done'` 或 `<workspace>/<role>/<output_kind>`）；消费者按派生 key 引用。
- spawn 时拒绝任何「没有声明/在线生产者会写」的 `depends_on`，硬报错 `unknown key — valid keys: [...]`（**不是**静默永不唤醒）。WakeCoordinator **fail-CLOSED**：未知/永不写的 dep → 标记 blocked-and-surfaced，不挂起。近似命中加「did you mean」建议。
- **直接修问题 #2**。借鉴：openai-agents-python（`handoff()` 对象非字符串 + `input_type` 严格 schema）、langgraph-supervisor（build 时 `agent_names - handoff_destinations == {}` 断言）、conductor（注册即失败 + responseTimeout requeue）、agno（服务端铸造 ID + fail-closed + NOT_FOUND 返回选项）、MetaGPT（`cause_by` 从类算出免 typo）、open-multi-agent（title→UUID，未解析/歧义/成环大声失败）。

**P0-B（effort M）· 自由文本 role_label → 可校验、可扩展的角色注册表**
- TOML/JSON，serde 校验（像 RooCode Zod `ModeConfig`）。每角色声明 `{ slug, system_prompt seed, when_to_use(薄路由提示), produces[]/consumes[] 输出种类, default_cli, default_model_tier, 工具/MCP allowlist }`。
- 内建默认写在 Rust，项目级 `.swarmx/roles.toml` 按 slug 扩展/覆盖，坏条目 soft-fail（精确记 issue、跳过）+ 生成 editor JSON-schema + 漂移测试。`swarm_spawn_worker` 选校验过的 slug，未知 slug 大声报错。
- **是解决 #1 #3 的底座**：把 role_label 从发明的名词变成携带 cli/model 默认值与 produces/consumes 接线的 typed 契约。**`when_to_use`（面向 router）与 `system_prompt`（面向 worker）分开**。借鉴：RooCode、BMAD `module.yaml` 3 层合并、OpenHands `AgentDefinition`、OMC `AgentPromptMetadata`、agent-roles-spec `role.toml`（其 source→projection 「own-and-revert」边界顺带修 M6b 的 `~/.claude.json` 互相覆盖）。

### P1 — 配给智能的主体

**P1-A（effort M）· 先上确定性规则 cli/model 选择器（在任何学习/LLM router 之前）**
- 有序 first-match-wins 规则，over 角色声明 tag + **可靠可抽取**任务信号：token/上下文大小带、code-fence/shell/文件路径密度、显式 risk flag、**orchestrator 在 spawn 时声明的 modality tag（不推断）**。映射到 (cli, model_tier)，多谓词命中时有文档化优先级。每决策吐 `reasons[]`+confidence 给 DAG UI。LLM-orchestrator 判断**只**留给模糊尾部（hybrid）。
- **关键警告（critic gap #2）**：**别**围绕「从自由文本推断 modality」设计——没有仓库可靠提取它。把 modality 当**声明的任务输入**（orchestrator 知道自己在 spawn 一个 UI 任务），规则键控**确实可抽取**的信号。借鉴：claude-code-router、Portkey gateway、litellm complexity_router、oh-my-claudecode、Dify IF_ELSE。

**P1-B（effort L）· per-(CLI, model) 能力卡 + 硬门控（不只软分层）**
- 每卡声明 `{ cli, model, modality_strengths, max_context, accepts_thinking/vision, cost_tier, shell/file-write allowed, preferred edit format, default fallback }`。两用：**(a) 硬 GATE** —— 打分前过滤不合格 (cli,model) 对（docs 角色选不到 codex shell worker；>150k token 任务选不到小上下文模型）；**(b) 软层偏好** —— 幸存者里排序。**在工具层强制门控**（拒绝越界 spawn），不靠信 LLM。
- **解决 #3**。门控比分层更重要，因为 **swarmx 的 CLI 是非对称的**（codex app-server vs Claude PTY、MCP per-agent 配置、vision/长上下文支持不同）→ 资格是硬约束。**effort 是 L 而非 M**：per-(cli,model) 粒度 + 底座非对称使其不止是模型层表。Aider 证明 per-MODEL 卡只在底座统一（litellm）时够用——swarmx 底座不统一，故需 per-(cli,model)。借鉴：RooCode 工具组+fileRegex（门控机制）、Aider 能力卡、litellm `model_info`、langflow `ModelMetadata`、lmos-router、semantic-router modelCards。

**P1-C（effort M）· 双阶段「强规划/廉价执行」作为分解门**
- orchestrator(opus/claude) 出精确 plan + DAG；廉价执行者(sonnet，或 codex 做 shell/backend) 应用，其 (cli,model) **从 plan 作者的角色卡钉死**，非每任务新猜。按 t-shirt 复杂度门控分解：XS/S → 单个便宜 worker（或纯机械编辑走 $0 确定性 codemod/脚本）；XL → 扇出 typed pipeline。review/verify 路由到**不同**于生产者的 (cli,model)。
- **攻击 #1 成本维度**。典范生产实例是 **Aider architect→editor**（critic gap #4，正是 swarmx 域）。借鉴：Aider、MetaGPT TL t-shirt sizing、ruflo Tier-0 WASM codemod($0)、plano、BMAD。

**P1-D（effort M）· 就绪门 + verifier 角色 + 传递性级联失败**
- 下游 wave 前校验所需上游黑板产物存在且对齐（PASS/CONCERNS/FAIL）。保证所需角色存在（risk=high 强制加 verifier/test-runner，代码改动加 reviewer）。生产者死亡时**传递性**标记依赖方 skipped/failed 并唤醒 orchestrator，而非留在永不出现的 key 上（把现有 per-role `.error` 扩成图级级联）。
- **是通向「正确性」信号的唯一有依据路径**（critic gap #8）：verifier worker 的 pass/fail 是真 reward 源，不像「没报错」。借鉴：BMAD readiness 门、OMC verify 阶段(floor sonnet)、agent-harness `_enforce_role_slots`、open-multi-agent `cascadeFailure/cascadeSkip`、autogen MagenticOne Progress Ledger。

### P2 — 系统化与学习地基

**P2-A（effort M）· 配给器可插拔 + 分层 override + 校准成本阈值**
- 一个接口背后分层：static 角色卡默认 → 项目 override → 规则引擎 → LLM-orchestrator fallback（新任务）。加成本层选择器 + 校准阈值。先 rule + 置信门升级（先跑 sonnet/haiku，仅低置信时升 opus）。LLM-orchestrator 当逃生舱不当默认路。**推迟在线 bandit；若做学习层选择器，走离线训练路线。**
- **解决 #1 + critic gap #1**：离线训练路线（RouteLLM）从标注语料 day-one 可用，**无在线量/Postgres/冷启动**，比在线 bandit 更便宜更低风险。**重要：从真实 Anthropic 定价重推成本数字，别抄 ruflo .04/.2/1.0**（critic gap #5）。借鉴：RouteLLM、NVIDIA `objective_fn`、claude-code-router 3 层 override、semantic-router 置信 looper、litellm。

**P2-B（effort L）· 为学习配给器打地基：每完成任务记 typed 结果日志**
- 记 `(任务描述 + 声明 modality/risk tag, 选的 cli+model+role, handoff_landed?, verifier_passed?, retries, cost, latency)` 到黑板。**先不接路由**——先攒标注语料 + replay-eval（拿未来 auto-allocator 对当前规则/手动基线打分）。
- 无 logged 结果指标，任何「任务感知配给」只是更花哨的猜（TensorZero 教训）。**用 `verifier_passed` 而非误导的 `handoff_landed` 逼出正确性问题**。诚实警告：无仓库证明运行时正确性 reward 便宜——故 instrumentation 先行、learning 后做。借鉴：TensorZero、LLMRouter eval harness、RouteLLM eval 纪律、ruflo per-bucket priors。

---

## 7. critic 校正的关键事实（避免踩坑）

对抗式 critic 追加 4 个仓库 + 标记 8 处分析薄弱点，已折回上面建议。最 load-bearing 的几条：

1. **modality 不可靠推断**：taxonomy 与建议曾倾向把 modality 当干净路由信号，但**无仓库可靠从自由文本提取它**。→ swarmx 应把 modality 当 **spawn 时声明的 orchestrator 输入**，规则键控可抽取代理（code-fence/shell/文件路径密度）。代价：把猜推到 orchestrator 这一层。
2. **学习路由可离线**：原综合偏向在线 bandit（需 ~1000 会话+冷启动）。RouteLLM 证明**离线训练 + 校准成本阈值可 day-one 上线**，软化了 P2 的冷启动风险框架。
3. **per-(cli,model) 而非 per-model 卡**：因底座非对称（codex app-server vs claude PTY），能力门控需 per-(cli,model) 粒度，把 P1-B 抬到 L effort。
4. **cost magic number 不可信**：ruflo/litellm 的数字是仓库自断言，必须从真实 Anthropic 定价 + 实测本地 CLI 延迟重推。
5. **拓扑错配（已承认）**：swarmx 是并行扇出 + 共享黑板 + 事件驱动 WakeCoordinator；样本绝大多数是星型/顺序/设计期静态 DAG。最近拓扑的是 MetaGPT `_watch` pub-sub 与 conductor pull-by-type 队列，但**两者都不在并行黑板 swarm 上做 LLM 驱动的 role/cli/model 路由**。→ 建议是把路由层**外推**到 swarmx 拓扑，而非照抄已验证的同类。
6. **reward ground-truth（未解决）**：「handoff produced + downstream ran」优化「没报错」而非「正确」。verifier worker 是识别出的唯一有依据正确性源，但其 per-task 成本/可行性未证。

---

## 8. 开放问题（设计前需拍板）

1. **modality 抽取**：声明（最可靠，但把猜推上一层）vs 从代理推断 vs 接受只能是 LLM 判断？建议默认「声明」。
2. **学习层 reward ground-truth**：能否做出足够便宜的 verifier 信号，还是只能靠离线标注历史？
3. **拓扑错配**：typed-handoff + capability-routing 遇上真并发会坏什么（key 上竞态、消费者醒后生产者又改了）？
4. **per-CLI 非对称成本**：能力卡要多细粒度？per-CLI affordance 多久变一次（重推负担）？
5. **成本数字**：今天 opus/sonnet/haiku 真实每 token 成本 + per-CLI spawn/延迟开销是多少？
6. **离线语料可得性**：swarmx 有足够带成功标签的历史 swarm 跑来训练，还是必须先上 P2-B instrumentation 等数据？
7. **CLI 选择硬门 vs 软偏好**：cli(claude vs codex) 是硬能力门（docs 角色永不得 codex）还是成本层可覆盖的软偏好？非对称性主张「affordance(shell/vision) 硬门、风格契合(prose vs backend)软偏好」。
8. **分解阈值**：XS/S→单便宜 worker vs XL→typed pipeline 的边界在哪，能否校准而非猜？
9. **角色爆炸 vs 复用**：ruflo 98+ 角色定义漂移；BMAD/OpenHands 保持小而精。swarmx 内建注册表多大合适，怎么防 `.swarmx/roles.toml` 覆盖碎片化成 per-project 漂移？

---

## 9. 用户点名仓库：SeemSeam/agent-roles-spec

- 全新（2026-06-02 push）、9 star、描述空、**模型/提供商中立**的 `role.toml` 角色身份规范。
- 对模型/CLI/成本/延迟**只字未提**——不解决 swarmx 的 per-task 分层问题。
- **唯一可借的具体点子**：`role.toml` 的**身份声明 + source→projection 的「own-and-revert」边界**——这恰好能用在 P0-B 的角色注册表上，且**顺带修 M6b 记录的 shared `~/.claude.json` mcpServers 互相覆盖**问题（每 agent own 自己投射、退出 revert）。
- 结论：作为「高星类似实现」它不够格，但作为**角色规范的最小格式参考**纳入是合理的。

---

## 10. 建议下一步

1. **先做 P0-A + P0-B**（typed handoff key + 角色注册表）——置信最高、风险最低、互为底座，且 P0-A 直接消灭已知的 WakeCoordinator 漂移 bug 类。
2. 同步起草 **per-(cli,model) 能力卡 schema**（P1-B 的数据先于逻辑），并**从真实 Anthropic 定价 + 实测 CLI spawn/延迟**填一张本仓库自己的成本表（不抄任何仓库的 magic number）。
3. P1-A 规则选择器落地时，把 **modality 作为 `swarm_spawn_worker` 的声明入参**，不做自由文本推断。
4. P2-B instrumentation 尽早开（哪怕配给逻辑还没上）——它是未来一切「学习配给」的前提，且能立刻用 replay-eval 给 P1 规则打分。

> 原始数据：39 个子 agent / 330 万 token / 664 次工具调用 / ~34 分钟。完整深读记录见 workflow transcript（`wf_7998e410-f06`）。
