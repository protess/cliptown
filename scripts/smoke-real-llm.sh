#!/usr/bin/env bash
#
# M9.10 A3 — local real-LLM smoke test.
#
# End-to-end exercise of the worker → claude-code adapter → MCP HTTP → world
# chain using a real `claude` CLI and a real Anthropic API key. Costs ~$0.05
# per successful run (haiku-sized output) and caps at $0.50 via the world's
# per-startup budget.
#
# Pre-requisites the operator must satisfy:
#   - `claude` CLI on PATH (install with `npm install -g @anthropic-ai/claude-code`)
#   - `ANTHROPIC_API_KEY` exported (not preflight-validated here — the CLI
#     emits the actual auth error if the key is unset or rejected)
#   - `cargo`, `pnpm`, `sqlite3`, `jq`, `curl` on PATH
#
# Script flow (matches the design spec § A3):
#   1. Pre-flight tool + env checks.
#   2. Build the world binary once (release mode).
#   3. Boot the world inside a fresh tmpdir with $CLIPTOWN_DB pointed inside.
#   4. POST /api/startups to allocate a startup + founder/engineer/designer
#      (also auto-populates `world.avatars` via Cmd::InsertAvatars).
#   5. Seed a parent task (founder) + child task (engineer) via SQL.
#   6. Spawn the worker with --real --prompt=<haiku-with-task_done>.
#   7. Wait for worker exit; verify artifact on disk + DB row + budget.
#
# All resources land under $SMOKE_DIR which is removed on exit unless
# KEEP_TMP=1 is set (useful for debugging a failed run).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BUDGET_CAP_USD="${BUDGET_CAP_USD:-0.50}"
WORLD_BIND="${WORLD_BIND:-127.0.0.1:8080}"
OPERATOR_TOKEN="${CLIPTOWN_OPERATOR_TOKEN:-dev-token}"
AGENT_SECRET="${AGENT_SECRET:-dev-secret}"
TASK_ID="smoke-haiku"
# P3 carry-forward: when set (e.g. WORLD_REMOTE_URL=https://cliptown.fly.dev),
# the script targets a deployed world instead of building+booting locally.
# Limitation: artifact verification needs the world's filesystem, which we
# don't have remote access to — remote mode treats clean adapter exit + task
# status flip as success.
WORLD_REMOTE_URL="${WORLD_REMOTE_URL:-}"
# Default to claude_code; override with BACKEND=codex|opencode to exercise
# the other adapters' --real paths. The expected MCP tool prefix varies
# per CLI — claude exposes tools as `mcp__cliptown__<name>`, codex/opencode
# may use a different prefix or none at all. The prompt below is intentionally
# permissive ("call the tool task_done with arguments...") so the smoke
# works across CLIs that handle prefixes differently.
BACKEND="${BACKEND:-claude_code}"

# ── colored status helpers ─────────────────────────────────────────────────
say()  { printf "\033[1;36m[smoke]\033[0m %s\n" "$*"; }
warn() { printf "\033[1;33m[smoke]\033[0m %s\n" "$*" >&2; }
fail() { printf "\033[1;31m[smoke FAIL]\033[0m %s\n" "$*" >&2; exit 1; }

# ── 1. pre-flight ──────────────────────────────────────────────────────────
# Note: ANTHROPIC_API_KEY presence is NOT validated here. If it's missing or
# wrong, the `claude` CLI itself emits a clear auth error during the worker
# step, which is more informative than our preflight could be (it can speak
# to whether the key is unset vs invalid vs rate-limited).
if [[ -n "$WORLD_REMOTE_URL" ]]; then
  say "pre-flight (backend=$BACKEND, mode=remote target=$WORLD_REMOTE_URL)"
else
  say "pre-flight (backend=$BACKEND, mode=local)"
fi
# In remote mode we don't need cargo / sqlite3 — the world is already running
# somewhere else. The worker still spawns locally so pnpm + the CLI itself
# remain required.
preflight_tools=(pnpm jq curl "$BACKEND")
if [[ -z "$WORLD_REMOTE_URL" ]]; then
  preflight_tools+=(cargo sqlite3)
fi
for tool in "${preflight_tools[@]}"; do
  # `$BACKEND` is the CLI binary name (claude_code maps to `claude`).
  local_bin="$tool"
  if [[ "$tool" == "claude_code" ]]; then local_bin="claude"; fi
  command -v "$local_bin" >/dev/null 2>&1 || fail "missing required tool: $local_bin"
