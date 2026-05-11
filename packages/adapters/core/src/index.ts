/**
 * BackendAdapter contract: each LLM CLI (Claude Code, codex, opencode) implements
 * this so the worker can spawn it uniformly and receive normalized hook events.
 *
 * Phase 1 invariants (M9.10 A1' — MCP-at-the-world):
 *   - `mcp_world_url` + `mcp_token` are mandatory: the adapter wires the CLI's
 *     MCP config to POST tool calls directly to the world's `/mcp` HTTP
 *     endpoint with the bearer token. There is no per-worker MCP server; the
 *     worker is a process supervisor.
 *   - Hooks (pre_tool, post_tool, session_stop, session_error) MUST be normalized
 *     across adapters so the worker doesn't need per-adapter branching.
 */

export interface AdapterCapabilities {
  /** Hook event names this adapter can produce. */
  hooks: ReadonlyArray<HookKind>;
  /** Whether the adapter can inject context into the CLI's prompt before spawn. */
  inject_context: boolean;
  /** Whether the adapter can block the CLI from exiting at session_stop until the worker acks. */
  block_on_stop: boolean;
}

export type HookKind = "pre_tool" | "post_tool" | "session_stop" | "session_error";

export interface SpawnOpts {
  /** Initial prompt to feed the CLI. */
  prompt: string;
  /** Working directory for the spawned process (typically workspaces/<startup_id>/). */
  cwd: string;
  /** Environment variables — merged onto inherited env. */
  env?: NodeJS.ProcessEnv;
  /**
   * Base HTTP URL of the world's MCP endpoint (e.g. `http://127.0.0.1:8080`).
   * The adapter MUST configure the CLI to POST `mcp__cliptown__*` tool calls
   * to `<mcp_world_url>/mcp` with the bearer token below.
   */
  mcp_world_url: string;
  /**
   * Bearer token for the world's MCP endpoint. Format: `<agent_id>:<secret>`,
   * matching `crates/world/src/mcp_http.rs::authenticate`. The CLI sends
   * `Authorization: Bearer <token>` on every tool call.
   */
  mcp_token: string;
  /** Optional override of the CLI binary path; useful in tests. */
  bin?: string;
  /** Optional callback for normalized hook events. */
  onHook?: (e: HookEvent) => void;
  /** Optional callback for stdout/stderr lines (raw, for diagnostics). */
  onLog?: (stream: "stdout" | "stderr", line: string) => void;
}

/**
 * Token/cost telemetry the adapter scraped from the CLI's final output. Lets
 * the worker forward a `ReportBudget` WS frame to the world so the per-startup
 * `budget_spent_usd` ladder reflects real spend. Optional — fixtures and CLIs
 * that don't surface usage simply leave this undefined.
 */
export interface UsageReport {
  in_tokens: number;
  out_tokens: number;
  cost_usd: number;
  model_id: string;
}

export interface SpawnResult {
  /** OS process id of the spawned CLI. */
  pid: number;
  /** Resolves when the CLI exits. `usage` is populated when the adapter could
   *  parse the CLI's final response (e.g. claude-code with `--output-format json`). */
  wait(): Promise<{ exit_code: number; signal?: string; usage?: UsageReport }>;
  /** Forcibly terminate the CLI (SIGTERM by default). */
  kill(signal?: NodeJS.Signals): void;
}

export interface HookEvent {
  kind: HookKind;
  /** Tool name for pre_tool/post_tool, "" for session events. */
  tool: string;
  /** Tool args for pre_tool, result for post_tool, payload for session events. */
  payload: unknown;
  /** Monotonic per-session sequence — useful for ordering across hooks. */
  seq: number;
  /** Wall-clock ms timestamp. */
  ts_ms: number;
}

export interface BackendAdapter {
  /** Stable identifier — matches `agents.backend` enum values: claude_code, codex, opencode. */
  id: "claude_code" | "codex" | "opencode";
  /** Capabilities advertised to the worker. */
  capabilities: AdapterCapabilities;
  /** Spawn the CLI subprocess with the given options. */
  spawn(opts: SpawnOpts): Promise<SpawnResult>;
}

export { startHookBridge, type HookBridge } from "./hook_bridge.js";
