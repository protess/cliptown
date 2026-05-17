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
  last_seen_at: number | null;
  health: "online" | "recently_lost" | "offline" | "about_to_gc";
  /**
   * Theme G slice 2: surfaced from `agents.is_peer_reviewer` via the
   * snapshot enrichment in `build_console_snapshot`. Lets the admin-only
   * AgentsPanel render a per-agent toggle without a side fetch.
   * Defaults to false when absent (older snapshots / unseeded agents).
   */
  is_peer_reviewer: boolean;
}

export interface StartupVM {
  id: string;
  name: string;
  budget_spent_usd?: number;
  budget_cap_usd?: number;
  last_event_ts?: number;
  /**
   * Theme G slice 2: per-startup auto-steal config, surfaced from the
   * snapshot enrichment. Lets the admin-only MainHeader settings popover
   * render hydrated. Defaults: enabled=false, after_secs=60 (SQL default).
   */
  auto_steal_enabled?: boolean;
  auto_steal_after_secs?: number;
}

export interface TaskVM {
  id: string;
  startup_id: string;
  title: string;
  status: string;
  assignee_agent_id?: string | null;
  required_room?: string | null;
  review_round?: number;
  max_review_rounds?: number;
  // Canonical path from cliptown-design § 11.4. World rejects any other
  // shape with `bad_artifact_path`. Set when the engineer's `task_done`
  // commits, visible to the operator while the task sits in
  // `awaiting_review`. Absent on tasks before submission.
  artifact_path?: string | null;
}

export interface SystemEventVM {
  ts: number;
  severity: "info" | "warn" | "alert" | "critical";
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

/**
 * Phase 0 stand-in for chat/directive history surfaced in the floating
 * ChatPanel (M4.10). The world doesn't yet emit dedicated `chat` or
 * `directive` ConsoleOutbound frames — those messages live in the SQLite
 * `messages` table written by cmd_console.rs::OperatorDirective and the
 * cmd_worker speak handler. The reducer below routes any inbound
 * `{type:"chat"|"directive"}` ConsoleOutbound frame into this array so the
 * panel populates as soon as the world starts publishing them (tracked for
 * M5+).
 */
export interface MessageVM {
  id: string;
  ts: number;
  startup_id: string;
  room_id: string | null;
  author_id: string;
  body: string;
  kind: "chat" | "directive";
  recipient_id: string | null;
}

export interface SkillVM {
  id: string;
  name: string;
  len: number;
  updated_at: number;
  attachments: string[];
  /** P3 carry-forward: admin-only global flag. Defaults to false. */
  is_global: boolean;
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
  messages: MessageVM[];
  skills: Record<string, Record<string, SkillVM>>;
  /**
   * P3 Theme B follow-up: operator-management state for the admin panel.
   * `null` = not yet fetched (panel sends `operator_list` on mount to
   * hydrate). `[]` = fetched but empty (or forbidden — non-admin caller).
   * `mintedToken` is set on `operator_create` so the UI can show the
   * freshly-minted bearer exactly once; cleared by the panel after copy.
   */
  operators: OperatorRow[] | null;
  mintedOperatorToken: { id: string; name: string; token: string } | null;
  /**
   * P3 carry-forward: identity of the currently-authenticated operator,
   * populated from the `hello_ok` ConsoleOutbound frame after WS connect.
   * `null` until the frame arrives. Admin-only UI surfaces gate on
   * `currentOperator?.role === "admin"`.
   */
  currentOperator: { id: string; name: string; role: string } | null;
}

export interface OperatorRow {
  id: string;
  name: string;
  role: string;
  created_at: number;
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
  messages: [],
  skills: {},
  operators: null,
  mintedOperatorToken: null,
  currentOperator: null,
};

const MAX_SYSTEM_EVENTS = 200;
const MAX_TOASTS = 20;
const MAX_MESSAGES = 500;

type Msg = Record<string, unknown> & { type?: unknown };

type Action =
  | { kind: "status"; status: ConnectionStatus }
  | { kind: "msg"; msg: Msg }
  | { kind: "localToast"; severity: string; body: string; sticky?: boolean }
  | { kind: "clearMintedOperatorToken" };

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
  const healthRaw = typeof a.health === "string" ? a.health : "offline";
  const VALID: ReadonlyArray<AvatarVM["health"]> = [
    "online", "recently_lost", "offline", "about_to_gc",
  ];
  const health: AvatarVM["health"] = (VALID as ReadonlyArray<string>).includes(healthRaw)
    ? (healthRaw as AvatarVM["health"])
    : "offline";
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
    last_seen_at: typeof a.last_seen_at === "number" ? a.last_seen_at : null,
    health,
    is_peer_reviewer: a.is_peer_reviewer === true,
  };
}

