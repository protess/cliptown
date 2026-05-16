# Deploy guide

Phase 3 Theme A — cliptown world+worker bundled in a single container.
Workers spawn as child processes of the world (via the agent supervisor),
so there's exactly one image and one process to deploy.

**Default deployment target is local.** cliptown is designed to host a
team-of-agents simulation that calls LLMs — and the most interesting
LLM story today is local (ollama, llama.cpp, vLLM, LM Studio), where a
cloud VM can't reach your GPU anyway. Cloud deploy is the right choice
when you want to share a live instance with collaborators or run
always-on against hosted provider APIs.

Reading order:

1. [Local: native (cargo + pnpm)](#local-native-cargo--pnpm) — fastest
   path; iterate on cliptown itself.
2. [Local: docker compose](#local-docker-compose) — same-as-prod image
   on your laptop. Use when you want isolation.
3. [Local LLM (ollama, etc.)](#local-llm-ollama-etc) — point adapters
   at `localhost`.
4. [Cloud: Fly.io](#cloud-flyio-single-vm-persistent-volume) — shared
   hosted instance.
5. [Other targets (sketch)](#other-targets-sketch) — Fargate, K8s,
   bare VPS.

## Local: native (cargo + pnpm)

The dev loop. Boots world + frontend with hot reload, no container
overhead. Best when you're hacking on cliptown.

```bash
# One-time install.
pnpm install
cargo build --workspace

# Boot world (axum + SQLite WAL in ./data) + frontend (Vite) together.
pnpm dev
```

Defaults:

- World: `http://localhost:8080` (operator console WS at `/console`).
- Frontend: `http://localhost:5173` — proxies to the world.
- SQLite: `./data/cliptown.db` (created on first boot).
- Workspaces: `./workspaces/<startup_id>/<task_id>/` (per-task execenv).

Stop with Ctrl-C; SQLite + workspaces persist between runs.

## Local: docker compose

Same image you'd ship to a cloud target, running on your laptop.

```bash
# Build + boot.
docker compose up -d --build

# Tail logs.
docker compose logs -f world

# Health check.
curl http://localhost:8080/health
# → {"ok": true}

# Stop (data persists in volumes).
docker compose down

# Wipe everything including SQLite data + workspaces.
docker compose down -v
```

### Required env

`.env` in the repo root (gitignored). Minimum:

```bash
CLIPTOWN_OPERATOR_TOKEN=<generate via `openssl rand -hex 32`>
```

For hosted-LLM mode, also set whichever provider keys your operators use:

```bash
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...
```

For local-LLM mode, see the next section instead — you set base-URL env
vars rather than provider keys.

Per-agent secrets follow the pattern
`CLIPTOWN_AGENT_SECRET_<agent_id>=<secret>`. Without these, agents fall
back to the literal string `"dev-secret"` — fine for local play, not
for prod.

### Volumes

- `/data` — SQLite database (`cliptown.db`) + WAL files.
- `/workspaces` — per-task execenv directories (P2.3). Mount on
  persistent disk in prod; per-task subdirs are GC'd at the operator's
  discretion until a real sweeper ships.

## Local LLM (ollama, etc.)

All three adapters (`claude-code`, `codex`, `opencode`) spawn their
respective CLI as a child process and inherit the worker's `process.env`
verbatim. To route an agent at a local model, set the CLI's base-URL
env var before booting the world — the adapter doesn't need to know.

### ollama via the codex adapter (most direct)

ollama exposes an OpenAI-compatible API at `http://localhost:11434/v1`.
The codex CLI honors `OPENAI_BASE_URL` + `OPENAI_API_KEY`, so:

```bash
# Pull a model.
ollama pull llama3.1

# In your .env (or shell before `pnpm dev`):
OPENAI_BASE_URL=http://localhost:11434/v1
OPENAI_API_KEY=ollama           # any non-empty string
CODEX_MODEL_ID=llama3.1
```

Then provision a startup whose `engineer` (or `founder` / `designer`)
backend is `codex`. The adapter spawn inherits all three vars; the
codex CLI talks to ollama instead of api.openai.com.

### ollama via the opencode adapter

opencode is provider-agnostic and has a model-spec convention. Set:

```bash
OPENCODE_MODEL=ollama/llama3.1
```

opencode resolves the `ollama/` prefix to a local endpoint via its own
config — see `packages/adapters/opencode/README.md` for the model-spec
grammar.

### claude-code with a local backend

The official Anthropic CLI can be pointed at a compatible proxy via
`ANTHROPIC_BASE_URL`. There is no first-class local Anthropic
implementation, so you need a translator layer (e.g., a LiteLLM proxy
that converts Anthropic Messages API → ollama). Out of scope for this
guide; use codex or opencode for local-LLM workflows.

### Verify with `scripts/smoke-ollama.sh`

The local-LLM path has a dedicated smoke. With ollama serving and a
model already pulled:

```bash
# Pre-flight (the script checks both).
ollama serve &
ollama pull llama3.1:8b

# codex+ollama (default). ~30s on Apple Silicon with the 8B model.
scripts/smoke-ollama.sh

# opencode+ollama, larger model:
BACKEND=opencode OLLAMA_MODEL=qwen2.5-coder:7b scripts/smoke-ollama.sh
```

The wrapper sets `OPENAI_BASE_URL=http://127.0.0.1:11434/v1` +
`OPENAI_API_KEY=ollama` + the backend's MODEL env var, then hands
off to `smoke-real-llm.sh` (local mode). A green `PASS — A3 smoke
complete` means cliptown talked to ollama end-to-end through the
adapter → MCP HTTP → world chain.

Cost: zero. ollama is local-only.

claude-code can NOT use this script — the Anthropic CLI has no
OpenAI-compat translation. Run ollama through a LiteLLM proxy if
you must use the claude adapter against a local backend.

### Docker compose notes

If you're running cliptown in docker compose and ollama on the host,
the container's `localhost` is itself, not the host. Use one of:

- `OPENAI_BASE_URL=http://host.docker.internal:11434/v1` (Mac/Windows).
- `OPENAI_BASE_URL=http://172.17.0.1:11434/v1` (Linux default bridge).
- Or run ollama in its own service inside the same compose file.

## Cloud: Fly.io (single VM, persistent volume)

Use when you want a shared hosted instance — e.g., several operators
collaborating against a hosted provider API (Anthropic / OpenAI).
**Local LLMs are not reachable from a Fly VM by design**; if your
workflow depends on ollama or similar, stay local.

cliptown is single-process today. Scale up by VM size, not by replicas.

### One-time setup

```bash
# Install flyctl: https://fly.io/docs/hands-on/install-flyctl/
fly auth login

# Name the app (must be globally unique on Fly).
fly launch --copy-config --no-deploy --name cliptown-<unique>

# Create a 5 GB volume in the same region as primary_region in fly.toml.
fly volumes create cliptown_data --size 5 --region iad

# Set secrets (these survive across deploys, encrypted at rest).
fly secrets set \
  CLIPTOWN_OPERATOR_TOKEN=$(openssl rand -hex 32) \
  ANTHROPIC_API_KEY=sk-ant-... \
  OPENAI_API_KEY=sk-...

# First deploy.
fly deploy
```

### Verify

```bash
# Health from the public hostname.
curl https://cliptown-<unique>.fly.dev/health

# Tail logs.
fly logs

# SSH in for SQL spelunking.
fly ssh console
sqlite3 /data/cliptown.db ".tables"
```

### Updating

Each `fly deploy` builds a new image, rolls it out behind a health
check, then drains the old VM. Volumes persist across deploys, so SQL
state survives.

### Rotating tokens

```bash
fly secrets set CLIPTOWN_OPERATOR_TOKEN=$(openssl rand -hex 32)
# Triggers a redeploy; in-flight WS connections drop and reconnect with
# the old token will fail. Update your operator UI's stored token
# before this rolls out, or accept a brief outage.
```

### Rolling back

```bash
fly releases             # list deploy history
fly releases rollback <n>
```

## Other targets (sketch)

Same image works on any Docker host. Quick notes:

- **AWS Fargate / ECS**: mount EFS for `/data` + `/workspaces`; one
  task definition with internal_port=8080 behind an ALB.
- **GCP Cloud Run**: doesn't fit — Cloud Run is stateless. Use Compute
  Engine or GKE with a persistent disk.
- **Vercel**: doesn't fit — Vercel is serverless functions + static
  hosting; no long-running process, no persistent disk, no
  server-side WebSocket. The frontend SPA alone could be hosted there
  but the world must run somewhere stateful.
- **Bare VPS (Hetzner, OVH, etc.)**: `docker compose up -d` + a reverse
  proxy (Caddy / nginx) terminating TLS.
- **K8s**: one Deployment with replicas=1, one PersistentVolumeClaim
  for `/data`, one for `/workspaces`. Don't scale-out replicas without
  rebuilding cliptown around shared state.

## Secrets pattern

- **`CLIPTOWN_OPERATOR_TOKEN`** — gate for the operator console WS.
  Phase 3 Theme B (#52) adds the `operators` table for multi-operator
  RBAC; this env var becomes the legacy fallback that seeds a synthetic
  admin identity.
- **`CLIPTOWN_AGENT_SECRET_<agent_id>`** — per-agent bearer secret for
  the worker → world MCP HTTP + the worker → world
  `/api/agents/:id/skills` endpoint. Generated externally; cliptown
  doesn't mint them.
- **`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / etc.** — provider auth
  for the CLI children spawned by the worker. Cliptown doesn't touch
  these; the CLIs read them directly.
- **`OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL`** — override the CLI's
  upstream endpoint. Used for local-LLM routing (ollama, vLLM,
  LiteLLM proxy, etc.). See [Local LLM](#local-llm-ollama-etc).
- **`CLIPTOWN_EXECENV_GC_ENABLED=1`** — opt in to the world-side
  periodic execenv GC daemon. Same selection criteria as
  `scripts/gc-execenv.sh` (terminal-state tasks past the age
  cutoff) but runs unattended every
  `CLIPTOWN_EXECENV_GC_INTERVAL_HOURS` (default 6h). Workspaces
  root from `CLIPTOWN_WORKSPACES_ROOT` (default `./workspaces`).
  Age cutoff `CLIPTOWN_EXECENV_GC_AGE_DAYS` (default 7d). Off by
  default — dev / smoke envs stay quiet.
- **`CLIPTOWN_PER_TASK_WORKERS=1`** — opt in to per-task worker
  spawn. The scheduler hands each `queued` task to the supervisor,
  which fires a one-shot `worker --real --task-id --prompt
  --preferred-backend --preferred-model …` process. The legacy
  long-running daemon spawn at startup-creation time is skipped.
  Completes the Phase 3 Theme C wire — `tasks.preferred_*` columns
  now flow through to adapter spawn. Leave unset for the legacy
  daemon model (smoke harness, anything that hasn't migrated).

Never commit any of these. `.env` is gitignored; `fly secrets` are
encrypted at rest.

## Health + readiness

- `GET /health` returns `{"ok": true}` once the bind succeeds. Use for
  load-balancer liveness.
- `GET /metrics` returns Prometheus text format (Phase 3 Theme D, #51).
  Scrape for liveness, MCP call/error rates, per-startup budget, task
  counts by status, agent health buckets.
- No separate readiness probe — the world is ready as soon as it
  binds (SQLite is sync, migrations apply at boot).

## Smoke against a deployed instance

`scripts/smoke-real-llm.sh` supports both local and remote targets.

**Local mode (default):** builds + boots its own world in a tmpdir,
seeds via SQL, runs the worker locally, verifies artifact + SQL row
+ execenv + skill + budget end-to-end.

**Remote mode** (`WORLD_REMOTE_URL=https://...`): skips build + boot,
seeds the task via `POST /api/admin/tasks` (operator-token gated,
manager-or-above), runs the worker locally against the remote
`/ws/worker`. The world's `task_done` MCP handler still validates
the artifact path on its (remote) filesystem; a clean adapter exit
is treated as success. FS-bound checks (artifact-on-disk, execenv
layout, skill files) and SQL-row inspection are skipped — no
client-side path to either.

```bash
# Local — the existing path.
ANTHROPIC_API_KEY=sk-ant-... bash scripts/smoke-real-llm.sh

# Remote against a Fly.io deploy.
WORLD_REMOTE_URL=https://cliptown-<unique>.fly.dev \
CLIPTOWN_OPERATOR_TOKEN=... \
ANTHROPIC_API_KEY=sk-ant-... \
  bash scripts/smoke-real-llm.sh
```

For ad-hoc manual checks against a remote deploy:

```bash
# Health.
curl https://cliptown-<unique>.fly.dev/health

# Create a startup via the operator-token-gated API.
curl -X POST https://cliptown-<unique>.fly.dev/api/startups \
  -H "Authorization: Bearer <CLIPTOWN_OPERATOR_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{"name":"smoke","goal_text":"test","budget_cap_usd":1.0,
       "backends":{"founder":"claude_code","engineer":"claude_code","designer":"claude_code"}}'

# Seed a task manually (admin/manager role).
curl -X POST https://cliptown-<unique>.fly.dev/api/admin/tasks \
  -H "Authorization: Bearer <CLIPTOWN_OPERATOR_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{"startup_id":"<sid>","title":"t","description":"d",
       "assignee_agent_id":"<aid>"}'
```
