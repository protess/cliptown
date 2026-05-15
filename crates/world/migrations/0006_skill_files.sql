-- P3 roadmap carry-forward: skills file attachments.
--
-- Today's `skills` table stores a single `content_md` blob — the skill IS its
-- markdown. Real skill bundles often need supporting files (templates,
-- examples, JSON configs). This migration adds a sibling table for arbitrary
-- text files attached to a skill; the worker materializes them into
-- `<workdir>/skills/<skill-name>/<filename>` alongside the existing
-- `<workdir>/skills/<skill-name>.md` (P2.3 execenv layout).
--
-- Constraints:
--   - text-only payload (BLOB columns + binary files are deliberately out of
--     scope; cliptown's adapter spawn paths are text/markdown-centric).
--   - per-skill name uniqueness keyed on the basename — operators can
--     replace a file by re-uploading with the same name (upsert by name).
--   - cascading delete from `skills` so detaching a skill also reaps its
--     attached files automatically. No orphan rows.

CREATE TABLE skill_files (
  id TEXT PRIMARY KEY,
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  content TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (skill_id, name)
);

CREATE INDEX idx_skill_files_skill ON skill_files(skill_id);
