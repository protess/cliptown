-- P6 Theme A: self-review gates.
--
-- `self_reviewed_at` stamps the most recent successful self_review run
-- on a task. NULL means the engineer has never run self_review for this
-- task (or the gate failed on the last attempt). The scheduler's
-- auto-recovery pass (P6.C) reads this alongside `review_round` to
-- decide whether a failure is "engineer hasn't even self-checked yet"
-- vs "engineer ran self_review and it passed, but manager still bounced
-- the work."
--
-- Cleared (set back to NULL) whenever:
--   - task transitions away from `awaiting_review` (e.g. manager
--     requests changes), since the next task_done needs a fresh check.

ALTER TABLE tasks ADD COLUMN self_reviewed_at INTEGER;
