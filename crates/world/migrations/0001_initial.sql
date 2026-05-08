CREATE TABLE towns (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  map_json TEXT NOT NULL
);

CREATE TABLE rooms (
  id TEXT PRIMARY KEY,
  town_id TEXT NOT NULL REFERENCES towns(id),
  name TEXT NOT NULL,
  type TEXT NOT NULL,
  bounds TEXT NOT NULL,
  private_to_startup_id TEXT
);

CREATE TABLE room_doors (
  id TEXT PRIMARY KEY,
  town_id TEXT NOT NULL REFERENCES towns(id),
  room_a TEXT NOT NULL,
  room_b TEXT NOT NULL,
  tile_x INTEGER NOT NULL,
  tile_y INTEGER NOT NULL
);

CREATE TABLE startups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  goal_text TEXT NOT NULL,
  budget_cap_usd REAL NOT NULL,
  budget_spent_usd REAL NOT NULL DEFAULT 0,
  town_id TEXT NOT NULL REFERENCES towns(id),
  workspace_path TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active','paused','dissolved')),
  config_overrides TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE agents (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  name TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('founder','engineer','designer')),
  backend TEXT NOT NULL CHECK (backend IN ('claude_code','codex','opencode')),
  model_id TEXT NOT NULL,
  position_json TEXT NOT NULL,
  home_room_id TEXT NOT NULL,
  manager_id TEXT REFERENCES agents(id),
  status TEXT NOT NULL CHECK (status IN ('idle','working','walking','talking','offline'))
);

CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  parent_id TEXT REFERENCES tasks(id),
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  assignee_agent_id TEXT REFERENCES agents(id),
  required_room TEXT,
  status TEXT NOT NULL CHECK (status IN ('proposed','queued','in_progress','awaiting_review','changes_requested','done','failed','escalated')),
  review_round INTEGER NOT NULL DEFAULT 0,
  audit_trail TEXT NOT NULL DEFAULT '[]',
  epistemic_log TEXT NOT NULL DEFAULT '[]',
  artifact_path TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX tasks_by_assignee ON tasks(assignee_agent_id);
CREATE INDEX tasks_by_startup_status ON tasks(startup_id, status);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  room_id TEXT,
  author_id TEXT NOT NULL,
  body TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('chat','directive','system')),
  ts INTEGER NOT NULL
);
CREATE INDEX messages_by_startup_ts ON messages(startup_id, ts DESC);

CREATE TABLE budget_events (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  agent_id TEXT REFERENCES agents(id),
  task_id TEXT REFERENCES tasks(id),
  in_tokens INTEGER NOT NULL,
  out_tokens INTEGER NOT NULL,
  cost_usd REAL NOT NULL,
  model_id TEXT NOT NULL,
  ts INTEGER NOT NULL
);

CREATE TABLE fs_audit (
  id TEXT PRIMARY KEY,
  startup_id TEXT NOT NULL REFERENCES startups(id),
  agent_id TEXT REFERENCES agents(id),
  op TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes INTEGER NOT NULL DEFAULT 0,
  ok INTEGER NOT NULL,
  error TEXT,
  ts INTEGER NOT NULL
);

CREATE TABLE system_events (
  id TEXT PRIMARY KEY,
  startup_id TEXT,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  severity TEXT NOT NULL CHECK (severity IN ('info','warn','alert','critical')),
  ts INTEGER NOT NULL
);
CREATE INDEX system_events_recent ON system_events(ts DESC);
