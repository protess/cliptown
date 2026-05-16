-- P4 Theme E1: peer review beyond manager review.
--
-- Today's `task_request_changes` is structurally manager-only — the caller
-- must be (a) the parent task's assignee or (b) the recorded
-- `agents.manager_id` of the assignee. Real teams want a designer to
-- review an engineer's artifact, an engineer to review a founder's spec.
-- This migration adds an opt-in `is_peer_reviewer` flag; the MCP handler
-- treats peers as a separate path that doesn't replace manager review.
--
-- Scope:
--   - flag is on `agents` (not a separate table) — peer-review eligibility
--     is a per-agent attribute, not per-task.
--   - peers can request changes on any task in their own startup, except
--     tasks they themselves are the assignee of (self-review is nonsense).
--   - manager-review path is untouched: a manager still reviews regardless
--     of whether they're flagged as peer reviewer.
--   - audit_trail entries carry an `actor: "peer"` discriminator so the
--     org graph reads cleanly post-hoc.

ALTER TABLE agents ADD COLUMN is_peer_reviewer INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_agents_peer_reviewer ON agents(is_peer_reviewer)
  WHERE is_peer_reviewer = 1;
