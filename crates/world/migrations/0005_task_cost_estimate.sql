-- P3 Theme C follow-up: cost variance telemetry.
--
-- `cost_estimate_usd` is the manager's (or admin's) up-front guess of what
-- this task will cost. NULL means "no estimate set" — variance comparison is
-- skipped for that task. When a worker reports actual spend via
-- `report_budget`, the world compares actual vs estimate and emits a
-- `task_cost_variance` system_event when the delta crosses ±50%.
--
-- Validation: handlers reject negative / non-finite estimates at the
-- boundary; no CHECK constraint to keep schema migrations free of cross-
-- version pain.

ALTER TABLE tasks ADD COLUMN cost_estimate_usd REAL NULL;
