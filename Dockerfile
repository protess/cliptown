# cliptown world+worker bundle.
#
# Phase 3 Theme A: single image carrying the world server binary + the
# worker tsx runtime. The world spawns workers as child processes via its
# agent supervisor (crates/world/src/agent_supervisor.rs), so both must
# live in the same image.
#
# Build:  docker build -t cliptown:latest .
# Run:    docker run -p 8080:8080 -v cliptown-data:/data cliptown:latest

# ── Stage 1: build the world binary (release) ─────────────────────────────
FROM rust:1.86-slim-bookworm AS world-builder

# sqlx requires libssl + pkg-config at build time. ca-certificates so cargo
# can fetch from crates.io behind corporate proxies.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml ./
COPY crates ./crates
RUN cargo build --release -p cliptown-world --bin cliptown-world

# ── Stage 2: build the worker (TS → JS via tsx) ───────────────────────────
# We don't actually transpile — tsx runs TS at spawn time. We just need
# node_modules + the source tree. This keeps the bundle thin (~30 MB
# instead of running a full webpack/rollup).
FROM node:20-bookworm-slim AS worker-deps

RUN corepack enable && corepack prepare pnpm@9.0.0 --activate

WORKDIR /build
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/protocol/package.json ./packages/protocol/
COPY packages/worker/package.json ./packages/worker/
COPY packages/adapters/core/package.json ./packages/adapters/core/
COPY packages/adapters/claude-code/package.json ./packages/adapters/claude-code/
COPY packages/adapters/codex/package.json ./packages/adapters/codex/
COPY packages/adapters/opencode/package.json ./packages/adapters/opencode/

RUN pnpm install --frozen-lockfile --prod=false

COPY packages ./packages

# ── Stage 3: runtime ──────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 sqlite3 curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -g 1000 cliptown \
    && useradd -m -u 1000 -g cliptown cliptown

# Install Node 20 — workers run via tsx (TS-at-spawn-time).
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && corepack enable \
    && corepack prepare pnpm@9.0.0 --activate

WORKDIR /app
COPY --from=world-builder /build/target/release/cliptown-world /usr/local/bin/cliptown-world
COPY --from=worker-deps /build /app
COPY crates/world/migrations /app/migrations
COPY cliptown.toml /app/cliptown.toml

# Workspace + sqlite persistence live here. Mount a volume for prod.
RUN mkdir -p /data /workspaces && chown -R cliptown:cliptown /app /data /workspaces

USER cliptown
WORKDIR /app

# Defaults — override via env at run time.
ENV CLIPTOWN_ADDR=0.0.0.0:8080 \
    CLIPTOWN_DB=/data/cliptown.db \
    CLIPTOWN_OPERATOR_TOKEN=dev-token \
    RUST_LOG=info

EXPOSE 8080
VOLUME ["/data", "/workspaces"]

HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/health || exit 1

CMD ["cliptown-world"]
