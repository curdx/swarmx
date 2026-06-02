-- 0011_worker_role_typed_handoff: P0 (F1 角色/任务感知配给).
--
-- workers 行加 3 列,支撑「角色注册表 + typed handoff」:
--   role_slug      校验过的角色注册表 slug(取代随手编的 role_label 语义;
--                  role_label 列保留作 UI 显示,由 slug 派生)。
--   produces_json  本 worker 产出的 typed output-kinds(JSON string array)。
--   consumes_json  本 worker 的 typed 上游依赖(JSON [{from_role,kind}]);
--                  解析成的 minted blackboard key 仍落在 depends_on_json。
--
-- 全部 nullable + 无默认 → 老行(NULL)由读取侧映射成空/缺省,向后兼容。
-- SQLite 一条 ALTER 只能加一列,故拆三条。

INSERT INTO schema_version VALUES (11);

ALTER TABLE workers ADD COLUMN role_slug TEXT;
ALTER TABLE workers ADD COLUMN produces_json TEXT;
ALTER TABLE workers ADD COLUMN consumes_json TEXT;
