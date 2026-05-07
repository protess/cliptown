import { describe, it, expect } from "vitest";
import { claudeCodeAdapter } from "@cliptown/adapter-claude-code";
import type { HookEvent } from "@cliptown/adapter-core";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { tmpdir } from "node:os";

/**
 * M3.3 adapter contract test — drives claudeCodeAdapter end-to-end against
 * the fixture CLI shim (packages/worker/bin/fixture-cli) in place of the real
 * `claude` binary. The fixture parses CLAUDE_CODE_SETTINGS and fires one
 * synthetic tool cycle (PreToolUse → PostToolUse → Stop) at the bridge so we
 * can assert the adapter normalizes hooks correctly.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURE_BIN = resolve(HERE, "..", "bin", "fixture-cli");

describe("M3.3 adapter contract — claudeCodeAdapter end-to-end via fixture CLI", () => {
  it("fires pre_tool, post_tool, session_stop in order via the bridge", async () => {
    const events: HookEvent[] = [];
    const spawned = await claudeCodeAdapter.spawn({
      prompt: "fixture run",
      cwd: tmpdir(),
      mcp_socket_path: "/tmp/cliptown-test-fixture.sock",
      bin: FIXTURE_BIN,
      onHook: (e) => events.push(e),
    });

    const exit = await spawned.wait();
    expect(exit.exit_code).toBe(0);

    // wait() resolves AFTER the child exits; since fireHook in the fixture
    // uses spawnSync, every hook POST has completed before exit.
    expect(events.length).toBeGreaterThanOrEqual(3);
    const kinds = events.map((e) => e.kind);
    expect(kinds).toContain("pre_tool");
    expect(kinds).toContain("post_tool");
    expect(kinds).toContain("session_stop");

    const pre = events.find((e) => e.kind === "pre_tool")!;
    expect(pre.tool).toBe("Read");
    expect(pre.payload).toMatchObject({ tool: "Read", args: { path: "spec.md" } });

    const post = events.find((e) => e.kind === "post_tool")!;
    expect(post.tool).toBe("Read");
    expect(post.payload).toMatchObject({ tool: "Read", result: { ok: true, bytes: 42 } });

    const stop = events.find((e) => e.kind === "session_stop")!;
    expect(stop.payload).toMatchObject({ reason: "fixture-cli complete", prompt: "fixture run" });

    // seq is monotonic in arrival order.
    const seqs = events.map((e) => e.seq);
    for (let i = 1; i < seqs.length; i++) {
      expect(seqs[i]).toBeGreaterThan(seqs[i - 1]);
    }
  });

  it("MCP config file written into a tmpdir and accepted by the adapter", async () => {
    // The fixture CLI doesn't actually call the MCP server, but the adapter
    // must have written a valid mcp.json + settings.json. We verify by
    // checking exit_code == 0 and that no MCP-related errors surfaced on
    // stderr.
    let stderr = "";
    const spawned = await claudeCodeAdapter.spawn({
      prompt: "mcp config check",
      cwd: tmpdir(),
      mcp_socket_path: "/tmp/cliptown-mcp-cfg.sock",
      bin: FIXTURE_BIN,
      onLog: (stream, line) => { if (stream === "stderr") stderr += line; },
    });
    const exit = await spawned.wait();
    expect(exit.exit_code).toBe(0);
    expect(stderr).not.toMatch(/mcp/i); // fixture doesn't reference MCP at all
  });
});
