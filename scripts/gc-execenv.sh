#!/usr/bin/env bash
#
# Phase 3 carry-forward #2 — per-task execenv GC.
#
# Removes `<workspaces_root>/<startup_id>/<task_id>/` directories whose
# corresponding task row is in a terminal state (done | failed | escalated)
# AND was last updated more than $MAX_AGE_DAYS ago. Active tasks (queued,
# in_progress, awaiting_review, changes_requested, proposed) are never
# touched regardless of age — they may still be doing work the operator
# wants to inspect.
#
# Artifacts directory (`workspaces/<sid>/artifacts/`) is preserved so the
# operator console + audit replays keep working. Only the per-task
# `workdir` siblings under `workspaces/<sid>/<tid>/` are reaped.
#
# Usage:
#   scripts/gc-execenv.sh [--dry-run] [--days <N>]
#       [--db <path>] [--workspaces <path>]
#
# Defaults:
#   --db          ./data/cliptown.db
#   --workspaces  ./workspaces
#   --days        7
#
# Requires `sqlite3` on PATH. Safe to run while the world server is up —
# the script opens the DB in read-only mode and does its own filesystem
# work outside the world process.

set -euo pipefail

DB="./data/cliptown.db"
WS_ROOT="./workspaces"
DAYS=7
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --days)        DAYS="$2"; shift 2 ;;
    --db)          DB="$2";   shift 2 ;;
    --workspaces)  WS_ROOT="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,30p' "$0"
      exit 0
      ;;
    *)
      echo "unknown flag: $1" >&2
      echo "  run with --help" >&2
      exit 2
      ;;
  esac
done

if [[ ! -f "$DB" ]]; then
  echo "db not found: $DB" >&2
  exit 1
fi
if [[ ! -d "$WS_ROOT" ]]; then
  echo "workspaces root not found: $WS_ROOT" >&2
  exit 1
fi
if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "sqlite3 not on PATH" >&2
  exit 1
fi

# Cutoff in unix seconds. sqlite3 stores updated_at as unix seconds (see
# crates/world/migrations/0001_initial.sql).
CUTOFF=$(( $(date +%s) - DAYS * 86400 ))

# Pull (startup_id, task_id) tuples for tasks that are GC-eligible:
# terminal state AND not touched recently.
mapfile -t ROWS < <(sqlite3 -readonly "$DB" \
  "SELECT startup_id || '/' || id FROM tasks
   WHERE status IN ('done','failed','escalated')
     AND updated_at < $CUTOFF
   ORDER BY updated_at ASC")

REAPED=0
KEPT=0
MISSING=0
for path in "${ROWS[@]}"; do
  target="$WS_ROOT/$path"
  if [[ ! -d "$target" ]]; then
    MISSING=$(( MISSING + 1 ))
    continue
  fi
  size_kb=$(du -sk "$target" 2>/dev/null | awk '{print $1}')
  if [[ "$DRY_RUN" -eq 1 ]]; then
    printf '[dry-run] would remove %s (%s KiB)\n' "$target" "${size_kb:-?}"
    KEPT=$(( KEPT + 1 ))
  else
    rm -rf -- "$target"
    printf 'removed %s (%s KiB)\n' "$target" "${size_kb:-?}"
    REAPED=$(( REAPED + 1 ))
  fi
done

echo "---"
echo "eligible:    ${#ROWS[@]}"
echo "removed:     $REAPED"
echo "would-keep:  $KEPT (dry-run)"
echo "missing:     $MISSING (already gone)"
echo "cutoff_days: $DAYS"
