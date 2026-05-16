#!/usr/bin/env bash
#
# P4 F1 — local-LLM smoke test via ollama.
#
# Wraps `smoke-real-llm.sh` with the env-var preset that points an adapter
# at a locally-running ollama instance instead of api.openai.com /
# api.anthropic.com. Validates the local-first deploy story
# (`docs/DEPLOY.md` § Local LLM): operators pulling cliptown to run
# against a self-hosted model should be able to bash this script and get a
# green "PASS" within ~30s on an apple-silicon laptop with `llama3.1:8b`
# already pulled.
#
# Requires:
#   - ollama on PATH AND already serving (`ollama serve` background).
#   - The model in $OLLAMA_MODEL (default llama3.1:8b) already pulled —
#     this script does NOT call `ollama pull` to avoid surprising the
#     operator with a multi-GB download.
#   - Either `codex` (default) or `opencode` CLI on PATH.
#   - cargo, pnpm, sqlite3, jq, curl (same as smoke-real-llm.sh).
#
# Usage:
#   scripts/smoke-ollama.sh
#   BACKEND=opencode OLLAMA_MODEL=qwen2.5-coder:7b scripts/smoke-ollama.sh
#
# Why not extend smoke-real-llm.sh with an OLLAMA_MODE flag? Both paths
# share 95% of their setup; the wrapper pattern keeps the existing smoke
# untouched so claude+anthropic regression coverage stays clean while
# local-LLM gets its own preset.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
BACKEND="${BACKEND:-codex}"
OLLAMA_HOST="${OLLAMA_HOST:-http://127.0.0.1:11434}"
OLLAMA_MODEL="${OLLAMA_MODEL:-llama3.1:8b}"

case "$BACKEND" in
  codex|opencode) ;;
  *)
    echo "smoke-ollama.sh: BACKEND must be codex or opencode (got: $BACKEND)" >&2
    echo "  claude-code can't talk to ollama directly without a translator proxy." >&2
    exit 2
    ;;
esac

# ── pre-flight ─────────────────────────────────────────────────────────────
say() { printf "\033[1;36m[smoke-ollama]\033[0m %s\n" "$*"; }
fail() { printf "\033[1;31m[smoke-ollama FAIL]\033[0m %s\n" "$*" >&2; exit 1; }

command -v ollama >/dev/null 2>&1 \
  || fail "ollama not on PATH. Install: https://ollama.com/download"
command -v "$BACKEND" >/dev/null 2>&1 \
  || fail "$BACKEND CLI not on PATH"

# Verify ollama is serving (don't try to start it — that's the operator's job).
if ! curl -sf "$OLLAMA_HOST/api/tags" >/dev/null 2>&1; then
  fail "ollama at $OLLAMA_HOST is not responding. Run \`ollama serve\` in another terminal."
fi

# Verify the requested model is pulled. Avoid the multi-GB surprise.
if ! curl -sf "$OLLAMA_HOST/api/tags" | jq -e \
  --arg m "$OLLAMA_MODEL" '.models | map(.name) | index($m)' >/dev/null
then
  fail "ollama model '$OLLAMA_MODEL' not pulled. Run: ollama pull $OLLAMA_MODEL"
fi

say "ollama ready: $OLLAMA_HOST serving $OLLAMA_MODEL"
say "backend: $BACKEND"

# ── env preset → run the existing local-mode smoke ─────────────────────────
# Both codex and opencode honor OpenAI-compatible env. ollama exposes a
# /v1 OpenAI-compat endpoint, so OPENAI_BASE_URL + OPENAI_API_KEY=ollama
# is the universal lever. The per-CLI MODEL env is the routing knob.
export OPENAI_BASE_URL="$OLLAMA_HOST/v1"
export OPENAI_API_KEY="${OPENAI_API_KEY:-ollama}"  # any non-empty string

case "$BACKEND" in
  codex)
    export CODEX_MODEL_ID="$OLLAMA_MODEL"
    say "env: OPENAI_BASE_URL=$OPENAI_BASE_URL CODEX_MODEL_ID=$CODEX_MODEL_ID"
    ;;
  opencode)
    # opencode's model spec is `provider/model`; ollama provider is honored
    # by splitProviderModel (see packages/adapters/opencode/test/model_spec.test.ts).
    export OPENCODE_MODEL="ollama/$OLLAMA_MODEL"
    say "env: OPENCODE_MODEL=$OPENCODE_MODEL"
    ;;
esac

# Hand off to the existing local-mode smoke. It already supports
# BACKEND=codex|opencode, so we just need to set BACKEND + the env vars
# above and let it do build + boot + run + verify.
exec env BACKEND="$BACKEND" bash "$REPO_ROOT/scripts/smoke-real-llm.sh" "$@"
