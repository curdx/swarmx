# flockmux vs. OpenRouter Fusion —— 思想定位与对外话术

> 调研日期：2026-06-17 ·  方法：官方文档逐条核对 + 多源联网调研 + 对抗式证伪（"同/异"双方辩护 + 裁决，裁决信心 0.85）+ 对照 flockmux 真实代码（`roles/orchestrator.md` 等）。
>
> 起因：有人问「OpenRouter Fusion 的思想跟 flockmux 是不是一回事？」——本文给出可对外引用的结论与依据。

---

## TL;DR

**不是同一个思想。它们是「多个 LLM 一起干活」这张大地图上**相邻但不同的两支表亲**，分处一道经典分岔线的两端：**

- **Fusion = 同题聚合（ensemble）**：N 个模型答**同一道题** → 裁判调和冗余答案 → 产出**一个更好的文字答案**。
- **flockmux = 异题分解（orchestration）**：队长把**一个目标**拆成不同子任务 → 分派给专长不同的 agent → 整合互不重叠的成果 → 产出**交付的真实软件**。

一句对外定位话术：

> **模型路由 / Fusion 类产品是「组合模型，把一个 prompt 答得更好」；flockmux 是「组合 agent，把不同的活分头干完并交付真实软件」。**

同一张地图、相邻分支、互补不互斥、能彼此嵌套——但不是同一个东西。

---

## 一、Fusion 到底是什么（已对官方文档逐条核对）

Fusion 是一个注入式的「多模型审议工具」（server tool `openrouter:fusion`，亦有等价的模型别名 `openrouter/fusion`，二者命中同一条流水线）。三段式：

1. **Panel（合议庭）**：1–8 个 `analysis_models` 拿到**完全相同的 prompt**、**并行**作答，每个都开了 `web_search` + `web_fetch`（默认 panel = 3 个质量预设模型）。
2. **Judge（裁判）**：一个裁判模型「**比较而非合并**」（"compares them — it doesn't merge them"），返回结构化 JSON：`consensus`（共识，视为高置信）/ `contradictions` / `partial_coverage` / `unique_insights` / `blind_spots`（盲区）。明确「不是简单多数投票」。
3. **Final（成稿）**：外层模型拿这份分析，**写出一个最终文字答案**。
4. **禁止递归**：用 `x-openrouter-fusion-depth` 头追踪深度，panel/judge 不能再注入 `openrouter:fusion`，把审议封在一层。

**计费**：付的是所有底层补全之和（panel 成员 + judge），3 模型 panel 约等于 4–5× 单次补全。**卖点**：用一堆便宜的异构模型拼出接近 Fable 5 的质量、约一半成本（DRACO 69.0% vs Fable 5 单模 65.3%；预算 panel = Gemini 3 Flash + Kimi K2.6 + DeepSeek V4 Pro 得 64.7% @ ~半成本）。约 2026-06 上线，正值 Fable 5 / Mythos 5 暂停期。

**定性**：这是「对同一任务做冗余 + 聚合」，输出只有文字——**没有文件系统、没有 commit、没有长活/有状态 agent、没有任务分解**。OpenRouter 自己把方法称作 "mixture-of-agents"。

---

## 二、决定性差异：同题聚合 vs 异题分解

| 维度 | OpenRouter Fusion | flockmux |
|---|---|---|
| **组合轴（最关键）** | 所有成员答**同一个** prompt → 聚合冗余答案 | 队长把目标拆成 DAG，每个 worker 干**不同**子任务 → 整合 |
| **合并的本质** | judge **调和重叠的竞争答案**（共识/矛盾/盲区） | orchestrator **拼装互不重叠的成果**（合代码） |
| **所在层** | 推理 / API 层，一次无状态补全调用 | Agent / 进程编排层，拉起真实长活 CLI 进程 |
| **产物** | 一段更好的**文字** | **提交到 git worktree 的真实代码** |
| **状态** | 无状态、一次性、禁递归（一层封顶） | 有状态、**可重启续跑**（双 ledger 在 blackboard；WakeCoordinator 依赖落地即唤醒） |
| **工具面 / 多厂商动机** | 仅 web_search/fetch；成员**可互换**，多厂商图**视角多样性** | 按**专长**选引擎 + 角色化 worker |
| **依赖语义** | 纯 fan-out，模型间零依赖 | 类型化 produces/consumes 连成 DAG，上游落地自动唤醒下游 |
| **解决的问题** | 让**一个答案**更准更稳 | 把**一个多步项目**真正做出来（吞吐 + 广度） |
| **组合方式** | 调和重叠的**重复答案** | 拼装互不重叠的**不同零件** |

