/**
 * WorldProvider + useWorld: React Context wrapper around `useConsole` so any
 * descendant can read connection status and the live ConsoleOutbound-derived
 * state without re-establishing a WebSocket.
 *
 * Configuration is read once at module load from Vite env vars:
 *   VITE_WORLD_WS_URL    — defaults to ws://127.0.0.1:8080/ws/console
 *   VITE_OPERATOR_TOKEN  — defaults to "dev-token" (Phase 0 dev seed)
 *
 * Phase 0 / M4.13 — the global keymap (j/k navigation, `t` to open the town
 * view of the selected startup) needs read+write access to the currently
 * selected startup id. Hoisting it into context (instead of Console's local
 * useState) lets the keymap react without prop drilling or a new global
 * store.
 */

import {
  createContext,
  useContext,
  useState,
  type ReactNode,
} from "react";
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
  selectedStartupId: string | null;
  setSelectedStartupId: (id: string | null) => void;
}

const WorldContext = createContext<WorldContextValue | null>(null);

export function WorldProvider({ children }: { children: ReactNode }) {
  const console = useConsole({ url: WS_URL, operatorToken: OPERATOR_TOKEN });
  const [selectedStartupId, setSelectedStartupId] = useState<string | null>(
    null,
  );
  return (
    <WorldContext.Provider
      value={{
        state: console.state,
        send: console.send,
        addToast: console.addToast,
        selectedStartupId,
        setSelectedStartupId,
      }}
    >
      {children}
    </WorldContext.Provider>
  );
}

export function useWorld(): WorldContextValue {
  const v = useContext(WorldContext);
  if (!v) {
    throw new Error("useWorld must be used inside <WorldProvider>");
  }
  return v;
}
