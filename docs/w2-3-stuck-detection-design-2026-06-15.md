# W2-3 卡住检测(看门狗)— 设计参考(基于 2025–2026 最权威实践对标)

> 日期:2026-06-15。来源:Workflow 4 路调研(Temporal/durable-execution、Claude Code Agent Teams、LangGraph/OpenAI、分布式 failure detector)。这是 W2-3「连续停滞看门狗」落地前的设计依据,回答一个核心质疑:**怎么知道 agent 卡住了,而不是模型正在干活?——绝不能只靠"时间过长"。**

## 核心共识(从理论到工业一条线)
**"慢"和"死"无法瞬时区分,所以不靠超时硬判,而靠"持续产出心跳/进度"代理活性,判定输出成"软怀疑、可恢复"——宁慢勿误杀。**
- **理论**:Chandra-Toueg(JACM 1996)证明异步系统 failure detector 天生 unreliable,只能在 completeness 与 accuracy 间权衡;"刚崩"与"慢"无法瞬时区分是数学事实。
- **工业经典**:Temporal——服务端**不靠超时判死**,靠 activity **heartbeat + heartbeat-timeout**,并把"判死延迟"与"最长合法执行时间"(start-to-close)解耦;还在心跳=慢(不杀),心跳停=死。(docs.temporal.io/encyclopedia/detecting-activity-failures)
- **自适应前沿**:phi-accrual(Cassandra/Akka,默认阈值 8)输出连续怀疑值 φ 而非二元;SWIM/Lifeguard 加"软怀疑态 Suspect + 可申辩 + 监控方自身健康"。(akka failure-detector;hashicorp Lifeguard,误报降 50x)

**关键反面印证:LLM agent 框架普遍没有真正的在线"卡住vs干活"判据。** Claude Code 官方称执行期是 *"complete observability blackout… 没法区分工具正常在跑 vs 卡住"*(issue #43584,closed not planned);LangGraph/OpenAI 只有 `recursion_limit`/`max_turns` 计数兜底。→ **flockmux 用分布式成熟范式填这个公认空白,方向对且超前。**

## flockmux 设计对照(现有 first-response watchdog 已是多信号:消息 AND 活动 AND 用量 全空才报,精神正确)
判据 = **进程活着 AND 四通道全静止(token用量/工具调用/PTY字节/消息)AND 有结构性待办(已到的 wake/依赖key/指向它的消息却没起 turn)**;区分「被晾住(idle在提示符,修=重投wake)」vs「turn中挂死(in-turn字节/token冻死,如卡网络/TTY授权弹窗)」;判出只标软态「疑似卡住」(可恢复≠Error≠kill)+「戳一下」。

- ✓✓ 拒绝纯计时 / 软态可恢复不杀 —— 与 Temporal/SWIM 同构,最站得住。
- 🏆 **领先(四路调研里无现成系统具备)**:① **"有结构性待办却没动"做主信号**(ground-truth 级,比心跳"该报没报"更硬,且适合不肯配合打点的黑盒 agent);② **被晾住 vs turn中挂死 分两类病因**。

## 落地前必补的 2 个盲区(不补会在生产咬人,调参解决不了)
1. **服务端自身健康(Lifeguard 洞见)**:四通道是**同一个服务端**收的,服务端 GC/IO/PTY 积压一卡 → 四通道**一起假静止** → 一批正常 agent 集体误判。**补**:服务端感知自身负载/事件循环延迟/PTY 背压,不健康时整体放宽判定门槛(Local Health Multiplier)。
2. **"在动 ≠ 在前进"的退化循环**:原地刷屏/反复同一工具,四通道全在动,静止判据抓不到。**补**:廉价的单 worker 累计 turn/step 计数硬闸(= LangGraph `recursion_limit` / OpenAI `max_turns` 标配兜底);配 RemainingSteps 软信号让它临界主动收尾。

## 次要打磨(不补不会崩)
- 「戳一下」加**指数退避 + 次数限 + 总封顶**(对真死的别无限戳;戳满升级到「需人工/AwaitingHuman」,**仍不 kill**)。升级前确保最近进度快照落盘。
- 布尔阈值 →**自适应怀疑度**(按每个 agent 自己历史静默分布;agent 静默**重尾**,**别套正态**,用经验分位数 p95/p99)。结构性主信号已扛住大部分,这是边际收益。
- 判定窗口**显式参数化**,锚到「可容忍判死延迟」(Temporal 节流思想 `≈0.8×容忍上限`,具体数自测,非普适常数)。
- 把两类病因的**处置路径在代码里正交化**(被晾住→重投wake;turn中挂死→诊断网络/TTY)。
- 被判疑似后**任一通道恢复一字节/token 立即清怀疑**,并把误报回灌历史分布(喂自适应阈值)。

## 明确不做
- 多观测者/gossip 交叉确认(单服务端拓扑前提不成立)。
- φ 照搬正态分布(agent 静默重尾,帮倒忙)。
- 为"更快判死"牺牲"软态/不 kill"红线。

## 定论
**"判得准"差不多了且有领先,别再纠结。真正还差的是两个判定逻辑本身的盲区——服务端自抽风会连累判定、不前进但没静止的退化循环——补上才算真差不多;其余是打磨。**

## 出处 / 事实分界
事实(出处):Chandra-Toueg 1996;Temporal heartbeat/timeout 解耦 + 节流公式;phi-accrual(Akka/Cassandra 默认阈值8);SWIM/Lifeguard(误报降50x);Claude Code #43584「observability blackout」;LangGraph recursion_limit / OpenAI max_turns。推断(标注):50x/0.8/30s 等具体数字迁移到 flockmux 需自测;调研未读 flockmux 实际看门狗代码,落地前需对照实现取舍。
