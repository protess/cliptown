/**
 * useReducer-based store for ConsoleOutbound traffic from the world.
 *
 * Phase 0 scope: handles WorldViewSnapshot, WorldViewDelta (no-op until the
 * world emits deltas), SystemEvent, BackendCatalog, Toast, Modal. Pairs with
 * `ConsoleClient` from ./ws.ts; mounted into React via ./hooks/useWorld.tsx.
 *
 * Type alignment: the canonical ConsoleOutbound shape lives in
 * `packages/protocol/dist/ConsoleOutbound.ts` (ts-rs from
 * crates/world/src/protocol/ws_messages.rs). Today its `snapshot`,
 * `changes`, `payload`, and `entries` fields are typed as `JsonValue`
 * (opaque), so this store does pragmatic structural unwrapping — we read
 * `world.avatars`, `world.backend_catalog`, etc. from the snapshot's
 * JsonValue shape without claiming exhaustive ConsoleOutbound coverage.
 *
 * Stand-ins (Phase 0 — track for follow-up so protocol coverage tightens):
 *   - StartupVM, TaskVM: not yet emitted by ConsoleOutbound (the WorldView
 *     ts-rs type has only `avatars` + `backend_catalog`). Defined locally
 *     so the UI can bind to them once the world starts publishing them.
 */

import { useReducer, useEffect, useRef } from "react";
import { ConsoleClient, type ConnectionStatus } from "./ws.js";

export interface AvatarVM {
  agent_id: string;
  startup_id: string;
  role: string;
  backend: string;
  current_pos: [number, number];
  target_pos: [number, number] | null;
  room_id: string;
  status: string;
}

export interface StartupVM {
  id: string;
  name: string;
  budget_spent_usd?: number;
  budget_cap_usd?: number;
  last_event_ts?: number;
}

export interface TaskVM {
  id: string;
  startup_id: string;
  title: string;
  status: string;
  assignee_agent_id?: string | null;
  required_room?: string | null;
}

export interface SystemEventVM {
  ts: number;
  severity: "info" | "warn" | "alert";
  kind: string;
  startup_id: string | null;
  payload: unknown;
}

export interface ToastVM {
  id: string;
  ts: number;
  severity: string;
  body: string;
  sticky: boolean;
}

export interface ModalVM {
  id: string;
  kind: string;
  payload: unknown;
}

export interface WorldState {
  status: ConnectionStatus;
  avatars: Record<string, AvatarVM>;
  startups: Record<string, StartupVM>;
  tasks: Record<string, TaskVM>;
  systemEvents: SystemEventVM[];
  backendCatalog: Record<string, unknown>;
  toasts: ToastVM[];
  modals: ModalVM[];
}

const INITIAL: WorldState = {
  status: "connecting",
  avatars: {},
  startups: {},
  tasks: {},
  systemEvents: [],
  backendCatalog: {},
  toasts: [],
  modals: [],
};

const MAX_SYSTEM_EVENTS = 200;
const MAX_TOASTS = 20;

type Msg = Record<string, unknown> & { type?: unknown };

type Action =
  | { kind: "status"; status: ConnectionStatus }
  | { kind: "msg"; msg: Msg }
  | { kind: "localToast"; severity: string; body: string; sticky?: boolean };

function asObject(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
}

function asString(v: unknown, fallback = ""): string {
  return typeof v === "string" ? v : fallback;
}

