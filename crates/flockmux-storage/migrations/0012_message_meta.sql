-- 0012_message_meta: structured metadata for system-generated messages.
--
-- System messages (wake pings, the worker-disband farewell) used to carry
-- their semantics only as prose in `body`, forcing the UI to regex-parse the
-- sentence back into data (e.g. extract a blackboard key out of "共享区 `K`
-- 有更新"). Add a nullable JSON `meta` column so the server stamps the
-- structure once — e.g. {"subtype":"wake","reason":"blackboard","key":"…"} or
-- {"subtype":"completion","signal":"reviewer.done"} — and the UI renders /
-- filters from `meta.subtype` instead of parsing prose. This is the
-- typed-payload pattern (GitHub event payloads, Slack message metadata).
--
-- Nullable + no DEFAULT: existing rows and all agent free-text messages stay
-- NULL (the UI falls back to body heuristics there). Safe ADD COLUMN, no
-- table rewrite; the messages_fts triggers reference explicit columns so a new
-- column doesn't touch them.
ALTER TABLE messages ADD COLUMN meta TEXT;

INSERT INTO schema_version VALUES (12);
