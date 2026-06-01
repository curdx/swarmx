-- 0010_message_thread_id: tag each message with the direction (thread) it
-- belongs to. 0009 deliberately left messages untagged and scoped the chat
-- client-side by "which agents are in this view". That works but is a
-- heuristic — a mis-tagged or null-thread agent can leak across direction
-- views. This column makes the scope ground truth: the server stamps it on
-- send (sender's thread, else recipient's), the UI hard-gates on it.
--
-- Nullable: pre-existing rows (and any sender/recipient with no thread) stay
-- NULL, which the UI treats as "main" — so legacy chat history keeps showing.
ALTER TABLE messages ADD COLUMN thread_id TEXT;

INSERT INTO schema_version VALUES (10);
