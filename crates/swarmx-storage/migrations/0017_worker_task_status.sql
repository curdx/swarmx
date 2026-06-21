-- 0017_worker_task_status: human-set status override for the Kanban control
-- plane (upgrading the read-only ledger into a writable board).
--
-- Each worker IS a task — the orchestrator spawned it for a unit of work. Its
-- *effective* status is normally DERIVED from lifecycle: alive→running,
-- handoff_signal written→done, `<signal>.error` present→blocked, killed→done.
-- This column is the HUMAN OVERRIDE: the operator marks a task blocked /
-- archived / done from the board, and that wins over the derived value. NULL =
-- no override (use the derived status). Nullable + no default → existing/legacy
-- rows keep deriving exactly as before. Safe ADD COLUMN, no table rewrite.
ALTER TABLE workers ADD COLUMN task_status TEXT;

INSERT INTO schema_version VALUES (17);
