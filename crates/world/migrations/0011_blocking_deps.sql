-- P4 Theme E2: time-bounded blocking dependencies.
--
-- Today's task graph captures parent_id (subtask hierarchy) but has no
-- "this task can't start until task X finishes" relation, and no notion
-- of a deadline. Real coordination needs both:
--   - `blocked_on` — pointer to another task that must reach a terminal
--     state before the scheduler dispatches this one. The scheduler
--     emits a `task_unblocked` system_event on the tick that crosses
--     this gate so the operator console can refresh.
--   - `deadline_at` — unix-seconds wall-clock deadline. When `now() >
--     deadline_at` and the task isn't yet terminal, the scheduler emits
--     a single `task_overdue` system_event. Dedup via
--     `deadline_notified_at` so we don't spam every tick.
--
-- Both columns are nullable; tasks without either field behave exactly
-- as before.

ALTER TABLE tasks ADD COLUMN blocked_on TEXT REFERENCES tasks(id) ON DELETE SET NULL;
ALTER TABLE tasks ADD COLUMN deadline_at INTEGER;
ALTER TABLE tasks ADD COLUMN deadline_notified_at INTEGER;

CREATE INDEX idx_tasks_blocked_on ON tasks(blocked_on) WHERE blocked_on IS NOT NULL;
CREATE INDEX idx_tasks_deadline ON tasks(deadline_at) WHERE deadline_at IS NOT NULL;
