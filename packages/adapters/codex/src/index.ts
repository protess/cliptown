import { spawn as nodeSpawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  startHookBridge,
  type AdapterCapabilities, type BackendAdapter, type HookBridge,
  type HookEvent, type SpawnOpts, type SpawnResult, type UsageReport,
} from "@cliptown/adapter-core";

/**
 * Codex CLI adapter. Wraps `codex exec --json` (codex-cli ≥ 0.124) so the
 * worker can drive Codex in non-interactive mode the same way it drives
 * Claude Code. Phase 1 (M9.10 follow-up) integration choices:
 *
 *   - MCP config is injected via `-c mcp_servers.cliptown.{url,bearer_token_env_var}`
 *     overrides (codex's `-c key=value` flag accepts dotted TOML paths). The
 *     bearer token is read from `CLIPTOWN_MCP_TOKEN` env, set per-spawn so
 *     each worker gets its own credential without writing config files.
 *   - `--ignore-user-config` skips `~/.codex/config.toml` so the adapter's
 *     overrides are the entire config; auth still resolves via `CODEX_HOME`,
 *     so existing `codex login` credentials work.
 *   - `--full-auto` skips approval prompts, `--skip-git-repo-check` allows
 *     running outside a git repo, `--ephemeral` avoids persisting session
 *     state. These three together yield a hermetic single-shot run.
 *   - `--json` makes stdout a JSONL stream of `thread.started`,
 *     `turn.started`, `item.completed`, `turn.completed` events. The
 *     adapter scrapes `turn.completed.usage` for `UsageReport`; `cost_usd`
 *     is omitted (codex does not report a dollar figure) so the world's
 *     budget ladder falls back to its `price_per_mtok` table.
 *   - Hooks bridge: codex does not implement Claude Code's per-tool
 *     PreToolUse/PostToolUse webhook contract, so this adapter advertises
 *     only `session_stop` / `session_error`. The bridge port is still
 *     started + exposed via `CODEX_HOOK_PORT` so future hook wiring has
 *     somewhere to land.
 *
 * Override the binary via `SpawnOpts.bin` or `CLIPTOWN_FIXTURE_CLI` for
 * tests against the fixture-cli shim.
 */

const CAPS: AdapterCapabilities = {
  hooks: ["session_stop", "session_error"],
  inject_context: false,
  block_on_stop: false,
};

const TOKEN_ENV_VAR = "CLIPTOWN_MCP_TOKEN";

/**
 * Tag for the `model_id` field of the worker's `report_budget` frame.
 *
 * codex CLI, when authenticated via a ChatGPT account (the common case for
 * developer machines), routes requests to whatever model the subscription
 * provides. The CLI rejects any explicit `--model` / `-c model=...` override
 * with `"The '<name>' model is not supported when using Codex with a
 * ChatGPT account."` — so we cannot pin a stable upstream model name. The
 * JSONL stream also does not surface the resolved model.
 *
 * We tag with a stable cliptown-owned identifier (`gpt-5-chatgpt`) instead
 * of letting the upstream name drift. The world's `price_per_mtok` table
 * has an entry for this identifier so the budget ladder fills in; if
 * OpenAI rotates the underlying model significantly we update the table
 * entry's per-Mtok rates without touching the adapter.
 *
 * Override via `CODEX_MODEL_ID` env when an operator runs codex against
 * a non-ChatGPT auth path (API key, OSS provider, etc.) where they know
 * the resolved model. The override flows straight to the world unchanged
 * so the operator can hand-edit the pricing table for that name.
 */
const MODEL_ID_ENV = "CODEX_MODEL_ID";
const DEFAULT_MODEL_ID = "gpt-5-chatgpt";

