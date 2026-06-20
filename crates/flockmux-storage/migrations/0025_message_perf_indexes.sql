-- 0025_message_perf_indexes: 补两个 messages 表热点查询的索引。
--
-- `messages` 是只追加(append-only)、未读 wake 永不被 prune 的表(见 prune
-- 逻辑),会随会话单调增长。0001 只给了 (to_agent, delivered_at) 和
-- (to_agent, id) 两个索引,以下三类高频查询因此退化成全表扫描或宽扫描
-- (用真实 5 万行库 EXPLAIN QUERY PLAN 证实):
--
--   1. count_unread / consume_wakes / mark_read
--        WHERE to_agent = ? AND read_at IS NULL [AND kind='wake']
--      走 idx_messages_to_id 只能定位 to_agent,然后要扫该 agent 名下
--      *全部历史消息* 去测 read_at —— 每个 agent 每回合(Stop hook)都跑。
--   2. agent_silent_since_ready (静默看门狗)
--        NOT EXISTS (SELECT 1 FROM messages WHERE from_agent = a.id)
--      from_agent 无索引 → 每次探测全表扫描。
--   3. reassign_unread_user_messages / latest_user_message_for_agents
--        WHERE from_agent = 'user' ...
--      同样全表扫描。
--
-- 两个索引解决以上全部:
--
-- (A) 部分索引 (partial index) on read_at IS NULL —— 只索引"当前未读"这个
--     工作集(通常几百条),read_at 一被置位该行就移出索引,所以索引体积小、
--     写放大极低。带上 kind 让 consume_wakes 的 kind='wake' 也走索引。
-- (B) from_agent 单列索引 —— 让看门狗的相关子查询用上 COVERING INDEX,
--     从全表扫描降到索引探测。
--
-- 刻意不加 messages(thread_id) 索引:聊天主查询是
--   (thread_id = ? OR thread_id IS NULL) ORDER BY id DESC LIMIT 200
-- 这个 OR-with-IS-NULL + LIMIT 形态下,对活跃 thread 规划器选择反向 PK 扫描
-- 并提前终止本就是最优,实测不会采用 thread_id 索引;加了纯属高写入表上的
-- 写放大、零收益。仅"打开稀疏旧 thread"冷路径会扫全表,非热路径,留待真有
-- 投诉时改写 OR 为 UNION 再处理。

CREATE INDEX idx_messages_to_unread
    ON messages(to_agent, kind) WHERE read_at IS NULL;

CREATE INDEX idx_messages_from_agent
    ON messages(from_agent);

INSERT INTO schema_version VALUES (25);
