-- P3 Theme C: per-task model routing.
--
-- Tasks were routed implicitly to whatever backend/model was provisioned on
-- the agent at startup. Real workloads need to override this per task — a
-- cheap haiku for trivial subtasks, a deep opus for the big reasoning steps —
-- without re-provisioning the agent.
--
-- Two nullable columns hold the override. NULL means "fall back to the
-- agent's default backend/model" (existing behavior). The scheduler reads
-- both and forwards them on `task_assigned`; the worker side honors the
-- preference when spawning the adapter session.
--
-- Validation lives at the MCP boundary (`task_set_preference` handler), not
-- in SQL — backend strings come from a frontend-controlled catalog that
-- evolves between releases, so a CHECK constraint would calcify and break
-- new backends on every release.

ALTER TABLE tasks ADD COLUMN preferred_backend TEXT NULL;
ALTER TABLE tasks ADD COLUMN preferred_model TEXT NULL;
