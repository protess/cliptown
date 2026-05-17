-- P6 Theme C: per-startup auto-recovery flag.
--
-- When `auto_recovery_enabled = 1`, the scheduler's post-dispatch
-- recovery pass tries to rescue a task that the manager keeps
-- bouncing before escalating to a human operator. The recovery
-- strategy v1 is peer reassignment: pick an idle same-role peer
-- and reassign the task (resetting status to `queued`) so the new
-- assignee picks up the work with the existing manager-feedback
-- directive thread.
--
-- Threshold: triggers when a task's `review_round >=
-- auto_recovery_max_attempts` AND status == `changes_requested`.
-- Default 2 attempts ≈ "after the second failed review, hand to a
-- peer; if the peer also fails, escalate."
--
-- Defaults match auto-steal (P4 Theme E3): OFF by default, opt-in
-- per startup via the admin-only StartupSetAutoRecovery ConsoleInbound.

ALTER TABLE startups ADD COLUMN auto_recovery_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE startups ADD COLUMN auto_recovery_max_attempts INTEGER NOT NULL DEFAULT 2;
