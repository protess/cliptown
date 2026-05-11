// packages/adapters/codex/src/event_parser.ts
import type { HookEvent, HookKind } from "@cliptown/adapter-core";

/**
 * Streaming JSONL parser for codex's `exec --json` stdout. Emits HookEvents
 * directly so the codex adapter can forward them into opts.onHook without
 * going through the (unused-for-codex) HTTP hook bridge.
 *
 * Pure module — no I/O. State is held in CodexParserState so a single
 * parser instance can survive across stdout chunks split mid-line. Tests
 * drive parseChunk + finalize directly with fixture strings.
 */

export interface CodexUsageAccum {
  in_tokens: number;
  out_tokens: number;
  saw: boolean;
}

export interface CodexParserState {
  seq: number;
  usage: CodexUsageAccum;
  /** Partial line buffer carried across parseChunk calls. */
  carry: string;
}

export function emptyState(): CodexParserState {
  return {
    seq: 0,
    usage: { in_tokens: 0, out_tokens: 0, saw: false },
    carry: "",
  };
}

export interface ParseChunkResult {
  hooks: HookEvent[];
}

export function parseChunk(chunk: string, state: CodexParserState): ParseChunkResult {
  const combined = state.carry + chunk;
  const lines = combined.split("\n");
  // Last element is the partial tail (no trailing \n) or "" if chunk ended on \n.
  state.carry = lines.pop() ?? "";
  const hooks: HookEvent[] = [];
  for (const line of lines) {
    parseLine(line, state, hooks);
  }
  return { hooks };
}

export interface FinalizeInfo {
  exit_code: number;
  signal: string | undefined;
  stderr_tail: string;
}

export interface FinalizeResult {
  hooks: HookEvent[];
}

export function finalize(state: CodexParserState, info: FinalizeInfo): FinalizeResult {
  const hooks: HookEvent[] = [];
  // Flush any held-back line that didn't end in \n.
  if (state.carry.length > 0) {
    parseLine(state.carry, state, hooks);
    state.carry = "";
  }
  const kind: HookKind = info.exit_code === 0 ? "session_stop" : "session_error";
  const payload: Record<string, unknown> = { exit_code: info.exit_code };
  if (info.signal) payload.signal = info.signal;
  if (kind === "session_error") payload.stderr_tail = info.stderr_tail;
  hooks.push({
    kind,
    tool: "",
    payload,
    seq: ++state.seq,
    ts_ms: Date.now(),
  });
  return { hooks };
}

function parseLine(line: string, state: CodexParserState, out: HookEvent[]): void {
  const trimmed = line.trim();
  if (trimmed.length === 0) return;
  let evt: { type?: string; item?: { type?: string; tool?: string }; usage?: { input_tokens?: number; cached_input_tokens?: number; output_tokens?: number } };
  try {
    evt = JSON.parse(line);
  } catch {
    return;
  }
  if (evt.type === "item.started" || evt.type === "item.completed") {
    const item = evt.item;
    if (!item) return;
    if (item.type === "command_execution") {
      out.push({
        kind: evt.type === "item.started" ? "pre_tool" : "post_tool",
        tool: "shell",
        payload: item,
        seq: ++state.seq,
        ts_ms: Date.now(),
      });
    } else if (item.type === "mcp_tool_call") {
      out.push({
        kind: evt.type === "item.started" ? "pre_tool" : "post_tool",
        tool: typeof item.tool === "string" && item.tool.length > 0 ? item.tool : "mcp",
        payload: item,
        seq: ++state.seq,
        ts_ms: Date.now(),
      });
    }
    return;
  }
  if (evt.type === "turn.completed" && evt.usage) {
    state.usage.in_tokens +=
      (evt.usage.input_tokens ?? 0) + (evt.usage.cached_input_tokens ?? 0);
    state.usage.out_tokens += evt.usage.output_tokens ?? 0;
    state.usage.saw = true;
  }
}