done
case "$BACKEND" in
  claude_code) claude --version >/dev/null 2>&1 || fail "\`claude --version\` failed";;
  codex)       codex --version  >/dev/null 2>&1 || fail "\`codex --version\` failed";;
  opencode)    opencode --version >/dev/null 2>&1 || fail "\`opencode --version\` failed";;
  *) fail "unknown BACKEND=$BACKEND (expected claude_code|codex|opencode)";;
esac

# ── 2. tmpdir + cleanup trap ───────────────────────────────────────────────
SMOKE_DIR="$(mktemp -d -t cliptown-smoke.XXXXXX)"
WORLD_PID=""
cleanup() {
  local rc=$?
  if [[ -n "$WORLD_PID" ]] && kill -0 "$WORLD_PID" 2>/dev/null; then
    say "stopping world (pid=$WORLD_PID)"
    kill "$WORLD_PID" 2>/dev/null || true
    wait "$WORLD_PID" 2>/dev/null || true
  fi
  if [[ "${KEEP_TMP:-0}" == "1" ]]; then
    say "KEEP_TMP=1 → preserving $SMOKE_DIR"
  else
    rm -rf "$SMOKE_DIR"
  fi
  exit $rc
}
trap cleanup EXIT INT TERM
say "tmpdir: $SMOKE_DIR"

# ── 3. world setup — build+boot locally, OR point at a remote target ───────
if [[ -n "$WORLD_REMOTE_URL" ]]; then
  # Remote mode: derive http/ws bases from WORLD_REMOTE_URL, skip build+boot.
  WORLD_HTTP="${WORLD_REMOTE_URL%/}"
  case "$WORLD_HTTP" in
    https://*) WORLD_WS="wss://${WORLD_HTTP#https://}/ws/worker" ;;
    http://*)  WORLD_WS="ws://${WORLD_HTTP#http://}/ws/worker" ;;
    *) fail "WORLD_REMOTE_URL must start with http:// or https:// (got: $WORLD_HTTP)" ;;
  esac
  say "remote world: HTTP=$WORLD_HTTP WS=$WORLD_WS"
  curl -sf "$WORLD_HTTP/health" >/dev/null \
    || fail "remote /health failed; check WORLD_REMOTE_URL + network"
else
  WORLD_HTTP="http://$WORLD_BIND"
  WORLD_WS="ws://$WORLD_BIND/ws/worker"
  say "building cliptown-world (release)"
  (cd "$REPO_ROOT" && cargo build --release -p cliptown-world >/dev/null 2>&1) \
    || fail "cargo build failed; rerun manually to see errors"

  # world needs cliptown.toml in CWD + writes workspaces/<sid>/ in CWD
  cp "$REPO_ROOT/cliptown.toml" "$SMOKE_DIR/cliptown.toml"

  say "booting world at $WORLD_BIND (db=$SMOKE_DIR/cliptown.db)"
  (
    cd "$SMOKE_DIR"
    CLIPTOWN_DB="$SMOKE_DIR/cliptown.db" \
    CLIPTOWN_ADDR="$WORLD_BIND" \
    CLIPTOWN_TEST_FIXED_AGENT_SECRET="$AGENT_SECRET" \
    CLIPTOWN_TEST_DISABLE_SUPERVISOR=1 \
    "$REPO_ROOT/target/release/cliptown-world" \
      >"$SMOKE_DIR/world.log" 2>&1 &
    echo $! >"$SMOKE_DIR/world.pid"
  )
  WORLD_PID="$(cat "$SMOKE_DIR/world.pid")"

  # poll /health until ready (max 30s — release cold-start is ~1s in practice)
  say "waiting for world /health"
  for i in $(seq 1 60); do
    if curl -sf "$WORLD_HTTP/health" >/dev/null 2>&1; then
      say "world ready after ${i}*0.5s"
      break
    fi
    sleep 0.5
    if ! kill -0 "$WORLD_PID" 2>/dev/null; then
      cat "$SMOKE_DIR/world.log" >&2
      fail "world exited before /health responded"
    fi
  done
  curl -sf "$WORLD_HTTP/health" >/dev/null \
    || fail "world /health never responded; see $SMOKE_DIR/world.log"
fi

