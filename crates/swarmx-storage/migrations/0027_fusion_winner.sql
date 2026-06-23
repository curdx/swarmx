-- 0027_fusion_winner: the decide/terminal stage of a fusion competition. After
-- the judge (0026) has cross-read every contestant's diff, the user (or the
-- judge direction) picks ONE winning contestant. We record which contestant won
-- so the UI can show the verdict and a re-opened batch stays auditable.
--
-- winner_thread_id: thread.id of the winning contestant direction. NULL until a
--   verdict is reached. MUST be one of contestant_thread_ids_json (enforced in
--   the handler, not by SQL — SQLite can't constrain against a JSON array).
--   When set, status is flipped to 'done' in the same UPDATE.
--
-- The winner's branch is what gets merged back to base; losers are left intact
-- (their worktrees/branches remain for inspection until the batch is deleted).

INSERT INTO schema_version VALUES (27);

ALTER TABLE fusion_batches ADD COLUMN winner_thread_id TEXT;
