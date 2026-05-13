CREATE TABLE skills (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  content_md TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (startup_id, name)
);

CREATE INDEX idx_skills_startup ON skills(startup_id);

CREATE TABLE agent_skills (
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  attached_at INTEGER NOT NULL,
  PRIMARY KEY (agent_id, skill_id)
);

CREATE INDEX idx_agent_skills_agent ON agent_skills(agent_id);
