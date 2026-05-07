import type { WorkerHandle } from "./ws.js";

/**
 * MCP proxy for the cliptown worker.
 *
 * Each method sends an `mcp_call { tool, corr_id, args }` frame to the world over
 * the worker WS, and resolves with the world's `mcp_reply.result` or rejects
 * with an `mcp_error`. Listener cleanup is guaranteed on every termination
 * path — success, error, and timeout — so 100s of sequential calls do not
 * accumulate listeners on the underlying WS.
 *
 * Tool names and argument shapes match spec §6.2 of
 * `docs/superpowers/specs/2026-05-07-cliptown-design.md`.
 *
 * Note: `operator_force_accept` and `operator_force_fail` from spec §6.2 are
 * intentionally NOT exposed here — they are operator-only and travel via
 * `/ws/console`, never as MCP calls from a worker-side CLI.
 */

export interface McpError extends Error {
  code?: string;
}

let corrCounter = 0;
const corrSeed =
  typeof process !== "undefined" && typeof process.pid === "number"
    ? process.pid
    : 0;

function nextCorrId(): string {
  corrCounter += 1;
  return `${corrSeed}-${Date.now()}-${corrCounter}`;
}

export interface McpCall {
  tool: string;
  args: Record<string, unknown>;
  corr_id?: string;
}

/**
 * Send an MCP call over the WS and await the reply.
 *
 * Cleanup contract: the message listener registered with `ws.onMessage` is
 * removed exactly once on every termination path:
 *   - `mcp_reply` matched on corr_id
 *   - `mcp_error` matched on corr_id
 *   - timeout expiration
 * Frames with mismatched corr_id are ignored without removing the listener,
 * so concurrent calls do not interfere.
 */
export async function callOverWS(
  ws: WorkerHandle,
  call: McpCall,
  timeoutMs: number = 60_000,
): Promise<unknown> {
  const corr_id = call.corr_id ?? nextCorrId();
  const frame = {
    type: "mcp_call",
    v: 1,
    tool: call.tool,
    args: call.args,
    corr_id,
  };

  return new Promise<unknown>((resolve, reject) => {
    let onMsg: ((m: unknown) => void) | null = null;
    let removed = false;

    const cleanup = () => {
      if (!removed && onMsg) {
        ws.offMessage(onMsg);
        removed = true;
      }
    };

    const t = setTimeout(() => {
      cleanup();
      reject(new Error("mcp_call_timeout"));
    }, timeoutMs);

    onMsg = (m: unknown) => {
      const o = m as {
        type?: string;
        corr_id?: string;
        result?: unknown;
        code?: string;
        message?: string;
      };
      if (o?.corr_id !== corr_id) return;
      clearTimeout(t);
      cleanup();
      if (o.type === "mcp_reply") {
        resolve(o.result);
      } else if (o.type === "mcp_error") {
        const err: McpError = new Error(o.message ?? "mcp_error");
        err.code = o.code;
        reject(err);
      } else {
        reject(
          new Error(`unexpected_mcp_frame: ${JSON.stringify(o)}`),
        );
      }
    };

    ws.onMessage(onMsg);
    ws.send(frame);
  });
}

// ---------- Tool argument types (spec §6.2) ----------

export interface MoveIntentArgs {
  target_room?: string;
  target_tile?: { x: number; y: number };
}

export interface SpeakArgs {
  body: string;
  kind: "chat" | "directive";
  to_agent_id?: string;
}

export interface TaskDoneArgs {
  task_id: string;
  artifact_path: string;
}

export interface TaskFailedArgs {
  task_id: string;
  reason: string;
}

export interface SubtaskCreateArgs {
  parent_id: string;
  title: string;
  description: string;
  assignee_agent_id?: string | null;
  required_room?: string;
}

export interface AcceptProposalArgs {
  task_id: string;
  assignee_agent_id: string;
  required_room?: string;
}

export interface RejectProposalArgs {
  task_id: string;
  reason: string;
}

export interface TaskAcceptArgs {
  task_id: string;
}

export interface TaskRequestChangesArgs {
  task_id: string;
  feedback: string;
  in_response_to_round: number;
}

export interface HypothesisStateArgs {
  task_id: string;
  id: string;
  claim: string;
  rationale: string;
}

export interface TestRecordArgs {
  task_id: string;
  hypothesis_id: string;
  id: string;
  method: string;
  params: Record<string, unknown>;
  expected: unknown;
  observed: unknown;
  outcome: "passed" | "failed" | "inconclusive";
}

