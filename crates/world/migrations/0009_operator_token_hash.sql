-- P3 carry-forward: hash operator bearer tokens at rest.
--
-- Migration 0003 stored `operators.token` as plaintext (UUID prefixed
-- `opt_`). Workable for a single-operator dev box but leaks every active
-- bearer if SQLite is ever exfiltrated (snapshot backup, ad-hoc dump for
-- debugging, etc.). Move to SHA-256 of the token — sufficient because:
--
--   1. server-minted tokens are 128-bit random UUIDs; rainbow-table /
--      brute-force attacks against 128-bit pre-images are infeasible, so
--      no need for a slow KDF like argon2.
--   2. lookup is by hex-encoded hash — `WHERE token_hash = ?` matches a
--      64-char hex string with no early-exit byte compare.
--
-- The 0003 schema declared `token TEXT NOT NULL UNIQUE`. To accept rows
-- with only `token_hash` set, we recreate the table with `token` nullable
-- (SQLite doesn't support DROP NOT NULL directly). `auth::validate_operator_token`
-- does the lazy migration: a successful plaintext match rewrites the
-- row to set `token_hash` and clear `token`.

CREATE TABLE operators_new (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  token TEXT,              -- nullable (legacy plaintext; deprecated)
  token_hash TEXT,         -- nullable for legacy rows; required for new
  role TEXT NOT NULL CHECK (role IN ('viewer','manager','admin')),
  created_at INTEGER NOT NULL
);

INSERT INTO operators_new (id, name, token, role, created_at)
  SELECT id, name, token, role, created_at FROM operators;

DROP TABLE operators;
ALTER TABLE operators_new RENAME TO operators;

CREATE INDEX idx_operators_token ON operators(token);
CREATE INDEX idx_operators_token_hash ON operators(token_hash);
