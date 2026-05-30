-- 0008_blackboard_path_id: index to back the "latest-per-path" query and prune.
--
-- list_blackboard_ops(None) and the F5 retention prune both compute
-- MAX(id) GROUP BY path / a correlated MAX(id) per path. The existing
-- idx_blackboard_path_at(path, at) doesn't cover id, so those queries fall
-- back to a scan. (path, id) lets SQLite get MAX(id) per path straight off
-- the index — cheap discovery, cheap prune.

INSERT INTO schema_version VALUES (8);

CREATE INDEX idx_blackboard_path_id ON blackboard_ops(path, id);
