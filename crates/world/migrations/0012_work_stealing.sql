-- P4 Theme E3: async work-stealing among idle peers.
--
-- When one engineer has a queue of tasks and another sits idle, cliptown
-- should rebalance automatically. Two surfaces:
--
--   1. Manual: agent calls `task_steal {task_id}` to claim a queued task
--      assigned to another same-role peer in their startup. The MCP path
--      validates (a) caller is idle, (b) caller is not the current
--      assignee, (c) same startup + same role, (d) task is queued.
--
--   2. Auto: per-startup opt-in. When `auto_steal_enabled = 1` and a
--      queued task hasn't been touched for `auto_steal_after_secs`, the
--      scheduler picks an idle same-role peer and reassigns.
--
-- Defaults: auto-steal OFF (auto_steal_enabled = 0). When operators flip
-- it on, the default threshold is 60 seconds — long enough that a normal
-- in-flight tick (1Hz scheduler) doesn't trip it, short enough that a
-- truly stuck task gets help within a minute.

ALTER TABLE startups ADD COLUMN auto_steal_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE startups ADD COLUMN auto_steal_after_secs INTEGER NOT NULL DEFAULT 60;
