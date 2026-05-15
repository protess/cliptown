-- P3 roadmap carry-forward: globally-visible skills.
--
-- Today a skill is scoped to one `startup_id` and surfaces only when an
-- agent in that startup has the row in `agent_skills`. Many real skills
-- (style guides, MCP usage primers, debugging templates) should be visible
-- to every agent in every startup — what we're calling "global" here.
--
-- The minimum-invasive shape: an `is_global` flag on the existing row.
-- When set, the skill auto-surfaces in every agent's execenv regardless of
-- `agent_skills` attachment. The skill still has an owner `startup_id` for
-- accounting (who created it, who can edit it) — admin operators can mark
-- any skill global; agents cannot.

ALTER TABLE skills ADD COLUMN is_global INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_skills_global ON skills(is_global) WHERE is_global = 1;
