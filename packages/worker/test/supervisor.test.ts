import { describe, it, expect, vi } from "vitest";
import { Supervisor } from "../src/supervisor.js";
import type { BackendAdapter, SpawnOpts, SpawnResult } from "@cliptown/adapter-core";

interface FakeSpawn {
  exitCode: number;
  signal?: string;
  delayMs?: number;
  throwOnSpawn?: string;
}

function makeFakeAdapter(plan: FakeSpawn[]): { adapter: BackendAdapter } {
  const state = { calls: 0 };
  const adapter: BackendAdapter = {
    id: "claude_code",
    capabilities: {
      hooks: ["pre_tool", "post_tool", "session_stop", "session_error"],
      inject_context: true,
      block_on_stop: true,
    },
    async spawn(_opts: SpawnOpts): Promise<SpawnResult> {
      const idx = Math.min(state.calls, plan.length - 1);
      const step = plan[idx];
      state.calls += 1;
      if (step.throwOnSpawn) throw new Error(step.throwOnSpawn);
      const delay = step.delayMs ?? 0;
      const result: SpawnResult = {
        pid: 9000 + state.calls,
        wait: async () => {
          if (delay > 0) await new Promise((r) => setTimeout(r, delay));
          return { exit_code: step.exitCode, signal: step.signal };
        },
        kill: () => { /* noop */ },
      };
      return result;
    },
  };
  return { adapter };
}

const noSleep = async (_ms: number) => { /* skip */ };

const baseSpawnOpts: SpawnOpts = {
  prompt: "x",
  cwd: "/tmp",
  mcp_world_url: "http://127.0.0.1:0",
  mcp_token: "e1:dev-secret",
};

describe("Supervisor — clean exit", () => {
  it("returns immediately on exit_code 0 without respawn", async () => {
    const { adapter } = makeFakeAdapter([{ exitCode: 0 }]);
    const sup = new Supervisor({ adapter, spawnOpts: baseSpawnOpts, sleep: noSleep });
    await sup.start();
    expect(sup.state().failures).toBe(0);
    expect(sup.state().running).toBe(false);
  });
});

describe("Supervisor — backoff + respawn", () => {
  it("respawns once on non-zero exit, then succeeds", async () => {
    const { adapter } = makeFakeAdapter([{ exitCode: 1 }, { exitCode: 0 }]);
    const calls: number[] = [];
    const sleep = async (ms: number) => { calls.push(ms); };
    const sup = new Supervisor({ adapter, spawnOpts: baseSpawnOpts, sleep });
    await sup.start();
    expect(calls).toEqual([1_000]);
    expect(sup.state().failures).toBe(1);
  });

  it("respawns three times then declares dead", async () => {
    const { adapter } = makeFakeAdapter([
      { exitCode: 1 }, { exitCode: 1 }, { exitCode: 1 }, { exitCode: 1 },
    ]);
    const calls: number[] = [];
    const sleep = async (ms: number) => { calls.push(ms); };
    let dead: { attempts: number; last_error?: string } | null = null;
    const sup = new Supervisor({
      adapter, spawnOpts: baseSpawnOpts, sleep,
      onDead: (i) => { dead = i; },
    });
    await sup.start();
    expect(calls).toEqual([1_000, 5_000, 30_000]);
    expect(dead).not.toBeNull();
    expect(dead!.attempts).toBe(4);
    expect(dead!.last_error).toMatch(/exit_code=1/);
  });

  it("treats SIGKILL as failure even with exit_code 0", async () => {
    const { adapter } = makeFakeAdapter([
      { exitCode: 0, signal: "SIGKILL" }, { exitCode: 0 },
    ]);
    const sup = new Supervisor({
      adapter, spawnOpts: baseSpawnOpts, sleep: noSleep,
    });
    await sup.start();
    expect(sup.state().failures).toBe(1);
  });

  it("counts spawn-throws as failures", async () => {
    const { adapter } = makeFakeAdapter([
      { exitCode: 0, throwOnSpawn: "ENOENT: claude not found" }, { exitCode: 0 },
    ]);
    const sup = new Supervisor({
      adapter, spawnOpts: baseSpawnOpts, sleep: noSleep,
    });
    await sup.start();
    expect(sup.state().failures).toBe(1);
  });

  it("uses overridden backoff schedule", async () => {
    const { adapter } = makeFakeAdapter([{ exitCode: 1 }, { exitCode: 1 }, { exitCode: 0 }]);
    const calls: number[] = [];
    const sup = new Supervisor({
      adapter, spawnOpts: baseSpawnOpts,
      backoffMs: [10, 20, 30],
      sleep: async (ms) => { calls.push(ms); },
    });
    await sup.start();
    expect(calls).toEqual([10, 20]);
  });
});

describe("Supervisor — stop()", () => {
  it("stop() prevents further respawns", async () => {
    const exit$ = vi.fn();
    const { adapter } = makeFakeAdapter([{ exitCode: 1, delayMs: 50 }, { exitCode: 0 }]);
    const sup = new Supervisor({ adapter, spawnOpts: baseSpawnOpts, sleep: async () => {} });
    const startP = sup.start();
    // Let the first spawn finish failing, then stop.
    await new Promise((r) => setTimeout(r, 80));
    await sup.stop();
    await startP;
    void exit$;
    expect(sup.state().stopped).toBe(true);
  });
});