function coerceStartup(s: Record<string, unknown>, id: string): StartupVM {
  return {
    id: asString(s.id, id),
    name: asString(s.name, id),
    budget_spent_usd: typeof s.budget_spent_usd === "number" ? s.budget_spent_usd : undefined,
    budget_cap_usd: typeof s.budget_cap_usd === "number" ? s.budget_cap_usd : undefined,
    last_event_ts: typeof s.last_event_ts === "number" ? s.last_event_ts : undefined,
    auto_steal_enabled: s.auto_steal_enabled === true ? true
      : s.auto_steal_enabled === false ? false
      : undefined,
    auto_steal_after_secs: typeof s.auto_steal_after_secs === "number"
      ? s.auto_steal_after_secs
      : undefined,
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
      out[id] = coerceStartup(s, id);
    }
  } else {
    const obj = asObject(raw);
    if (obj) {
      for (const [k, v] of Object.entries(obj)) {
        const s = asObject(v);
        if (!s) continue;
        out[k] = coerceStartup(s, k);
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
        review_round: typeof t.review_round === "number" ? t.review_round : undefined,
        max_review_rounds: typeof t.max_review_rounds === "number" ? t.max_review_rounds : undefined,
        artifact_path:
          typeof t.artifact_path === "string" ? t.artifact_path : null,
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
          review_round: typeof t.review_round === "number" ? t.review_round : undefined,
          max_review_rounds: typeof t.max_review_rounds === "number" ? t.max_review_rounds : undefined,
          artifact_path:
            typeof t.artifact_path === "string" ? t.artifact_path : null,
        };
      }
    }
  }
  return out;
}

function coerceSkill(a: Record<string, unknown>): SkillVM {
  return {
    id: asString(a.id),
    name: asString(a.name),
    len: typeof a.len === "number" ? a.len : 0,
    updated_at: typeof a.updated_at === "number" ? a.updated_at : 0,
    attachments: Array.isArray(a.attachments)
      ? (a.attachments as unknown[]).filter((x): x is string => typeof x === "string")
      : [],
    is_global: a.is_global === true,
  };
}

function severityFromString(s: unknown): SystemEventVM["severity"] {
  if (s === "warn" || s === "alert" || s === "critical") return s;
  return "info";
}

/**
 * Theme G slice 1: turn the JSON payload of an E-theme system_event into
 * a single-line, human-readable string. Used for both the toast body
 * (when the reducer auto-surfaces these) and the TopBar marquee. Returns
 * an empty string for kinds the helper doesn't recognize, letting the
 * caller fall back to JSON.stringify.
 */
export function prettifySystemEventPayload(
  kind: string,
  payload: unknown,
): string {
  const p =
    payload && typeof payload === "object" && !Array.isArray(payload)
      ? (payload as Record<string, unknown>)
      : null;
  if (!p) return "";
  const id = typeof p.task_id === "string" ? p.task_id : "?";
  switch (kind) {
    case "task_stolen": {
      const mode = typeof p.mode === "string" ? ` (${p.mode})` : "";
      const newA = typeof p.new_assignee === "string" ? p.new_assignee : "?";
      const prev = typeof p.previous_assignee === "string" ? p.previous_assignee : "?";
      return `${id} stolen by ${newA} ← ${prev}${mode}`;
    }
    case "task_unblocked": {
      const blocker = typeof p.blocker_id === "string" ? p.blocker_id : "?";
      return `${id} unblocked from ${blocker}`;
    }
    case "task_overdue": {
      const secs = typeof p.overdue_by_secs === "number" ? p.overdue_by_secs : 0;
      return `${id} overdue by ${secs}s`;
    }
    default:
      return "";
  }
}

/**
 * Theme G slice 1: SystemEvent kinds that should also raise a toast so
 * the operator notices without scanning the marquee. Sticky behavior is
 * derived from severity (warn/alert/critical → sticky).
 */
const TOAST_WORTHY_KINDS = new Set([
  "task_stolen",
  "task_unblocked",
  "task_overdue",
]);