function newId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `id-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function indexAvatars(raw: unknown): Record<string, AvatarVM> {
  const out: Record<string, AvatarVM> = {};
  // WorldView.avatars is `{ [agent_id]: AvatarView }` (see
  // packages/protocol/dist/WorldView.ts), but we accept arrays too in case a
  // future view reshapes it.
  const obj = asObject(raw);
  if (obj) {
    for (const [k, v] of Object.entries(obj)) {
      const a = asObject(v);
      if (!a) continue;
      out[k] = coerceAvatar(a, k);
    }
    return out;
  }
  if (Array.isArray(raw)) {
    for (const v of raw) {
      const a = asObject(v);
      if (!a) continue;
      const id = asString(a.agent_id);
      if (!id) continue;
      out[id] = coerceAvatar(a, id);
    }
  }
  return out;
}

function coerceAvatar(a: Record<string, unknown>, agent_id: string): AvatarVM {
  const cp = a.current_pos;
  const tp = a.target_pos;
  return {
    agent_id: asString(a.agent_id, agent_id),
    startup_id: asString(a.startup_id),
    role: asString(a.role),
    backend: asString(a.backend),
    current_pos: Array.isArray(cp) && cp.length >= 2 && typeof cp[0] === "number" && typeof cp[1] === "number"
      ? [cp[0], cp[1]]
      : [0, 0],
    target_pos: Array.isArray(tp) && tp.length >= 2 && typeof tp[0] === "number" && typeof tp[1] === "number"
      ? [tp[0], tp[1]]
      : null,
    room_id: asString(a.room_id),
    status: asString(a.status),
  };
}

function indexStartups(raw: unknown): Record<string, StartupVM> {
  const out: Record<string, StartupVM> = {};
  if (Array.isArray(raw)) {
    for (const v of raw) {
      const s = asObject(v);
      if (!s) continue;
      const id = asString(s.id);
      if (!id) continue;
      out[id] = {
        id,
        name: asString(s.name, id),
        budget_spent_usd: typeof s.budget_spent_usd === "number" ? s.budget_spent_usd : undefined,
        budget_cap_usd: typeof s.budget_cap_usd === "number" ? s.budget_cap_usd : undefined,
        last_event_ts: typeof s.last_event_ts === "number" ? s.last_event_ts : undefined,
      };
    }
  } else {
    const obj = asObject(raw);
    if (obj) {
      for (const [k, v] of Object.entries(obj)) {
        const s = asObject(v);
        if (!s) continue;
        out[k] = {
          id: asString(s.id, k),
          name: asString(s.name, k),
          budget_spent_usd: typeof s.budget_spent_usd === "number" ? s.budget_spent_usd : undefined,
          budget_cap_usd: typeof s.budget_cap_usd === "number" ? s.budget_cap_usd : undefined,
          last_event_ts: typeof s.last_event_ts === "number" ? s.last_event_ts : undefined,
        };
      }
    }
  }
  return out;
}

function indexTasks(raw: unknown): Record<string, TaskVM> {
  const out: Record<string, TaskVM> = {};
  if (Array.isArray(raw)) {
    for (const v of raw) {
      const t = asObject(v);
      if (!t) continue;
      const id = asString(t.id);
      if (!id) continue;
      out[id] = {
        id,
        startup_id: asString(t.startup_id),
        title: asString(t.title),
        status: asString(t.status),
        assignee_agent_id:
          typeof t.assignee_agent_id === "string" ? t.assignee_agent_id : null,
        required_room:
          typeof t.required_room === "string" ? t.required_room : null,
      };
    }
  } else {
    const obj = asObject(raw);
    if (obj) {
      for (const [k, v] of Object.entries(obj)) {
        const t = asObject(v);
        if (!t) continue;
        out[k] = {
          id: asString(t.id, k),
          startup_id: asString(t.startup_id),
          title: asString(t.title),
          status: asString(t.status),
          assignee_agent_id:
            typeof t.assignee_agent_id === "string" ? t.assignee_agent_id : null,
          required_room:
            typeof t.required_room === "string" ? t.required_room : null,
        };
      }
    }
  }
  return out;
}

function severityFromString(s: unknown): SystemEventVM["severity"] {
  if (s === "warn" || s === "alert") return s;
  return "info";
}

function reducer(state: WorldState, action: Action): WorldState {
  if (action.kind === "status") return { ...state, status: action.status };
  if (action.kind === "localToast") {
    const t: ToastVM = {
      id: newId(),
      ts: Date.now(),
      severity: action.severity,
      body: action.body,
      sticky: action.sticky ?? false,
    };
    const next = [...state.toasts, t];
    if (next.length > MAX_TOASTS) next.splice(0, next.length - MAX_TOASTS);
    return { ...state, toasts: next };
  }
  const m = action.msg;
  switch (m.type) {
    case "world_view_snapshot": {
      // ConsoleOutbound::WorldViewSnapshot.snapshot is a JsonValue containing
      // a serialized WorldView. We unwrap defensively rather than assuming
      // the field is non-null.
      const snap = asObject(m.snapshot) ?? {};
      const avatars = indexAvatars(snap.avatars);
      const startups = indexStartups(snap.startups);
      const tasks = indexTasks(snap.tasks);
      const catalog = asObject(snap.backend_catalog) ?? {};
      return {
        ...state,
        avatars,
        startups: Object.keys(startups).length > 0 ? startups : state.startups,
        tasks: Object.keys(tasks).length > 0 ? tasks : state.tasks,
        backendCatalog: catalog,
      };
    }
    case "world_view_delta": {
      // Phase 0: world does not emit deltas yet; harmless no-op so the UI
      // doesn't crash when the field eventually arrives.
      return state;
    }
    case "system_event": {
      const ev: SystemEventVM = {
        ts: typeof m.ts === "number" ? m.ts : Date.now(),
        severity: severityFromString(m.severity),
        kind: asString(m.kind),
        startup_id: typeof m.startup_id === "string" ? m.startup_id : null,
        payload: m.payload ?? null,
      };
      const next = [ev, ...state.systemEvents];
      if (next.length > MAX_SYSTEM_EVENTS) next.length = MAX_SYSTEM_EVENTS;
      return { ...state, systemEvents: next };
    }
    case "backend_catalog": {
      return {
        ...state,
        backendCatalog: asObject(m.entries) ?? {},
      };
    }
    case "toast": {
      const t: ToastVM = {
        id: newId(),
        ts: Date.now(),
        severity: asString(m.severity, "info"),
        body: asString(m.body),
        sticky: typeof m.sticky === "boolean" ? m.sticky : false,
      };
      const next = [...state.toasts, t];
      if (next.length > MAX_TOASTS) next.splice(0, next.length - MAX_TOASTS);
      return { ...state, toasts: next };
    }
    case "modal": {
      const md: ModalVM = {
        id: newId(),
        kind: asString(m.kind),
        payload: m.payload ?? null,
      };
      return { ...state, modals: [...state.modals, md] };
    }
    default:
      return state;
  }
}

export interface UseConsoleOpts {
  url: string;
  operatorToken: string;
}

export interface UseConsoleResult {
  state: WorldState;
  send: (msg: object) => void;
  /**
   * Phase 0 stand-in: pushes a toast directly into local state without going
   * through the world. M4.6 uses this for "agent-driven only" snap-back
   * feedback. The world will eventually publish authoritative toasts via
   * ConsoleOutbound::Toast (case "toast" in the reducer); this helper is for
   * UI-side ephemeral feedback only.
   */
  addToast: (severity: string, body: string, sticky?: boolean) => void;
}

export function useConsole(opts: UseConsoleOpts): UseConsoleResult {
  const [state, dispatch] = useReducer(reducer, INITIAL);
  const clientRef = useRef<ConsoleClient | null>(null);

  useEffect(() => {
    const client = new ConsoleClient({
      url: opts.url,
      operatorToken: opts.operatorToken,
      onMessage: (msg) => dispatch({ kind: "msg", msg: msg as Msg }),
      onStatus: (s) => dispatch({ kind: "status", status: s }),
    });
    clientRef.current = client;
    client.start();
    return () => {
      client.close();
      clientRef.current = null;
    };
  }, [opts.url, opts.operatorToken]);

  return {
    state,
    send: (msg) => clientRef.current?.send(msg),
    addToast: (severity, body, sticky) =>
      dispatch({ kind: "localToast", severity, body, sticky }),
  };
}
