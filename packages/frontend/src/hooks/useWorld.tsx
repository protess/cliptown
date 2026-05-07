/**
 * WorldProvider + useWorld: React Context wrapper around `useConsole` so any
 * descendant can read connection status and the live ConsoleOutbound-derived
 * state without re-establishing a WebSocket.
 *
 * Configuration is read once at module load from Vite env vars:
 *   VITE_WORLD_WS_URL    — defaults to ws://127.0.0.1:8080/ws/console
 *   VITE_OPERATOR_TOKEN  — defaults to "dev-token" (Phase 0 dev seed)
 */

import { createContext, useContext, type ReactNode } from "react";
import { useConsole, type WorldState } from "../store.js";

const WS_URL: string =
  (import.meta.env.VITE_WORLD_WS_URL as string | undefined) ??
  "ws://127.0.0.1:8080/ws/console";

const OPERATOR_TOKEN: string =
  (import.meta.env.VITE_OPERATOR_TOKEN as string | undefined) ?? "dev-token";

interface WorldContextValue {
  state: WorldState;
  send: (msg: object) => void;
  addToast: (severity: string, body: string, sticky?: boolean) => void;
}

const WorldContext = createContext<WorldContextValue | null>(null);

export function WorldProvider({ children }: { children: ReactNode }) {
  const value = useConsole({ url: WS_URL, operatorToken: OPERATOR_TOKEN });
  return (
    <WorldContext.Provider value={value}>{children}</WorldContext.Provider>
  );
}

export function useWorld(): WorldContextValue {
  const v = useContext(WorldContext);
  if (!v) {
    throw new Error("useWorld must be used inside <WorldProvider>");
  }
  return v;
}
