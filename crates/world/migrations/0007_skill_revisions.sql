-- P3 roadmap carry-forward: skill revision history.
--
-- Today's `skills` table holds only the live row — every `upsert` overwrites
-- `content_md` in place, losing prior versions. For audit + rollback we need
-- an append-only revision log keyed by (skill_id, rev_seq), where rev_seq is
-- a monotonic per-skill counter.
--
-- The cliptown audit trail already lives on `tasks.audit_trail` (JSON
-- string) and as `system_events`. This table is the dedicated content
-- replay surface — content_md can be large, embedding it in either of those
-- would balloon them.
--
-- Author identity is one of agent OR operator (mutually exclusive in
-- practice: agent-side `skill_upsert` MCP tool writes agent_id; operator-
-- side `skill_upsert_operator` writes operator_id). Both nullable to allow
-- future system-authored revisions (e.g. migration backfill).

CREATE TABLE skill_revisions (
  id TEXT PRIMARY KEY,
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  rev_seq INTEGER NOT NULL,
  content_md TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  created_by_agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL,
  created_by_operator_id TEXT REFERENCES operators(id) ON DELETE SET NULL,
  UNIQUE (skill_id, rev_seq)
);

CREATE INDEX idx_skill_revisions_skill_seq ON skill_revisions(skill_id, rev_seq DESC);