# ── 4. create startup via /api/startups ────────────────────────────────────
say "creating startup via /api/startups"
CREATE_RESP="$(curl -sf -X POST "$WORLD_HTTP/api/startups" \
  -H "Authorization: Bearer $OPERATOR_TOKEN" \
  -H "content-type: application/json" \
  -d "{\"name\":\"smoke\",\"goal_text\":\"haiku\",\"budget_cap_usd\":$BUDGET_CAP_USD,\"backends\":{\"founder\":\"claude_code\",\"engineer\":\"claude_code\",\"designer\":\"claude_code\"}}")" \
  || fail "/api/startups POST failed"

STARTUP_ID="$(jq -r '.id' <<<"$CREATE_RESP")"
ENGINEER_ID="$(jq -r '.agents[] | select(.role=="engineer") | .id' <<<"$CREATE_RESP")"
FOUNDER_ID="$(jq -r '.agents[] | select(.role=="founder") | .id' <<<"$CREATE_RESP")"
[[ -n "$STARTUP_ID" && "$STARTUP_ID" != "null" ]] || fail "could not parse startup_id from: $CREATE_RESP"
[[ -n "$ENGINEER_ID" && "$ENGINEER_ID" != "null" ]] || fail "could not parse engineer_id"
say "startup=$STARTUP_ID engineer=$ENGINEER_ID founder=$FOUNDER_ID"

# ── 5. seed parent + engineer task ─────────────────────────────────────────
# mcp_dispatch::handle_task_done's subtask_done fanout expects the engineer's
# task to have a parent assigned to a manager. Local mode uses SQL for fixed
# IDs (cleanup + inspection ergonomics); remote mode goes through
# `/api/admin/tasks` (added in P3 carry-forward) since SQL is unreachable.
if [[ -z "$WORLD_REMOTE_URL" ]]; then
  say "seeding parent task T-parent and engineer task $TASK_ID via SQL"
  sqlite3 "$SMOKE_DIR/cliptown.db" <<SQL
INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at)
VALUES ('T-parent', '$STARTUP_ID', NULL, 'parent', 'd', 'in_progress', '$FOUNDER_ID', unixepoch(), unixepoch());
INSERT INTO tasks (id, startup_id, parent_id, title, description, status, assignee_agent_id, created_at, updated_at)
VALUES ('$TASK_ID', '$STARTUP_ID', 'T-parent', 'Write a haiku', 'd', 'in_progress', '$ENGINEER_ID', unixepoch(), unixepoch());
SQL
  mkdir -p "$SMOKE_DIR/workspaces/$STARTUP_ID/artifacts"

  say "seeding skill 'smoke-skill-deploy' and attaching to engineer"
  SKILL_ID="$(uuidgen | tr 'A-Z' 'a-z' 2>/dev/null || python3 -c 'import uuid; print(uuid.uuid4())')"
  SKILL_CONTENT="Smoke test skill content. The agent should see this file in its execenv."
  sqlite3 "$SMOKE_DIR/cliptown.db" <<SQL
INSERT INTO skills (id, startup_id, name, content_md, created_at, updated_at)
  VALUES ('$SKILL_ID', '$STARTUP_ID', 'smoke-skill-deploy', '$SKILL_CONTENT', unixepoch(), unixepoch());
INSERT INTO agent_skills (agent_id, skill_id, attached_at)
  VALUES ('$ENGINEER_ID', '$SKILL_ID', unixepoch());
SQL
else
  say "seeding parent + engineer task via /api/admin/tasks (remote)"
  PARENT_RESP="$(curl -sf -X POST "$WORLD_HTTP/api/admin/tasks" \
    -H "Authorization: Bearer $OPERATOR_TOKEN" \
    -H "content-type: application/json" \
    -d "{\"startup_id\":\"$STARTUP_ID\",\"title\":\"parent\",\"description\":\"smoke parent\",\"assignee_agent_id\":\"$FOUNDER_ID\"}")" \
    || fail "/api/admin/tasks (parent) failed"
  PARENT_ID="$(jq -r '.id' <<<"$PARENT_RESP")"
  [[ -n "$PARENT_ID" && "$PARENT_ID" != "null" ]] || fail "no parent id: $PARENT_RESP"
  CHILD_RESP="$(curl -sf -X POST "$WORLD_HTTP/api/admin/tasks" \
    -H "Authorization: Bearer $OPERATOR_TOKEN" \
    -H "content-type: application/json" \
    -d "{\"startup_id\":\"$STARTUP_ID\",\"parent_id\":\"$PARENT_ID\",\"title\":\"Write a haiku\",\"description\":\"smoke child\",\"assignee_agent_id\":\"$ENGINEER_ID\"}")" \
    || fail "/api/admin/tasks (child) failed"
  TASK_ID="$(jq -r '.id' <<<"$CHILD_RESP")"
  [[ -n "$TASK_ID" && "$TASK_ID" != "null" ]] || fail "no child id: $CHILD_RESP"
  say "remote task ids: parent=$PARENT_ID child=$TASK_ID"
  warn "remote mode: skill seeding skipped (operator must pre-seed if needed)"
