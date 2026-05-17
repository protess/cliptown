# P5 Theme E: frontend image for the docker-compose deploy.
#
# Stage 1 builds the SPA via pnpm. Stage 2 serves it with nginx +
# a small config that proxies /api/* and /ws/* to the world service
# (parity with the Vite dev proxy in vite.config.ts).
#
# Build (from repo root): docker compose build frontend
# Or standalone:          docker build -f packages/frontend/Dockerfile -t cliptown-frontend .

FROM node:20-bookworm-slim AS builder

RUN corepack enable && corepack prepare pnpm@9.0.0 --activate

WORKDIR /build

# Pull in the workspace skeleton + protocol package (typed bindings)
# before the frontend source so the install layer caches well.
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/protocol ./packages/protocol
COPY packages/frontend/package.json ./packages/frontend/
COPY packages/adapters/core/package.json ./packages/adapters/core/
COPY packages/adapters/claude-code/package.json ./packages/adapters/claude-code/
COPY packages/adapters/codex/package.json ./packages/adapters/codex/
COPY packages/adapters/opencode/package.json ./packages/adapters/opencode/
COPY packages/worker/package.json ./packages/worker/

RUN pnpm install --frozen-lockfile --prod=false

COPY packages/frontend ./packages/frontend

# `pnpm build` runs tsc + vite build. Output lands in packages/frontend/dist.
RUN pnpm --filter @cliptown/frontend build

FROM nginx:1.27-alpine AS runtime

# Strip the default nginx config; we ship our own.
RUN rm /etc/nginx/conf.d/default.conf
COPY packages/frontend/nginx.conf /etc/nginx/conf.d/default.conf

COPY --from=builder /build/packages/frontend/dist /usr/share/nginx/html

EXPOSE 80
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
    CMD wget -qO- http://127.0.0.1/health.txt || exit 1