export interface HypothesisResolveArgs {
  task_id: string;
  id: string;
  status: "supported" | "refuted" | "inconclusive";
  note: string;
}

export interface VerifyArgs {
  method:
    | "read_assert"
    | "lint_markdown"
    | "lint_typescript"
    | "lint_json"
    | "lint_yaml"
    | (string & {});
  params: Record<string, unknown>;
}

export interface AskPeerArgs {
  body: string;
  to_agent_id?: string;
  timeout_ms: number;
}

export interface ObserveWorldArgs {
  query: "peers_in_room" | "my_position" | "budget_remaining";
}

export interface ReadArtifactArgs {
  path: string;
}

// ---------- Proxy interface ----------

export interface McpProxy {
  move_intent(a: MoveIntentArgs): Promise<unknown>;
  speak(a: SpeakArgs): Promise<unknown>;
  task_done(a: TaskDoneArgs): Promise<unknown>;
  task_failed(a: TaskFailedArgs): Promise<unknown>;
  subtask_create(a: SubtaskCreateArgs): Promise<unknown>;
  accept_proposal(a: AcceptProposalArgs): Promise<unknown>;
  reject_proposal(a: RejectProposalArgs): Promise<unknown>;
  task_accept(a: TaskAcceptArgs): Promise<unknown>;
  task_request_changes(a: TaskRequestChangesArgs): Promise<unknown>;
  hypothesis_state(a: HypothesisStateArgs): Promise<unknown>;
  test_record(a: TestRecordArgs): Promise<unknown>;
  hypothesis_resolve(a: HypothesisResolveArgs): Promise<unknown>;
  verify(a: VerifyArgs): Promise<unknown>;
  ask_peer(a: AskPeerArgs): Promise<unknown>;
  observe_world(a: ObserveWorldArgs): Promise<unknown>;
  read_artifact(a: ReadArtifactArgs): Promise<unknown>;
}

/**
 * The 16 worker-callable MCP tools from spec §6.2 (excludes operator-only
 * `operator_force_accept` / `operator_force_fail` which travel via `/ws/console`).
 */
export const MCP_TOOL_NAMES: ReadonlyArray<keyof McpProxy> = [
  "move_intent",
  "speak",
  "task_done",
  "task_failed",
  "subtask_create",
  "accept_proposal",
  "reject_proposal",
  "task_accept",
  "task_request_changes",
  "hypothesis_state",
  "test_record",
  "hypothesis_resolve",
  "verify",
  "ask_peer",
  "observe_world",
  "read_artifact",
] as const;

/**
 * Build an MCP proxy bound to a worker WS handle. Each method narrowly types
 * its args and forwards through `callOverWS` with a fresh corr_id.
 */
export function createMcpProxy(ws: WorkerHandle): McpProxy {
  const make =
    <A extends Record<string, unknown>>(tool: string) =>
    (args: A): Promise<unknown> =>
      callOverWS(ws, { tool, args });

  return {
    move_intent: make<MoveIntentArgs & Record<string, unknown>>("move_intent"),
    speak: make<SpeakArgs & Record<string, unknown>>("speak"),
    task_done: make<TaskDoneArgs & Record<string, unknown>>("task_done"),
    task_failed: make<TaskFailedArgs & Record<string, unknown>>("task_failed"),
    subtask_create: make<SubtaskCreateArgs & Record<string, unknown>>(
      "subtask_create",
    ),
    accept_proposal: make<AcceptProposalArgs & Record<string, unknown>>(
      "accept_proposal",
    ),
    reject_proposal: make<RejectProposalArgs & Record<string, unknown>>(
      "reject_proposal",
    ),
    task_accept: make<TaskAcceptArgs & Record<string, unknown>>("task_accept"),
    task_request_changes: make<
      TaskRequestChangesArgs & Record<string, unknown>
    >("task_request_changes"),
    hypothesis_state: make<HypothesisStateArgs & Record<string, unknown>>(
      "hypothesis_state",
    ),
    test_record: make<TestRecordArgs & Record<string, unknown>>("test_record"),
    hypothesis_resolve: make<HypothesisResolveArgs & Record<string, unknown>>(
      "hypothesis_resolve",
    ),
    verify: make<VerifyArgs & Record<string, unknown>>("verify"),
    ask_peer: make<AskPeerArgs & Record<string, unknown>>("ask_peer"),
    observe_world: make<ObserveWorldArgs & Record<string, unknown>>(
      "observe_world",
    ),
    read_artifact: make<ReadArtifactArgs & Record<string, unknown>>(
      "read_artifact",
    ),
  };
}
