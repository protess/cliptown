#!/bin/sh
# Test fixture for the supervisor's per-task spawn path (P3 Theme C / Option B).
# Writes the literal argv it received to $CLIPTOWN_TEST_ARGS_FILE so the test
# can assert that --real / --task-id / --prompt / --preferred-* land on the
# command line.
#
# POSIX sh — the supervisor invokes this via `/bin/sh <path>` and on Linux
# /bin/sh is typically dash, which doesn't support bash's [[ ]] form.
if [ -n "${CLIPTOWN_TEST_ARGS_FILE:-}" ]; then
  : > "$CLIPTOWN_TEST_ARGS_FILE"
  for arg in "$@"; do
    printf '%s\n' "$arg" >> "$CLIPTOWN_TEST_ARGS_FILE"
  done
fi
exit 0
