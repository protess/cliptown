import type { HookEvent, UsageReport } from "@cliptown/adapter-core";

/**
 * Maps opencode `/event` SSE events to HookEvents. The interesting
 * surface is `message.part.updated` with `part.type === "tool"`, which
 * fires multiple times per call (pending → running → ... → completed).
 * We dedup by callID and emit exactly one pre_tool (on first running)
 * and one post_tool (on first terminal status). step-finish parts feed
 * the UsageReport accumulator.
 *
 * Wire format: this mapper consumes events from opencode's HTTP SSE
 * `/event` stream (the outer envelope is `{type, properties}` and the
 * tool/step parts live under `properties.part`). It does NOT consume
 * `opencode run --format json` stdout, which uses a different shape
 * (outer `{type: "step_finish" | "tool_use", part: {...}}`). The two
 * formats share inner `part.type` values (`step-finish`, `tool`, etc.)
 * but differ in envelope.
 *
 * Pure module — no I/O. State lives in MapState so a single mapper
 * instance can span an opencode session.
 */

interface MapUsageAccum {
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

/**
 * Convert the mapper's running usage accumulator into a UsageReport.
 * Returns undefined when no step-finish frame was observed (the agent
 * never billed tokens). Mirrors `event_parser.toUsageReport` from the
 * codex adapter so callers don't have to reach into state.usage.
 *
 * opencode reports a USD cost figure directly, so cost_usd is always
 * populated (it may be 0 for free-tier plans).
 */
export function toUsageReport(state: MapState, modelId: string): UsageReport | undefined {
  if (!state.usage.saw) return undefined;
  return {
    in_tokens: state.usage.in_tokens,
    out_tokens: state.usage.out_tokens,
    cost_usd: state.usage.cost_usd,
    model_id: modelId,
  };
}
