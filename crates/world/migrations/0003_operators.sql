-- P3 Theme B: operator RBAC. The single CLIPTOWN_OPERATOR_TOKEN model
-- (one shared dev-token) doesn't scale past dev. operators table maps
-- bearer tokens to a (name, role) tuple so we can gate ConsoleInbound
-- variants by role and surface operator identity in audit trails.
--
-- Roles (ascending privilege):
--   viewer  — read snapshots, possess/unpossess. No mutations.
--   manager — viewer + task lifecycle ops (accept/reject/force-* +
--             directives + skill attach/detach).
--   admin   — manager + future operator-management commands (out of
--             scope for this PR's wire surface).
--
-- The migration seeds a default admin with token `dev-token` so existing
-- workflows keep working. Override at deploy time by writing new rows
-- and rotating tokens.

CREATE TABLE operators (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  token TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL CHECK (role IN ('viewer','manager','admin')),
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_operators_token ON operators(token);

-- Seed the default admin so the env-var operator-token contract still
-- works out of the box. Existing CLIPTOWN_OPERATOR_TOKEN env var
-- continues to validate via legacy fallback for now (auth.rs handles
-- both paths until operators promote to the table-only model).
INSERT INTO operators (id, name, token, role, created_at)
  VALUES ('op_default', 'default-admin', 'dev-token', 'admin', unixepoch());
