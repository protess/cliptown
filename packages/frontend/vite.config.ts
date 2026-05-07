import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: false,
    host: "127.0.0.1",
    // Proxy /api and /ws to the world server so the browser sees same-origin
    // requests during dev (no CORS preflight, no cross-origin WebSocket
    // handshake). Production deployments bake the world URL into env vars
    // and skip the proxy entirely.
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
      },
      "/ws": {
        target: "ws://127.0.0.1:8080",
        ws: true,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
  },
});
