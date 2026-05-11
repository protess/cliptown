#!/usr/bin/env node
/**
 * Stand-in for the `claude` CLI used in M3.3 contract tests.
 *
 * Behavior: parses Claude Code-like args + reads CLAUDE_CODE_SETTINGS,
 * loads the hook scripts from settings.json, and fires
 * PreToolUse → PostToolUse → Stop in order with synthetic payloads.
 * Each invocation runs the hook's command via `sh -c` and pipes a JSON
 * payload to stdin (matching `curl --data-binary @-`). Exits 0 on success.
 *
 * Used by claudeCodeAdapter.spawn() in M3.3 contract tests via a wrapper
 * shim at packages/worker/bin/fixture-cli.
 */

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";

interface ClaudeHookEntry {
  type: "command";
  command: string;
}

interface ClaudeHookGroup {
  matcher?: string;
  hooks: ClaudeHookEntry[];
}

interface ClaudeSettings {
  hooks: {
    PreToolUse?: ClaudeHookGroup[];
    PostToolUse?: ClaudeHookGroup[];
    Stop?: ClaudeHookGroup[];
    Notification?: ClaudeHookGroup[];
  };
}

function fireHook(command: string, payload: unknown): void {
  const body = JSON.stringify(payload);
  const r = spawnSync("sh", ["-c", command], {
    input: body,
    encoding: "utf-8",
    stdio: ["pipe", "pipe", "pipe"],
  });
  if (r.status !== 0) {
    process.stderr.write(`[fixture-cli] hook failed (status=${r.status}): ${r.stderr}\n`);
  }
}

function flattenCommands(group: ClaudeHookGroup[] | undefined): string[] {
  if (!group) return [];
  const out: string[] = [];
  for (const g of group) for (const h of g.hooks) out.push(h.command);
  return out;
}

function main(): void {
  // Accept Claude Code-like flags without crashing — most are ignored.
  const { values } = parseArgs({
    args: process.argv.slice(2),
    options: {
      "print":             { type: "string" },
      "prompt":            { type: "string" },   // codex/opencode fixture path
      "allowedTools":      { type: "string" },
      "mcp-config":        { type: "string" },
      "strict-mcp-config": { type: "boolean", default: false },
    },
    strict: true,
    allowPositionals: false,
  });

  const settingsPath = process.env.CLAUDE_CODE_SETTINGS;
  if (!settingsPath) {
    // codex/opencode adapters spawn fixture-cli without CLAUDE_CODE_SETTINGS;
    // they synthesize their own hook sequence. Just exit 0.
    process.stdout.write(`[fixture-cli] no CLAUDE_CODE_SETTINGS — codex/opencode fixture path; exiting 0\n`);
    process.exit(0);
  }
  const settings = JSON.parse(readFileSync(settingsPath, "utf-8")) as ClaudeSettings;

  // Fire one synthetic tool invocation: PreToolUse → PostToolUse → Stop.
  for (const c of flattenCommands(settings.hooks.PreToolUse)) {
    fireHook(c, { tool: "Read", args: { path: "spec.md" } });
  }
  for (const c of flattenCommands(settings.hooks.PostToolUse)) {
    fireHook(c, { tool: "Read", result: { ok: true, bytes: 42 } });
  }
  for (const c of flattenCommands(settings.hooks.Stop)) {
    fireHook(c, { reason: "fixture-cli complete", prompt: values["print"] ?? "" });
  }

  process.stdout.write(`[fixture-cli] done; prompt=${JSON.stringify(values["print"] ?? "")}\n`);
  process.exit(0);
}

main();
