import { createHash } from "node:crypto";
import { readFileSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * Deterministic LLM stand-in for tests + bring-up. Given a prompt, looks up a
 * pre-recorded `tool_use` sequence by prompt hash → fixture name, and replays
 * one entry per `next()` call. Returns null when the sequence is exhausted.
 *
 * Fixture format: one JSON `tool_use` object per line in
 * `packages/worker/fixtures/<name>.jsonl`.
 *
 * Tool-use shape:
 *   { kind: "mcp", tool: <MCP tool name>, args: {...} }
 *   { kind: "writeFile", path: <relative path>, content: <string> }
 *   { kind: "done" }
 */

export type ToolUse =
  | { kind: "mcp"; tool: string; args: Record<string, unknown> }
  | { kind: "writeFile"; path: string; content: string }
  | { kind: "done" };

/** Map from prompt SHA-256 (hex) to fixture name. Extend as fixtures land. */
export interface FixtureMap {
  [promptHash: string]: string;
}

export interface LLMMockOpts {
  /** Map: prompt-hash → fixture name. Required if `defaultFixture` is not set. */
  routes?: FixtureMap;
  /** Used when no route matches — useful for tests that don't pin a hash. */
  defaultFixture?: string;
  /** Override fixtures dir (defaults to packages/worker/fixtures). */
  fixturesDir?: string;
}

const DEFAULT_FIXTURES_DIR = (() => {
  // resolve relative to this file: packages/worker/src → packages/worker/fixtures
  const here = dirname(fileURLToPath(import.meta.url));
  return resolve(here, "..", "fixtures");
})();

export class LLMMock {
  private routes: FixtureMap;
  private defaultFixture: string | undefined;
  private fixturesDir: string;
  private current: ToolUse[] = [];
  private cursor = 0;
  private loaded: string | null = null;

  constructor(opts: LLMMockOpts = {}) {
    this.routes = opts.routes ?? {};
    this.defaultFixture = opts.defaultFixture;
    this.fixturesDir = opts.fixturesDir ?? DEFAULT_FIXTURES_DIR;
  }

  /**
   * Set or change the active prompt. Resolves the prompt hash → fixture name
   * via routes (or defaultFixture), loads the fixture, resets cursor.
   * Returns the resolved fixture name.
   */
  setPrompt(prompt: string): string {
    const hash = createHash("sha256").update(prompt).digest("hex");
    const name = this.routes[hash] ?? this.defaultFixture;
    if (!name) {
      throw new Error(`no fixture for prompt-hash ${hash.slice(0, 12)}…`);
    }
    this.loadFixture(name);
    return name;
  }

  /** Load a fixture by name (skips hash lookup). Resets cursor. */
  loadFixture(name: string): void {
    const path = resolve(this.fixturesDir, `${name}.jsonl`);
    if (!existsSync(path)) {
      throw new Error(`fixture not found: ${path}`);
    }
    const raw = readFileSync(path, "utf-8");
    this.current = raw
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.length > 0)
      .map((l) => JSON.parse(l) as ToolUse);
    this.cursor = 0;
    this.loaded = name;
  }

  /** Return the next tool_use, or null if the sequence is exhausted. */
  next(): ToolUse | null {
    if (this.cursor >= this.current.length) return null;
    const t = this.current[this.cursor++];
    return t;
  }

  /** For tests / diagnostics. */
  remaining(): number {
    return Math.max(0, this.current.length - this.cursor);
  }

  /** For tests. Returns the loaded fixture name or null. */
  loadedFixture(): string | null {
    return this.loaded;
  }
}