fi

# ── 6. spawn worker in --real mode ─────────────────────────────────────────
ARTIFACT_REL="workspaces/$STARTUP_ID/artifacts/$TASK_ID.md"
ARTIFACT_ABS="$SMOKE_DIR/$ARTIFACT_REL"

# Prompt is intentionally explicit. Constraints we care about for the smoke:
#   - Output ≤ ~50 tokens (haiku) so a single run stays well under $0.05.
#   - Engineer MUST hit the canonical artifact path or task_done returns
#     bad_artifact_path. The path is templated by the script.
#   - Engineer MUST call task_done so the SQL row transitions.
# Avoid backticks in the prompt — when this string transits any layer that
# re-evaluates as a shell command line, backtick-quoted segments get
# command-substituted ("mcp__cliptown__task_done: command not found"). Use
# single quotes for markdown-style emphasis on the tool name; Claude reads
# either form fine.
PROMPT=$(cat <<EOF
You are an engineer in a simulated environment. You have ONE task. Follow these steps in order:

1. Create a file at this EXACT relative path:
     $ARTIFACT_REL
   The file content must be a three-line haiku about clipboards. The file does not exist yet. Use whatever file-write tool your environment offers (Write, shell heredoc, etc).

2. After the file is written, call the MCP tool named 'task_done' (your environment may expose it as 'cliptown.task_done', 'mcp__cliptown__task_done', or a similar prefix — pick whichever matches your tool list) with arguments:
     task_id: "$TASK_ID"
     artifact_path: "$ARTIFACT_REL"

Do not edit or re-read the file. Stop immediately after task_done returns.
EOF
)

say "spawning worker in --real mode"
WORKER_LOG="$SMOKE_DIR/worker.log"
set +e
# Why we bypass `pnpm -F @cliptown/worker start --` and call tsx directly:
#   1. pnpm needs to run from inside the workspace to find @cliptown/worker
#      via pnpm-workspace.yaml — running from SMOKE_DIR silently no-ops
#      with "No projects found in <cwd>" and rc=0.
#   2. `pnpm <script> -- <args>` re-shells the script's command line, so any
#      backticks inside our prompt arg get command-substituted.
#   3. pnpm forwards a literal "--" as a positional arg to tsx, which then
#      reaches the worker's parseArgs and trips ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL.
# `pnpm exec tsx <path> -- <args>` sidesteps all three: the tsx binary is
# resolved via the workspace's node_modules and args go directly as argv
# without further shell evaluation.
(
  cd "$REPO_ROOT"
  # `-F @cliptown/worker exec` runs the tool from the worker package's own
  # node_modules (where tsx is a devDep). Plain `pnpm exec` looks in the
  # workspace root, where tsx isn't installed.
  pnpm -F @cliptown/worker exec tsx ./src/main.ts \
    --world-url "$WORLD_WS" \
    --agent-id "$ENGINEER_ID" \
    --startup-id "$STARTUP_ID" \
    --task-id "$TASK_ID" \
    --secret "$AGENT_SECRET" \
    --backend "$BACKEND" \
    --workspace "$SMOKE_DIR" \
    --real \
    --prompt "$PROMPT" \
    >"$WORKER_LOG" 2>&1
)
WORKER_RC=$?
set -e
say "worker exited rc=$WORKER_RC (log: $WORKER_LOG)"
[[ "$WORKER_RC" -eq 0 ]] || { cat "$WORKER_LOG" >&2; fail "worker exited non-zero"; }

