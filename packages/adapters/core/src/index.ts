/**
 * BackendAdapter contract: each LLM CLI (Claude Code, codex, opencode) implements
 * this so the worker can spawn it uniformly and receive normalized hook events.
 *
 * Phase 0 invariants (from spec §6):
 *   - mcp_socket_path is mandatory: the adapter must wire the CLI's MCP config
 *     to point at the worker's MCP socket so tool_use → MCP calls round-trip.
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
   * Path to the worker's MCP UNIX socket. The adapter MUST configure the CLI
   * to connect to this socket for `mcp__cliptown__*` tool calls.
   */
  mcp_socket_path: string;
  /** Optional override of the CLI binary path; useful in tests. */
  bin?: string;
  /** Optional callback for normalized hook events. */
  onHook?: (e: HookEvent) => void;
  /** Optional callback for stdout/stderr lines (raw, for diagnostics). */
  onLog?: (stream: "stdout" | "stderr", line: string) => void;
}

export interface SpawnResult {
  /** OS process id of the spawned CLI. */
  pid: number;
  /** Resolves when the CLI exits, with exit code + signal. */
  wait(): Promise<{ exit_code: number; signal?: string }>;
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