> 最稳的分隔标准是**组合轴（同题 / 异题）**，它在代码里可验证：类型化 produces/consumes 依赖和 DAG 计划在「同题聚合」下毫无意义，是「分解」的签名。「推理层 / 进程层」这条次要标准并非百分百干净（Multi-Agent Debate 是同题聚合却带进程式多轮通信），但组合轴始终成立。

---

## 三、两条血统（联网调研到的「类似思想」）

### Fusion 的血统 —— 集成 / 冗余（让单个答案更好）

| 系统 | 怎么组合 | 引用 |
|---|---|---|
| Self-Consistency (Wang et al., 2022) | 一个模型采样多条 CoT，多数投票 | [arXiv 2203.11171](https://arxiv.org/abs/2203.11171) |
| More Agents Is All You Need / Agent Forest (2024) | N 个独立采样 + 投票，精度随数量上升 | [arXiv 2402.05120](https://arxiv.org/abs/2402.05120) |
| LLM-Blender (ACL 2023) | PairRanker 排序 + GenFuser 融合 top-K | [arXiv 2306.02561](https://arxiv.org/abs/2306.02561) |
| **Mixture-of-Agents (Together AI, 2024)** | proposer 各答同题 → aggregator 综合，可叠层（**Fusion 的直系父亲**） | [arXiv 2406.04692](https://arxiv.org/abs/2406.04692) · [博客](https://www.together.ai/blog/together-moa) |
| Multi-Agent Debate (Du et al., ICML 2024) | 多实例答同题，多轮互读互改至收敛 | [arXiv 2305.14325](https://arxiv.org/abs/2305.14325) |
| 推理时计算扩展（umbrella） | best-of-N / 投票 / 树搜索，对同一 query 花更多算力换精度 | [arXiv 2408.00724](https://arxiv.org/abs/2408.00724) |

**同源但不同策略的产品**：NotDiamond / Martian / Unify / RouteLLM / OpenRouter Auto Router——这些是「选 1 个模型」的**路由**（第三类），不聚合。Fusion 与它们都在推理/API 层，但 Fusion 属「聚合多个」，路由属「选一个」。

### flockmux 的血统 —— 编排 / 分工（把工作做完）

| 系统 | 怎么组合 | 引用 |
|---|---|---|
| **Microsoft Magentic-One** | 双 ledger（Task + Progress）编排器，按能力路由到专长 agent；stall→replan。**flockmux 直接照搬**（见下） | [MSR](https://www.microsoft.com/en-us/research/articles/magentic-one-a-generalist-multi-agent-system-for-solving-complex-tasks/) |
| Anthropic 多 agent 研究系统 | LeadResearcher 拆问题、并行 spawn 子 agent（各自独立 context）再综合；明确收益**「不是靠冗余」** | [Anthropic Engineering](https://www.anthropic.com/engineering/built-multi-agent-research-system) |
| Microsoft AutoGen | GroupChatManager 调度多个会话 agent 分工（Magentic-One 跑在它之上） | [docs](https://microsoft.github.io/autogen/0.2/docs/Use-Cases/agent_chat/) |
| CrewAI | role/goal/task 角色 agent，sequential / hierarchical(manager 委派) | [docs](https://docs.crewai.com/en/learn/hierarchical-process) |
| MetaGPT | SOP 流水线 PM→Architect→Engineer→QA，各产出类型化交付物 | [arXiv 2308.00352](https://arxiv.org/abs/2308.00352) |
| ChatDev | 虚拟软件公司「chat chain」分阶段两两对话 | [PDF](https://arxiv.org/pdf/2307.07924) |
| LangGraph supervisor | 有状态图，supervisor 经 handoff 委派 worker 子图 | [docs](https://reference.langchain.com/python/langgraph-supervisor) |
| OpenAI Swarm | 最小 Agents + Handoffs 原语（API 层表达分工/路由） | [GitHub](https://github.com/openai/swarm) |

**两边的学术祖先几乎不重叠**——这是「相邻表亲」而非「同一物种」的硬证据。

### flockmux 在血统里的位置（代码核对）

`roles/orchestrator.md` 第 4 行原文写着「**Magentic-One 双 ledger 模式**」，并几乎逐字实现了：

- Task Ledger（Facts / Guesses / Acceptance / Plan-DAG）+ Progress Ledger（Status / Current step / Assignments / Blockers），均存为 blackboard key（`{workspace_id}/{thread_slug}/task.ledger.md` 等）——**为了重启续跑 + 给用户可见**（"the ledger IS the recovery"）。
- 真任务拆成 DAG，`swarm_spawn_worker` 派 frontend/backend/reviewer/test-runner/researcher/fixer/docs-writer 等角色，各在隔离 git worktree 干**不同**子任务、commit 真实代码。
- 类型化 produces/consumes 依赖（`{from_role, kind}`，server mint handoff key），上游落地自动唤醒下游。
- 「并行广度调研」派发照搬 Anthropic 风格（orchestrator.md 直接写「Anthropic Research 风格 / scaling rules」）。
- 按**专长**选引擎：claude=前端/文案/调研，codex=后端/shell/审查，opencode=第三独立引擎。

---

## 四、诚实交代：它们在哪儿「押同一个韵」

对抗式证伪中「同一思想」方能找到的真实重叠（裁决方采信为 overlap）：

1. **都有一个不亲自干活、只负责合并的「特权协调者」**：Fusion 的 judge ≈ flockmux 的 orchestrator 收割那一步。形似。
2. **flockmux 确有一个 Fusion 形状的二级模式**：`orchestrator.md` 的「并行广度调研」——一次性 spawn N 个 worker 并行、下一轮收割综合。**拓扑同形（fan-out → synthesize）**——这是「同一思想」论点最强的地方。但**输入分布相反**：Fusion 给 N 个成员**同一个** prompt（冗余），flockmux 给 N 个 researcher **不同角度**、写不同 blackboard key（广度）；且它只是 triage 表里的一个分支，不是 flockmux 的身份。
3. **都用异构多厂商模型**——但动机不同（多样性聚合 vs 专长分工）。
4. **可组合**：Fusion 完全可以当成 flockmux 某个 worker 内部那一次推理调用的模型端点。能干净嵌套，说明确实共享一条概念边界。
5. **聚合是「原语 / 手法」而非身份**：编排框架（含 flockmux orchestrator、甚至 Claude Code 的 Workflow "judge panel"）随时可以把一次 Fusion 式聚合塞进某一步当工具用——所以「会用聚合」不等于「就是 Fusion」。

**为何最终仍判「不同」**：上述重叠多为「supercategory 相同 + 协调者角色相似」；而组合轴 / 层 / 产物 / 状态 / 工具面 / 依赖语义 / 解决的问题 / 学术血统——**8 条承重维度全部分叉**。份量压倒性落在「不同」，所以是 **cousins-not-same**；又因同图、可嵌套，所以是 **cousins-not-unrelated**。

---

## 五、taxonomy 速查（把两者放同一张图）

学术界给「多个 LLM 一起干活」划三类（见下方 survey）：

1. **模型路由 routing**（选 1/N）：NotDiamond / RouteLLM / OpenRouter Auto——*两者都不是*（flockmux 仅在「每个 worker 选哪个引擎」处带一丝路由味）。
2. **集成 / 聚合 ensemble**（同题多答再合并）：**Fusion 在此**（ensemble-after-inference / MoA）。
3. **编排 / 分工 orchestration**（异题分解再整合）：**flockmux 在此**（Magentic-One 同范式）。

| | Fusion | flockmux |
|---|---|---|
| taxonomy 归属 | ensemble-after-inference / Mixture-of-Agents（产品化 MoA） | 协作式分工多 agent 系统（Magentic-One 双 ledger） |
| 关键 survey | [LLM Ensemble survey 2502.18036](https://arxiv.org/html/2502.18036) | [Multi-Agent Collaboration survey 2501.06322](https://arxiv.org/html/2501.06322v1) |

---

## 六、可直接复用的对外话术

- 一句话：**「Fusion 组合模型把一个答案答得更好；flockmux 组合 agent 把不同的活分头干完并交付软件。」**
- 防误解：**「我们不是 Fusion / 模型路由的竞品，是它们够不到的上一层——有状态、用工具、会 commit、能续跑的『拆解—委派—整合』编排。Fusion 甚至可以当我们某个 worker 内部那一次推理的模型端点。」**
- 若被追问「你们的并行调研不就是 Fusion 吗」：**「拓扑同形，输入相反——Fusion 给每个成员同一道题求冗余，我们给每个 worker 不同角度求广度；而且那只是我们一个调度分支，核心是 Magentic-One 式的异题分解与代码交付。」**

---

## 附录：主要来源

**Fusion 本体**
- [Fusion server tool 文档](https://openrouter.ai/docs/guides/features/server-tools/fusion) · [Fusion plugin 文档](https://openrouter.ai/docs/guides/features/plugins/fusion) · [产品页](https://openrouter.ai/fusion) · [Auto Router 文档](https://openrouter.ai/docs/guides/routing/routers/auto-router)

**集成 / 聚合血统**
- MoA [arXiv 2406.04692](https://arxiv.org/abs/2406.04692) · Self-Consistency [2203.11171](https://arxiv.org/abs/2203.11171) · More Agents [2402.05120](https://arxiv.org/abs/2402.05120) · LLM-Blender [2306.02561](https://arxiv.org/abs/2306.02561) · Multi-Agent Debate [2305.14325](https://arxiv.org/abs/2305.14325) · Inference Scaling [2408.00724](https://arxiv.org/abs/2408.00724)

**编排 / 分工血统**
- [Magentic-One](https://www.microsoft.com/en-us/research/articles/magentic-one-a-generalist-multi-agent-system-for-solving-complex-tasks/) · [Anthropic 多 agent 研究系统](https://www.anthropic.com/engineering/built-multi-agent-research-system) · [AutoGen](https://microsoft.github.io/autogen/0.2/docs/Use-Cases/agent_chat/) · [CrewAI](https://docs.crewai.com/en/learn/hierarchical-process) · MetaGPT [2308.00352](https://arxiv.org/abs/2308.00352) · ChatDev [2307.07924](https://arxiv.org/pdf/2307.07924) · [LangGraph supervisor](https://reference.langchain.com/python/langgraph-supervisor) · [OpenAI Swarm](https://github.com/openai/swarm)

**taxonomy / survey**
- LLM Ensemble survey [2502.18036](https://arxiv.org/html/2502.18036) · Multi-Agent Collaboration survey [2501.06322](https://arxiv.org/html/2501.06322v1) · LLM Routing survey [2502.00409](https://arxiv.org/html/2502.00409v2)

**对照的本仓代码**
- `roles/orchestrator.md`（Magentic-One 双 ledger、拆解—委派、类型化 handoff）· `crates/flockmux-server/src/{wake,roles,spells}.rs`（依赖唤醒 / 类型化 spawn / ledger）· `README.md`（架构总览）