function reducer(state: WorldState, action: Action): WorldState {
  if (action.kind === "status") return { ...state, status: action.status };
  if (action.kind === "clearMintedOperatorToken") {
    return { ...state, mintedOperatorToken: null };
  }
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
    case "hello_ok": {
      return {
        ...state,
        currentOperator: {
          id: typeof m.operator_id === "string" ? m.operator_id : "",
          name: typeof m.operator_name === "string" ? m.operator_name : "",
          role: typeof m.role === "string" ? m.role : "viewer",
        },
      };
    }
    case "world_view_snapshot": {
      // ConsoleOutbound::WorldViewSnapshot.snapshot is a JsonValue containing
      // a serialized WorldView. We unwrap defensively rather than assuming
      // the field is non-null.
      const snap = asObject(m.snapshot) ?? {};
      const avatars = indexAvatars(snap.avatars);
      const catalog = asObject(snap.backend_catalog) ?? {};
      // Codex round-5 P2#4: distinguish "field absent" (preserve previous
      // state) from "field present but empty" (clear). The previous code
      // collapsed both cases via `Object.keys(...).length > 0`, so a
      // last-task-terminal or last-startup-dissolved snapshot left ghost
      // entries in the UI forever. Only fall back to previous state when
      // the snapshot genuinely omits the field.
      const startupsField = (snap as Record<string, unknown>).startups;
      const tasksField = (snap as Record<string, unknown>).tasks;
      const startups =
        startupsField === undefined ? state.startups : indexStartups(startupsField);
      const tasks =
        tasksField === undefined ? state.tasks : indexTasks(tasksField);
      return {
        ...state,
        avatars,
        startups,
        tasks,
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
      const nextEvents = [ev, ...state.systemEvents];
      if (nextEvents.length > MAX_SYSTEM_EVENTS) nextEvents.length = MAX_SYSTEM_EVENTS;
      // Theme G slice 1: auto-surface E-theme events as toasts. The
      // marquee rotates every 3s and is easy to miss; a transient toast
      // catches the eye without requiring the operator to scrub the
      // history rail. Sticky for warn-or-above so an overdue task
      // doesn't disappear while the operator is mid-action.
      if (TOAST_WORTHY_KINDS.has(ev.kind)) {
        const body = prettifySystemEventPayload(ev.kind, ev.payload) ||
          `${ev.kind}: ${JSON.stringify(ev.payload)}`;
        const sticky = ev.severity === "warn" || ev.severity === "alert" || ev.severity === "critical";
        const toast: ToastVM = {
          id: newId(),
          ts: Date.now(),
          severity: ev.severity,
          body,
          sticky,
        };
        const nextToasts = [...state.toasts, toast];
        if (nextToasts.length > MAX_TOASTS) nextToasts.splice(0, nextToasts.length - MAX_TOASTS);
        return { ...state, systemEvents: nextEvents, toasts: nextToasts };
      }
      return { ...state, systemEvents: nextEvents };
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
    case "chat":
    case "directive": {
      // Phase 0 forward-compat: ConsoleOutbound doesn't define `chat` /
      // `directive` frame variants today (see packages/protocol/dist/
      // ConsoleOutbound.ts — only world_view_snapshot, world_view_delta,
      // system_event, backend_catalog, toast, modal). When the world starts
      // emitting them (M5+), this reducer appends them to `messages` for the
      // floating ChatPanel from M4.10. Until then the panel renders empty.
      const kind = m.type === "directive" ? "directive" : "chat";
      // Codex M20: prefer protocol field message_id; fall back to m.id for the
      // synthetic-frame test path in e2e/ship-gate.spec.ts which still passes id.
      const id = typeof m.message_id === "string"
        ? m.message_id
        : typeof m.id === "string" || typeof m.id === "number"
          ? String(m.id)
          : newId();
      // Dedup: skip if we've already seen this id. Costs O(N) per append but
      // prevents future double-emission or retry-storm dupes (Codex NIT #20).
      if (state.messages.some(x => x.id === id)) {
        return state;
      }
      const recipient =
        typeof m.to_agent_id === "string"
          ? m.to_agent_id
          : typeof m.recipient_id === "string"
            ? m.recipient_id
            : null;
      const msg: MessageVM = {
        id,
        ts: typeof m.ts === "number" ? m.ts : Date.now(),
        startup_id: asString(m.startup_id),
        room_id: typeof m.room_id === "string" ? m.room_id : null,
        author_id: asString(m.author_id, asString(m.from)),
        body: asString(m.body),
        kind,
        recipient_id: recipient,
      };
      const next = [...state.messages, msg];
      if (next.length > MAX_MESSAGES) next.splice(0, next.length - MAX_MESSAGES);
      return { ...state, messages: next };
    }
    case "skills_snapshot": {
      const startups = (m as { startups?: Record<string, unknown> }).startups ?? {};
      const out: Record<string, Record<string, SkillVM>> = {};
      for (const [sid, arr] of Object.entries(startups)) {
        if (!Array.isArray(arr)) continue;
        const inner: Record<string, SkillVM> = {};
        for (const raw of arr) {
          if (typeof raw !== "object" || raw === null) continue;
          const skill = coerceSkill(raw as Record<string, unknown>);
          if (skill.id) inner[skill.id] = skill;
        }
        out[sid] = inner;
      }
      return { ...state, skills: out };
    }
    case "skill_changed": {
      const sid = asString((m as { startup_id?: unknown }).startup_id);
      const kind = asString((m as { kind?: unknown }).kind);
      const skill_id = asString((m as { skill_id?: unknown }).skill_id);
      const agent_id: string | null =
        typeof (m as { agent_id?: unknown }).agent_id === "string"
          ? (m as { agent_id: string }).agent_id
          : null;
      const raw = (m as { skill?: unknown }).skill;
      const next = { ...(state.skills ?? {}) };
      const inner = { ...(next[sid] ?? {}) };
      if (kind === "upsert" && typeof raw === "object" && raw !== null) {
        inner[skill_id] = coerceSkill(raw as Record<string, unknown>);
      } else if (kind === "delete") {
        delete inner[skill_id];
      } else if (kind === "attach" && inner[skill_id] && agent_id) {
        const existing = inner[skill_id];
        if (!existing.attachments.includes(agent_id)) {
          inner[skill_id] = { ...existing, attachments: [...existing.attachments, agent_id] };
        }
      } else if (kind === "detach" && inner[skill_id] && agent_id) {
        const existing = inner[skill_id];
        inner[skill_id] = {
          ...existing,
          attachments: existing.attachments.filter((a) => a !== agent_id),
        };
      }
      next[sid] = inner;
      return { ...state, skills: next };
    }
    // P3 Theme B follow-up: operator-management replies. The world responds
    // with `{type:"ok", kind:"operator_*"}` envelopes; the reducer hoists
    // the payload into `state.operators` / `state.mintedOperatorToken` so
    // the OperatorsPanel can render without a separate fetch round-trip.
    case "ok": {
      const kind = typeof m.kind === "string" ? m.kind : "";
      if (kind === "operator_list" && Array.isArray(m.operators)) {
        const rows = (m.operators as unknown[])
          .map((o): OperatorRow | null => {
            const obj = asObject(o);
            if (!obj) return null;
            return {
              id: String(obj.id ?? ""),
              name: String(obj.name ?? ""),
              role: String(obj.role ?? ""),
              created_at: Number(obj.created_at ?? 0),
            };
          })
          .filter((o): o is OperatorRow => o !== null && o.id !== "");
        return { ...state, operators: rows };
      }
      if (kind === "operator_create" && typeof m.id === "string" && typeof m.token === "string") {
        const row: OperatorRow = {
          id: String(m.id),
          name: String(m.name ?? ""),
          role: String(m.role ?? ""),
          created_at: Math.floor(Date.now() / 1000),
        };
        return {
          ...state,
          operators: state.operators ? [...state.operators, row] : [row],
          mintedOperatorToken: { id: row.id, name: row.name, token: String(m.token) },
        };
      }
      if (kind === "operator_revoke" && typeof m.id === "string") {
        return {
          ...state,
          operators: state.operators ? state.operators.filter((o) => o.id !== m.id) : null,
        };
      }
      if (kind === "operator_set_role" && typeof m.id === "string" && typeof m.role === "string") {
        const id = String(m.id);
        const role = String(m.role);
        return {
          ...state,
          operators: state.operators
            ? state.operators.map((o) => (o.id === id ? { ...o, role } : o))
            : null,
        };
      }
      return state;
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
  /** P3 Theme B follow-up: clear the post-`operator_create` minted-token banner. */
  clearMintedOperatorToken: () => void;
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

  // Dev-only test hooks for Playwright. The pair lets `e2e/ship-gate.spec.ts`
  // take over the store deterministically: `__cliptownStopWS` disconnects the
  // live world so its snapshots stop racing the test, and `__cliptownDispatch`
  // injects synthetic ConsoleOutbound frames (snapshots, chat/directive) that
  // exercise UI surfaces whose world emit path ships later (M5+ owns the
  // chat/directive frame protocol; ship-gate § 11.7 needs to assert the
  // ChatPanel rendering NOW). `import.meta.env.DEV` is `false` in production
  // builds, so this entire block tree-shakes away.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const w = window as typeof window & {
      __cliptownDispatch?: (msg: unknown) => void;
      __cliptownStopWS?: () => void;
    };
    w.__cliptownDispatch = (msg) => dispatch({ kind: "msg", msg: msg as Msg });
    w.__cliptownStopWS = () => {
      clientRef.current?.close();
      clientRef.current = null;
    };
    return () => {
      delete w.__cliptownDispatch;
      delete w.__cliptownStopWS;
    };
  }, []);

  return {
    state,
    send: (msg) => clientRef.current?.send(msg),
    addToast: (severity, body, sticky) =>
      dispatch({ kind: "localToast", severity, body, sticky }),
    clearMintedOperatorToken: () => dispatch({ kind: "clearMintedOperatorToken" }),
  };
}
