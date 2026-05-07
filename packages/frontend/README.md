# @cliptown/frontend

cliptown's React + Pixi.js operator console.

## Run

```bash
pnpm --filter @cliptown/frontend dev
```

Open http://127.0.0.1:5173/. Routes:

- `/` redirects to `/console`
- `/console` operator dashboard (skeleton; M4.3+)
- `/town/:id` 2D world view (skeleton; M4.9+)

## Build

```bash
pnpm --filter @cliptown/frontend build
```

Outputs to `dist/`.

## Fonts

See `public/fonts/README.md` for IBM Plex setup.
