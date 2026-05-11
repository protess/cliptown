import type { HookEvent } from "@cliptown/adapter-core";

/**
 * Maps opencode `/event` SSE events to HookEvents. The interesting
 * surface is `message.part.updated` with `part.type === "tool"`, which
 * fires multiple times per call (pending → running → ... → completed).
 * We dedup by callID and emit exactly one pre_tool (on first running)
 * and one post_tool (on first terminal status). step-finish parts feed
 * the UsageReport accumulator.
 *
 * Pure module — no I/O. State lives in MapState so a single mapper
 * instance can span an opencode session.
 */

export interface MapUsageAccum {
  in_tokens: number;
  out_tokens: number;
  cost_usd: number;
  saw: boolean;
}

interface CallTracker {
  fired_pre: boolean;
  fired_post: boolean;
}

export interface MapState {
  seq: number;
  usage: MapUsageAccum;
  calls: Map<string, CallTracker>;
}

export function emptyMapState(): MapState {
  return {
    seq: 0,
    usage: { in_tokens: 0, out_tokens: 0, cost_usd: 0, saw: false },
    calls: new Map(),
  };
}

export interface SseEvent {
  type?: string;
  properties?: {
    part?: {
      type?: string;
      tool?: string;
      callID?: string;
      state?: {
        status?: string;
        input?: unknown;
        output?: unknown;
        time?: { start?: number; end?: number };
      };
      tokens?: {
        input?: number;
        output?: number;
        reasoning?: number;
        cache?: { write?: number; read?: number };
      };
      cost?: number;
    };
  };
}

export interface MapResult {
  hooks: HookEvent[];
}

export function mapEvent(evt: SseEvent, state: MapState): MapResult {
  const hooks: HookEvent[] = [];
  const t = evt.type;

  if (t === "session.idle") {
    hooks.push({
      kind: "session_stop",
      tool: "",
      payload: {},
      seq: ++state.seq,
      ts_ms: Date.now(),
    });
    return { hooks };
  }

  if (t === "session.status") {
    // Spec note: exact error variant shape unconfirmed. If a future probe
    // shows a terminal-error variant carrying e.g. {phase:"errored"} or
    // {error}, add it here. Until then this branch is dormant.
    return { hooks };
  }

  if (t !== "message.part.updated") return { hooks };
  const part = evt.properties?.part;
  if (!part) return { hooks };

  if (part.type === "step-finish") {
    const tokens = part.tokens;
    if (tokens) {
      state.usage.in_tokens +=
        (tokens.input ?? 0) +
        (tokens.cache?.write ?? 0) +
        (tokens.cache?.read ?? 0);
      state.usage.out_tokens += (tokens.output ?? 0) + (tokens.reasoning ?? 0);
      state.usage.cost_usd += part.cost ?? 0;
      state.usage.saw = true;
    }
    return { hooks };
  }

  if (part.type !== "tool") return { hooks };

  const callID = part.callID;
  if (!callID) return { hooks };
  const status = part.state?.status ?? "";
  const tool = part.tool ?? "";

  let tracker = state.calls.get(callID);
  if (!tracker) {
    tracker = { fired_pre: false, fired_post: false };
    state.calls.set(callID, tracker);
  }

  if (status === "running" && !tracker.fired_pre) {
    tracker.fired_pre = true;
    hooks.push({
      kind: "pre_tool",
      tool,
      payload: part.state?.input ?? null,
      seq: ++state.seq,
      ts_ms: Date.now(),
    });
    return { hooks };
  }

  if ((status === "completed" || status === "failed") && !tracker.fired_post) {
    // If the model emits completed without ever passing through running,
    // synthesize a pre first so consumers always see pre before post.
    if (!tracker.fired_pre) {
      tracker.fired_pre = true;
      hooks.push({
        kind: "pre_tool",
        tool,
        payload: part.state?.input ?? null,
        seq: ++state.seq,
        ts_ms: Date.now(),
      });
    }
    tracker.fired_post = true;
    hooks.push({
      kind: "post_tool",
      tool,
      payload: {
        output: part.state?.output ?? null,
        status,
        time: part.state?.time,
      },
      seq: ++state.seq,
      ts_ms: Date.now(),
    });
    return { hooks };
  }

  return { hooks };
}
