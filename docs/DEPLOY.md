# Deploy guide

Phase 3 Theme A — cliptown world+worker bundled in a single container.
Workers spawn as child processes of the world (via the agent supervisor),
so there's exactly one image and one process to deploy.

## Local: docker compose

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

For real-LLM mode, also set whichever provider keys your operators use:

```bash
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...
```

Per-agent secrets follow the pattern `CLIPTOWN_AGENT_SECRET_<agent_id>=<secret>`. Without these, agents fall back to the literal string `"dev-secret"` — fine for local play, not for prod.

### Volumes

- `/data` — SQLite database (`cliptown.db`) + WAL files.
- `/workspaces` — per-task execenv directories (P2.3). Mount on persistent disk in prod; per-task subdirs are GC'd at the operator's discretion until a real sweeper ships.

## Fly.io (single VM, persistent volume)

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

Each `fly deploy` builds a new image, rolls it out behind a health check, then drains the old VM. Volumes persist across deploys, so SQL state survives.

### Rotating tokens

```bash
fly secrets set CLIPTOWN_OPERATOR_TOKEN=$(openssl rand -hex 32)
# Triggers a redeploy; in-flight WS connections drop and reconnect with the
# old token will fail. Update your operator UI's stored token before this
# rolls out, or accept a brief outage.
```

### Rolling back

```bash
fly releases             # list deploy history
fly releases rollback <n>
```

## Other targets (sketch)

Same image works on any Docker host. Quick notes:

- **AWS Fargate / ECS**: mount EFS for `/data` + `/workspaces`; one task definition with internal_port=8080 behind an ALB.
- **GCP Cloud Run**: doesn't fit — Cloud Run is stateless. Use Compute Engine or GKE with a persistent disk.
- **Bare VPS (Hetzner, OVH, etc.)**: `docker compose up -d` + a reverse proxy (Caddy / nginx) terminating TLS.
- **K8s**: one Deployment with replicas=1, one PersistentVolumeClaim for `/data`, one for `/workspaces`. Don't scale-out replicas without rebuilding cliptown around shared state.

## Secrets pattern

- **`CLIPTOWN_OPERATOR_TOKEN`** — gate for the operator console WS. Single
  value today; multi-operator + RBAC is Phase 3 Theme B work.
- **`CLIPTOWN_AGENT_SECRET_<agent_id>`** — per-agent bearer secret for the
  worker → world MCP HTTP + the worker → world `/api/agents/:id/skills`
  endpoint. Generated externally; cliptown doesn't mint them.
- **`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / etc.** — provider auth for the
  CLI children spawned by the worker. Cliptown doesn't touch these; the
  CLIs read them directly.

Never commit any of these. `.env` is gitignored; `fly secrets` are
encrypted at rest.

## Health + readiness

- `GET /health` returns `{"ok": true}` once the bind succeeds. Use for
  load-balancer liveness.
- No separate readiness probe — the world is ready as soon as it binds
  (SQLite is sync, migrations apply at boot).

## Smoke against a deployed instance

`scripts/smoke-real-llm.sh` currently boots its own world server in a
tmpdir. Targeting a remote deploy needs a parameterization pass — track
in the carry-forward list in `docs/superpowers/specs/2026-05-13-phase-3-roadmap.md`.

For now, verify a remote deploy manually:

```bash
# Health.
curl https://cliptown-<unique>.fly.dev/health

# Create a startup via the operator-token-gated API.
curl -X POST https://cliptown-<unique>.fly.dev/api/startups \
  -H "Authorization: Bearer <CLIPTOWN_OPERATOR_TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{"name":"smoke","goal_text":"test","budget_cap_usd":1.0,
       "backends":{"founder":"claude_code","engineer":"claude_code","designer":"claude_code"}}'
```
