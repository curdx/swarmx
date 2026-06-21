-- 0003_in_reply_to: thread messages via parent pointer.
--
-- ALTER TABLE ADD COLUMN with no DEFAULT is safe + back-compat: existing rows
-- get NULL, no rewrite. The optional FK ref keeps intent explicit even though
-- SQLite's foreign_keys pragma is OFF by default in this DB — we don't enforce
-- it at the engine level (a delete cascade isn't desired anyway), but the ref
-- documents the relationship for tooling.
--
-- The partial index keeps lookups for "replies to message N" fast without
-- bloating storage for the common case (in_reply_to IS NULL).

INSERT INTO schema_version VALUES (3);

ALTER TABLE messages ADD COLUMN in_reply_to INTEGER REFERENCES messages(id);

CREATE INDEX idx_messages_in_reply_to ON messages(in_reply_to)
    WHERE in_reply_to IS NOT NULL;
