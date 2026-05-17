#!/usr/bin/env bash
# P5 Theme F: restore the world DB from a hot snapshot taken by
# crates/world/src/backup.rs.
#
# Procedure (the world must be STOPPED before running this):
#   1. Snapshot the current live DB to a sidecar `.pre-restore` file
#      so the operation is undoable.
#   2. Copy the chosen snapshot over the live path.
#   3. Delete the WAL + SHM files — they encode unflushed pages
#      relative to the OLD DB and would corrupt the new one on next
#      open.
#
# Usage:
#   scripts/restore-from-snapshot.sh <snapshot-path> [--live-db <path>]
#
# Defaults --live-db to ./data/cliptown.db (the dev location) when
# unset. For docker compose deploys, pass --live-db
# /var/lib/docker/volumes/cliptown_cliptown-data/_data/cliptown.db
# or copy the snapshot into the container volume and restore inside.

set -euo pipefail

usage() {
    cat <<EOF
restore-from-snapshot.sh — swap the live cliptown DB for a snapshot

Usage:
  scripts/restore-from-snapshot.sh <snapshot-path> [--live-db <path>]

Options:
  --live-db PATH   Path to the cliptown SQLite DB to overwrite.
                   Default: ./data/cliptown.db

Safety:
  - The world server MUST be stopped first. The script does not
    enforce this; opening the live DB during the swap would
    corrupt it.
  - A backup of the current live DB is written to
    <live-db>.pre-restore before the swap. Move it aside if you
    want to keep it.
  - WAL + SHM files alongside the live DB are deleted post-swap.
EOF
}

if [ $# -lt 1 ] || [ "$1" = "-h" ] || [ "$1" = "--help" ]; then
    usage
    exit 1
fi

SNAPSHOT="$1"
shift
LIVE_DB="./data/cliptown.db"
while [ $# -gt 0 ]; do
    case "$1" in
        --live-db) LIVE_DB="$2"; shift 2 ;;
        *) echo "unknown arg: $1" >&2; usage; exit 2 ;;
    esac
done

if [ ! -f "$SNAPSHOT" ]; then
    echo "snapshot not found: $SNAPSHOT" >&2
    exit 1
fi

# Sanity check on the snapshot — `sqlite3 <file> ".tables"` exits
# non-zero on a corrupt or non-SQLite file.
if command -v sqlite3 >/dev/null 2>&1; then
    if ! sqlite3 "$SNAPSHOT" ".tables" >/dev/null 2>&1; then
        echo "snapshot does not appear to be a valid SQLite file: $SNAPSHOT" >&2
        exit 1
    fi
fi

LIVE_DIR=$(dirname "$LIVE_DB")
mkdir -p "$LIVE_DIR"

if [ -f "$LIVE_DB" ]; then
    BACKUP="${LIVE_DB}.pre-restore"
    echo "→ backing up current live DB to: $BACKUP"
    cp -p "$LIVE_DB" "$BACKUP"
fi

echo "→ copying snapshot over live DB: $SNAPSHOT → $LIVE_DB"
cp -p "$SNAPSHOT" "$LIVE_DB"

# WAL/SHM cleanup. These files encode pages relative to the OLD DB;
# leaving them would corrupt the freshly-restored DB on next open.
for ext in -wal -shm; do
    if [ -f "${LIVE_DB}${ext}" ]; then
        echo "→ removing stale ${LIVE_DB}${ext}"
        rm -f "${LIVE_DB}${ext}"
    fi
done

echo "✓ restore complete. Start the world process to use the snapshot state."