export const codexAdapter: BackendAdapter = {
  id: "codex",
  capabilities: CAPS,
  async spawn(opts: SpawnOpts): Promise<SpawnResult> {
    const onHook = opts.onHook ?? (() => { /* noop */ });
    const modelId = opts.env?.[MODEL_ID_ENV] ?? process.env[MODEL_ID_ENV] ?? DEFAULT_MODEL_ID;

    let bridge: HookBridge | null = null;
    // Codex doesn't need a config file — the adapter injects all config via
    // `-c` overrides — but we still mint a tmpdir for any future scratch
    // files and so the cleanup contract matches the other adapters.
    let cfgDir: string | null = null;

    try {
      bridge = await startHookBridge(onHook);
      cfgDir = await mkdtemp(join(tmpdir(), "ct-codex-"));
    } catch (e) {
      if (bridge) await bridge.close();
      if (cfgDir) await rm(cfgDir, { recursive: true, force: true });
      throw e;
    }

    const bin = opts.bin ?? process.env.CLIPTOWN_FIXTURE_CLI ?? "codex";
    const isFixture = !!(opts.bin || process.env.CLIPTOWN_FIXTURE_CLI);
    const env = {
      ...process.env,
      ...opts.env,
      CODEX_HOOK_PORT: String(bridge.port),
      [TOKEN_ENV_VAR]: opts.mcp_token,
    };

    let args: string[];
    if (isFixture) {
      // Fixture CLIs don't speak codex's flags; pass the prompt through
      // unchanged so the contract test's synthetic hook sequence runs.
      args = ["--prompt", opts.prompt];
    } else {
      args = [
        "exec",
        "--ignore-user-config",
        // `--full-auto` auto-approves shell commands only. MCP tool calls
        // (which is how the agent reports task_done back to the world) are
        // gated separately and get "user cancelled MCP tool call" in
        // non-interactive mode unless we lift the gate entirely. The
        // bypass flag is intended for "environments that are externally
        // sandboxed" — our SMOKE_DIR tmpdir + per-spawn bearer token fits
        // that description. The world's `mcp_http::authenticate` is the
        // real enforcement boundary.
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        "--ephemeral",
        "--cd", opts.cwd,
        "--color", "never",
        "--json",
        "-c", `mcp_servers.cliptown.url="${opts.mcp_world_url}/mcp"`,
        "-c", `mcp_servers.cliptown.bearer_token_env_var="${TOKEN_ENV_VAR}"`,
        opts.prompt,
      ];
    }

    const child = nodeSpawn(
      bin,
      args,
      // stdin=ignore: codex's exec mode reads stdin for piped instructions
      // when the prompt arg is "-". Closing stdin ensures it doesn't sit
      // waiting on us.
      { cwd: opts.cwd, env, stdio: ["ignore", "pipe", "pipe"] },
    );

    let stdoutBuf = "";
    child.stdout?.on("data", (b: Buffer) => {
      const s = b.toString("utf-8");
      stdoutBuf += s;
      opts.onLog?.("stdout", s);
    });
    if (opts.onLog) {
      child.stderr?.on("data", (b: Buffer) => opts.onLog?.("stderr", b.toString("utf-8")));
    }

    const exit = new Promise<{ exit_code: number; signal?: string }>((resolve) => {
      child.on("exit", (code, signal) => {
        resolve({ exit_code: code ?? -1, signal: signal ?? undefined });
      });
    });

    const _bridge = bridge;
    const _cfgDir = cfgDir;

    const result: SpawnResult = {
      pid: child.pid ?? -1,
      async wait() {
        const r = await exit;
        await Promise.allSettled([
          _bridge.close(),
          rm(_cfgDir, { recursive: true, force: true }),
        ]);
        const usage = isFixture ? undefined : parseUsage(stdoutBuf, modelId);
        return { ...r, usage };
      },
      kill(signal: NodeJS.Signals = "SIGTERM") {
        try { child.kill(signal); } catch { /* noop */ }
      },
    };

    return result;
  },
};

/**
 * Scrape `turn.completed.usage` from codex's JSONL stream. Codex emits
 * `{input_tokens, cached_input_tokens, output_tokens}` per turn; we sum
 * input + cached to feed the world's tokens-billed accounting (matches how
 * the claude-code adapter treats cache reads). `cost_usd` is intentionally
 * undefined — codex doesn't report a dollar figure, so the world falls
 * back to its `price_per_mtok` table keyed on the `modelId` argument.
 */
function parseUsage(buf: string, modelId: string): UsageReport | undefined {
  let in_tokens = 0;
  let out_tokens = 0;
  let saw = false;
  for (const line of buf.split("\n")) {
    if (!line.trim()) continue;
    let evt: { type?: string; usage?: { input_tokens?: number; cached_input_tokens?: number; output_tokens?: number } };
    try {
      evt = JSON.parse(line);
    } catch {
      continue;
    }
    if (evt.type === "turn.completed" && evt.usage) {
      in_tokens += (evt.usage.input_tokens ?? 0) + (evt.usage.cached_input_tokens ?? 0);
      out_tokens += evt.usage.output_tokens ?? 0;
      saw = true;
    }
  }
  if (!saw) return undefined;
  return { in_tokens, out_tokens, model_id: modelId };
}

export type { HookEvent };
