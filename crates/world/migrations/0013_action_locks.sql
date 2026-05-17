-- P5 Theme C: soft-locks on destructive operator actions.
--
-- When two operators share a cliptown, the worst case is both clicking
-- "Force-Accept T1" at the same time, or one revoking the other's token
-- mid-action. A 30s soft-lock on the (action, target) tuple makes the
-- second click bounce with a friendly "locked by Alice — 25s" error
-- instead of clobbering silently.
--
-- `lock_key` is a free-form string with a stable namespace prefix —
-- examples: `task:T1:force_accept`, `task:T1:force_fail`,
-- `operator:op_xyz:revoke`, `startup:s1:delete`. UNIQUE so the SQL
-- INSERT itself is the test-and-set; no advisory locking needed.
--
-- TTL: `expires_at` is unix seconds. A periodic GC tick drops rows
-- past expiry and broadcasts unlocks. Successful action handlers
-- DELETE their own row explicitly so peers see the unlock immediately
-- rather than waiting for the GC.

CREATE TABLE action_locks (
  id TEXT PRIMARY KEY,
  lock_key TEXT NOT NULL UNIQUE,
  operator_id TEXT NOT NULL REFERENCES operators(id) ON DELETE CASCADE,
  acquired_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);

CREATE INDEX action_locks_expires_at_idx ON action_locks(expires_at);
