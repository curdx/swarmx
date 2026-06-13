-- 0024_cron_tz_offset: interpret a job's `cron_expr` in a fixed UTC offset.
--
-- The scheduler used to evaluate every expression in UTC, which forced users to
-- mentally convert their local wall-clock to UTC before scheduling (the #1
-- usability complaint: "0 9 * * *" meant 09:00 UTC, not 09:00 here). We now
-- store the offset the expression is written in — minutes east of UTC, e.g.
-- +480 for UTC+8 — and decompose `now + offset` into calendar fields, so the
-- fields (incl. day-of-week / day-of-month, which roll over correctly because
-- we shift the whole instant) read as local time.
--
-- A fixed offset rather than an IANA zone keeps the tz database out of the
-- build (the existing design choice); the only thing it does not model is DST.
-- DEFAULT 0 preserves the exact prior behaviour for every existing row.
ALTER TABLE cron_jobs ADD COLUMN tz_offset_minutes INTEGER NOT NULL DEFAULT 0;

INSERT INTO schema_version VALUES (24);