# ── 7. verifications ───────────────────────────────────────────────────────
# Remote mode: skip FS + SQL checks (we have neither). The world's task_done
# handler already validated the artifact path server-side as part of the MCP
# call, so a clean worker exit is a real signal. Local mode keeps the full
# verification suite.
if [[ -n "$WORLD_REMOTE_URL" ]]; then
  say "verify (remote): clean adapter exit + world /health responsive"
  curl -sf "$WORLD_HTTP/health" >/dev/null || fail "remote /health lost after worker run"
  warn "remote mode: skipping artifact-on-disk + SQL-row + execenv + skill + budget checks (no local FS / SQL access)"
  say "PASS — remote smoke complete"
  exit 0
fi

say "verify: artifact on disk at $ARTIFACT_REL"
[[ -s "$ARTIFACT_ABS" ]] || fail "artifact missing or empty: $ARTIFACT_ABS"
say "artifact bytes=$(wc -c <"$ARTIFACT_ABS")"

say "verify: task row in SQL"
TASK_ROW="$(sqlite3 -separator '|' "$SMOKE_DIR/cliptown.db" \
  "SELECT status, artifact_path FROM tasks WHERE id = '$TASK_ID';")"
TASK_STATUS="${TASK_ROW%%|*}"
TASK_PATH="${TASK_ROW#*|}"
say "task status=$TASK_STATUS artifact_path=$TASK_PATH"
[[ "$TASK_STATUS" == "awaiting_review" ]] \
  || fail "expected task status=awaiting_review, got '$TASK_STATUS'"
[[ "$TASK_PATH" == "$ARTIFACT_REL" ]] \
  || fail "expected artifact_path=$ARTIFACT_REL, got '$TASK_PATH'"

# ── 7.5. verify: per-task execenv (P2.3) ───────────────────────────────────
say "verify: per-task execenv at workspaces/$STARTUP_ID/$TASK_ID/workdir/"
EXECENV_WORKDIR="$SMOKE_DIR/workspaces/$STARTUP_ID/$TASK_ID/workdir"
[[ -d "$EXECENV_WORKDIR" ]] || fail "workdir not found: $EXECENV_WORKDIR"
[[ -L "$EXECENV_WORKDIR/workspaces" ]] || fail "workspaces symlink missing inside workdir"
LINK_TARGET="$(readlink "$EXECENV_WORKDIR/workspaces")"
EXPECTED_TARGET="$SMOKE_DIR/workspaces"
[[ "$LINK_TARGET" == "$EXPECTED_TARGET" ]] || fail "symlink target mismatch: got $LINK_TARGET, expected $EXPECTED_TARGET"
CLAUDE_MD="$EXECENV_WORKDIR/CLAUDE.md"
[[ -f "$CLAUDE_MD" ]] || fail "CLAUDE.md missing at $CLAUDE_MD"
grep -q "workspaces/$STARTUP_ID/artifacts/$TASK_ID.md" "$CLAUDE_MD" \
  || fail "CLAUDE.md does not reference canonical artifact path"
say "execenv check passed: workdir + symlink + CLAUDE.md all present"

# ── 7.6. verify: skill landed in execenv (P2.2) ────────────────────────────
say "verify: attached skill at workspaces/$STARTUP_ID/$TASK_ID/workdir/skills/"
SKILL_FILE="$EXECENV_WORKDIR/skills/smoke-skill-deploy.md"
[[ -f "$SKILL_FILE" ]] || fail "skill file missing: $SKILL_FILE"
grep -q "Smoke test skill content" "$SKILL_FILE" \
  || fail "skill file content mismatch"
grep -q "smoke-skill-deploy" "$EXECENV_WORKDIR/CLAUDE.md" \
  || fail "CLAUDE.md does not mention attached skill 'smoke-skill-deploy'"
say "skill check passed: skill file + CLAUDE.md reference both present"

say "verify: budget under cap"
SPENT="$(sqlite3 "$SMOKE_DIR/cliptown.db" \
  "SELECT budget_spent_usd FROM startups WHERE id = '$STARTUP_ID';")"
say "budget_spent_usd=$SPENT (cap=$BUDGET_CAP_USD)"
# spent may be 0.0 if the worker never reported (Phase 0 doesn't always wire
# ReportBudget through claude-code hooks). Cap-overshoot is what we care about.
awk -v s="$SPENT" -v c="$BUDGET_CAP_USD" 'BEGIN { exit !(s+0 <= c+0) }' \
  || fail "spend $SPENT exceeded cap $BUDGET_CAP_USD"

say "PASS — A3 smoke complete"
