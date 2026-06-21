-- 0001_init: agents / messages / blackboard_ops + FTS5 indexes.
--
-- Timestamps are stored as INTEGER unix-ms (i64). We deliberately avoid the
-- `time` crate so this schema stays decoupled from any Rust date library —
-- callers convert to/from chrono / time at the edge.
--
-- FTS5 uses content='<table>' external-content mode so the body lives once
-- in the source table and the FTS index only stores token positions. The
-- AFTER UPDATE triggers do a delete+insert pair (the FTS5 docs explicitly
-- recommend this — there is no "update" verb).
--
-- Wrapped in a single transaction by schema.rs::run_migrations.

CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
INSERT INTO schema_version VALUES (1);

CREATE TABLE agents (
    id              TEXT PRIMARY KEY,
    cli             TEXT NOT NULL,
    role            TEXT NOT NULL,
    workspace       TEXT NOT NULL,
    spawned_at      INTEGER NOT NULL,
    killed_at       INTEGER,
    shim_ready_at   INTEGER,
    shim_exit_at    INTEGER,
    shim_exit_code  INTEGER
);
CREATE INDEX idx_agents_alive ON agents(killed_at) WHERE killed_at IS NULL;

CREATE TABLE messages (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    from_agent    TEXT NOT NULL,
    to_agent      TEXT NOT NULL,
    kind          TEXT NOT NULL,
    body          TEXT NOT NULL,
    sent_at       INTEGER NOT NULL,
    delivered_at  INTEGER,
    read_at       INTEGER
);
CREATE INDEX idx_messages_to_pending ON messages(to_agent, delivered_at)
    WHERE delivered_at IS NULL;
CREATE INDEX idx_messages_to_id ON messages(to_agent, id);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    body,
    content='messages',
    content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', old.id, old.body);
END;
CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', old.id, old.body);
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;

CREATE TABLE blackboard_ops (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id  TEXT,
    op        TEXT NOT NULL,
    path      TEXT NOT NULL,
    content   TEXT NOT NULL,
    sha256    TEXT NOT NULL,
    at        INTEGER NOT NULL
);
CREATE INDEX idx_blackboard_path_at ON blackboard_ops(path, at);

CREATE VIRTUAL TABLE blackboard_fts USING fts5(
    content,
    content='blackboard_ops',
    content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER blackboard_ai AFTER INSERT ON blackboard_ops BEGIN
    INSERT INTO blackboard_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER blackboard_ad AFTER DELETE ON blackboard_ops BEGIN
    INSERT INTO blackboard_fts(blackboard_fts, rowid, content) VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER blackboard_au AFTER UPDATE ON blackboard_ops BEGIN
    INSERT INTO blackboard_fts(blackboard_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO blackboard_fts(rowid, content) VALUES (new.id, new.content);
END;
